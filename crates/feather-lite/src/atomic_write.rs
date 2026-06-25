//! Atomic file writing helpers for generated artifacts.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_TEMP_ATTEMPTS: usize = 16;

/// Writes a complete file through a same-directory temporary path and rename.
pub(crate) fn write_atomic(path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "atomic write target must include a file name",
        )
    })?;

    for attempt in 0..MAX_TEMP_ATTEMPTS {
        let temp_path = temp_file_path(parent, file_name, attempt);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(mut file) => {
                let result = (|| -> io::Result<()> {
                    file.write_all(bytes.as_ref())?;
                    file.sync_all()?;
                    drop(file);
                    fs::rename(&temp_path, path)?;
                    Ok(())
                })();
                if let Err(error) = result {
                    let _ = fs::remove_file(&temp_path);
                    return Err(error);
                }
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "failed to reserve atomic write temp file",
    ))
}

/// Removes a side-effect file only when the current operation created it.
pub(crate) fn remove_file_if_created(path: &Path, existed_before: bool) {
    if !existed_before {
        let _ = fs::remove_file(path);
    }
}

/// Builds a hidden temporary file name reserved beside the final artifact.
fn temp_file_path(parent: &Path, file_name: &OsStr, attempt: usize) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    parent.join(format!(
        ".{}.tmp-{}-{stamp}-{attempt}",
        file_name.to_string_lossy(),
        std::process::id()
    ))
}
