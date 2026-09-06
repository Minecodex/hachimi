//! Host environment snapshots and refreshable system-tool discovery.
//!
//! Discovery is intentionally checkout-agnostic. A resolved executable is
//! canonicalized once and handed to higher layers as an immutable lease so a
//! restricted worker never resolves a program name from its working tree.

use std::{
    collections::{BTreeSet, HashSet},
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hachimi_process_policy::{ProcessPolicy, tokio_command};
use hachimi_protocol::{
    ShellKind, ShellLaunchSpec, SystemRuntimeSnapshot, SystemToolCapability, SystemToolId,
    SystemToolSource, SystemToolState, SystemToolStatus,
};
use parking_lot::RwLock;
use tempfile::TempDir;
use thiserror::Error;
#[cfg(target_os = "macos")]
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

#[cfg(target_os = "macos")]
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PROBE_OUTPUT: usize = 1024 * 1024;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{message}")]
pub struct SystemRuntimeError {
    pub code: String,
    pub message: String,
}

impl SystemRuntimeError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified_ns: u128,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume_serial_number: u32,
    #[cfg(windows)]
    file_index: u64,
}

impl FileIdentity {
    fn read(path: &Path) -> Result<Self, SystemRuntimeError> {
        let metadata = std::fs::metadata(path).map_err(|error| {
            SystemRuntimeError::new(
                "system_tool_changed",
                format!(
                    "system tool metadata is unavailable at {}: {error}",
                    path.display()
                ),
            )
        })?;
        if !metadata.is_file() {
            return Err(SystemRuntimeError::new(
                "system_tool_changed",
                format!("system tool is no longer a file: {}", path.display()),
            ));
        }
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_nanos());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            Ok(Self {
                len: metadata.len(),
                modified_ns,
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(windows)]
        {
            let (volume_serial_number, file_index) = windows_file_identity(path)?;
            Ok(Self {
                len: metadata.len(),
                modified_ns,
                volume_serial_number,
                file_index,
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            Ok(Self {
                len: metadata.len(),
                modified_ns,
            })
        }
    }
}

#[cfg(windows)]
fn windows_file_identity(path: &Path) -> Result<(u32, u64), SystemRuntimeError> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_READ_ATTRIBUTES,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
            OPEN_EXISTING,
        },
    };

    let path = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(SystemRuntimeError::new(
            "system_tool_changed",
            format!(
                "system tool identity is unavailable: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    let succeeded = unsafe { GetFileInformationByHandle(handle, &mut information) } != 0;
    let identity_error = (!succeeded).then(std::io::Error::last_os_error);
    unsafe {
        CloseHandle(handle);
    }
    if let Some(error) = identity_error {
        return Err(SystemRuntimeError::new(
            "system_tool_changed",
            format!("system tool identity could not be read: {error}"),
        ));
    }
    let file_index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok((information.dwVolumeSerialNumber, file_index))
}

#[derive(Debug, Clone)]
pub struct GitRuntimeLease {
    path: PathBuf,
    revision: u64,
    identity: FileIdentity,
    capabilities: BTreeSet<SystemToolCapability>,
}

impl GitRuntimeLease {
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn capabilities(&self) -> &BTreeSet<SystemToolCapability> {
        &self.capabilities
    }

    pub fn verify(&self) -> Result<(), SystemRuntimeError> {
        let canonical = self.path.canonicalize().map_err(|error| {
            SystemRuntimeError::new(
                "system_git_changed",
                format!("resolved Git is no longer available: {error}"),
            )
        })?;
        let identity = FileIdentity::read(&canonical)
            .map_err(|error| SystemRuntimeError::new("system_git_changed", error.message))?;
        if canonical != self.path || identity != self.identity {
            return Err(SystemRuntimeError::new(
                "system_git_changed",
                "resolved Git changed after system runtime discovery",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct ShellRuntime {
    path: PathBuf,
    kind: ShellKind,
    source: SystemToolSource,
    version: Option<String>,
    capabilities: BTreeSet<SystemToolCapability>,
    identity: FileIdentity,
}

#[derive(Debug, Clone)]
struct ResolvedGit {
    path: PathBuf,
    version: String,
    source: SystemToolSource,
    capabilities: BTreeSet<SystemToolCapability>,
    identity: FileIdentity,
}

#[derive(Debug, Clone)]
struct RuntimeState {
    snapshot: SystemRuntimeSnapshot,
    environment: HostEnvironmentSnapshot,
    git: Option<ResolvedGit>,
    shell: Option<ShellRuntime>,
}

#[derive(Debug, Clone, Default)]
pub struct HostEnvironmentSnapshot {
    #[cfg_attr(not(unix), allow(dead_code))]
    shell_path: Option<PathBuf>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    home_dir: Option<PathBuf>,
    #[cfg_attr(not(windows), allow(dead_code))]
    system_root: Option<PathBuf>,
    temporary_dir: Option<PathBuf>,
    #[cfg_attr(not(windows), allow(dead_code))]
    pathext: Option<OsString>,
    locale: Option<OsString>,
    shell_path_entries: Vec<PathBuf>,
    process_path_entries: Vec<PathBuf>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    source: SystemToolSource,
}

#[derive(Clone, Debug)]
pub struct SystemRuntimeManager {
    state: Arc<RwLock<RuntimeState>>,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
}

impl Default for SystemRuntimeManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-wide handle used at crate boundaries that cannot own application
/// state directly. The handle is stable, while the contained snapshot remains
/// refreshable and revisioned.
#[must_use]
pub fn system_runtime_manager() -> SystemRuntimeManager {
    static MANAGER: OnceLock<SystemRuntimeManager> = OnceLock::new();
    MANAGER.get_or_init(SystemRuntimeManager::new).clone()
}

impl SystemRuntimeManager {
    #[must_use]
    pub fn new() -> Self {
        let observed_at_ms = now_ms();
        let unavailable = |tool| SystemToolStatus {
            tool,
            state: SystemToolState::Unavailable,
            executable_path: None,
            version: None,
            source: None,
            capabilities: Vec::new(),
            error_code: Some("system_runtime_starting".into()),
            observed_at_ms,
        };
        Self {
            state: Arc::new(RwLock::new(RuntimeState {
                snapshot: SystemRuntimeSnapshot {
                    revision: 0,
                    tools: vec![
                        unavailable(SystemToolId::Git),
                        unavailable(SystemToolId::DefaultShell),
                    ],
                    warnings: Vec::new(),
                },
                environment: HostEnvironmentSnapshot::default(),
                git: None,
                shell: None,
            })),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> SystemRuntimeSnapshot {
        self.state.read().snapshot.clone()
    }

    pub async fn refresh(&self) -> SystemRuntimeSnapshot {
        let _refresh = self.refresh_lock.lock().await;
        let started = Instant::now();
        let environment = capture_host_environment().await;
        let (git_result, shell_result) = tokio::join!(
            discover_git(&environment),
            discover_default_shell(&environment)
        );
        let revision = self.state.read().snapshot.revision.saturating_add(1);
        let observed_at_ms = now_ms();
        let mut warnings = environment.warnings.clone();
        warnings.extend(git_result.warnings.iter().cloned());
        warnings.sort();
        warnings.dedup();
        let (git, git_status) = git_result.into_status(observed_at_ms);
        let (shell, shell_status) = shell_result.into_status(observed_at_ms);
        let snapshot = SystemRuntimeSnapshot {
            revision,
            tools: vec![git_status, shell_status],
            warnings,
        };
        tracing::info!(
            revision,
            elapsed_ms = started.elapsed().as_millis(),
            git_state = ?snapshot.tools[0].state,
            git_source = ?snapshot.tools[0].source,
            git_version = ?snapshot.tools[0].version,
            git_capabilities = ?snapshot.tools[0].capabilities,
            git_error_code = ?snapshot.tools[0].error_code,
            shell_state = ?snapshot.tools[1].state,
            shell_source = ?snapshot.tools[1].source,
            shell_version = ?snapshot.tools[1].version,
            shell_capabilities = ?snapshot.tools[1].capabilities,
            shell_error_code = ?snapshot.tools[1].error_code,
            warnings = snapshot.warnings.len(),
            "system runtime discovery completed"
        );
        *self.state.write() = RuntimeState {
            snapshot: snapshot.clone(),
            environment,
            git,
            shell,
        };
        snapshot
    }

    pub fn require_git(
        &self,
        capability: SystemToolCapability,
    ) -> Result<GitRuntimeLease, SystemRuntimeError> {
        let state = self.state.read();
        let git = state.git.as_ref().ok_or_else(|| {
            let code = state
                .snapshot
                .tools
                .iter()
                .find(|tool| tool.tool == SystemToolId::Git)
                .and_then(|tool| tool.error_code.clone())
                .unwrap_or_else(|| "system_git_missing".into());
            SystemRuntimeError::new(code, "system Git is unavailable")
        })?;
        if !git.capabilities.contains(&capability) {
            return Err(SystemRuntimeError::new(
                "system_git_capability_missing",
                format!("resolved Git does not provide {capability:?}"),
            ));
        }
        let lease = GitRuntimeLease {
            path: git.path.clone(),
            revision: state.snapshot.revision,
            identity: git.identity.clone(),
            capabilities: git.capabilities.clone(),
        };
        lease.verify()?;
        Ok(lease)
    }

    pub fn default_shell(&self) -> Result<ShellLaunchSpec, SystemRuntimeError> {
        let state = self.state.read();
        let shell = state.shell.as_ref().ok_or_else(|| {
            SystemRuntimeError::new("system_shell_missing", "default shell is unavailable")
        })?;
        if !shell
            .capabilities
            .contains(&SystemToolCapability::ShellInteractive)
        {
            return Err(SystemRuntimeError::new(
                "system_shell_capability_missing",
                "default shell does not provide an interactive PTY",
            ));
        }
        let canonical = shell.path.canonicalize().map_err(|error| {
            SystemRuntimeError::new(
                "system_shell_changed",
                format!("default shell is no longer available: {error}"),
            )
        })?;
        let identity = FileIdentity::read(&canonical)
            .map_err(|error| SystemRuntimeError::new("system_shell_changed", error.message))?;
        if canonical != shell.path || identity != shell.identity {
            return Err(SystemRuntimeError::new(
                "system_shell_changed",
                "default shell changed after system runtime discovery",
            ));
        }
        let (interactive_args, command_args) = match shell.kind {
            ShellKind::Posix => (vec!["-l".into()], vec!["-lc".into()]),
            ShellKind::PowerShell => (
                vec!["-NoLogo".into()],
                vec!["-NoLogo".into(), "-NoProfile".into(), "-Command".into()],
            ),
            ShellKind::CommandPrompt => (
                vec!["/D".into()],
                vec!["/D".into(), "/S".into(), "/C".into()],
            ),
        };
        Ok(ShellLaunchSpec {
            executable_path: shell.path.to_string_lossy().into_owned(),
            kind: shell.kind,
            interactive_args,
            command_args,
            runtime_revision: state.snapshot.revision,
        })
    }

    /// Returns absolute, normalized executable candidates using the same host
    /// environment snapshot as Git discovery. Callers supply OS-specific
    /// candidates (for example browser App Paths) in their preferred order.
    #[must_use]
    pub fn executable_candidates(
        &self,
        names: &[&str],
        preferred: impl IntoIterator<Item = PathBuf>,
    ) -> Vec<PathBuf> {
        let state = self.state.read();
        let mut candidates = preferred
            .into_iter()
            .filter(|path| path.is_absolute())
            .map(|path| path.canonicalize().unwrap_or(path))
            .collect::<Vec<_>>();
        candidates.extend(
            path_candidates(
                &state.environment.shell_path_entries,
                host_environment_path_source(),
                names,
            )
            .into_iter()
            .map(|candidate| candidate.path),
        );
        candidates.extend(
            path_candidates(
                &state.environment.process_path_entries,
                SystemToolSource::ProcessEnvironment,
                names,
            )
            .into_iter()
            .map(|candidate| candidate.path),
        );
        let mut seen = HashSet::new();
        candidates
            .into_iter()
            .map(|path| path.canonicalize().unwrap_or(path))
            .filter(|path| seen.insert(path_key(path)))
            .collect()
    }

    #[must_use]
    pub fn aggregate_error_code(&self) -> Option<String> {
        self.snapshot()
            .tools
            .into_iter()
            .find(|tool| tool.state != SystemToolState::Ready)
            .and_then(|tool| tool.error_code)
    }
}

#[derive(Debug)]
struct GitDiscovery {
    resolved: Option<ResolvedGit>,
    status: SystemToolState,
    error_code: Option<String>,
    warnings: Vec<String>,
}

impl GitDiscovery {
    fn into_status(self, observed_at_ms: i64) -> (Option<ResolvedGit>, SystemToolStatus) {
        let status = SystemToolStatus {
            tool: SystemToolId::Git,
            state: self.status,
            executable_path: self
                .resolved
                .as_ref()
                .map(|git| git.path.to_string_lossy().into_owned()),
            version: self.resolved.as_ref().map(|git| git.version.clone()),
            source: self.resolved.as_ref().map(|git| git.source),
            capabilities: self
                .resolved
                .as_ref()
                .map_or_else(Vec::new, |git| git.capabilities.iter().copied().collect()),
            error_code: self.error_code,
            observed_at_ms,
        };
        (self.resolved, status)
    }
}

#[derive(Debug)]
struct ShellDiscovery {
    resolved: Option<ShellRuntime>,
    status: SystemToolState,
    error_code: Option<String>,
}

impl ShellDiscovery {
    fn into_status(self, observed_at_ms: i64) -> (Option<ShellRuntime>, SystemToolStatus) {
        let status = SystemToolStatus {
            tool: SystemToolId::DefaultShell,
            state: self.status,
            executable_path: self
                .resolved
                .as_ref()
                .map(|shell| shell.path.to_string_lossy().into_owned()),
            version: self
                .resolved
                .as_ref()
                .and_then(|shell| shell.version.clone()),
            source: self.resolved.as_ref().map(|shell| shell.source),
            capabilities: self.resolved.as_ref().map_or_else(Vec::new, |shell| {
                shell.capabilities.iter().copied().collect()
            }),
            error_code: self.error_code,
            observed_at_ms,
        };
        (self.resolved, status)
    }
}

async fn capture_host_environment() -> HostEnvironmentSnapshot {
    let process_path_entries =
        std::env::var_os("PATH").map_or_else(Vec::new, |value| sanitized_path_entries(&value));
    let mut snapshot = HostEnvironmentSnapshot {
        shell_path: {
            #[cfg(unix)]
            {
                login_shell()
            }
            #[cfg(not(unix))]
            {
                None
            }
        },
        home_dir: host_home_dir(),
        system_root: absolute_environment_path(&["SystemRoot", "SYSTEMROOT"]),
        temporary_dir: absolute_environment_path(&["TMPDIR", "TEMP", "TMP"]),
        pathext: std::env::var_os("PATHEXT"),
        locale: std::env::var_os("LC_ALL").or_else(|| std::env::var_os("LANG")),
        shell_path_entries: Vec::new(),
        process_path_entries,
        warnings: Vec::new(),
    };

    #[cfg(target_os = "macos")]
    if let Some(shell) = snapshot.shell_path.as_deref() {
        match capture_login_shell_path(shell, snapshot.home_dir.as_deref()).await {
            Ok(path) => snapshot.shell_path_entries = sanitized_path_entries(&path),
            Err(error) => snapshot.warnings.push(error.code),
        }
    }

    #[cfg(windows)]
    {
        snapshot.shell_path_entries = windows_registry_path_entries();
    }

    snapshot
}

fn absolute_environment_path(names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .find_map(std::env::var_os)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.canonicalize().unwrap_or(path))
}

fn host_home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    if let Some(home) = unix_account_paths().0 {
        return Some(home);
    }
    absolute_environment_path(&["HOME", "USERPROFILE"])
}

#[cfg(target_os = "macos")]
async fn capture_login_shell_path(
    shell: &Path,
    home_dir: Option<&Path>,
) -> Result<OsString, SystemRuntimeError> {
    capture_login_shell_path_with_timeout(shell, home_dir, SNAPSHOT_TIMEOUT).await
}

#[cfg(target_os = "macos")]
async fn capture_login_shell_path_with_timeout(
    shell: &Path,
    home_dir: Option<&Path>,
    timeout: Duration,
) -> Result<OsString, SystemRuntimeError> {
    let mut command = tokio_command(shell, ProcessPolicy::HiddenCaptured);
    command
        .args(["-l", "-c", "printf '\\0HACHIMI_PATH=%s\\0' \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(home) = home_dir {
        command.current_dir(home);
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|error| {
        SystemRuntimeError::new(
            "host_environment_snapshot_failed",
            format!("login shell could not start: {error}"),
        )
    })?;
    #[cfg(unix)]
    let pid = child.id();
    let stdout = child.stdout.take().ok_or_else(|| {
        SystemRuntimeError::new(
            "host_environment_snapshot_failed",
            "login shell snapshot stdout was unavailable",
        )
    })?;
    let capture = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout
            .take((MAX_PROBE_OUTPUT + 1) as u64)
            .read_to_end(&mut bytes)
            .await?;
        Ok::<_, std::io::Error>(bytes)
    });
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(result) => result.map_err(|error| {
            SystemRuntimeError::new(
                "host_environment_snapshot_failed",
                format!("login shell snapshot failed: {error}"),
            )
        })?,
        Err(_) => {
            if let Some(pid) = pid {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            capture.abort();
            let _ = capture.await;
            return Err(SystemRuntimeError::new(
                "host_environment_snapshot_timeout",
                "login shell snapshot timed out",
            ));
        }
    };
    let stdout = capture
        .await
        .map_err(|error| {
            SystemRuntimeError::new(
                "host_environment_snapshot_failed",
                format!("login shell snapshot reader failed: {error}"),
            )
        })?
        .map_err(|error| {
            SystemRuntimeError::new(
                "host_environment_snapshot_failed",
                format!("login shell snapshot output failed: {error}"),
            )
        })?;
    if !status.success() || stdout.len() > MAX_PROBE_OUTPUT {
        return Err(SystemRuntimeError::new(
            "host_environment_snapshot_failed",
            "login shell snapshot was unsuccessful or exceeded its output limit",
        ));
    }
    stdout
        .split(|byte| *byte == 0)
        .find_map(|entry| entry.strip_prefix(b"HACHIMI_PATH="))
        .map(bytes_to_os_string)
        .ok_or_else(|| {
            SystemRuntimeError::new(
                "host_environment_snapshot_failed",
                "login shell snapshot did not contain PATH",
            )
        })
}

#[cfg(target_os = "macos")]
fn bytes_to_os_string(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt as _;
    OsString::from_vec(bytes.to_vec())
}

fn sanitized_path_entries(value: &OsStr) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let current_dir = std::env::current_dir()
        .ok()
        .and_then(|path| path.canonicalize().ok())
        .map(|path| path_key(&path));
    std::env::split_paths(value)
        .filter(|path| path.is_absolute())
        .filter_map(|path| {
            let path = path.canonicalize().unwrap_or(path);
            let key = path_key(&path);
            (current_dir.as_ref() != Some(&key) && seen.insert(key)).then_some(path)
        })
        .collect()
}

fn path_key(path: &Path) -> String {
    let value = path.to_string_lossy();
    if cfg!(windows) {
        value.to_ascii_lowercase()
    } else {
        value.into_owned()
    }
}

const fn host_environment_path_source() -> SystemToolSource {
    if cfg!(windows) {
        SystemToolSource::OsRegistry
    } else {
        SystemToolSource::ShellSnapshot
    }
}

#[cfg(unix)]
fn login_shell() -> Option<PathBuf> {
    if let Some(shell) = unix_account_paths().1
        && shell.is_absolute()
        && shell.is_file()
    {
        return Some(shell);
    }
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_file())
        .or_else(|| cfg!(target_os = "macos").then(|| PathBuf::from("/bin/zsh")))
}

#[cfg(unix)]
fn unix_account_paths() -> (Option<PathBuf>, Option<PathBuf>) {
    unsafe {
        let record = libc::getpwuid(libc::geteuid());
        if record.is_null() {
            return (None, None);
        }
        let home = (!(*record).pw_dir.is_null()).then(|| {
            std::ffi::CStr::from_ptr((*record).pw_dir)
                .to_string_lossy()
                .into_owned()
                .into()
        });
        let shell = (!(*record).pw_shell.is_null()).then(|| {
            std::ffi::CStr::from_ptr((*record).pw_shell)
                .to_string_lossy()
                .into_owned()
                .into()
        });
        (home, shell)
    }
}

async fn discover_git(environment: &HostEnvironmentSnapshot) -> GitDiscovery {
    #[cfg(debug_assertions)]
    if debug_git_gate_forces_missing() {
        return GitDiscovery {
            resolved: None,
            status: SystemToolState::Unavailable,
            error_code: Some("system_git_missing".into()),
            warnings: Vec::new(),
        };
    }
    let mut candidates = Vec::new();
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("HACHIMI_GIT_EXECUTABLE")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        candidates.push(Candidate {
            path,
            source: SystemToolSource::TestOverride,
        });
    }
    let git_names = git_names(environment);
    candidates.extend(path_candidates(
        &environment.shell_path_entries,
        host_environment_path_source(),
        &git_names,
    ));
    candidates.extend(path_candidates(
        &environment.process_path_entries,
        SystemToolSource::ProcessEnvironment,
        &git_names,
    ));
    candidates.extend(platform_git_candidates());
    let candidates = dedupe_candidates(candidates);
    tracing::debug!(
        tool = "git",
        candidate_count = candidates.len(),
        "probing system Git candidates"
    );
    let mut best: Option<ResolvedGit> = None;
    let mut saw_timeout = false;
    let mut saw_probe_failure = false;
    let mut warnings = Vec::new();
    for candidate in candidates {
        if !candidate.path.is_file() {
            continue;
        }
        let started = Instant::now();
        match probe_git(
            &candidate.path,
            candidate.source,
            environment.temporary_dir.as_deref(),
        )
        .await
        {
            Ok(git) => {
                tracing::debug!(
                    tool = "git",
                    path = %git.path.display(),
                    source = ?git.source,
                    version = %git.version,
                    elapsed_ms = started.elapsed().as_millis(),
                    capabilities = ?git.capabilities,
                    "system Git candidate accepted"
                );
                let complete = [
                    SystemToolCapability::GitInspect,
                    SystemToolCapability::GitLocalMutation,
                    SystemToolCapability::GitWorktree,
                ]
                .iter()
                .all(|capability| git.capabilities.contains(capability));
                if complete {
                    return GitDiscovery {
                        resolved: Some(git),
                        status: SystemToolState::Ready,
                        error_code: None,
                        warnings,
                    };
                }
                if best
                    .as_ref()
                    .is_none_or(|current| git.capabilities.len() > current.capabilities.len())
                {
                    best = Some(git);
                }
            }
            Err(error) => {
                saw_timeout |= error.code == "system_git_probe_timeout";
                saw_probe_failure = true;
                warnings.push(error.code.clone());
                tracing::debug!(
                    tool = "git",
                    path = %candidate.path.display(),
                    source = ?candidate.source,
                    elapsed_ms = started.elapsed().as_millis(),
                    code = %error.code,
                    message = %error.message,
                    "system Git candidate rejected"
                );
            }
        }
    }
    if let Some(git) = best {
        return GitDiscovery {
            resolved: Some(git),
            status: SystemToolState::Degraded,
            error_code: Some("system_git_capability_missing".into()),
            warnings,
        };
    }
    GitDiscovery {
        resolved: None,
        status: SystemToolState::Unavailable,
        error_code: Some(
            if saw_timeout {
                "system_git_probe_timeout"
            } else if saw_probe_failure {
                "system_git_probe_failed"
            } else {
                "system_git_missing"
            }
            .into(),
        ),
        warnings,
    }
}

#[cfg(debug_assertions)]
fn debug_git_gate_forces_missing() -> bool {
    std::env::var_os("HACHIMI_SYSTEM_RUNTIME_TEST_GIT_GATE")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|value| value.trim() == "missing")
}

fn git_names(environment: &HostEnvironmentSnapshot) -> Vec<OsString> {
    #[cfg(windows)]
    {
        let mut names = environment
            .pathext
            .as_deref()
            .map(|value| value.to_string_lossy())
            .into_iter()
            .flat_map(|value| {
                value
                    .split(';')
                    .filter(|extension| {
                        extension.eq_ignore_ascii_case(".exe")
                            || extension.eq_ignore_ascii_case(".com")
                    })
                    .map(|extension| OsString::from(format!("git{extension}")))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if names.is_empty() {
            names.push("git.exe".into());
        }
        names
    }
    #[cfg(not(windows))]
    {
        let _ = environment;
        vec!["git".into()]
    }
}

fn path_candidates<N: AsRef<OsStr>>(
    entries: &[PathBuf],
    source: SystemToolSource,
    names: &[N],
) -> Vec<Candidate> {
    entries
        .iter()
        .flat_map(|root| names.iter().map(move |name| root.join(name.as_ref())))
        .map(|path| Candidate { path, source })
        .collect()
}

fn dedupe_candidates(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| candidate.path.is_absolute())
        .map(|mut candidate| {
            candidate.path = candidate.path.canonicalize().unwrap_or(candidate.path);
            candidate
        })
        .filter(|candidate| seen.insert(path_key(&candidate.path)))
        .collect()
}

#[cfg(target_os = "macos")]
fn platform_git_candidates() -> Vec<Candidate> {
    [
        "/opt/homebrew/bin/git",
        "/usr/local/bin/git",
        "/opt/local/bin/git",
        "/usr/bin/git",
    ]
    .into_iter()
    .map(|path| Candidate {
        path: path.into(),
        source: SystemToolSource::WellKnown,
    })
    .collect()
}

#[cfg(windows)]
fn platform_git_candidates() -> Vec<Candidate> {
    let mut candidates = windows_git_app_paths()
        .into_iter()
        .map(|path| Candidate {
            path,
            source: SystemToolSource::OsRegistry,
        })
        .collect::<Vec<_>>();
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        let Some(root) = std::env::var_os(variable) else {
            continue;
        };
        let root = PathBuf::from(root);
        let path = if variable == "LOCALAPPDATA" {
            root.join("Programs/Git/cmd/git.exe")
        } else {
            root.join("Git/cmd/git.exe")
        };
        candidates.push(Candidate {
            path,
            source: SystemToolSource::WellKnown,
        });
    }
    candidates
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_git_candidates() -> Vec<Candidate> {
    vec![Candidate {
        path: "/usr/bin/git".into(),
        source: SystemToolSource::WellKnown,
    }]
}

async fn probe_git(
    candidate: &Path,
    source: SystemToolSource,
    temporary_root: Option<&Path>,
) -> Result<ResolvedGit, SystemRuntimeError> {
    let root = temporary_root
        .map_or_else(TempDir::new, TempDir::new_in)
        .map_err(|error| {
            SystemRuntimeError::new(
                "system_git_probe_failed",
                format!("Git probe directory could not be created: {error}"),
            )
        })?;
    std::fs::write(root.path().join("global.gitconfig"), b"").map_err(|error| {
        SystemRuntimeError::new(
            "system_git_probe_failed",
            format!("Git probe config could not be created: {error}"),
        )
    })?;
    validate_executable(candidate, "Git")?;
    let canonical = candidate.canonicalize().map_err(|error| {
        SystemRuntimeError::new(
            "system_git_probe_failed",
            format!("Git path could not be canonicalized: {error}"),
        )
    })?;
    let path = resolve_effective_git_executable(canonical, Some(root.path())).await?;
    validate_executable(&path, "Git")?;
    let version_output = run_git_probe(&path, Some(root.path()), ["--version"], None, &[]).await?;
    let version = String::from_utf8_lossy(&version_output.stdout)
        .trim()
        .strip_prefix("git version ")
        .unwrap_or_default()
        .trim()
        .to_owned();
    if version.is_empty() {
        return Err(SystemRuntimeError::new(
            "system_git_probe_failed",
            "Git returned an unrecognized version",
        ));
    }

    let mut capabilities = BTreeSet::new();
    if git_probe_succeeds(&path, root.path(), ["init"]).await
        && git_probe_succeeds(&path, root.path(), ["rev-parse", "--git-dir"]).await
        && git_probe_succeeds(&path, root.path(), ["symbolic-ref", "--quiet", "HEAD"]).await
    {
        capabilities.insert(SystemToolCapability::GitInspect);
    }
    if capabilities.contains(&SystemToolCapability::GitInspect) {
        let tree = run_git_probe(&path, Some(root.path()), ["mktree"], Some(&[]), &[])
            .await
            .ok()
            .and_then(|output| output.status.success().then_some(output.stdout))
            .map(|stdout| String::from_utf8_lossy(&stdout).trim().to_owned())
            .filter(|value| !value.is_empty());
        let commit = if let Some(tree) = tree {
            run_git_probe(
                &path,
                Some(root.path()),
                ["commit-tree", tree.as_str(), "-m", "runtime probe"],
                None,
                &[
                    ("GIT_AUTHOR_NAME", "Hachimi Runtime Probe"),
                    ("GIT_AUTHOR_EMAIL", "runtime-probe@hachimi.invalid"),
                    ("GIT_COMMITTER_NAME", "Hachimi Runtime Probe"),
                    ("GIT_COMMITTER_EMAIL", "runtime-probe@hachimi.invalid"),
                ],
            )
            .await
            .ok()
            .and_then(|output| output.status.success().then_some(output.stdout))
            .map(|stdout| String::from_utf8_lossy(&stdout).trim().to_owned())
            .filter(|value| !value.is_empty())
        } else {
            None
        };
        if let Some(commit) = commit
            && git_probe_succeeds(
                &path,
                root.path(),
                ["symbolic-ref", "HEAD", "refs/heads/main"],
            )
            .await
            && git_probe_succeeds(
                &path,
                root.path(),
                ["update-ref", "refs/heads/main", commit.as_str()],
            )
            .await
        {
            capabilities.insert(SystemToolCapability::GitLocalMutation);
        }
    }
    if capabilities.contains(&SystemToolCapability::GitLocalMutation) {
        let probe_file = root.path().join("runtime-probe.txt");
        let linked = root.path().join("linked-worktree");
        let worktree_ok = std::fs::write(&probe_file, b"probe\n").is_ok()
            && git_probe_succeeds(&path, root.path(), ["add", "runtime-probe.txt"]).await
            && git_probe_succeeds(
                &path,
                root.path(),
                ["restore", "--staged", "--", "runtime-probe.txt"],
            )
            .await
            && git_probe_succeeds(&path, root.path(), ["switch", "--detach", "HEAD"]).await
            && git_probe_succeeds(
                &path,
                root.path(),
                [
                    "worktree",
                    "add",
                    "--detach",
                    linked.to_string_lossy().as_ref(),
                    "HEAD",
                ],
            )
            .await
            && git_probe_succeeds(
                &path,
                root.path(),
                [
                    "worktree",
                    "remove",
                    "--force",
                    linked.to_string_lossy().as_ref(),
                ],
            )
            .await;
        if worktree_ok {
            capabilities.insert(SystemToolCapability::GitWorktree);
        }
    }
    if !capabilities.contains(&SystemToolCapability::GitInspect) {
        return Err(SystemRuntimeError::new(
            "system_git_probe_failed",
            "Git could not inspect an isolated probe repository",
        ));
    }
    let identity = FileIdentity::read(&path)
        .map_err(|error| SystemRuntimeError::new("system_git_probe_failed", error.message))?;
    Ok(ResolvedGit {
        path,
        version,
        source,
        capabilities,
        identity,
    })
}

#[cfg(target_os = "macos")]
async fn resolve_effective_git_executable(
    candidate: PathBuf,
    probe_cwd: Option<&Path>,
) -> Result<PathBuf, SystemRuntimeError> {
    if candidate != Path::new("/usr/bin/git") {
        return Ok(candidate);
    }
    let mut command = tokio_command("/usr/bin/xcrun", ProcessPolicy::HiddenCaptured);
    command
        .args(["--find", "git"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", probe_cwd.unwrap_or_else(|| Path::new("/")))
        .current_dir(probe_cwd.unwrap_or_else(|| Path::new("/")))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(PROBE_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            SystemRuntimeError::new(
                "system_git_probe_timeout",
                "Apple Git toolchain resolution timed out",
            )
        })?
        .map_err(|error| {
            SystemRuntimeError::new(
                "system_git_probe_failed",
                format!("Apple Git toolchain resolution could not start: {error}"),
            )
        })?;
    if !output.status.success()
        || output.stdout.len() > MAX_PROBE_OUTPUT
        || output.stderr.len() > MAX_PROBE_OUTPUT
    {
        return Err(SystemRuntimeError::new(
            "system_git_probe_failed",
            "Apple Git toolchain resolution failed",
        ));
    }
    let resolved = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if !resolved.is_absolute() || resolved == candidate {
        return Err(SystemRuntimeError::new(
            "system_git_probe_failed",
            "Apple Git toolchain resolver returned no concrete executable",
        ));
    }
    resolved.canonicalize().map_err(|error| {
        SystemRuntimeError::new(
            "system_git_probe_failed",
            format!("Apple Git executable could not be canonicalized: {error}"),
        )
    })
}

#[cfg(not(target_os = "macos"))]
async fn resolve_effective_git_executable(
    candidate: PathBuf,
    _probe_cwd: Option<&Path>,
) -> Result<PathBuf, SystemRuntimeError> {
    Ok(candidate)
}

async fn git_probe_succeeds<const N: usize>(git: &Path, cwd: &Path, args: [&str; N]) -> bool {
    run_git_probe(git, Some(cwd), args, None, &[])
        .await
        .is_ok_and(|output| output.status.success())
}

async fn run_git_probe<I, S>(
    git: &Path,
    cwd: Option<&Path>,
    args: I,
    input: Option<&[u8]>,
    environment: &[(&str, &str)],
) -> Result<std::process::Output, SystemRuntimeError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = tokio_command(git, ProcessPolicy::HiddenCaptured);
    command
        .env_clear()
        .arg("-c")
        .arg(format!(
            "core.hooksPath={}",
            if cfg!(windows) { "NUL" } else { "/dev/null" }
        ))
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GCM_INTERACTIVE", "Never")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("LC_ALL", "C");
    #[cfg(windows)]
    for name in ["SystemRoot", "WINDIR", "ComSpec", "PATHEXT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    #[cfg(not(windows))]
    command.env("PATH", "/usr/bin:/bin");
    if let Some(cwd) = cwd {
        command
            .current_dir(cwd)
            .env("HOME", cwd)
            .env("USERPROFILE", cwd)
            .env("TMPDIR", cwd)
            .env("TEMP", cwd)
            .env("TMP", cwd)
            .env("GIT_CONFIG_GLOBAL", cwd.join("global.gitconfig"));
    }
    for (name, value) in environment {
        command.env(name, value);
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|error| {
        SystemRuntimeError::new(
            "system_git_probe_failed",
            format!("Git probe could not start: {error}"),
        )
    })?;
    if let Some(input) = input
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin.write_all(input).await.map_err(|error| {
            SystemRuntimeError::new(
                "system_git_probe_failed",
                format!("Git probe input failed: {error}"),
            )
        })?;
        stdin.shutdown().await.map_err(|error| {
            SystemRuntimeError::new(
                "system_git_probe_failed",
                format!("Git probe input shutdown failed: {error}"),
            )
        })?;
    }
    #[cfg(unix)]
    let pid = child.id();
    let output = match tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output()).await {
        Ok(output) => output.map_err(|error| {
            SystemRuntimeError::new(
                "system_git_probe_failed",
                format!("Git probe failed: {error}"),
            )
        })?,
        Err(_) => {
            #[cfg(unix)]
            if let Some(pid) = pid {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            return Err(SystemRuntimeError::new(
                "system_git_probe_timeout",
                "Git probe timed out",
            ));
        }
    };
    if output.stdout.len() > MAX_PROBE_OUTPUT || output.stderr.len() > MAX_PROBE_OUTPUT {
        return Err(SystemRuntimeError::new(
            "system_git_probe_failed",
            "Git probe exceeded its output limit",
        ));
    }
    Ok(output)
}

async fn discover_default_shell(environment: &HostEnvironmentSnapshot) -> ShellDiscovery {
    let candidates = shell_candidates(environment);
    tracing::debug!(
        tool = "default_shell",
        candidate_count = candidates.len(),
        "probing default shell candidates"
    );
    let mut saw_timeout = false;
    let mut saw_probe_failure = false;
    let mut best = None;
    for (candidate, kind, source) in candidates {
        if !candidate.is_file() {
            continue;
        }
        let started = Instant::now();
        match probe_shell(
            &candidate,
            kind,
            source,
            environment.locale.as_deref(),
            environment
                .home_dir
                .as_deref()
                .or(environment.temporary_dir.as_deref()),
        )
        .await
        {
            Ok(shell) => {
                let interactive = shell
                    .capabilities
                    .contains(&SystemToolCapability::ShellInteractive);
                tracing::debug!(
                    tool = "default_shell",
                    path = %shell.path.display(),
                    source = ?shell.source,
                    version = ?shell.version,
                    elapsed_ms = started.elapsed().as_millis(),
                    capabilities = ?shell.capabilities,
                    "default shell candidate accepted"
                );
                if interactive {
                    return ShellDiscovery {
                        resolved: Some(shell),
                        status: SystemToolState::Ready,
                        error_code: None,
                    };
                }
                best.get_or_insert(shell);
            }
            Err(error) => {
                saw_timeout |= error.code == "system_shell_probe_timeout";
                saw_probe_failure = true;
                tracing::debug!(
                    tool = "default_shell",
                    path = %candidate.display(),
                    source = ?source,
                    elapsed_ms = started.elapsed().as_millis(),
                    code = %error.code,
                    message = %error.message,
                    "default shell candidate rejected"
                );
            }
        }
    }
    if let Some(shell) = best {
        return ShellDiscovery {
            resolved: Some(shell),
            status: SystemToolState::Degraded,
            error_code: Some("system_shell_capability_missing".into()),
        };
    }
    ShellDiscovery {
        resolved: None,
        status: SystemToolState::Unavailable,
        error_code: Some(
            if saw_timeout {
                "system_shell_probe_timeout"
            } else if saw_probe_failure {
                "system_shell_probe_failed"
            } else {
                "system_shell_missing"
            }
            .into(),
        ),
    }
}

#[cfg(unix)]
fn shell_candidates(
    environment: &HostEnvironmentSnapshot,
) -> Vec<(PathBuf, ShellKind, SystemToolSource)> {
    let mut candidates = Vec::new();
    if let Some(shell) = environment.shell_path.clone() {
        candidates.push((shell, ShellKind::Posix, SystemToolSource::ShellSnapshot));
    }
    for path in ["/bin/zsh", "/bin/bash", "/bin/sh"] {
        candidates.push((
            PathBuf::from(path),
            ShellKind::Posix,
            SystemToolSource::WellKnown,
        ));
    }
    dedupe_shell_candidates(candidates)
}

#[cfg(windows)]
fn shell_candidates(
    environment: &HostEnvironmentSnapshot,
) -> Vec<(PathBuf, ShellKind, SystemToolSource)> {
    let mut candidates = Vec::new();
    if let Some(root) = environment.system_root.as_ref() {
        candidates.push((
            root.join("System32/WindowsPowerShell/v1.0/powershell.exe"),
            ShellKind::PowerShell,
            SystemToolSource::WellKnown,
        ));
    }
    candidates.extend(
        path_candidates(
            &environment.shell_path_entries,
            SystemToolSource::OsRegistry,
            &["pwsh.exe", "powershell.exe"],
        )
        .into_iter()
        .map(|candidate| (candidate.path, ShellKind::PowerShell, candidate.source)),
    );
    if let Some(system_root) = environment.system_root.as_ref() {
        candidates.push((
            system_root.join("System32/cmd.exe"),
            ShellKind::CommandPrompt,
            SystemToolSource::WellKnown,
        ));
    }
    dedupe_shell_candidates(candidates)
}

#[cfg(not(any(unix, windows)))]
fn shell_candidates(
    _environment: &HostEnvironmentSnapshot,
) -> Vec<(PathBuf, ShellKind, SystemToolSource)> {
    Vec::new()
}

fn dedupe_shell_candidates(
    candidates: Vec<(PathBuf, ShellKind, SystemToolSource)>,
) -> Vec<(PathBuf, ShellKind, SystemToolSource)> {
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|(path, _, _)| seen.insert(path_key(path)))
        .collect()
}

async fn probe_shell(
    candidate: &Path,
    kind: ShellKind,
    source: SystemToolSource,
    locale: Option<&OsStr>,
    probe_cwd: Option<&Path>,
) -> Result<ShellRuntime, SystemRuntimeError> {
    validate_executable(candidate, "shell")?;
    let path = candidate.canonicalize().map_err(|error| {
        SystemRuntimeError::new(
            "system_shell_probe_failed",
            format!("shell path could not be canonicalized: {error}"),
        )
    })?;
    let mut command = tokio_command(&path, ProcessPolicy::HiddenCaptured);
    match kind {
        ShellKind::Posix => {
            command.args(["-lc", "printf HACHIMI_SHELL_OK"]);
        }
        ShellKind::PowerShell => {
            command.args([
                "-NoLogo",
                "-NoProfile",
                "-Command",
                "Write-Output HACHIMI_SHELL_OK",
            ]);
        }
        ShellKind::CommandPrompt => {
            command.args(["/D", "/S", "/C", "echo HACHIMI_SHELL_OK"]);
        }
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(cwd) = probe_cwd.filter(|path| path.is_absolute() && path.is_dir()) {
        command.current_dir(cwd);
    }
    if let Some(locale) = locale {
        command.env("LC_ALL", locale);
    }
    let output = tokio::time::timeout(PROBE_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            SystemRuntimeError::new("system_shell_probe_timeout", "shell probe timed out")
        })?
        .map_err(|error| {
            SystemRuntimeError::new(
                "system_shell_probe_failed",
                format!("shell probe could not start: {error}"),
            )
        })?;
    if !output.status.success()
        || output.stdout.len() > MAX_PROBE_OUTPUT
        || !String::from_utf8_lossy(&output.stdout).contains("HACHIMI_SHELL_OK")
    {
        return Err(SystemRuntimeError::new(
            "system_shell_probe_failed",
            "shell command probe failed",
        ));
    }
    let version = probe_shell_version(&path, kind, probe_cwd).await;
    let mut capabilities = BTreeSet::from([SystemToolCapability::ShellCommand]);
    match probe_shell_pty(
        path.clone(),
        kind,
        locale.map(OsStr::to_owned),
        probe_cwd.map(Path::to_owned),
    )
    .await
    {
        Ok(()) => {
            capabilities.insert(SystemToolCapability::ShellInteractive);
        }
        Err(error) => {
            tracing::debug!(
                tool = "default_shell",
                path = %path.display(),
                code = %error.code,
                message = %error.message,
                "default shell PTY capability unavailable"
            );
        }
    }
    let identity = FileIdentity::read(&path)
        .map_err(|error| SystemRuntimeError::new("system_shell_probe_failed", error.message))?;
    Ok(ShellRuntime {
        path,
        kind,
        source,
        version,
        capabilities,
        identity,
    })
}

async fn probe_shell_version(
    path: &Path,
    kind: ShellKind,
    probe_cwd: Option<&Path>,
) -> Option<String> {
    let mut command = tokio_command(path, ProcessPolicy::HiddenCaptured);
    match kind {
        ShellKind::Posix => {
            command.arg("--version");
        }
        ShellKind::PowerShell => {
            command.args([
                "-NoLogo",
                "-NoProfile",
                "-Command",
                "$PSVersionTable.PSVersion.ToString()",
            ]);
        }
        ShellKind::CommandPrompt => {
            command.args(["/D", "/S", "/C", "ver"]);
        }
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(cwd) = probe_cwd.filter(|path| path.is_absolute() && path.is_dir()) {
        command.current_dir(cwd);
    }
    let output = tokio::time::timeout(PROBE_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() || output.stdout.len() > MAX_PROBE_OUTPUT {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(256).collect())
}

async fn probe_shell_pty(
    path: PathBuf,
    kind: ShellKind,
    locale: Option<OsString>,
    probe_cwd: Option<PathBuf>,
) -> Result<(), SystemRuntimeError> {
    tokio::task::spawn_blocking(move || {
        probe_shell_pty_blocking(&path, kind, locale.as_deref(), probe_cwd.as_deref())
    })
    .await
    .map_err(|error| {
        SystemRuntimeError::new(
            "system_shell_probe_failed",
            format!("shell PTY probe task failed: {error}"),
        )
    })?
}

fn probe_shell_pty_blocking(
    path: &Path,
    kind: ShellKind,
    locale: Option<&OsStr>,
    probe_cwd: Option<&Path>,
) -> Result<(), SystemRuntimeError> {
    use std::io::Read as _;

    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 8,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| {
            SystemRuntimeError::new(
                "system_shell_probe_failed",
                format!("shell PTY could not be opened: {error}"),
            )
        })?;
    let mut command = CommandBuilder::new(path);
    match kind {
        ShellKind::Posix => command.args(["-lc", "printf HACHIMI_SHELL_PTY_OK"]),
        ShellKind::PowerShell => command.args([
            "-NoLogo",
            "-NoProfile",
            "-Command",
            "Write-Output HACHIMI_SHELL_PTY_OK",
        ]),
        ShellKind::CommandPrompt => command.args(["/D", "/S", "/C", "echo HACHIMI_SHELL_PTY_OK"]),
    }
    if let Some(cwd) = probe_cwd.filter(|path| path.is_absolute() && path.is_dir()) {
        command.cwd(cwd);
    }
    if let Some(locale) = locale {
        command.env("LC_ALL", locale);
    }
    command.env("TERM", "dumb");
    let mut child = pair.slave.spawn_command(command).map_err(|error| {
        SystemRuntimeError::new(
            "system_shell_probe_failed",
            format!("shell PTY process could not start: {error}"),
        )
    })?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|error| {
        SystemRuntimeError::new(
            "system_shell_probe_failed",
            format!("shell PTY output could not be captured: {error}"),
        )
    })?;
    let output = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8 * 1024];
        while bytes.len() <= MAX_PROBE_OUTPUT {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => bytes.extend_from_slice(&buffer[..read]),
            }
        }
        bytes
    });
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = output.join();
                return Err(SystemRuntimeError::new(
                    "system_shell_probe_timeout",
                    "shell PTY probe timed out",
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = output.join();
                return Err(SystemRuntimeError::new(
                    "system_shell_probe_failed",
                    format!("shell PTY status failed: {error}"),
                ));
            }
        }
    };
    let output = output.join().map_err(|_| {
        SystemRuntimeError::new("system_shell_probe_failed", "shell PTY reader panicked")
    })?;
    if !status.success()
        || output.len() > MAX_PROBE_OUTPUT
        || !String::from_utf8_lossy(&output).contains("HACHIMI_SHELL_PTY_OK")
    {
        return Err(SystemRuntimeError::new(
            "system_shell_probe_failed",
            "shell PTY capability probe failed",
        ));
    }
    Ok(())
}

