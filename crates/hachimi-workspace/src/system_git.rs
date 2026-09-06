use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{GIT_EXECUTABLE_ENV, GIT_RUNTIME_REVISION_ENV, git_alias};

pub(crate) fn git_program() -> OsString {
    configured_git_program().unwrap_or_else(test_git_fallback)
}

fn configured_git_program() -> Option<OsString> {
    let _runtime_revision = std::env::var(GIT_RUNTIME_REVISION_ENV)
        .ok()?
        .parse::<u64>()
        .ok()?;
    std::env::var_os(GIT_EXECUTABLE_ENV)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_file())
        .and_then(|path| path.canonicalize().ok())
        .map(PathBuf::into_os_string)
}

#[cfg(test)]
fn test_git_fallback() -> OsString {
    debug_git_executable().map_or_else(
        || "__hachimi_system_git_unavailable__".into(),
        PathBuf::into_os_string,
    )
}

#[cfg(not(test))]
fn test_git_fallback() -> OsString {
    "__hachimi_system_git_unavailable__".into()
}

#[cfg(debug_assertions)]
pub(super) fn debug_git_executable() -> Option<PathBuf> {
    std::env::var_os(GIT_EXECUTABLE_ENV)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_file())
        .and_then(|path| path.canonicalize().ok())
}

#[cfg(not(debug_assertions))]
pub(super) fn debug_git_executable() -> Option<PathBuf> {
    None
}

pub(crate) fn restricted_process_cwd(fallback: &Path) -> PathBuf {
    let Some(alias_root) = std::env::var_os(git_alias::GIT_WORK_TREE_ALIAS_ENV).map(PathBuf::from)
    else {
        return fallback.to_owned();
    };
    let Some(real_root) = std::env::var_os(git_alias::GIT_WORK_TREE_REAL_ENV).map(PathBuf::from)
    else {
        return fallback.to_owned();
    };
    let Ok(relative) = fallback.strip_prefix(real_root) else {
        return fallback.to_owned();
    };
    alias_root.join(relative)
}
