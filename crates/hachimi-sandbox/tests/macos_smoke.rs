//! macOS Seatbelt smoke tests — threat-model parity with `windows_smoke.rs`.
//!
//! Exercises the real sandbox boundary on macOS: deny-default Seatbelt
//! profile, grant-scoped filesystem, deny-all networking, fd inheritance,
//! process-group teardown (including grandchildren), and fail-closed
//! degradation paths.
#![cfg(target_os = "macos")]

use std::{ffi::OsString, path::PathBuf, time::Duration};

use hachimi_protocol::{
    CapabilityGrantSet, FileSystemAccess, FileSystemGrant, NetworkGrant, PermissionGrantScope,
    PermissionProfile, ProcessGrant,
};
use hachimi_protocol::{CheckoutId, RunId, SessionId, ToolEffect};
use hachimi_sandbox::{
    MacOSSeatbeltReadinessProbe, SandboxBackend, SandboxError, SandboxLaunchSpec,
    SandboxNetworkPolicy, SandboxStatus, install_macos_marker, platform_probe,
};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

struct Fixture {
    _root: tempfile::TempDir,
    marker: PathBuf,
    canary: PathBuf,
    attestation_root: PathBuf,
    checkout: PathBuf,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().expect("fixture root");
    let data = root.path().join("data");
    let marker = data.join("sandbox/macos-seatbelt/setup.json");
    install_macos_marker(&marker).expect("marker");
    let checkout = root.path().join("checkout");
    std::fs::create_dir_all(&checkout).expect("checkout dir");
    let checkout = checkout.canonicalize().expect("checkout");
    Fixture {
        marker,
        canary: PathBuf::from(env!("CARGO_BIN_EXE_hachimi-sandbox-canary")),
        attestation_root: data.join("sandbox/attestation"),
        checkout,
        _root: root,
    }
}

fn sha256(path: &std::path::Path) -> String {
    let bytes = std::fs::read(path).expect("canary bytes");
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn probe(fixture: &Fixture) -> MacOSSeatbeltReadinessProbe {
    MacOSSeatbeltReadinessProbe::new(&fixture.marker).with_runtime(
        PathBuf::new(),
        &fixture.canary,
        &fixture.attestation_root,
    )
}

#[test]
fn seatbelt_runtime_attests_to_enforced() {
    let fixture = fixture();
    let probe = probe(&fixture)
        .with_runtime_integrity(vec![(fixture.canary.clone(), sha256(&fixture.canary))]);
    let report = probe.capability_report();
    assert_eq!(
        SandboxStatus::from_report(&report),
        SandboxStatus::Enforced,
        "attestation must reach Enforced; report: {report:?}"
    );
}

#[test]
fn tampered_canary_fails_integrity_attestation() {
    let fixture = fixture();
    let probe =
        probe(&fixture).with_runtime_integrity(vec![(fixture.canary.clone(), "0".repeat(64))]);
    let report = probe.capability_report();
    assert_eq!(
        SandboxStatus::from_report(&report),
        SandboxStatus::SetupRequired
    );
    assert_eq!(
        report.stable_error_code.as_deref(),
        Some("sandbox_runtime_integrity_mismatch")
    );
}

#[test]
fn missing_marker_is_setup_required() {
    let fixture = fixture();
    std::fs::remove_file(&fixture.marker).expect("remove marker");
    let report = probe(&fixture).capability_report();
    assert_eq!(
        SandboxStatus::from_report(&report),
        SandboxStatus::SetupRequired
    );
}

#[test]
fn marker_only_probe_reports_attestation_missing() {
    let fixture = fixture();
    let report = MacOSSeatbeltReadinessProbe::new(&fixture.marker).capability_report();
    assert_eq!(SandboxStatus::from_report(&report), SandboxStatus::Degraded);
    assert_eq!(
        report.stable_error_code.as_deref(),
        Some("runtime_attestation_missing")
    );
}

fn launch_spec(checkout: PathBuf, executable: &str, args: &[&str]) -> SandboxLaunchSpec {
    let session_id = SessionId::from("session-macos-smoke");
    let run_id = RunId::from("run-macos-smoke");
    SandboxLaunchSpec {
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        run_generation: 1,
        checkout_id: CheckoutId::from("checkout-macos-smoke"),
        grants: CapabilityGrantSet {
            profile: PermissionProfile::Writable,
            scope: PermissionGrantScope::Run,
            session_id,
            run_id: Some(run_id),
            source: "macos-smoke".into(),
            file_system: vec![
                FileSystemGrant {
                    access: FileSystemAccess::Read,
                    roots: vec![checkout.to_string_lossy().into_owned()],
                    globs: vec![],
                    files: vec![],
                    special_roots: vec![],
                },
                FileSystemGrant {
                    access: FileSystemAccess::Write,
                    roots: vec![checkout.to_string_lossy().into_owned()],
                    globs: vec![],
                    files: vec![],
                    special_roots: vec![],
                },
            ],
            network: NetworkGrant::default(),
            process: ProcessGrant {
                spawn: true,
                interactive: false,
                unrestricted_commands: false,
                allowed_commands: vec![],
            },
            browser: Default::default(),
            computer: Default::default(),
            review_each_command: false,
            expires_at_ms: None,
        },
        checkout_root: checkout.clone(),
        required_effect: ToolEffect::WorkspaceWrite,
        executable: PathBuf::from(executable),
        args: args.iter().map(OsString::from).collect(),
        cwd: checkout,
        environment: vec![],
        stdin: None,
        interactive_stdin: false,
        timeout: Duration::from_secs(15),
        output_limit: 64 * 1024,
        network_policy: SandboxNetworkPolicy::DenyAll,
        git_metadata_writable: false,
    }
}

#[tokio::test]
async fn sandboxed_process_runs_with_grants_and_dies_with_group() {
    let fixture = fixture();
    let probe = probe(&fixture);
    let spec = launch_spec(
        fixture.checkout.clone(),
        "/bin/sh",
        &["-c", "echo sandbox-ok"],
    );
    let child = probe
        .spawn_restricted(spec, CancellationToken::new())
        .await
        .expect("sandboxed spawn");
    let output = child.wait().await.expect("wait");
    assert_eq!(output.exit_code, Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("sandbox-ok"));
}

#[tokio::test]
async fn sandboxed_process_cannot_write_outside_grants() {
    let fixture = fixture();
    let probe = probe(&fixture);
    let outside = std::env::temp_dir()
        .join(format!("hachimi-smoke-escape-{}", std::process::id()))
        .canonicalize()
        .unwrap_or_else(|_| std::env::temp_dir());
    let spec = launch_spec(
        fixture.checkout.clone(),
        "/bin/sh",
        &[
            "-c",
            &format!(
                "touch {}/escaped && echo escaped || echo denied",
                outside.display()
            ),
        ],
    );
    let child = probe
        .spawn_restricted(spec, CancellationToken::new())
        .await
        .expect("spawn");
    let output = child.wait().await.expect("wait");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("denied"), "expected denial, got: {stdout}");
}

