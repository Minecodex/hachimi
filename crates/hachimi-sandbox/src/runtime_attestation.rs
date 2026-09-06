// SPDX-License-Identifier: Apache-2.0
// Security boundary reviewed against openai/codex windows-sandbox-rs ACL/spawn smoke tests.
// Commit: 4c43465133428898aa84f0bfc02c306ed65fb66a
// Modified for Hachimi: AppContainer canaries and per-Run Checkout/Git/TEMP attestation.

use std::{io::Read, path::Path, path::PathBuf, process::Stdio};

use hachimi_protocol::{SandboxCapabilityReport, SandboxReadiness};
use sha2::{Digest, Sha256};

use crate::{
    SandboxSetupMarker,
    appcontainer::{APP_CONTAINER_NAME, AppContainerSid},
    deny_restricted_code_read, grant_restricted_code_access,
};

pub const SANDBOX_POLICY_VERSION: &str = "hachimi-windows-appcontainer-v4";
#[cfg(target_os = "macos")]
pub const MACOS_SANDBOX_POLICY_VERSION: &str = "hachimi-macos-seatbelt-v2";

/// Directory segment for per-user sandbox state on the host platform.
pub fn backend_dir_key() -> &'static str {
    #[cfg(windows)]
    {
        "windows"
    }
    #[cfg(target_os = "macos")]
    {
        "macos-seatbelt"
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        "unsupported"
    }
}

/// The policy version active on the host platform.
pub fn current_policy_version() -> &'static str {
    #[cfg(windows)]
    {
        SANDBOX_POLICY_VERSION
    }
    #[cfg(target_os = "macos")]
    {
        MACOS_SANDBOX_POLICY_VERSION
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        "hachimi-unsupported-platform"
    }
}

#[cfg(target_os = "macos")]
mod macos_attestation {
    use std::{path::Path, path::PathBuf, process::Stdio};

    use hachimi_protocol::{SandboxCapabilityReport, SandboxReadiness};

    use super::{SandboxSetupMarker, degraded as degraded_marker, hash_file};
    use crate::seatbelt::{BASE_POLICY, SEATBELT_EXECUTABLE};

    /// Live Seatbelt attestation: proves the sandbox binary exists, the staged
    /// canary matches its staged SHA-256, and the deny-default profile really
    /// enforces filesystem, network and process-tree boundaries.
    pub(crate) fn attest_macos_runtime(
        marker_path: &Path,
        canary: &Path,
        attestation_root: &Path,
        expected_integrity: &[(PathBuf, String)],
    ) -> SandboxCapabilityReport {
        if !Path::new(SEATBELT_EXECUTABLE).is_file() {
            return degraded_marker(
                SandboxReadiness::Unavailable,
                "seatbelt_missing",
                "/usr/bin/sandbox-exec is unavailable on this macOS build",
                None,
            );
        }
        let marker = match std::fs::read(marker_path) {
            Ok(bytes) => match serde_json::from_slice::<SandboxSetupMarker>(&bytes) {
                Ok(marker) => marker,
                Err(_) => {
                    return degraded_marker(
                        SandboxReadiness::SetupRequired,
                        "setup_marker_invalid",
                        "the macOS Seatbelt setup marker is invalid",
                        None,
                    );
                }
            },
            Err(_) => {
                return degraded_marker(
                    SandboxReadiness::SetupRequired,
                    "setup_marker_missing",
                    "macOS Seatbelt setup has not completed",
                    None,
                );
            }
        };
        if marker.version != super::MACOS_SANDBOX_POLICY_VERSION {
            return degraded_marker(
                SandboxReadiness::SetupRequired,
                "sandbox_policy_version_mismatch",
                "the installed Seatbelt policy must be repaired",
                Some(marker.version),
            );
        }
        if !canary.is_file() {
            return degraded_marker(
                SandboxReadiness::SetupRequired,
                "sandbox_runtime_binary_missing",
                "the sandbox canary binary is missing",
                Some(marker.version),
            );
        }
        for (path, expected) in expected_integrity {
            if !path.is_absolute() || !hash_file(path).is_ok_and(|actual| actual == *expected) {
                return degraded_marker(
                    SandboxReadiness::SetupRequired,
                    "sandbox_runtime_integrity_mismatch",
                    "a staged macOS sandbox binary failed SHA-256 attestation",
                    Some(marker.version),
                );
            }
        }
        match run_macos_canaries(canary, attestation_root) {
            Ok(()) => SandboxCapabilityReport {
                backend: "macos_seatbelt_v1".into(),
                readiness: SandboxReadiness::Ready,
                os_enforced: true,
                filesystem_enforced: true,
                process_enforced: true,
                network_enforced: true,
                version: Some(marker.version),
                stable_error_code: None,
                diagnostics: vec![
                    "seatbelt deny-default, filesystem boundary, deny-all network, fd-inheritance and process-group canaries passed"
                        .into(),
                ],
            },
            Err((code, message)) => degraded_marker(
                SandboxReadiness::Degraded,
                code,
                &message,
                Some(marker.version),
            ),
        }
    }

