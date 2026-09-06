use std::{
    io::Write,
    path::{Path, PathBuf},
};

use atomic_write_file::AtomicWriteFile;
use serde::Serialize;
use sha2::{Digest, Sha256};

use hachimi_protocol::RuntimeComponentId;

use crate::{
    runtime_supervisor::RuntimeSupervisor, workbench_commands::set_managed_workspace_runtime,
};

#[derive(Debug, Clone)]
pub(super) struct ManagedSandboxRuntime {
    pub root: PathBuf,
    pub setup: PathBuf,
    pub launcher: PathBuf,
    pub canary: PathBuf,
    pub worker: PathBuf,
    pub expected_integrity: Vec<(PathBuf, String)>,
    pub issue_codes: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeManifest<'a> {
    policy_version: &'a str,
    files: Vec<RuntimeManifestFile<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeManifestFile<'a> {
    name: &'a str,
    sha256: &'a str,
}

fn sidecar_definitions() -> Vec<(&'static str, &'static str)> {
    let all = [
        (
            "hachimi-sandbox-setup",
            env!("HACHIMI_SANDBOX_SETUP_SHA256"),
        ),
        (
            "hachimi-sandbox-launcher",
            env!("HACHIMI_SANDBOX_LAUNCHER_SHA256"),
        ),
        (
            "hachimi-sandbox-canary",
            env!("HACHIMI_SANDBOX_CANARY_SHA256"),
        ),
        (
            "hachimi-sandbox-attest",
            env!("HACHIMI_SANDBOX_ATTEST_SHA256"),
        ),
        (
            "hachimi-workspace-worker",
            env!("HACHIMI_WORKSPACE_WORKER_SHA256"),
        ),
    ];
    if cfg!(windows) {
        all.to_vec()
    } else {
        // macOS Seatbelt launches via /usr/bin/sandbox-exec, so only the
        // canary and the workspace worker are staged.
        all.into_iter()
            .filter(|entry| {
                matches!(
                    entry.0,
                    "hachimi-sandbox-canary" | "hachimi-workspace-worker"
                )
            })
            .collect()
    }
}

pub(super) fn stage(
    data_root: &Path,
    resource_root: &Path,
) -> Result<ManagedSandboxRuntime, String> {
    let root = data_root
        .join("sandbox")
        .join(hachimi_sandbox::backend_dir_key())
        .join("runtime")
        .join(hachimi_sandbox::current_policy_version());
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let definitions = sidecar_definitions();
    let mut issues = Vec::new();
    for (name, expected) in &definitions {
        let result = packaged_sidecar_path(resource_root, name)
            .and_then(|source| stage_file(&source, &root.join(executable_name(name)), expected));
        if let Err(error) = result {
            let code = sidecar_error_code(name);
            tracing::error!(code, %error, resource = name, "Packaged runtime resource staging failed");
            issues.push(code);
        }
    }
    let runtime = ManagedSandboxRuntime {
        setup: root.join(executable_name("hachimi-sandbox-setup")),
        launcher: root.join(executable_name("hachimi-sandbox-launcher")),
        canary: root.join(executable_name("hachimi-sandbox-canary")),
        worker: root.join(executable_name("hachimi-workspace-worker")),
        root,
        expected_integrity: definitions
            .iter()
            .map(|(name, expected)| {
                (
                    data_root
                        .join("sandbox")
                        .join(hachimi_sandbox::backend_dir_key())
                        .join("runtime")
                        .join(hachimi_sandbox::current_policy_version())
                        .join(executable_name(name)),
                    (*expected).to_owned(),
                )
            })
            .collect(),
        issue_codes: Vec::new(),
    };
    if let Err(error) = write_manifest(&runtime, &definitions) {
        tracing::error!(code = "runtime_manifest_write_failed", %error, "Runtime manifest write failed");
        issues.push("runtime_manifest_write_failed");
    }
    if runtime.worker.is_file()
        && let Err(error) =
            set_managed_workspace_runtime(runtime.root.clone(), runtime.worker.clone())
    {
        tracing::error!(code = "workspace_worker_registration_failed", %error, "Workspace Worker registration failed");
        issues.push("workspace_worker_registration_failed");
    }
    let mut runtime = runtime;
    issues.sort_unstable();
    issues.dedup();
    runtime.issue_codes = issues;
    Ok(runtime)
}

pub(super) fn stage_or_degrade(
    data_root: &Path,
    resource_root: &Path,
    supervisor: RuntimeSupervisor,
) -> ManagedSandboxRuntime {
    let runtime = stage(data_root, resource_root).unwrap_or_else(|error| {
        tracing::error!(code = "internal_resource_storage_failed", %error, "Internal runtime staging root is unavailable");
        let mut runtime = layout(data_root);
        runtime.issue_codes.push("internal_resource_storage_failed");
        runtime
    });
    publish_health(&supervisor, &runtime);
    if !runtime.issue_codes.is_empty() {
        let data_root = data_root.to_owned();
        let resource_root = resource_root.to_owned();
        let retry = supervisor.retry_signal(RuntimeComponentId::InternalResources);
        tauri::async_runtime::spawn(async move {
            loop {
                retry.notified().await;
                let next = stage(&data_root, &resource_root).unwrap_or_else(|error| {
                    tracing::error!(code = "internal_resource_storage_failed", %error, "Internal runtime restaging failed");
                    let mut runtime = layout(&data_root);
                    runtime.issue_codes.push("internal_resource_storage_failed");
                    runtime
                });
                publish_health(&supervisor, &next);
                if next.issue_codes.is_empty() {
                    break;
                }
            }
        });
    }
    runtime
}

