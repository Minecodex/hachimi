use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use hachimi_protocol::{AttachmentId, AttachmentRecord};
use sha2::{Digest, Sha256};

use crate::{MAX_ATTACHMENT_BYTES, WorkbenchError, now_ms};

pub(super) fn stage_attachment(
    source: &Path,
    attachment_root: &Path,
) -> Result<(AttachmentRecord, PathBuf), WorkbenchError> {
    let canonical = std::fs::canonicalize(source)?;
    let metadata = std::fs::metadata(&canonical)?;
    if !metadata.is_file() {
        return Err(WorkbenchError::InvalidAttachmentFile);
    }
    if metadata.len() > MAX_ATTACHMENT_BYTES {
        return Err(WorkbenchError::AttachmentTooLarge);
    }
    let original_name = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.chars().count() <= 255)
        .ok_or(WorkbenchError::InvalidAttachmentFile)?
        .to_owned();
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or_default());
    File::open(&canonical)?
        .take(MAX_ATTACHMENT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ATTACHMENT_BYTES {
        return Err(WorkbenchError::AttachmentTooLarge);
    }
    let content_hash = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    std::fs::create_dir_all(attachment_root)?;
    let managed_path = attachment_root.join(&content_hash);
    if !managed_path.is_file() {
        let temporary_path = attachment_root.join(format!(
            ".{content_hash}.{}.tmp",
            AttachmentId::random().as_str()
        ));
        let mut temporary = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)?;
        temporary.write_all(&bytes)?;
        temporary.sync_all()?;
        drop(temporary);
        if let Err(error) = std::fs::rename(&temporary_path, &managed_path) {
            if managed_path.is_file() {
                let _ = std::fs::remove_file(&temporary_path);
            } else {
                return Err(WorkbenchError::Io(error));
            }
        }
    }
    let attachment = AttachmentRecord {
        id: AttachmentId::random(),
        content_hash,
        original_name: original_name.clone(),
        mime_type: attachment_mime_type(&original_name).into(),
        byte_size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        created_at_ms: now_ms(),
    };
    Ok((attachment, managed_path))
}

fn attachment_mime_type(name: &str) -> &'static str {
    let extension = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "csv" => "text/csv",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        _ => "application/octet-stream",
    }
}