    fn run_macos_canaries(
        canary: &Path,
        attestation_root: &Path,
    ) -> Result<(), (&'static str, String)> {
        let root = attestation_root.join(format!("runtime-{}", std::process::id()));
        let allowed = root.join("allowed");
        let forbidden = root.join("forbidden");
        let created = std::fs::create_dir_all(&allowed).is_ok()
            && std::fs::create_dir_all(&forbidden).is_ok();
        if !created {
            return Err((
                "sandbox_canary_prepare_failed",
                "runtime attestation directories could not be created".into(),
            ));
        }
        let allowed = allowed
            .canonicalize()
            .map_err(|error| ("sandbox_canary_prepare_failed", error.to_string()))?;
        let forbidden = forbidden
            .canonicalize()
            .map_err(|error| ("sandbox_canary_prepare_failed", error.to_string()))?;
        let result = run_macos_canary_set(canary, &allowed, &forbidden);
        let _ = std::fs::remove_dir_all(&root);
        result
    }

    fn run_macos_canary_set(
        canary: &Path,
        allowed: &Path,
        forbidden: &Path,
    ) -> Result<(), (&'static str, String)> {
        let policy = canary_policy(allowed, forbidden);
        // 1. deny-default must be active even inside the allowed directory.
        require_seatbelt_success(
            canary,
            &policy,
            allowed,
            forbidden,
            &["--assert-seatbelt"],
            "sandbox_seatbelt_attestation_failed",
            "the Seatbelt deny-default profile is not active",
        )?;
        // 2. fd inheritance boundary: only stdio may be open at entry.
        require_seatbelt_success(
            canary,
            &policy,
            allowed,
            forbidden,
            &["--read-fd3"],
            "sandbox_fd_inheritance_attestation_failed",
            "an unexpected file descriptor was inherited by the sandboxed canary",
        )?;
        // 3. granted write works.
        let allowed_file = allowed.join("write-ok.txt");
        require_seatbelt_success(
            canary,
            &policy,
            allowed,
            forbidden,
            &["--touch", &allowed_file.to_string_lossy()],
            "sandbox_allowed_write_failed",
            "the sandboxed canary could not write its allowed directory",
        )?;
        // 4. writes outside grants are denied.
        let denied_file = forbidden.join("write-denied.txt");
        if run_seatbelt_canary(
            canary,
            &policy,
            allowed,
            forbidden,
            &["--touch", &denied_file.to_string_lossy()],
        )
        .is_ok_and(|status| status.success())
        {
            return Err((
                "sandbox_forbidden_write_succeeded",
                "the sandboxed canary escaped its filesystem grant".into(),
            ));
        }
        // 5. reads outside grants are denied.
        let forbidden_secret = forbidden.join("read-denied.txt");
        std::fs::write(&forbidden_secret, b"secret")
            .map_err(|error| ("sandbox_deny_read_attestation_failed", error.to_string()))?;
        if run_seatbelt_canary(
            canary,
            &policy,
            allowed,
            forbidden,
            &["--read", &forbidden_secret.to_string_lossy()],
        )
        .is_ok_and(|status| status.success())
        {
            return Err((
                "sandbox_forbidden_read_succeeded",
                "the sandboxed canary read a deny-read path".into(),
            ));
        }
        // 6. deny-all network.
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|error| ("sandbox_network_canary_prepare_failed", error.to_string()))?;
        let address = listener
            .local_addr()
            .map_err(|error| ("sandbox_network_canary_prepare_failed", error.to_string()))?;
        if run_seatbelt_canary(
            canary,
            &policy,
            allowed,
            forbidden,
            &["--network", &address.to_string()],
        )
        .is_ok_and(|status| status.success())
        {
            return Err((
                "sandbox_network_not_denied",
                "the sandboxed canary connected to a local network endpoint".into(),
            ));
        }
        drop(listener);
        // 7. process-group teardown reaps the sandboxed tree including
        // grandchildren spawned by the canary.
        let escaped_marker = allowed.join("grandchild-escaped.txt");
        let sleeper = sandboxed_sleeper(canary, &policy, allowed, forbidden, &escaped_marker)?;
        std::thread::sleep(std::time::Duration::from_millis(500));
        unsafe {
            libc::kill(-(sleeper as i32), libc::SIGKILL);
        }
        std::thread::sleep(std::time::Duration::from_millis(3500));
        if escaped_marker.exists() {
            return Err((
                "sandbox_process_tree_escape",
                "a sandboxed grandchild survived process-group teardown".into(),
            ));
        }
        Ok(())
    }

    fn canary_policy(_allowed: &Path, _forbidden: &Path) -> String {
        format!(
            "{base}(deny network*)\n\
             (allow file-read* file-test-existence file-map-executable (literal (param \"CANARY\")))\n\
             (allow file-read* file-write* (subpath (param \"ALLOWED\")))\n\
             (deny file-read* file-write* (subpath (param \"FORBIDDEN\")))\n\
             (allow file-read-metadata file-test-existence (path-ancestors (param \"ALLOWED\")))\n\
             (allow file-read-metadata file-test-existence (path-ancestors (param \"FORBIDDEN\")))\n",
            base = BASE_POLICY
        )
    }

    fn canary_definitions(allowed: &Path, forbidden: &Path) -> Vec<(String, PathBuf)> {
        vec![
            ("ALLOWED".into(), allowed.to_path_buf()),
            ("FORBIDDEN".into(), forbidden.to_path_buf()),
        ]
    }

    fn canary_command(
        canary: &Path,
        policy: &str,
        definitions: &[(String, PathBuf)],
        cwd: &Path,
        args: &[&str],
    ) -> std::process::Command {
        let mut command = hachimi_process_policy::std_command(
            SEATBELT_EXECUTABLE,
            hachimi_process_policy::ProcessPolicy::HiddenCaptured,
        );
        command.arg("-p").arg(policy).arg("-D").arg(format!(
            "CANARY={}",
            canary
                .canonicalize()
                .unwrap_or_else(|_| canary.to_path_buf())
                .display()
        ));
        for (key, path) in definitions {
            command.arg("-D").arg(format!("{key}={}", path.display()));
        }
        command
            .arg("--")
            .arg(canary)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    fn run_seatbelt_canary(
        canary: &Path,
        policy: &str,
        allowed: &Path,
        forbidden: &Path,
        args: &[&str],
    ) -> std::io::Result<std::process::ExitStatus> {
        canary_command(
            canary,
            policy,
            &canary_definitions(allowed, forbidden),
            allowed,
            args,
        )
        .status()
    }

    fn require_seatbelt_success(
        canary: &Path,
        policy: &str,
        allowed: &Path,
        forbidden: &Path,
        args: &[&str],
        code: &'static str,
        message: &str,
    ) -> Result<(), (&'static str, String)> {
        if run_seatbelt_canary(canary, policy, allowed, forbidden, args)
            .is_ok_and(|status| status.success())
        {
            Ok(())
        } else {
            Err((code, message.into()))
        }
    }

    /// Spawns a sandboxed canary whose grandchild writes a marker after 3s.
    /// Returns the sandbox-exec process id (a process-group leader).
    fn sandboxed_sleeper(
        canary: &Path,
        policy: &str,
        allowed: &Path,
        forbidden: &Path,
        marker: &Path,
    ) -> Result<u32, (&'static str, String)> {
        use std::os::unix::process::CommandExt as _;
        let mut command = canary_command(
            canary,
            policy,
            &canary_definitions(allowed, forbidden),
            allowed,
            &[
                "--spawn-child-sleep-touch",
                &canary.to_string_lossy(),
                &marker.to_string_lossy(),
            ],
        );
        command.process_group(0);
        command
            .spawn()
            .map(|child| child.id())
            .map_err(|error| ("sandbox_process_tree_spawn_failed", error.to_string()))
    }

    /// Per-checkout boundary proof for workspace launches: the checkout and
    /// Run temp are writable, the worker program is readable, read-only roots
    /// (`.git`, shared common dir) reject writes.
    pub(crate) fn attest_macos_workspace_boundaries(
        canary: &Path,
        checkout: &Path,
        run_temp: &Path,
        worker_program: &Path,
        read_only_roots: &[PathBuf],
    ) -> Result<(), String> {
        let checkout = checkout
            .canonicalize()
            .map_err(|error| format!("checkout canonicalization: {error}"))?;
        let run_temp = run_temp
            .canonicalize()
            .map_err(|error| format!("run temp canonicalization: {error}"))?;
        let mut policy = format!(
            "{base}(deny network*)\n\
             (allow file-read* file-test-existence file-map-executable (literal (param \"CANARY\")))\n\
             (allow file-read* file-test-existence file-map-executable (literal (param \"WORKER\")))\n\
             (allow file-read* file-write* (subpath (param \"CHECKOUT\")))\n\
             (allow file-read* file-write* (subpath (param \"RUN_TEMP\")))\n\
             (allow file-read-metadata file-test-existence (path-ancestors (param \"CANARY\")))\n\
             (allow file-read-metadata file-test-existence (path-ancestors (param \"WORKER\")))\n\
             (allow file-read-metadata file-test-existence (path-ancestors (param \"CHECKOUT\")))\n\
             (allow file-read-metadata file-test-existence (path-ancestors (param \"RUN_TEMP\")))\n",
            base = BASE_POLICY
        );
        let mut definitions: Vec<(String, PathBuf)> = vec![
            ("WORKER".into(), worker_program.to_path_buf()),
            ("CHECKOUT".into(), checkout.clone()),
            ("RUN_TEMP".into(), run_temp.clone()),
        ];
        for (index, root) in read_only_roots.iter().enumerate() {
            let root = root.canonicalize().map_err(|error| error.to_string())?;
            let key = format!("READONLY_{index}");
            policy.push_str(&format!(
                "(allow file-read* file-test-existence (subpath (param \"{key}\")))\n(deny file-write* (subpath (param \"{key}\")))\n(allow file-read-metadata file-test-existence (path-ancestors (param \"{key}\")))\n"
            ));
            definitions.push((key, root));
        }
        let nonce = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let checkout_probe = checkout.join(format!(".hachimi-sandbox-write-{nonce}"));
        let temp_probe = run_temp.join(format!("hachimi-sandbox-temp-{nonce}"));
        let result = (|| {
            require_workspace_success(
                canary,
                &policy,
                &definitions,
                &checkout,
                &["--touch", &checkout_probe.to_string_lossy()],
                "sandbox_checkout_write_attestation_failed",
            )?;
            require_workspace_success(
                canary,
                &policy,
                &definitions,
                &checkout,
                &["--touch", &temp_probe.to_string_lossy()],
                "sandbox_temp_write_attestation_failed",
            )?;
            require_workspace_success(
                canary,
                &policy,
                &definitions,
                &checkout,
                &["--read", &worker_program.to_string_lossy()],
                "sandbox_worker_read_attestation_failed",
            )?;
            for root in read_only_roots {
                let root = root.canonicalize().map_err(|error| error.to_string())?;
                let denied = root.join(format!("hachimi-sandbox-denied-{nonce}"));
                if canary_command(
                    canary,
                    &policy,
                    &definitions,
                    &checkout,
                    &["--touch", &denied.to_string_lossy()],
                )
                .status()
                .is_ok_and(|status| status.success())
                {
                    let _ = std::fs::remove_file(&denied);
                    return Err("sandbox_read_only_root_write_succeeded".to_owned());
                }
            }
            Ok(())
        })();
        let _ = std::fs::remove_file(checkout_probe);
        let _ = std::fs::remove_file(temp_probe);
        result
    }

    fn require_workspace_success(
        canary: &Path,
        policy: &str,
        definitions: &[(String, PathBuf)],
        cwd: &Path,
        args: &[&str],
        code: &str,
    ) -> Result<(), String> {
        if canary_command(canary, policy, definitions, cwd, args)
            .status()
            .is_ok_and(|status| status.success())
        {
            Ok(())
        } else {
            Err(code.to_owned())
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos_attestation::attest_macos_runtime;

#[allow(clippy::needless_return)]
pub fn attest_workspace_boundaries(
    launcher: &Path,
    canary: &Path,
    checkout: &Path,
    run_temp: &Path,
    worker_program: &Path,
    read_only_roots: &[std::path::PathBuf],
) -> Result<(), String> {
    #[cfg(windows)]
    {
        return attest_workspace_boundaries_inner(
            launcher,
            canary,
            checkout,
            run_temp,
            worker_program,
            read_only_roots,
        );
    }
    #[cfg(target_os = "macos")]
    {
        let _ = launcher;
        return macos_attestation::attest_macos_workspace_boundaries(
            canary,
            checkout,
            run_temp,
            worker_program,
            read_only_roots,
        );
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (
            launcher,
            canary,
            checkout,
            run_temp,
            worker_program,
            read_only_roots,
        );
        return Err("workspace boundary attestation requires Windows or macOS".into());
    }
}

#[cfg(windows)]
fn attest_workspace_boundaries_inner(
    launcher: &Path,
    canary: &Path,
    checkout: &Path,
    run_temp: &Path,
    worker_program: &Path,
    read_only_roots: &[std::path::PathBuf],
) -> Result<(), String> {
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let checkout_probe = checkout.join(format!(".hachimi-sandbox-write-{nonce}"));
    let temp_probe = run_temp.join(format!("hachimi-sandbox-temp-{nonce}"));
    let result = (|| {
        require_canary_success(
            launcher,
            canary,
            checkout,
            &["--touch", &checkout_probe.to_string_lossy()],
            "sandbox_checkout_write_attestation_failed",
        )?;
        require_canary_success(
            launcher,
            canary,
            run_temp,
            &["--touch", &temp_probe.to_string_lossy()],
            "sandbox_temp_write_attestation_failed",
        )?;
        require_canary_success(
            launcher,
            canary,
            checkout,
            &["--read", &worker_program.to_string_lossy()],
            "sandbox_worker_read_attestation_failed",
        )?;
        for root in read_only_roots {
            let denied = root.join(format!("hachimi-sandbox-denied-{nonce}"));
            if run_canary(
                launcher,
                canary,
                checkout,
                &["--touch", &denied.to_string_lossy()],
            )
            .is_ok_and(|status| status.success())
            {
                let _ = std::fs::remove_file(&denied);
                return Err("sandbox_read_only_root_write_succeeded".into());
            }
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(checkout_probe);
    let _ = std::fs::remove_file(temp_probe);
    result
}

#[cfg(windows)]
fn require_canary_success(
    launcher: &Path,
    canary: &Path,
    cwd: &Path,
    arguments: &[&str],
    code: &str,
) -> Result<(), String> {
    if run_canary(launcher, canary, cwd, arguments).is_ok_and(|status| status.success()) {
        Ok(())
    } else {
        Err(code.into())
    }
}

pub fn attest_windows_runtime(
    marker_path: &Path,
    launcher: &Path,
    canary: &Path,
    attestation_root: &Path,
) -> SandboxCapabilityReport {
    attest_windows_runtime_with_integrity(marker_path, launcher, canary, attestation_root, &[])
}

pub(crate) fn attest_windows_runtime_with_integrity(
    marker_path: &Path,
    launcher: &Path,
    canary: &Path,
    attestation_root: &Path,
    expected_integrity: &[(PathBuf, String)],
) -> SandboxCapabilityReport {
    if !cfg!(windows) {
        return degraded(
            SandboxReadiness::Unavailable,
            "unsupported_os",
            "the enforced sandbox backend is Windows-only",
            None,
        );
    }
    let marker = match read_marker(marker_path) {
        Ok(marker) => marker,
        Err(report) => return report,
    };
    if marker.version != SANDBOX_POLICY_VERSION {
        return degraded(
            SandboxReadiness::SetupRequired,
            "sandbox_policy_version_mismatch",
            "the installed sandbox policy must be repaired",
            Some(marker.version),
        );
    }
    let resolved_identity = AppContainerSid::resolve()
        .and_then(|identity| identity.to_string_sid())
        .ok();
    if marker.app_container_name.as_deref() != Some(APP_CONTAINER_NAME)
        || marker.app_container_sid.as_deref() != resolved_identity.as_deref()
    {
        return degraded(
            SandboxReadiness::SetupRequired,
            "sandbox_identity_mismatch",
            "the installed AppContainer identity must be repaired",
            Some(marker.version),
        );
    }
    let mut missing = Vec::new();
    if !marker.acl_component {
        missing.push("filesystem ACL component");
    }
    if !marker.token_component {
        missing.push("restricted token component");
    }
    if !marker.network_component {
        missing.push("deny-all network policy component");
    }
    if !missing.is_empty() {
        return degraded(
            SandboxReadiness::Degraded,
            "sandbox_setup_incomplete",
            &format!("missing {}", missing.join(", ")),
            Some(marker.version),
        );
    }
    if !launcher.is_file() || !canary.is_file() {
        return degraded(
            SandboxReadiness::SetupRequired,
            "sandbox_runtime_binary_missing",
            "the sandbox launcher or canary binary is missing",
            Some(marker.version),
        );
    }
    if !expected_integrity.is_empty()
        && let Err((code, message)) = verify_managed_runtime(launcher, expected_integrity)
    {
        return degraded(
            SandboxReadiness::SetupRequired,
            code,
            &message,
            Some(marker.version),
        );
    }
    let canary_root = attestation_root.join(format!("runtime-{}", std::process::id()));
    let allowed = canary_root.join("allowed");
    let forbidden = canary_root.join("forbidden");
    if std::fs::create_dir_all(&allowed).is_err() || std::fs::create_dir_all(&forbidden).is_err() {
        return degraded(
            SandboxReadiness::Degraded,
            "sandbox_canary_prepare_failed",
            "runtime attestation directories could not be created",
            Some(marker.version),
        );
    }
    let result = run_canaries(launcher, canary, &allowed, &forbidden);
    let _ = std::fs::remove_dir_all(&canary_root);
    match result {
        Ok(()) => SandboxCapabilityReport {
            backend: "windows_restricted_process_v1".into(),
            readiness: SandboxReadiness::Ready,
            os_enforced: true,
            filesystem_enforced: true,
            process_enforced: true,
            network_enforced: true,
            version: Some(marker.version),
            stable_error_code: None,
            diagnostics: vec![
                "restricted token, Job Object, filesystem boundary, and deny-all network canaries passed"
                    .into(),
            ],
        },
        Err((code, message)) => degraded(
            SandboxReadiness::Degraded,
            code,
            message,
            Some(marker.version),
        ),
    }
}

fn verify_managed_runtime(
    launcher: &Path,
    expected_integrity: &[(PathBuf, String)],
) -> Result<(), (&'static str, String)> {
    let root = launcher.parent().ok_or_else(|| {
        (
            "sandbox_runtime_path_invalid",
            "managed Sandbox Runtime has no root directory".into(),
        )
    })?;
    for (path, expected) in expected_integrity {
        if !path.is_absolute()
            || !path.starts_with(root)
            || !hash_file(path).is_ok_and(|actual| actual == expected.as_str())
        {
            return Err((
                "sandbox_runtime_integrity_mismatch",
                format!(
                    "managed Runtime file failed SHA-256 attestation: {}",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn run_canaries(
    launcher: &Path,
    canary: &Path,
    allowed: &Path,
    forbidden: &Path,
) -> Result<(), (&'static str, &'static str)> {
    grant_restricted_code_access(allowed, true).map_err(|_| {
        (
            "sandbox_acl_attestation_failed",
            "restricted-code ACL could not be applied to the canary directory",
        )
    })?;
    deny_restricted_code_read(forbidden).map_err(|_| {
        (
            "sandbox_deny_read_attestation_failed",
            "the restricted-code deny-read ACL could not be applied to the canary directory",
        )
    })?;
    let allowed_file = allowed.join("write-ok.txt");
    if !run_canary(launcher, canary, allowed, &["--assert-job"])
        .is_ok_and(|status| status.success())
    {
        return Err((
            "sandbox_job_attestation_failed",
            "the restricted canary was not assigned to a Job Object",
        ));
    }
    if !run_canary(
        launcher,
        canary,
        allowed,
        &["--touch", &allowed_file.to_string_lossy()],
    )
    .is_ok_and(|status| status.success())
    {
        return Err((
            "sandbox_allowed_write_failed",
            "the restricted canary could not write its allowed directory",
        ));
    }
    let forbidden_file = forbidden.join("write-denied.txt");
    if run_canary(
        launcher,
        canary,
        allowed,
        &["--touch", &forbidden_file.to_string_lossy()],
    )
    .is_ok_and(|status| status.success())
    {
        return Err((
            "sandbox_forbidden_write_succeeded",
            "the restricted canary escaped its filesystem grant",
        ));
    }
    let forbidden_secret = forbidden.join("read-denied.txt");
    std::fs::write(&forbidden_secret, b"secret").map_err(|_| {
        (
            "sandbox_deny_read_attestation_failed",
            "the deny-read canary fixture could not be created",
        )
    })?;
    if run_canary(
        launcher,
        canary,
        allowed,
        &["--read", &forbidden_secret.to_string_lossy()],
    )
    .is_ok_and(|status| status.success())
    {
        return Err((
            "sandbox_forbidden_read_succeeded",
            "the restricted canary read a deny-read path",
        ));
    }
    let listener =
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|_| {
            (
                "sandbox_network_canary_prepare_failed",
                "the local network canary listener could not start",
            )
        })?;
    let address = listener.local_addr().map_err(|_| {
        (
            "sandbox_network_canary_prepare_failed",
            "the local network canary address could not be read",
        )
    })?;
    if run_canary(
        launcher,
        canary,
        allowed,
        &["--network", &address.to_string()],
    )
    .is_ok_and(|status| status.success())
    {
        return Err((
            "sandbox_network_not_denied",
            "the restricted canary connected to a local network endpoint",
        ));
    }
    Ok(())
}

fn run_canary(
    launcher: &Path,
    canary: &Path,
    cwd: &Path,
    arguments: &[&str],
) -> std::io::Result<std::process::ExitStatus> {
    hachimi_process_policy::std_command(
        launcher,
        hachimi_process_policy::ProcessPolicy::HiddenCaptured,
    )
    .arg("--")
    .arg(canary)
    .args(arguments)
    .current_dir(cwd)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .status()
}

fn read_marker(marker_path: &Path) -> Result<SandboxSetupMarker, SandboxCapabilityReport> {
    let bytes = std::fs::read(marker_path).map_err(|error| {
        let (readiness, code) = if error.kind() == std::io::ErrorKind::NotFound {
            (SandboxReadiness::SetupRequired, "setup_marker_missing")
        } else {
            (SandboxReadiness::Degraded, "setup_marker_unreadable")
        };
        degraded(
            readiness,
            code,
            "per-user Windows sandbox setup has not completed",
            None,
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        degraded(
            SandboxReadiness::SetupRequired,
            "setup_marker_invalid",
            "the Windows sandbox setup marker is invalid",
            None,
        )
    })
}

fn degraded(
    readiness: SandboxReadiness,
    code: &str,
    diagnostic: &str,
    version: Option<String>,
) -> SandboxCapabilityReport {
    degraded_with_backend(
        if cfg!(target_os = "macos") {
            "macos_seatbelt_v1"
        } else {
            "windows_restricted_process_v1"
        },
        readiness,
        code,
        diagnostic,
        version,
    )
}

fn degraded_with_backend(
    backend: &str,
    readiness: SandboxReadiness,
    code: &str,
    diagnostic: &str,
    version: Option<String>,
) -> SandboxCapabilityReport {
    SandboxCapabilityReport {
        backend: backend.into(),
        readiness,
        os_enforced: false,
        filesystem_enforced: false,
        process_enforced: false,
        network_enforced: false,
        version,
        stable_error_code: Some(code.into()),
        diagnostics: vec![diagnostic.into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_runtime_integrity_detects_sidecar_tampering() {
        let root = tempfile::tempdir().expect("root");
        let launcher = root.path().join("hachimi-sandbox-launcher.exe");
        std::fs::write(&launcher, b"launcher").expect("launcher");
        let expected = vec![(
            launcher.clone(),
            hash_file(&launcher).expect("launcher hash"),
        )];
        verify_managed_runtime(&launcher, &expected).expect("integrity");

        std::fs::write(&launcher, b"tampered").expect("tamper launcher");
        assert_eq!(
            verify_managed_runtime(&launcher, &expected)
                .expect_err("tamper must fail")
                .0,
            "sandbox_runtime_integrity_mismatch"
        );
    }
}
