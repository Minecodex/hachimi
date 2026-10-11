use super::*;

#[cfg(windows)]
#[tokio::test]
async fn windows_shell_pty_probe_negotiates_cursor_and_finishes_output_drain() {
    let system = PathBuf::from(std::env::var_os("SystemRoot").expect("Windows system root"))
        .join("System32");
    for (executable, kind) in [
        ("cmd.exe", ShellKind::CommandPrompt),
        (
            "WindowsPowerShell/v1.0/powershell.exe",
            ShellKind::PowerShell,
        ),
    ] {
        tokio::time::timeout(
            Duration::from_secs(15),
            probe_shell_pty(system.join(executable), kind, None, None),
        )
        .await
        .expect("the probe must not wait indefinitely for ConPTY EOF")
        .expect("the installed Windows shell supports a captured PTY");
    }
}

#[cfg(unix)]
fn write_executable(path: &Path, content: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::write(path, content).expect("write executable fixture");
    let mut permissions = std::fs::metadata(path)
        .expect("fixture metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("fixture permissions");
}

#[cfg(unix)]
fn installed_git() -> Option<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| sanitized_path_entries(&path))
        .into_iter()
        .flatten()
        .map(|root| root.join("git"))
        .find(|path| path.is_file())
        .and_then(|path| path.canonicalize().ok())
}

#[test]
fn path_snapshot_drops_relative_and_duplicate_entries() {
    let temp = tempfile::tempdir().expect("temp");
    let value = std::env::join_paths([
        PathBuf::from("relative"),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
    ])
    .expect("PATH");
    assert_eq!(
        sanitized_path_entries(&value),
        vec![temp.path().canonicalize().unwrap()]
    );
}