fn layout(data_root: &Path) -> ManagedSandboxRuntime {
    let root = data_root
        .join("sandbox")
        .join(hachimi_sandbox::backend_dir_key())
        .join("runtime")
        .join(hachimi_sandbox::current_policy_version());
    let definitions = sidecar_definitions();
    ManagedSandboxRuntime {
        setup: root.join(executable_name("hachimi-sandbox-setup")),
        launcher: root.join(executable_name("hachimi-sandbox-launcher")),
        canary: root.join(executable_name("hachimi-sandbox-canary")),
        worker: root.join(executable_name("hachimi-workspace-worker")),
        root,
        expected_integrity: definitions
            .into_iter()
            .map(|(name, expected)| {
                (
                    data_root
                        .join("sandbox")
                        .join(hachimi_sandbox::backend_dir_key())
                        .join("runtime")
                        .join(hachimi_sandbox::current_policy_version())
                        .join(executable_name(name)),
                    expected.to_owned(),
                )
            })
            .collect(),
        issue_codes: Vec::new(),
    }
}

fn publish_health(supervisor: &RuntimeSupervisor, runtime: &ManagedSandboxRuntime) {
    supervisor
        .replace_internal_resource_issues("sandbox_runtime", runtime.issue_codes.iter().copied());
}

fn sidecar_error_code(name: &str) -> &'static str {
    match name {
        "hachimi-workspace-worker" => "workspace_worker_invalid",
        "hachimi-sandbox-setup" => "sandbox_setup_invalid",
        "hachimi-sandbox-launcher" => "sandbox_launcher_invalid",
        "hachimi-sandbox-canary" | "hachimi-sandbox-attest" => "sandbox_attestation_invalid",
        _ => "internal_resource_invalid",
    }
}

fn stage_file(source: &Path, destination: &Path, expected_hash: &str) -> Result<(), String> {
    let source_bytes = std::fs::read(source)
        .map_err(|error| format!("managed Runtime source {}: {error}", source.display()))?;
    if hash(&source_bytes) != expected_hash {
        return Err(format!(
            "managed Runtime source hash mismatch: {}",
            source.display()
        ));
    }
    if std::fs::read(destination)
        .ok()
        .is_some_and(|bytes| hash(&bytes) == expected_hash)
    {
        return Ok(());
    }
    atomic_write(destination, &source_bytes)?;
    let installed = std::fs::read(destination).map_err(|error| error.to_string())?;
    if hash(&installed) != expected_hash {
        return Err(format!(
            "managed Runtime attestation failed: {}",
            destination.display()
        ));
    }
    Ok(())
}

fn write_manifest(
    runtime: &ManagedSandboxRuntime,
    definitions: &[(&str, &str)],
) -> Result<(), String> {
    let manifest = RuntimeManifest {
        policy_version: hachimi_sandbox::current_policy_version(),
        files: definitions
            .iter()
            .map(|(name, sha256)| RuntimeManifestFile { name, sha256 })
            .collect(),
    };
    atomic_write(
        &runtime.root.join("runtime-manifest.json"),
        &serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut file = AtomicWriteFile::open(path).map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())?;
    file.commit().map_err(|error| error.to_string())?;
    // Staged sidecars are executables; atomic writes create 0644 by default,
    // which is not executable on unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn packaged_sidecar_path(resource_root: &Path, name: &str) -> Result<PathBuf, String> {
    let file_name = executable_name(name);
    let mut candidates = vec![resource_root.join("internal-runtime").join(&file_name)];
    if let Ok(executable) = std::env::current_exe()
        && let Some(parent) = executable.parent()
    {
        candidates.push(parent.join(&file_name));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "packaged Runtime resource is missing: {}",
                resource_root
                    .join("internal-runtime")
                    .join(file_name)
                    .display()
            )
        })
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_sidecar_prefers_internal_runtime_resource() {
        let resource_root = tempfile::tempdir().expect("resource root");
        let runtime = resource_root.path().join("internal-runtime");
        std::fs::create_dir_all(&runtime).expect("internal runtime");
        let sidecar = runtime.join(executable_name("hachimi-sandbox-launcher"));
        std::fs::write(&sidecar, b"launcher").expect("sidecar");

        assert_eq!(
            packaged_sidecar_path(resource_root.path(), "hachimi-sandbox-launcher")
                .expect("packaged sidecar"),
            sidecar
        );
    }
}
