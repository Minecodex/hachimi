// SPDX-License-Identifier: Apache-2.0
// Adapted from openai/codex codex-rs/sandboxing/src/seatbelt.rs
// @ 4f39251a010a8bd7d692d25fb33832ff06f1635a.
// Modified for Hachimi: grant-driven SBPL generation from SandboxLaunchSpec,
// .git read-only carve-outs inside write roots, and C2.1 deny-all networking.

//! macOS Seatbelt (`sandbox-exec`) launch support.
//!
//! The generator turns a validated [`SandboxLaunchSpec`] into a static SBPL
//! profile plus `-D` path parameters. Only `/usr/bin/sandbox-exec` is ever
//! executed, so a PATH-injected lookalike cannot weaken the boundary.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

use hachimi_protocol::{CapabilityGrantSet, FileSystemAccess};

use crate::process_backend::{SandboxError, SandboxLaunchSpec};

pub(crate) const SEATBELT_EXECUTABLE: &str = "/usr/bin/sandbox-exec";

pub(crate) const BASE_POLICY: &str = include_str!("seatbelt_base_policy.sbpl");

/// C2.1: sandboxed workers never receive network access.
const NETWORK_POLICY: &str = "(deny network*)\n";

/// Wraps an interactive terminal command in the Seatbelt profile.
///
/// Terminal sessions don't carry Run-scoped grants; the boundary is the bound
/// checkout cwd (read/write with a read-only `.git`), the Run temporary
/// directory from the environment, the toolchain read paths from the base
/// policy, and deny-all networking.
pub fn seatbelt_terminal_args(
    command: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<Vec<OsString>, String> {
    let checkout = cwd
        .canonicalize()
        .map_err(|error| format!("terminal cwd cannot be canonicalized: {error}"))?;
    let session_id = hachimi_protocol::SessionId::from("session-terminal");
    let run_id = hachimi_protocol::RunId::from("run-terminal");
    let grants = CapabilityGrantSet {
        profile: hachimi_protocol::PermissionProfile::Writable,
        scope: hachimi_protocol::PermissionGrantScope::Run,
        session_id: session_id.clone(),
        run_id: Some(run_id.clone()),
        file_system: vec![
            hachimi_protocol::FileSystemGrant {
                access: FileSystemAccess::Read,
                roots: vec![checkout.to_string_lossy().into_owned()],
                globs: vec![],
                files: vec![],
                special_roots: vec![],
            },
            hachimi_protocol::FileSystemGrant {
                access: FileSystemAccess::Write,
                roots: vec![checkout.to_string_lossy().into_owned()],
                globs: vec![],
                files: vec![],
                special_roots: vec![],
            },
        ],
        process: hachimi_protocol::ProcessGrant {
            spawn: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let spec = SandboxLaunchSpec {
        session_id,
        run_id,
        run_generation: 1,
        checkout_id: hachimi_protocol::CheckoutId::from("checkout-terminal"),
        checkout_root: checkout.clone(),
        grants,
        required_effect: hachimi_protocol::ToolEffect::WorkspaceWrite,
        executable: PathBuf::from(
            command
                .first()
                .filter(|value| !value.trim().is_empty())
                .ok_or("terminal command must not be empty")?,
        ),
        args: command[1..].iter().map(OsString::from).collect::<Vec<_>>(),
        cwd: checkout,
        environment: environment
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect(),
        stdin: None,
        interactive_stdin: true,
        timeout: std::time::Duration::ZERO,
        output_limit: 1024,
        network_policy: crate::process_backend::SandboxNetworkPolicy::DenyAll,
        git_metadata_writable: false,
    };
    seatbelt_command_args(&spec).map_err(|error| error.to_string())
}

/// Builds the `sandbox-exec` argument vector for a validated launch spec.
///
/// Layout: `-p <policy> -D KEY=path ... -- <executable> <args...>`.
pub(crate) fn seatbelt_command_args(
    spec: &SandboxLaunchSpec,
) -> Result<Vec<OsString>, SandboxError> {
    let mut policy = String::from(BASE_POLICY);
    policy.push_str(NETWORK_POLICY);
    let mut definitions: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut read_index = 0_usize;
    let mut write_index = 0_usize;
    let mut deny_index = 0_usize;

    for grant in &spec.grants.file_system {
        if !grant.globs.is_empty() || !grant.files.is_empty() {
            return Err(SandboxError::InvalidBinding(
                "glob/file-scoped grants are not supported by the macOS Seatbelt backend yet"
                    .into(),
            ));
        }
        for special in &grant.special_roots {
            match (special.as_str(), grant.access) {
                (":root", FileSystemAccess::Read) => {
                    policy.push_str("(allow file-read* file-test-existence (regex #\"^/\"))\n");
                }
                (":root", FileSystemAccess::Write) => {
                    policy.push_str("(allow file-read* file-write* (regex #\"^/\"))\n");
                }
                (":root", FileSystemAccess::Deny) => {
                    policy.push_str("(deny file-read* file-write* (regex #\"^/\"))\n");
                }
                _ => {
                    return Err(SandboxError::InvalidBinding(format!(
                        "unknown special grant root for Seatbelt: {special}"
                    )));
                }
            }
        }
        for root in &grant.roots {
            let root = canonical_root(root)?;
            match grant.access {
                FileSystemAccess::Read => {
                    let key = format!("READ_{read_index}");
                    read_index += 1;
                    policy.push_str(&format!(
                        "(allow file-read* file-test-existence (subpath (param \"{key}\")))\n"
                    ));
                    definitions.insert(key, root);
                }
                FileSystemAccess::Write => {
                    let key = format!("WRITE_{write_index}");
                    write_index += 1;
                    policy.push_str(&format!(
                        "(allow file-read* file-write* (subpath (param \"{key}\")))\n"
                    ));
                    // A sandboxed process must not replace an authority
                    // boundary that will be reused by the next launch.
                    policy.push_str(&format!(
                        "(deny file-write-unlink (require-all (literal (param \"{key}\")) (vnode-type DIRECTORY)))\n"
                    ));
                    // Repository metadata stays read-only inside writable
                    // checkouts unless this launch holds a verified Git
                    // mutation lease.
                    if !spec.git_metadata_writable {
                        let git_metadata =
                            format!("^{}/\\.git(/.*)?$", regex_escape(&root.to_string_lossy()));
                        policy
                            .push_str(&format!("(deny file-write* (regex #\"{git_metadata}\"))\n"));
                        policy.push_str(&format!(
                            "(deny file-write* (literal (param \"{key}_GITFILE\")))\n"
                        ));
                        definitions.insert(format!("{key}_GITFILE"), root.join(".git"));
                    }
                    definitions.insert(key, root);
                }
                FileSystemAccess::Deny => {
                    let key = format!("DENY_{deny_index}");
                    deny_index += 1;
                    policy.push_str(&format!(
                        "(deny file-read* file-write* (subpath (param \"{key}\")))\n"
                    ));
                    definitions.insert(key, root);
                }
            }
        }
    }

    // The launched executable, the resolved Git runtime, the bound cwd and the
    // Run-scoped temporary directory always ride along.
    let mut extras: Vec<(String, PathBuf, bool)> = Vec::new(); // (key, path, writable)
    if let Some(parent) = spec.executable.parent() {
        extras.push(("EXE_DIR".into(), parent.to_path_buf(), false));
    }
    extras.push(("CWD".into(), spec.cwd.clone(), false));
    if let Some(git) = environment_value(spec, "HACHIMI_GIT_EXECUTABLE") {
        let git = PathBuf::from(git);
        extras.push(("GIT_EXE".into(), git.clone(), false));
        if let Some(runtime_root) = git.parent().and_then(Path::parent) {
            // Git subcommands, templates and helpers live beside `bin` in
            // both Apple Command Line Tools/Xcode and Homebrew layouts. The
            // root is derived from the already verified absolute lease.
            extras.push(("GIT_RUNTIME".into(), runtime_root.to_path_buf(), false));
        }
    }
    for name in ["TMPDIR", "TEMP", "TMP"] {
        if let Some(temp) = environment_value(spec, name) {
            extras.push((format!("RUN_TEMP_{name}"), PathBuf::from(temp), true));
        }
    }
    for (key, path, writable) in extras {
        let Ok(path) = std::fs::canonicalize(&path).or_else(|_| Ok::<PathBuf, ()>(path.clone()))
        else {
            continue;
        };
        if writable {
            policy.push_str(&format!(
                "(allow file-read* file-write* (subpath (param \"{key}\")))\n"
            ));
        } else {
            policy.push_str(&format!(
                "(allow file-read* file-test-existence file-map-executable (subpath (param \"{key}\")))\n(allow file-read* file-map-executable (literal (param \"{key}_EXE\")))\n"
            ));
            definitions.insert(format!("{key}_EXE"), path.clone());
        }
        definitions.insert(key, path);
    }

    // stat(2)/chdir(2) need metadata reads on every ancestor of a granted
    // path; Seatbelt evaluates real paths, so grant ancestors explicitly.
    let ancestor_keys = definitions.keys().cloned().collect::<Vec<_>>();
    for key in ancestor_keys {
        policy.push_str(&format!(
            "(allow file-read-metadata file-test-existence (path-ancestors (param \"{key}\")))\n"
        ));
    }

    let mut arguments = vec![OsString::from("-p"), OsString::from(policy)];
    for (key, path) in definitions {
        arguments.push(OsString::from("-D"));
        arguments.push(OsString::from(format!("{key}={}", path.to_string_lossy())));
    }
    arguments.push(OsString::from("--"));
    arguments.push(spec.executable.clone().into_os_string());
    arguments.extend(spec.args.iter().cloned());
    Ok(arguments)
}

fn canonical_root(root: &str) -> Result<PathBuf, SandboxError> {
    let path = Path::new(root);
    std::fs::canonicalize(path).map_err(|error| {
        SandboxError::InvalidBinding(format!(
            "grant root cannot be canonicalized: {root}: {error}"
        ))
    })
}

fn environment_value(spec: &SandboxLaunchSpec, name: &str) -> Option<String> {
    spec.environment
        .iter()
        .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.to_string_lossy().into_owned())
        .filter(|value| !value.is_empty())
}

fn regex_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if "\\^$.|?*+()[]{}".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use hachimi_protocol::{
        CapabilityGrantSet, FileSystemGrant, NetworkGrant, PermissionGrantScope, ProcessGrant,
    };
    use hachimi_protocol::{CheckoutId, RunId, SessionId, ToolEffect};

    fn spec_with_grants(grants: Vec<FileSystemGrant>) -> SandboxLaunchSpec {
        let checkout = std::fs::canonicalize(std::env::temp_dir()).expect("temp");
        let grants_set = CapabilityGrantSet {
            scope: PermissionGrantScope::Run,
            session_id: SessionId::from("session-seatbelt"),
            run_id: Some(RunId::from("run-seatbelt")),
            file_system: grants,
            network: NetworkGrant::default(),
            process: ProcessGrant {
                spawn: true,
                interactive: false,
                unrestricted_commands: false,
                allowed_commands: vec![],
            },
            ..Default::default()
        };
        SandboxLaunchSpec {
            session_id: grants_set.session_id.clone(),
            run_id: RunId::from("run-seatbelt"),
            run_generation: 1,
            checkout_id: CheckoutId::from("checkout-seatbelt"),
            checkout_root: checkout.clone(),
            grants: grants_set,
            required_effect: ToolEffect::WorkspaceWrite,
            executable: PathBuf::from("/bin/sh"),
            args: vec![],
            cwd: checkout,
            environment: vec![(OsString::from("TMPDIR"), OsString::from("/tmp/hachimi-run"))],
            stdin: None,
            interactive_stdin: false,
            timeout: std::time::Duration::from_secs(5),
            output_limit: 1024,
            network_policy: crate::process_backend::SandboxNetworkPolicy::DenyAll,
            git_metadata_writable: false,
        }
    }

    fn policy_of(spec: &SandboxLaunchSpec) -> String {
        let args = seatbelt_command_args(spec).expect("seatbelt args");
        args[1].to_string_lossy().into_owned()
    }

    #[test]
    fn write_grants_emit_subpath_allow_with_git_and_anchor_protection() {
        let checkout = std::env::temp_dir();
        let spec = spec_with_grants(vec![
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
        ]);
        let policy = policy_of(&spec);
        assert!(policy.contains("(deny network*)"));
        assert!(policy.contains("(subpath (param \"READ_0\"))"));
        assert!(policy.contains("(subpath (param \"WRITE_0\"))"));
        assert!(policy.contains("file-write-unlink"));
        assert!(policy.contains("\\.git"));
        assert!(policy.contains("WRITE_0_GITFILE"));
        // deny default baseline stays intact
        assert!(policy.contains("(deny default)"));
    }

    #[test]
    fn glob_grants_fail_closed() {
        let checkout = std::env::temp_dir();
        let spec = spec_with_grants(vec![FileSystemGrant {
            access: FileSystemAccess::Write,
            roots: vec![checkout.to_string_lossy().into_owned()],
            globs: vec!["**/*.rs".into()],
            files: vec![],
            special_roots: vec![],
        }]);
        assert!(matches!(
            seatbelt_command_args(&spec),
            Err(SandboxError::InvalidBinding(_))
        ));
    }

    #[test]
    fn verified_git_lease_grants_its_toolchain_helpers_read_and_execute_access() {
        let mut spec = spec_with_grants(Vec::new());
        spec.environment.push((
            OsString::from("HACHIMI_GIT_EXECUTABLE"),
            OsString::from("/Library/Developer/CommandLineTools/usr/bin/git"),
        ));
        let args = seatbelt_command_args(&spec).expect("seatbelt args");
        let policy = args[1].to_string_lossy();
        assert!(policy.contains("GIT_RUNTIME"));
        assert!(args.iter().any(|argument| {
            argument
                .to_string_lossy()
                .contains("GIT_RUNTIME=/Library/Developer/CommandLineTools/usr")
        }));
    }

    #[test]
    fn unknown_special_roots_fail_closed() {
        let spec = spec_with_grants(vec![FileSystemGrant {
            access: FileSystemAccess::Read,
            roots: vec![],
            globs: vec![],
            files: vec![],
            special_roots: vec![":everything".into()],
        }]);
        assert!(matches!(
            seatbelt_command_args(&spec),
            Err(SandboxError::InvalidBinding(_))
        ));
    }

    #[test]
    fn root_special_read_allows_full_disk_read() {
        let spec = spec_with_grants(vec![FileSystemGrant {
            access: FileSystemAccess::Read,
            roots: vec![],
            globs: vec![],
            files: vec![],
            special_roots: vec![":root".into()],
        }]);
        let policy = policy_of(&spec);
        assert!(policy.contains("(allow file-read* file-test-existence (regex #\"^/\"))"));
    }
}
