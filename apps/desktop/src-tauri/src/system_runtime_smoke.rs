use std::{path::Path, process::Stdio};

use hachimi_process_policy::{ProcessPolicy, std_command};
use hachimi_protocol::SystemToolCapability;

pub(super) fn run_if_requested() -> bool {
    if !std::env::args_os().any(|argument| argument == "--system-runtime-smoke") {
        return false;
    }
    if let Err(error) = run() {
        eprintln!("system-runtime-smoke-failed: {error}");
        std::process::exit(2);
    }
    true
}

fn run() -> Result<(), String> {
    let manager = hachimi_system_runtime::system_runtime_manager();
    let snapshot = tauri::async_runtime::block_on(manager.refresh());
    let git = manager
        .require_git(SystemToolCapability::GitLocalMutation)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    #[cfg(target_os = "macos")]
    run_macos_workspace_smoke(git.clone())?;
    #[cfg(not(target_os = "macos"))]
    run_direct_git_smoke(git.executable())?;
    println!(
        "system-runtime-smoke-ready revision={} git={}",
        snapshot.revision,
        git.executable().display()
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_macos_workspace_smoke(git: hachimi_system_runtime::GitRuntimeLease) -> Result<(), String> {
    use std::{path::PathBuf, sync::Arc, time::Duration};

    use hachimi_protocol::{
        CapabilityGrantSet, CheckoutId, FileSystemAccess, FileSystemGrant, NetworkGrant,
        PermissionGrantScope, PermissionProfile, ProcessGrant, RunId, SessionId,
    };
    use hachimi_sandbox::{
        SandboxBackend, SandboxStatus, attest_workspace_boundaries, install_macos_marker,
        platform_probe, prepare_git_mutation_acl, prepare_workspace_acl, restore_git_mutation_acl,
    };
    use hachimi_workspace::{
        WorkspaceHostClient, WorkspaceLaunchCheck, WorkspaceLaunchGuard,
        WorkspaceLaunchValidationFuture, WorkspaceOperation, WorkspaceOutput,
        WorkspaceSandboxContext,
    };
    use sha2::{Digest, Sha256};
    use tokio_util::sync::CancellationToken;

    struct AllowSmokeLaunch;
    impl WorkspaceLaunchGuard for AllowSmokeLaunch {
        fn validate(&self, _check: WorkspaceLaunchCheck) -> WorkspaceLaunchValidationFuture {
            Box::pin(async { Ok(()) })
        }
    }

    fn sha256(path: &Path) -> Result<String, String> {
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        Ok(Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }

    fn bundled_runtime(name: &str) -> Result<PathBuf, String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let contents = executable
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| "app executable is not inside a macOS bundle".to_owned())?;
        let path = contents.join("Resources/internal-runtime").join(name);
        if !path.is_file() {
            return Err(format!(
                "bundled runtime component is missing: {}",
                path.display()
            ));
        }
        path.canonicalize().map_err(|error| error.to_string())
    }

    let smoke_root = std::env::temp_dir().join(format!(
        "hachimi-system-runtime-smoke-{}",
        uuid::Uuid::new_v4()
    ));
    let cleanup = SmokeDirectory(smoke_root.clone());
    let checkout = smoke_root.join("checkout");
    let data = smoke_root.join("data");
    std::fs::create_dir_all(&checkout).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&data).map_err(|error| error.to_string())?;
    std::fs::write(checkout.join("global.gitconfig"), b"").map_err(|error| error.to_string())?;
    run_git(
        git.executable(),
        &checkout,
        &["init", "--initial-branch=main"],
    )?;
    std::fs::write(checkout.join("staged.txt"), b"staged\n").map_err(|error| error.to_string())?;
    std::fs::write(checkout.join("untracked.txt"), b"untracked\n")
        .map_err(|error| error.to_string())?;
    run_git(git.executable(), &checkout, &["add", "staged.txt"])?;
    let index_path = checkout.join(".git/index");
    let index_before = std::fs::read(&index_path).map_err(|error| error.to_string())?;

    let worker = bundled_runtime("hachimi-workspace-worker")?;
    let canary = bundled_runtime("hachimi-sandbox-canary")?;
    let marker = data.join("sandbox/macos-seatbelt/setup.json");
    install_macos_marker(&marker)?;
    let expected_integrity = vec![
        (canary.clone(), sha256(&canary)?),
        (worker.clone(), sha256(&worker)?),
    ];
    let backend: Arc<dyn SandboxBackend> = Arc::new(
        platform_probe(marker)
            .with_runtime(PathBuf::new(), &canary, data.join("sandbox/attestation"))
            .with_runtime_integrity(expected_integrity),
    );
    let report = backend.capability_report();
    if SandboxStatus::from_report(&report) != SandboxStatus::Enforced {
        return Err(format!(
            "macOS Seatbelt attestation was not enforced: {:?} {:?}",
            report.stable_error_code, report.diagnostics
        ));
    }

    let session_id = SessionId::random();
    let run_id = RunId::random();
    let checkout_id = CheckoutId::random();
    let mut host = WorkspaceHostClient::new_with_git_runtime(
        &worker,
        &checkout,
        checkout_id.as_str(),
        1,
        Some(git.clone()),
    );
    let read_only_roots = prepare_workspace_acl(
        &checkout,
        host.run_temp_dir(),
        &worker,
        Some(git.executable()),
    )?;
    attest_workspace_boundaries(
        Path::new("unused-on-macos"),
        &canary,
        &checkout,
        host.run_temp_dir(),
        &worker,
        &read_only_roots,
    )?;
    let root = checkout.to_string_lossy().into_owned();
    let temp = host.run_temp_dir().to_string_lossy().into_owned();
    let grants = CapabilityGrantSet {
        profile: PermissionProfile::Writable,
        scope: PermissionGrantScope::Run,
        session_id: session_id.clone(),
        run_id: Some(run_id.clone()),
        source: "macos_app_system_runtime_smoke".into(),
        file_system: vec![FileSystemGrant {
            access: FileSystemAccess::Write,
            roots: vec![root, temp],
            globs: Vec::new(),
            files: Vec::new(),
            special_roots: Vec::new(),
        }],
        network: NetworkGrant::default(),
        process: ProcessGrant {
            spawn: true,
            interactive: false,
            unrestricted_commands: false,
            allowed_commands: vec![worker.to_string_lossy().into_owned()],
        },
        browser: Default::default(),
        computer: Default::default(),
        review_each_command: false,
        expires_at_ms: None,
    };
    host = host
        .with_sandbox(
            backend,
            WorkspaceSandboxContext {
                session_id,
                run_id,
                grants,
                git_metadata_writable: false,
            },
            Arc::new(AllowSmokeLaunch),
        )
        .with_git_metadata_writable(true);
    let mutation_acl = prepare_git_mutation_acl(&checkout, git.executable())?;
    let result = tauri::async_runtime::block_on(host.execute(
        WorkspaceOperation::GitCreateEmptyInitialCommit {
            author_name: "Hachimi Runtime Smoke".into(),
            author_email: "runtime-smoke@hachimi.invalid".into(),
            history_limit: 5,
        },
        Duration::from_secs(30),
        CancellationToken::new(),
    ));
    restore_git_mutation_acl(&mutation_acl)?;
    let output = result.map_err(|error| format!("{:?}: {}", error.code, error.message))?;
    if !matches!(
        output,
        WorkspaceOutput::GitMutation { ref response }
            if response.commit_sha.as_deref().is_some_and(|sha| !sha.is_empty())
    ) {
        return Err("workspace Worker returned no initial-commit receipt".into());
    }
    let index_after = std::fs::read(&index_path).map_err(|error| error.to_string())?;
    if index_before != index_after {
        return Err("empty initial commit changed the staged index".into());
    }
    if !checkout.join("untracked.txt").is_file() {
        return Err("empty initial commit removed the untracked file".into());
    }
    let staged = run_git_output(
        git.executable(),
        &checkout,
        &["diff", "--cached", "--name-only"],
    )?;
    if String::from_utf8_lossy(&staged).trim() != "staged.txt" {
        return Err("staged content changed during the empty initial commit".into());
    }
    let committed = run_git_output(
        git.executable(),
        &checkout,
        &["ls-tree", "-r", "--name-only", "HEAD"],
    )?;
    if !committed.is_empty() {
        return Err("initial commit unexpectedly contains files".into());
    }
    drop(cleanup);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn run_direct_git_smoke(git: &Path) -> Result<(), String> {
    let root = std::env::temp_dir().join(format!(
        "hachimi-system-runtime-smoke-{}",
        uuid::Uuid::new_v4()
    ));
    let cleanup = SmokeDirectory(root.clone());
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    std::fs::write(root.join("global.gitconfig"), b"").map_err(|error| error.to_string())?;
    run_git(git, &root, &["init", "--initial-branch=main"])?;
    run_git(
        git,
        &root,
        &[
            "commit",
            "--allow-empty",
            "--no-gpg-sign",
            "-m",
            "runtime smoke",
        ],
    )?;
    run_git(git, &root, &["rev-parse", "--verify", "HEAD"])?;
    drop(cleanup);
    Ok(())
}

fn run_git(git: &Path, root: &Path, args: &[&str]) -> Result<(), String> {
    run_git_output(git, root, args).map(|_| ())
}

fn run_git_output(git: &Path, root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let disabled_hooks = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let output = std_command(git, ProcessPolicy::HiddenCaptured)
        .arg("-c")
        .arg(format!("core.hooksPath={disabled_hooks}"))
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("global.gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_AUTHOR_NAME", "Hachimi Runtime Smoke")
        .env("GIT_AUTHOR_EMAIL", "runtime-smoke@hachimi.invalid")
        .env("GIT_COMMITTER_NAME", "Hachimi Runtime Smoke")
        .env("GIT_COMMITTER_EMAIL", "runtime-smoke@hachimi.invalid")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| error.to_string())?;
    output
        .status
        .success()
        .then_some(output.stdout)
        .ok_or_else(|| {
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(512)
                .collect()
        })
}

struct SmokeDirectory(std::path::PathBuf);

impl Drop for SmokeDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
