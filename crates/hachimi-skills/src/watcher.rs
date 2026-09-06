// SPDX-License-Identifier: Apache-2.0
// Adapted from OpenAI Codex commit 4c43465133428898aa84f0bfc02c306ed65fb66a:
// codex-rs/app-server/src/skills_watcher.rs and codex-rs/core/src/file_watcher.rs.
// Modified for Hachimi: dynamically watch the managed User root, configured
// extra roots, and context-bound Repo roots without granting write or Exec.

use std::path::PathBuf;

use crate::{SkillHost, SkillHostError};

pub struct SkillChangeWatch {
    receiver: tokio::sync::mpsc::UnboundedReceiver<Vec<PathBuf>>,
    #[cfg(windows)]
    stop_event: usize,
    #[cfg(windows)]
    thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(not(windows))]
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    #[cfg(not(windows))]
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SkillChangeWatch {
    pub async fn recv(&mut self) -> Option<Vec<PathBuf>> {
        self.receiver.recv().await
    }

    pub fn try_recv(&mut self) -> Option<Vec<PathBuf>> {
        self.receiver.try_recv().ok()
    }
}

#[cfg(not(windows))]
impl Drop for SkillChangeWatch {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(windows)]
impl Drop for SkillChangeWatch {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Threading::SetEvent;
        // SAFETY: stop_event remains owned by the watcher thread until it is joined below.
        unsafe {
            SetEvent(self.stop_event as windows_sys::Win32::Foundation::HANDLE);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl SkillHost {
    fn watch_root_snapshot(&self) -> Vec<PathBuf> {
        let mut roots = self
            .discovered_roots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for configured in self
            .catalog_roots
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .chain(
                self.known_context_roots
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter(),
            )
        {
            if let Ok(canonical) = std::fs::canonicalize(&configured.path)
                && canonical.is_dir()
            {
                roots.insert(canonical);
            }
        }
        roots.into_iter().collect()
    }

    #[cfg(windows)]
    pub fn watch_changes(&self) -> Result<SkillChangeWatch, SkillHostError> {
        use std::ptr;
        use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::CreateEventW};

        let roots = self.watch_root_snapshot();
        let registrations = open_registrations(&roots);
        if !registrations.iter().any(|entry| entry.root == self.root) {
            close_registrations(registrations);
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: default security, manual reset, initially non-signaled, unnamed event.
        let stop_event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if stop_event.is_null() {
            close_registrations(registrations);
            return Err(std::io::Error::last_os_error().into());
        }
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let host = self.clone();
        let stop_value = stop_event as usize;
        let thread = match std::thread::Builder::new()
            .name("hachimi-skill-watch".into())
            .spawn(move || watch_loop(host, registrations, stop_value, sender))
        {
            Ok(thread) => thread,
            Err(error) => {
                // SAFETY: no worker was started, so this scope still owns stop_event.
                unsafe { CloseHandle(stop_event) };
                return Err(error.into());
            }
        };
        Ok(SkillChangeWatch {
            receiver,
            stop_event: stop_value,
            thread: Some(thread),
        })
    }

    /// Polling fallback for macOS/Linux: snapshots each watched root every
    /// 250ms and reports roots whose (path -> size/mtime) map changed.
    /// Sufficient for Skill directories; a `notify`/FSEvents upgrade is a
    /// deliberate later step.
    #[cfg(not(windows))]
    pub fn watch_changes(&self) -> Result<SkillChangeWatch, SkillHostError> {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let host = self.clone();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            // Roots are re-snapshotted every iteration so context-bound Repo
            // roots registered after watch startup are picked up.
            let mut roots = host.watch_root_snapshot();
            let mut previous = portable_snapshot(&roots);
            while !stop_thread.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let current_roots = host.watch_root_snapshot();
                if current_roots != roots {
                    roots = current_roots;
                    previous = portable_snapshot(&roots);
                    continue;
                }
                let current = portable_snapshot(&roots);
                let changed = roots
                    .iter()
                    .filter(|root| previous.get(*root) != current.get(*root))
                    .cloned()
                    .collect::<Vec<_>>();
                previous = current;
                if !changed.is_empty() && sender.send(changed).is_err() {
                    break;
                }
            }
        });
        Ok(SkillChangeWatch {
            receiver,
            stop,
            thread: Some(thread),
        })
    }
}

/// Path → (size, mtime) stamp so content-only edits are observed.
#[cfg(not(windows))]
type PortableSnapshot = std::collections::BTreeMap<
    PathBuf,
    std::collections::BTreeMap<PathBuf, (u64, Option<std::time::SystemTime>)>,
