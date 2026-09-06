// SPDX-License-Identifier: Apache-2.0
// Unix PTY backend following the ConPTY control contract.
// Process-tree termination uses the child's session process group
// (killpg), mirroring the kill-on-close Job Object semantics on Windows.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::PathBuf,
    time::Duration,
};

use hachimi_protocol::{ProcessOutputStream, ProcessTerminalSize};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::sync::{mpsc, oneshot};

use crate::{ProcessError, RuntimeControl, RuntimeOutput, SpawnedRuntime};

use super::PIPE_READ_CHUNK;

struct SpawnedUnixPty {
    master: Box<dyn portable_pty::MasterPty + Send>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

pub(crate) async fn spawn_unix_pty(
    launcher: Option<PathBuf>,
    command: Vec<String>,
    cwd: PathBuf,
    environment: BTreeMap<String, String>,
    size: ProcessTerminalSize,
    timeout: Option<Duration>,
) -> Result<SpawnedRuntime, ProcessError> {
    // Restricted terminal sessions ride `/usr/bin/sandbox-exec` on macOS; the
    // launcher binary path is a Windows contract and is not used here.
    if launcher.is_some() && !cfg!(target_os = "macos") {
        return Err(ProcessError::Pty(
            "the restricted process launcher is only supported on Windows and macOS".into(),
        ));
    }
    #[cfg(target_os = "macos")]
    let seatbelt_args = if launcher.is_some() {
        Some(
            hachimi_sandbox::seatbelt_terminal_args(&command, &cwd, &environment)
                .map_err(ProcessError::Pty)?,
        )
    } else {
        None
    };
    #[cfg(not(target_os = "macos"))]
    let seatbelt_args: Option<Vec<std::ffi::OsString>> = None;
    let spawned = tokio::task::spawn_blocking(move || {
        spawn_native(&command, &cwd, &environment, size, seatbelt_args)
    })
    .await
    .map_err(|error| ProcessError::Pty(error.to_string()))??;
    let SpawnedUnixPty {
        master,
        reader,
        writer,
        child,
    } = spawned;
    let child_pid = child.process_id();
    let (output_tx, output_rx) = mpsc::channel(256);
    let reader_task = tokio::task::spawn_blocking(move || read_pty(reader, output_tx));
    let (write_tx, mut write_rx) = mpsc::channel::<PtyWrite>(128);
    let writer_task = tokio::task::spawn_blocking(move || write_pty(writer, &mut write_rx));
    let (control_tx, mut control_rx) = mpsc::channel(128);
    let (exit_tx, exit_rx) = oneshot::channel();
    tokio::spawn(async move {
        let mut write_tx = Some(write_tx);
        let (wait_tx, mut wait_rx) = oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let mut child = child;
            let code = child
                .wait()
                .map(|status| status.exit_code() as i32)
                .unwrap_or(-1);
            let _ = wait_tx.send(code);
        });
        let expiration = super::wait_timeout(timeout);
        tokio::pin!(expiration);
        let mut timed_out = false;
        let exit_code = loop {
            tokio::select! {
                code = &mut wait_rx => break code.unwrap_or(-1),
                control = control_rx.recv() => match control {
                    Some(RuntimeControl::Write { bytes, close, response }) => {
                        let result = if bytes.is_empty() && !close {
                            Ok(())
                        } else if let Some(sender) = write_tx.as_ref() {
                            let (written_tx, written_rx) = oneshot::channel();
                            if sender.send(PtyWrite { bytes, response: written_tx }).await.is_err() {
                                Err(ProcessError::StdinClosed)
                            } else {
                                written_rx.await.unwrap_or(Err(ProcessError::ControlClosed))
                            }
                        } else {
                            Err(ProcessError::StdinClosed)
                        };
                        if close { write_tx.take(); }
                        let _ = response.send(result);
                    }
                    Some(RuntimeControl::Resize { size, response }) => {
                        let _ = response.send(resize_pty(master.as_ref(), size));
                    }
                    Some(RuntimeControl::Terminate { response }) => {
                        let _ = response.send(terminate_process_group(child_pid));
                    }
                    None => {
                        let _ = terminate_process_group(child_pid);
                        break -1;
                    }
                },
                () = &mut expiration, if timeout.is_some() && !timed_out => {
                    timed_out = true;
                    let _ = terminate_process_group(child_pid);
                }
            }
        };
        write_tx.take();
        // Dropping the master releases the PTY so the reader observes EOF once
        // the child has exited.
        drop(master);
        let _ = tokio::time::timeout(Duration::from_secs(2), reader_task).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), writer_task).await;
        let _ = exit_tx.send(if timed_out { 124 } else { exit_code });
    });
    Ok(SpawnedRuntime {
        control_tx,
        output_rx,
        exit_rx,
    })
}