#[test]
fn path_snapshot_drops_the_process_current_directory() {
    let current = std::env::current_dir()
        .expect("current directory")
        .canonicalize()
        .expect("canonical current directory");
    let other = tempfile::tempdir().expect("other PATH directory");
    let value = std::env::join_paths([current, other.path().to_path_buf()]).expect("PATH");
    assert_eq!(
        sanitized_path_entries(&value),
        vec![other.path().canonicalize().unwrap()]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn shell_command_and_interactive_capabilities_are_probed_independently() {
    let root = tempfile::tempdir().expect("shell probe root");
    let shell = probe_shell(
        Path::new("/bin/sh"),
        ShellKind::Posix,
        SystemToolSource::WellKnown,
        None,
        Some(root.path()),
    )
    .await
    .expect("system shell probe");
    assert!(
        shell
            .capabilities
            .contains(&SystemToolCapability::ShellCommand)
    );
    assert!(
        shell
            .capabilities
            .contains(&SystemToolCapability::ShellInteractive)
    );

    let wrapper = root.path().join("non-interactive-only-shell");
    write_executable(
        &wrapper,
        "#!/bin/sh\nif [ -t 1 ]; then exit 9; fi\nexec /bin/sh \"$@\"\n",
    );
    let partial = probe_shell(
        &wrapper,
        ShellKind::Posix,
        SystemToolSource::TestOverride,
        None,
        Some(root.path()),
    )
    .await
    .expect("command-only shell probe");
    assert!(
        partial
            .capabilities
            .contains(&SystemToolCapability::ShellCommand)
    );
    assert!(
        !partial
            .capabilities
            .contains(&SystemToolCapability::ShellInteractive)
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn hanging_login_shell_snapshot_times_out_and_reaps_the_group() {
    let root = tempfile::tempdir().expect("shell snapshot root");
    let shell = root.path().join("hanging-login-shell");
    write_executable(&shell, "#!/bin/sh\nexec /bin/sleep 30\n");
    let started = Instant::now();
    let error = capture_login_shell_path_with_timeout(
        &shell,
        Some(root.path()),
        Duration::from_millis(100),
    )
    .await
    .expect_err("hanging login shell must time out");
    assert_eq!(error.code, "host_environment_snapshot_timeout");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_well_known_candidates_prefer_homebrew_before_apple_git() {
    let paths = platform_git_candidates()
        .into_iter()
        .map(|candidate| candidate.path)
        .collect::<Vec<_>>();
    assert_eq!(paths[0], PathBuf::from("/opt/homebrew/bin/git"));
    assert_eq!(paths[3], PathBuf::from("/usr/bin/git"));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn apple_git_is_accepted_by_capability_instead_of_version() {
    let path = Path::new("/usr/bin/git");
    if !path.is_file() {
        return;
    }
    let resolved = probe_git(path, SystemToolSource::WellKnown, None)
        .await
        .expect("Apple Git capability probe");
    assert_ne!(
        resolved.path, path,
        "Apple Git shim must resolve to its toolchain binary"
    );
    assert!(
        resolved
            .capabilities
            .contains(&SystemToolCapability::GitLocalMutation)
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn finder_style_minimal_path_still_discovers_capable_git() {
    let environment = HostEnvironmentSnapshot {
        process_path_entries: vec![
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
            PathBuf::from("/usr/sbin"),
            PathBuf::from("/sbin"),
        ],
        temporary_dir: absolute_environment_path(&["TMPDIR", "TEMP", "TMP"]),
        ..HostEnvironmentSnapshot::default()
    };
    let discovered = discover_git(&environment).await;
    let git = discovered
        .resolved
        .expect("Finder-style environment should resolve Apple or well-known Git");
    assert!(
        git.capabilities
            .contains(&SystemToolCapability::GitLocalMutation)
    );
}

#[test]
fn runtime_lease_detects_executable_drift() {
    let directory = tempfile::tempdir().expect("runtime");
    let path = directory
        .path()
        .join(if cfg!(windows) { "git.exe" } else { "git" });
    std::fs::write(&path, b"first").expect("fixture executable");
    let path = path.canonicalize().expect("canonical fixture");
    let lease = GitRuntimeLease {
        identity: FileIdentity::read(&path).expect("identity"),
        path: path.clone(),
        revision: 7,
        capabilities: BTreeSet::from([SystemToolCapability::GitInspect]),
    };
    std::fs::write(path, b"replacement-with-a-different-size").expect("replace fixture");
    assert_eq!(
        lease.verify().expect_err("drift must fail").code,
        "system_git_changed"
    );
}

#[tokio::test]
async fn installed_git_is_capability_probed_without_a_version_floor() {
    let Some(git) = std::env::var_os("PATH")
        .map(|path| sanitized_path_entries(&path))
        .into_iter()
        .flatten()
        .map(|root| root.join(if cfg!(windows) { "git.exe" } else { "git" }))
        .find(|path| path.is_file())
    else {
        return;
    };
    let resolved = probe_git(&git, SystemToolSource::ProcessEnvironment, None)
        .await
        .expect("Git probe");
    assert!(
        resolved
            .capabilities
            .contains(&SystemToolCapability::GitLocalMutation)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn damaged_git_candidate_is_rejected() {
    let directory = tempfile::tempdir().expect("runtime");
    let candidate = directory.path().join("git");
    write_executable(&candidate, "#!/bin/sh\nexit 7\n");
    assert_eq!(
        probe_git(&candidate, SystemToolSource::TestOverride, None)
            .await
            .expect_err("damaged Git must fail")
            .code,
        "system_git_probe_failed"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn hanging_git_candidate_times_out() {
    let directory = tempfile::tempdir().expect("runtime");
    let candidate = directory.path().join("git");
    write_executable(&candidate, "#!/bin/sh\nexec /bin/sleep 30\n");
    assert_eq!(
        probe_git(&candidate, SystemToolSource::TestOverride, None)
            .await
            .expect_err("hanging Git must fail")
            .code,
        "system_git_probe_timeout"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn partial_git_candidate_retains_only_proven_capabilities() {
    let Some(real_git) = installed_git() else {
        return;
    };
    let directory = tempfile::tempdir().expect("runtime");
    let candidate = directory.path().join("git");
    write_executable(
        &candidate,
        &format!(
            "#!/bin/sh\nfor arg in \"$@\"; do\n  [ \"$arg\" = worktree ] && exit 9\ndone\nexec \"{}\" \"$@\"\n",
            real_git.display()
        ),
    );
    let resolved = probe_git(&candidate, SystemToolSource::TestOverride, None)
        .await
        .expect("partial Git probe");
    assert!(
        resolved
            .capabilities
            .contains(&SystemToolCapability::GitInspect)
    );
    assert!(
        resolved
            .capabilities
            .contains(&SystemToolCapability::GitLocalMutation)
    );
    assert!(
        !resolved
            .capabilities
            .contains(&SystemToolCapability::GitWorktree)
    );
}

#[cfg(windows)]
#[test]
fn windows_registry_and_standard_candidates_are_absolute_and_native() {
    let mut candidates = windows_registry_path_entries();
    candidates.extend(
        windows_git_app_paths().into_iter().chain(
            platform_git_candidates()
                .into_iter()
                .map(|value| value.path),
        ),
    );
    assert!(candidates.iter().all(|path| path.is_absolute()));
    assert!(
        candidates
            .iter()
            .all(|path| { !path.to_string_lossy().to_ascii_lowercase().contains("wsl") })
    );
}

#[cfg(windows)]
#[test]
fn windows_registry_expand_strings_are_resolved_before_path_filtering() {
    let expanded = expand_windows_environment(OsString::from(r"%SystemRoot%\System32"));
    let path = PathBuf::from(expanded);
    assert!(path.is_absolute());
    assert!(!path.to_string_lossy().contains('%'));
}

#[cfg(windows)]
#[tokio::test]
async fn windows_registry_or_standard_install_is_found_without_process_path() {
    let environment = HostEnvironmentSnapshot {
        shell_path_entries: windows_registry_path_entries(),
        process_path_entries: Vec::new(),
        system_root: absolute_environment_path(&["SystemRoot", "SYSTEMROOT"]),
        temporary_dir: absolute_environment_path(&["TEMP", "TMP"]),
        pathext: std::env::var_os("PATHEXT"),
        ..HostEnvironmentSnapshot::default()
    };
    let has_installed_candidate = platform_git_candidates()
        .into_iter()
        .any(|candidate| candidate.path.is_file())
        || environment
            .shell_path_entries
            .iter()
            .any(|root| root.join("git.exe").is_file());
    if !has_installed_candidate {
        assert_ne!(
            std::env::var("HACHIMI_REQUIRE_STANDARD_GIT_DISCOVERY").as_deref(),
            Ok("1"),
            "standard-user CI requires Git for Windows to be installed outside process PATH"
        );
        return;
    }
    let discovered = discover_git_from_snapshot(&environment, false).await;
    let git = discovered
        .resolved
        .expect("registry or standard Git should not need process PATH");
    assert!(matches!(
        git.source,
        SystemToolSource::OsRegistry | SystemToolSource::WellKnown
    ));
}

#[tokio::test]
async fn refresh_produces_a_monotonic_runtime_snapshot() {
    let manager = SystemRuntimeManager::new();
    let first = manager.refresh().await;
    let first_lease = manager.require_git(SystemToolCapability::GitInspect).ok();
    let second = manager.refresh().await;
    assert_eq!(first.revision + 1, second.revision);
    assert_eq!(second.tools.len(), 2);
    if let Some(first_lease) = first_lease {
        let second_lease = manager
            .require_git(SystemToolCapability::GitInspect)
            .expect("Git remained available after refresh");
        assert_eq!(first_lease.revision(), first.revision);
        assert_eq!(second_lease.revision(), second.revision);
        assert_ne!(first_lease.revision(), second_lease.revision());
        first_lease
            .verify()
            .expect("an active lease remains pinned and valid after refresh");
    }
}