fn validate_executable(path: &Path, label: &str) -> Result<(), SystemRuntimeError> {
    if !path.is_absolute() || !path.is_file() {
        return Err(SystemRuntimeError::new(
            format!("system_{}_probe_failed", label.to_ascii_lowercase()),
            format!("{label} executable is unavailable: {}", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path)
            .map_err(|error| {
                SystemRuntimeError::new(
                    format!("system_{}_probe_failed", label.to_ascii_lowercase()),
                    error.to_string(),
                )
            })?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err(SystemRuntimeError::new(
                format!("system_{}_probe_failed", label.to_ascii_lowercase()),
                format!("{label} is not executable: {}", path.display()),
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn windows_registry_path_entries() -> Vec<PathBuf> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let mut values = Vec::new();
    if let Some(value) = registry_string(HKEY_CURRENT_USER, r"Environment", "Path") {
        values.extend(sanitized_path_entries(&value));
    }
    if let Some(value) = registry_string(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
        "Path",
    ) {
        values.extend(sanitized_path_entries(&value));
    }
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|path| seen.insert(path_key(path)))
        .collect()
}

#[cfg(windows)]
fn windows_git_app_paths() -> Vec<PathBuf> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .filter_map(|root| {
            registry_string(
                root,
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\git.exe",
                "",
            )
        })
        .map(PathBuf::from)
        .collect()
}

#[cfg(windows)]
fn registry_string(
    root: windows_sys::Win32::System::Registry::HKEY,
    key: &str,
    name: &str,
) -> Option<OsString> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::System::Registry::{RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW};
    let key = key.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let name = name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut bytes = 0_u32;
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            flags,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if status != 0 || bytes < 2 {
        return None;
    }
    let mut value = vec![0_u16; usize::try_from(bytes / 2).ok()?];
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            flags,
            std::ptr::null_mut(),
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        return None;
    }
    while value.last() == Some(&0) {
        value.pop();
    }
    Some(expand_windows_environment(OsString::from_wide(&value)))
}

#[cfg(windows)]
fn expand_windows_environment(value: OsString) -> OsString {
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
    use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;

    let source = value.encode_wide().chain(Some(0)).collect::<Vec<_>>();
    let required = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), std::ptr::null_mut(), 0) };
    if required == 0 {
        return value;
    }
    let mut expanded = vec![0_u16; usize::try_from(required).unwrap_or_default()];
    let written =
        unsafe { ExpandEnvironmentStringsW(source.as_ptr(), expanded.as_mut_ptr(), required) };
    if written == 0 || written > required {
        return value;
    }
    while expanded.last() == Some(&0) {
        expanded.pop();
    }
    OsString::from_wide(&expanded)
}

fn now_ms() -> i64 {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests;
