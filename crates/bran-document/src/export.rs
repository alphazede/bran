//! Shared export gate: explicit format and destination, DLP first, never overwrite.

use crate::{Format, Refusal};
use bran_core::export::{validate_emitted_string, ExportError};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMPORARY_SERIAL: AtomicU64 = AtomicU64::new(0);

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
    // Only a file this call created exclusively is ever removed; an existing
    // file with a colliding name is skipped, never touched.
    let (temporary, written) = (0..64)
        .find_map(|_| {
            let serial = TEMPORARY_SERIAL.fetch_add(1, Ordering::Relaxed);
            let name = format!(".{}.bran-export-{serial}", file_name.to_string_lossy());
            let temporary = parent.join(name);
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Err(error) if error.kind() == ErrorKind::AlreadyExists => None,
                Err(_) => Some(Err(())),
                Ok(mut file) => {
                    let written = file.write_all(bytes).and_then(|()| file.sync_all());
                    Some(Ok((temporary, written)))
                }
            }
        })
        .ok_or(Refusal::ExportIo)?
        .map_err(|()| Refusal::ExportIo)?;
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

#[cfg(test)]
mod tests {
    use super::{write_new, TEMPORARY_SERIAL};
    use crate::Format;
    use std::fs;
    use std::sync::atomic::Ordering;

    #[test]
    fn colliding_temporary_names_are_skipped_not_removed() {
        let root = std::env::temp_dir().join(format!(
            "bran-document-export-unit-{}",
            TEMPORARY_SERIAL.load(Ordering::SeqCst)
        ));
        fs::create_dir_all(&root).unwrap();
        let next = TEMPORARY_SERIAL.load(Ordering::SeqCst);
        let stale: Vec<_> = (next..next + 2)
            .map(|serial| root.join(format!(".x.docx.bran-export-{serial}")))
            .collect();
        for path in &stale {
            fs::write(path, b"user data").unwrap();
        }
        let written = write_new(&root, "x.docx", Format::Docx, b"export", &[]);
        let kept: Vec<_> = stale.iter().map(|path| fs::read(path).ok()).collect();
        let count = fs::read_dir(&root).unwrap().count();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(written.map(|path| path.ends_with("x.docx")), Ok(true));
        assert!(kept
            .iter()
            .all(|data| data.as_deref() == Some(b"user data".as_slice())));
        assert_eq!(
            count, 3,
            "only the export and the two stale files may remain"
        );
    }
}