>;

#[cfg(not(windows))]
fn portable_snapshot(roots: &[PathBuf]) -> PortableSnapshot {
    fn collect(
        path: &std::path::Path,
        output: &mut std::collections::BTreeMap<PathBuf, (u64, Option<std::time::SystemTime>)>,
    ) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let stamp = std::fs::metadata(&path)
                .ok()
                .map(|metadata| (metadata.len(), metadata.modified().ok()))
                .unwrap_or_default();
            output.insert(path.clone(), stamp);
            if path.is_dir() {
                collect(&path, output);
            }
        }
    }
    roots
        .iter()
        .map(|root| {
            let mut entries = std::collections::BTreeMap::new();
            collect(root, &mut entries);
            (root.clone(), entries)
        })
        .collect()
}

#[cfg(windows)]
struct NativeRegistration {
    root: PathBuf,
    handle: usize,
}

#[cfg(windows)]
impl Drop for NativeRegistration {
    fn drop(&mut self) {
        use windows_sys::Win32::Storage::FileSystem::FindCloseChangeNotification;
        // SAFETY: the registration owns this notification handle.
        unsafe {
            FindCloseChangeNotification(self.handle as windows_sys::Win32::Foundation::HANDLE)
        };
    }
}

#[cfg(windows)]
fn open_registrations(roots: &[PathBuf]) -> Vec<NativeRegistration> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        Storage::FileSystem::{
            FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
            FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FindFirstChangeNotificationW,
        },
    };

    // WaitForMultipleObjects accepts at most 64 handles; reserve one for shutdown.
    roots
        .iter()
        .take(63)
        .filter_map(|root| {
            let wide = root
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            // SAFETY: wide is a NUL-terminated path buffer valid for this call.
            let handle = unsafe {
                FindFirstChangeNotificationW(
                    wide.as_ptr(),
                    1,
                    FILE_NOTIFY_CHANGE_FILE_NAME
                        | FILE_NOTIFY_CHANGE_DIR_NAME
                        | FILE_NOTIFY_CHANGE_LAST_WRITE
                        | FILE_NOTIFY_CHANGE_SIZE
                        | FILE_NOTIFY_CHANGE_CREATION,
                )
            };
            (handle != INVALID_HANDLE_VALUE).then(|| NativeRegistration {
                root: root.clone(),
                handle: handle as usize,
            })
        })
        .collect()
}

#[cfg(windows)]
fn close_registrations(registrations: Vec<NativeRegistration>) {
    drop(registrations);
}

#[cfg(windows)]
fn watch_loop(
    host: SkillHost,
    mut registrations: Vec<NativeRegistration>,
    stop_value: usize,
    sender: tokio::sync::mpsc::UnboundedSender<Vec<PathBuf>>,
) {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Storage::FileSystem::FindNextChangeNotification,
        System::Threading::WaitForMultipleObjects,
    };

    let stop_event = stop_value as windows_sys::Win32::Foundation::HANDLE;
    loop {
        let mut handles = Vec::with_capacity(registrations.len() + 1);
        handles.push(stop_event);
        handles.extend(
            registrations
                .iter()
                .map(|entry| entry.handle as windows_sys::Win32::Foundation::HANDLE),
        );
        // A bounded timeout lets newly discovered Repo roots join the registration set.
        // SAFETY: handles stay alive until WaitForMultipleObjects returns.
        let result =
            unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, 500) };
        if result == WAIT_OBJECT_0 || result == WAIT_FAILED {
            break;
        }
        if result != WAIT_TIMEOUT {
            let index = result.saturating_sub(WAIT_OBJECT_0 + 1) as usize;
            let Some(registration) = registrations.get(index) else {
                break;
            };
            let _ = sender.send(vec![registration.root.clone()]);
            // A deleted/replaced root invalidates the native handle and is rebuilt below.
            if unsafe {
                FindNextChangeNotification(
                    registration.handle as windows_sys::Win32::Foundation::HANDLE,
                )
            } == 0
            {
                let roots = host.watch_root_snapshot();
                let replacements = open_registrations(&roots);
                close_registrations(std::mem::replace(&mut registrations, replacements));
            }
        }

        let roots = host.watch_root_snapshot();
        let current = registrations
            .iter()
            .map(|entry| entry.root.clone())
            .collect::<Vec<_>>();
        if roots != current {
            let replacements = open_registrations(&roots);
            close_registrations(std::mem::replace(&mut registrations, replacements));
        }
    }
    close_registrations(registrations);
    // SAFETY: the watcher thread owns and closes stop_event exactly once.
    unsafe { CloseHandle(stop_event) };
}