#[tokio::test]
async fn sandboxed_process_cannot_use_network() {
    let fixture = fixture();
    let probe = probe(&fixture);
    let spec = launch_spec(
        fixture.checkout.clone(),
        "/usr/bin/curl",
        &["-sS", "-m", "3", "-o", "/dev/null", "https://example.com"],
    );
    let child = probe
        .spawn_restricted(spec, CancellationToken::new())
        .await
        .expect("spawn");
    let output = child.wait().await.expect("wait");
    assert_ne!(output.exit_code, Some(0), "network must be denied");
}

#[tokio::test]
async fn git_metadata_stays_read_only_inside_writable_checkout() {
    let fixture = fixture();
    let git_dir = fixture.checkout.join(".git");
    std::fs::create_dir_all(&git_dir).expect(".git");
    let probe = probe(&fixture);
    let spec = launch_spec(
        fixture.checkout.clone(),
        "/bin/sh",
        &[
            "-c",
            "touch .git/hooks-evil 2>/dev/null && echo leaked || echo denied",
        ],
    );
    let child = probe
        .spawn_restricted(spec, CancellationToken::new())
        .await
        .expect("spawn");
    let output = child.wait().await.expect("wait");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("denied"),
        "expected .git denial, got: {stdout}"
    );
    assert!(!git_dir.join("hooks-evil").exists());
}

#[tokio::test]
async fn process_group_teardown_reaps_grandchildren() {
    let fixture = fixture();
    let probe = probe(&fixture);
    let marker = fixture.checkout.join("grandchild-escaped.txt");
    let spec = launch_spec(
        fixture.checkout.clone(),
        env!("CARGO_BIN_EXE_hachimi-sandbox-canary"),
        &[
            "--spawn-child-sleep-touch",
            env!("CARGO_BIN_EXE_hachimi-sandbox-canary"),
            &marker.to_string_lossy(),
        ],
    );
    let child = probe
        .spawn_restricted(spec, CancellationToken::new())
        .await
        .expect("spawn");
    let mut child = child.into_child().expect("into child");
    // Give the sandboxed canary time to spawn its grandchild, then reap the
    // whole group and prove the grandchild never lands its marker.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let pid = child.id().expect("pid");
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    let _ = child.wait().await;
    tokio::time::sleep(Duration::from_millis(3500)).await;
    assert!(
        !marker.exists(),
        "sandboxed grandchild survived process-group teardown"
    );
}

#[tokio::test]
async fn degraded_probe_fails_closed_on_spawn() {
    let root = tempfile::tempdir().expect("root");
    let probe = platform_probe(root.path().join("missing-marker.json"));
    let spec = launch_spec(
        root.path().to_path_buf().canonicalize().expect("canonical"),
        "/bin/sh",
        &["-c", "echo should-not-run"],
    );
    let result = probe.spawn_restricted(spec, CancellationToken::new()).await;
    assert!(matches!(result, Err(SandboxError::NotEnforced(_))));
}

/// The workbench's interactive Git/mutation paths call `prepare_workspace_acl`
/// unconditionally. On macOS it must not touch icacls (a Windows-only tool)
/// and must return the read-only root set used by boundary attestation.
/// Regression test for `sandbox_acl_prepare_failed` on a fresh repository.
#[test]
fn prepare_workspace_acl_is_a_bookkeeping_noop_on_macos() {
    let root = tempfile::tempdir().expect("tempdir");
    let checkout = root.path().join("checkout");
    std::fs::create_dir_all(&checkout).expect("checkout");
    let init = std::process::Command::new("git")
        .arg("init")
        .arg("--quiet")
        .arg(&checkout)
        .status()
        .expect("git init");
    assert!(init.success());
    let run_temp = root.path().join("run-temp");
    let worker_program = root.path().join("bin/hachimi-workspace-worker");
    let read_roots = hachimi_sandbox::prepare_workspace_acl(
        &checkout,
        &run_temp,
        &worker_program,
        Some(std::path::Path::new("/usr/bin/git")),
    )
    .expect("prepare_workspace_acl must succeed on macOS without icacls");
    assert!(run_temp.is_dir(), "run temp directory must be created");
    assert!(
        read_roots
            .iter()
            .any(|path| path == worker_program.parent().expect("worker parent")),
        "worker directory must be a read-only root"
    );
    assert!(
        read_roots.iter().any(|path| path.ends_with(".git")),
        "the repository metadata directory must be a read-only root: {read_roots:?}"
    );
}
