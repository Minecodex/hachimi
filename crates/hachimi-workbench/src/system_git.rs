use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use hachimi_process_policy::{ProcessPolicy, tokio_command};
use hachimi_system_runtime::SystemRuntimeManager;
use tokio_util::sync::CancellationToken;

use crate::WorkbenchError;

pub(super) async fn git_optional(
    runtime: &SystemRuntimeManager,
    root: &Path,
    args: &[&str],
) -> Result<Option<String>, WorkbenchError> {
    let Ok(executable) = resolved_git_executable(runtime, args) else {
        return Ok(None);
    };
    let output = tokio_command(executable, ProcessPolicy::HiddenCaptured)
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|error| WorkbenchError::Git(error.to_string()))?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

pub(super) async fn git_required(
    runtime: &SystemRuntimeManager,
    root: &Path,
    args: &[&str],
    cancellation: Option<&CancellationToken>,
) -> Result<String, WorkbenchError> {
    let executable = resolved_git_executable(runtime, args)?;
    let mut command = tokio_command(executable, ProcessPolicy::HiddenCaptured);
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = if let Some(cancellation) = cancellation {
        tokio::select! {
            () = cancellation.cancelled() => return Err(WorkbenchError::Cancelled),
            output = command.output() => output,
        }
    } else {
        command.output().await
    }
    .map_err(|error| WorkbenchError::Git(error.to_string()))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(1_024)
            .collect::<String>();
        return Err(WorkbenchError::Git(message));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(super) async fn git_required_bytes(
    runtime: &SystemRuntimeManager,
    root: &Path,
    args: &[&str],
) -> Result<Vec<u8>, WorkbenchError> {
    let executable = resolved_git_executable(runtime, args)?;
    let output = tokio_command(executable, ProcessPolicy::HiddenCaptured)
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| WorkbenchError::Git(error.to_string()))?;
    if !output.status.success() {
        return Err(WorkbenchError::Git(
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(1_024)
                .collect(),
        ));
    }
    Ok(output.stdout)
}

fn resolved_git_executable(
    runtime: &SystemRuntimeManager,
    args: &[&str],
) -> Result<PathBuf, WorkbenchError> {
    match runtime.require_git(git_capability(args)) {
        Ok(lease) => Ok(lease.executable().to_owned()),
        Err(error) => {
            #[cfg(test)]
            if let Some(path) = std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|root| root.join(if cfg!(windows) { "git.exe" } else { "git" }))
                    .find(|path| path.is_file())
                    .and_then(|path| path.canonicalize().ok())
            }) {
                return Ok(path);
            }
            Err(WorkbenchError::Git(format!(
                "{}: {}",
                error.code, error.message
            )))
        }
    }
}

fn git_capability(args: &[&str]) -> hachimi_protocol::SystemToolCapability {
    use hachimi_protocol::SystemToolCapability;

    match args.first().copied() {
        Some("worktree" | "switch" | "restore") => SystemToolCapability::GitWorktree,
        Some(
            "add" | "apply" | "branch" | "clean" | "commit" | "merge" | "reset" | "update-ref",
        ) => SystemToolCapability::GitLocalMutation,
        _ => SystemToolCapability::GitInspect,
    }
}
