use std::path::Path;

// Only newly created TempDir fixtures can enter this helper. Hosted Windows
// runners may create them with an Administrators-group owner, while the real
// Workspace contract requires the individual user SID.
pub(crate) fn fixture_root(directory: &tempfile::TempDir) -> &Path {
    #[cfg(windows)]
    match hachimi_sandbox::validate_checkout_root(directory.path()) {
        Ok(_) => {}
        Err(hachimi_sandbox::PathSecurityError::OwnershipMismatch) => {
            let system = std::path::PathBuf::from(
                std::env::var_os("SystemRoot").expect("Windows system directory"),
            )
            .join("System32");
            let identity = std::process::Command::new(system.join("whoami.exe"))
                .output()
                .expect("current fixture identity");
            assert!(identity.status.success());
            let owner = String::from_utf8(identity.stdout).expect("fixture identity encoding");
            // Existing Git metadata is part of this same disposable fixture.
            let ownership = std::process::Command::new(system.join("icacls.exe"))
                .arg(directory.path())
                .args(["/setowner", owner.trim(), "/T", "/Q"])
                .output()
                .expect("assign fixture ownership");
            assert!(
                ownership.status.success(),
                "fixture ownership assignment failed: {}",
                String::from_utf8_lossy(&ownership.stderr)
            );
        }
        Err(error) => panic!("fresh fixture root failed validation: {error}"),
    }
    directory.path()
}
