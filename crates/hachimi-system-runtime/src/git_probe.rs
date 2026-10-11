use super::*;

fn git_probe_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    if let Some(value) = path.to_str() {
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(local) = value.strip_prefix(r"\\?\")
            && Path::new(local).is_absolute()
        {
            return PathBuf::from(local);
        }
    }
    path.to_path_buf()
}

pub(super) async fn probe_git(
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
    // Git's configuration parser cannot consume Windows verbatim path spellings.
    // Keep the owned TempDir alive and use an equivalent spelling for this probe only.
    let probe_root = git_probe_path(root.path());
    std::fs::write(probe_root.join("global.gitconfig"), b"").map_err(|error| {
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
    let path = resolve_effective_git_executable(canonical, Some(&probe_root)).await?;
    validate_executable(&path, "Git")?;
    let version_output = run_git_probe(&path, Some(&probe_root), ["--version"], None, &[]).await?;
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
    if git_probe_succeeds(&path, &probe_root, ["init"]).await
        && git_probe_succeeds(&path, &probe_root, ["rev-parse", "--git-dir"]).await
        && git_probe_succeeds(&path, &probe_root, ["symbolic-ref", "--quiet", "HEAD"]).await
    {
        capabilities.insert(SystemToolCapability::GitInspect);
    }
    if capabilities.contains(&SystemToolCapability::GitInspect) {
        let tree = run_git_probe(&path, Some(&probe_root), ["mktree"], Some(&[]), &[])
            .await
            .ok()
            .and_then(|output| output.status.success().then_some(output.stdout))
            .map(|stdout| String::from_utf8_lossy(&stdout).trim().to_owned())
            .filter(|value| !value.is_empty());
        let commit = if let Some(tree) = tree {
            run_git_probe(
                &path,
                Some(&probe_root),
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
                &probe_root,
                ["symbolic-ref", "HEAD", "refs/heads/main"],
            )
            .await
            && git_probe_succeeds(
                &path,
                &probe_root,
                ["update-ref", "refs/heads/main", commit.as_str()],
            )
            .await
        {
            capabilities.insert(SystemToolCapability::GitLocalMutation);
        }
    }
    if capabilities.contains(&SystemToolCapability::GitLocalMutation) {
        let probe_file = probe_root.join("runtime-probe.txt");
        let linked = probe_root.join("linked-worktree");
        let worktree_ok = std::fs::write(&probe_file, b"probe\n").is_ok()
            && git_probe_succeeds(&path, &probe_root, ["add", "runtime-probe.txt"]).await
            && git_probe_succeeds(
                &path,
                &probe_root,
                ["restore", "--staged", "--", "runtime-probe.txt"],
            )
            .await
            && git_probe_succeeds(&path, &probe_root, ["switch", "--detach", "HEAD"]).await
            && git_probe_succeeds(
                &path,
                &probe_root,
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
                &probe_root,
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

async fn git_probe_succeeds<const N: usize>(git: &Path, cwd: &Path, args: [&str; N]) -> bool {
    let result = run_git_probe(git, Some(cwd), args, None, &[]).await;
    #[cfg(test)]
    match &result {
        Ok(output) if !output.status.success() => eprintln!(
            "isolated Git probe {args:?} in {} failed: {}",
            cwd.display(),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(error) => eprintln!("isolated Git probe {args:?} failed: {}", error.message),
        _ => {}
    }
    result.is_ok_and(|output| output.status.success())
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