fn spawn_native(
    command: &[String],
    cwd: &std::path::Path,
    environment: &BTreeMap<String, String>,
    size: ProcessTerminalSize,
    seatbelt_args: Option<Vec<std::ffi::OsString>>,
) -> Result<SpawnedUnixPty, ProcessError> {
    if size.rows == 0 || size.cols == 0 {
        return Err(ProcessError::InvalidRequest(
            "terminal size must be positive",
        ));
    }
    let Some(program) = command.first().filter(|value| !value.trim().is_empty()) else {
        return Err(ProcessError::InvalidRequest("command must not be empty"));
    };
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| ProcessError::Pty(error.to_string()))?;
    let mut builder = match seatbelt_args {
        Some(arguments) => {
            let mut builder = CommandBuilder::new("/usr/bin/sandbox-exec");
            builder.args(&arguments);
            builder
        }
        None => {
            let mut builder = CommandBuilder::new(program);
            builder.args(&command[1..]);
            builder
        }
    };
    builder.cwd(cwd);
    builder.env_clear();
    for (name, value) in environment {
        builder.env(name, value);
    }
    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(|error| ProcessError::Pty(error.to_string()))?;
    let master = pair.master;
    let reader = master
        .try_clone_reader()
        .map_err(|error| ProcessError::Pty(error.to_string()))?;
    let writer = master
        .take_writer()
        .map_err(|error| ProcessError::Pty(error.to_string()))?;
    drop(pair.slave);
    Ok(SpawnedUnixPty {
        master,
        reader,
        writer,
        child,
    })
}

fn resize_pty(
    master: &(dyn portable_pty::MasterPty + Send),
    size: ProcessTerminalSize,
) -> Result<(), ProcessError> {
    if size.rows == 0 || size.cols == 0 {
        return Err(ProcessError::InvalidRequest(
            "terminal size must be positive",
        ));
    }
    master
        .resize(PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| ProcessError::Pty(error.to_string()))
}

/// The PTY child is a session leader, so its process group id equals its pid
/// and the group covers its descendants unless they explicitly detach.
fn terminate_process_group(pid: Option<u32>) -> Result<(), ProcessError> {
    let Some(pid) = pid else {
        return Err(ProcessError::Pty("process id is unavailable".into()));
    };
    let result = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    if result == 0 || unsafe { *libc::__error() } == libc::ESRCH {
        Ok(())
    } else {
        Err(ProcessError::Pty(format!(
            "failed to kill the process group of {pid}"
        )))
    }
}

#[derive(Debug)]
struct PtyWrite {
    bytes: Vec<u8>,
    response: oneshot::Sender<Result<(), ProcessError>>,
}

fn read_pty(mut reader: Box<dyn Read + Send>, output: mpsc::Sender<RuntimeOutput>) {
    let mut buffer = [0_u8; PIPE_READ_CHUNK];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if output
                    .blocking_send(RuntimeOutput {
                        stream: ProcessOutputStream::Stdout,
                        bytes: buffer[..read].to_vec(),
                    })
                    .is_err()
                {
                    break;
                }
            }
            // The master reports EIO once the child's slave side is gone.
            Err(_) => break,
        }
    }
}

fn write_pty(mut writer: Box<dyn Write + Send>, receiver: &mut mpsc::Receiver<PtyWrite>) {
    while let Some(write) = receiver.blocking_recv() {
        let result = writer
            .write_all(&write.bytes)
            .and_then(|()| writer.flush())
            .map_err(ProcessError::Spawn);
        let failed = result.is_err();
        let _ = write.response.send(result);
        if failed {
            break;
        }
    }
}
