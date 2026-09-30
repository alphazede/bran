//! Shared export gate: explicit format and destination, DLP first, never overwrite.

use crate::{Format, Refusal};
use bran_core::export::{validate_emitted_string, ExportError};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

/// Writes `bytes` to `root/relative` only if every check passes first:
/// DLP and public-boundary on the emitted text, a contained relative
/// destination whose extension matches `format`, and no existing file.
/// The file appears atomically through a hard link, so a failure leaves
/// neither a partial export nor a temporary file.
pub fn write_new(
    root: &Path,
    relative: &str,
    format: Format,
    bytes: &[u8],
    emitted_text: &[&str],
) -> Result<PathBuf, Refusal> {
    for text in emitted_text {
        match validate_emitted_string(text) {
            Err(ExportError::DlpViolation(_)) => return Err(Refusal::DlpFindings),
            Err(_) => return Err(Refusal::PublicBoundary),
            Ok(()) => {}
        }
    }
    crate::opc::validate_part_name(relative).map_err(|_| Refusal::ExportContainment)?;
    let relative = Path::new(relative);
    if relative
        .extension()
        .and_then(|extension| extension.to_str())
        != Some(format.extension())
    {
        return Err(Refusal::ExportFormatMismatch);
    }
    let root = root
        .canonicalize()
        .map_err(|_| Refusal::ExportContainment)?;
    let parent = root
        .join(relative.parent().unwrap_or(Path::new("")))
        .canonicalize()
        .map_err(|_| Refusal::ExportContainment)?;
    let file_name = relative.file_name().ok_or(Refusal::ExportContainment)?;
    if !parent.starts_with(&root) {
        return Err(Refusal::ExportContainment);
    }
    let destination = parent.join(file_name);
    let temporary = parent.join(format!(".{}.bran-export", file_name.to_string_lossy()));
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(Refusal::ExportIo);
    }
    // ponytail: hard link gives atomic no-overwrite; filesystems without hard
    // links refuse with export-io rather than risk a non-atomic rename.
    let linked = fs::hard_link(&temporary, &destination);
    let _ = fs::remove_file(&temporary);
    match linked {
        Ok(()) => Ok(destination),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(Refusal::ExportExists),
        Err(_) => Err(Refusal::ExportIo),
    }
}
