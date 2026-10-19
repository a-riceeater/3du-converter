//! Policy and file-handling helpers shared by every exporter.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Diagnostic, Severity};

/// How much loss an export may accept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LossPolicy {
    /// Reject any warning: the output must represent the source exactly.
    Strict,
    /// Accept representational warnings (e.g. curves flattened to straight moves),
    /// reject approximations of drill data.
    #[default]
    Default,
    /// Accept approximations the user explicitly approved.
    AllowLossy,
}

impl LossPolicy {
    /// Diagnostics this policy refuses.
    pub fn violations<'a>(&self, diags: &'a [Diagnostic]) -> Vec<&'a Diagnostic> {
        diags
            .iter()
            .filter(|d| match self {
                LossPolicy::Strict => d.severity >= Severity::Warning,
                LossPolicy::Default => d.severity >= Severity::Lossy,
                LossPolicy::AllowLossy => false,
            })
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("export refused by the {policy:?} loss policy: {}", .refused.iter().map(|d| d.code.as_str()).collect::<Vec<_>>().join(", "))]
    Refused { policy: LossPolicy, refused: Vec<Diagnostic> },
    #[error("{0} already exists (pass --overwrite to replace it)")]
    Exists(PathBuf),
    #[error("cannot export: {0}")]
    Unsupported(String),
    #[error("generated output failed validation: {0}")]
    Validation(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(String),
}

impl ExportError {
    pub fn code(&self) -> &'static str {
        match self {
            ExportError::Refused { .. } => "refused_lossy",
            ExportError::Exists(_) => "output_exists",
            ExportError::Unsupported(_) => "unsupported",
            ExportError::Validation(_) => "validation_failed",
            ExportError::Io(_) => "io",
            ExportError::Database(_) => "database",
        }
    }
}

pub fn enforce(policy: LossPolicy, diags: &[Diagnostic]) -> Result<(), ExportError> {
    let refused: Vec<Diagnostic> = policy.violations(diags).into_iter().cloned().collect();
    if refused.is_empty() {
        Ok(())
    } else {
        Err(ExportError::Refused { policy, refused })
    }
}

/// A hidden sibling path for writing before the final rename.
pub fn temp_path_for(target: &Path) -> PathBuf {
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "output".into());
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    target.with_file_name(format!(".{name}.{}.{nonce}.partial", std::process::id()))
}

pub fn check_target(target: &Path, overwrite: bool) -> Result<(), ExportError> {
    if target.exists() && !overwrite {
        return Err(ExportError::Exists(target.to_path_buf()));
    }
    Ok(())
}

/// Moves a fully written and validated temporary file into place.
pub fn commit(temp: &Path, target: &Path, overwrite: bool) -> Result<(), ExportError> {
    if let Err(e) = check_target(target, overwrite) {
        let _ = fs::remove_file(temp);
        return Err(e);
    }
    if overwrite && target.exists() && cfg!(windows) {
        // rename() cannot replace an existing file on every Windows filesystem.
        fs::remove_file(target)?;
    }
    fs::rename(temp, target).map_err(|e| {
        let _ = fs::remove_file(temp);
        ExportError::Io(e)
    })
}

/// Writes bytes atomically: temp file, flush, validate, rename.
pub fn write_atomic(
    target: &Path,
    bytes: &[u8],
    overwrite: bool,
    validate: impl FnOnce(&Path) -> Result<(), ExportError>,
) -> Result<(), ExportError> {
    check_target(target, overwrite)?;
    let temp = temp_path_for(target);
    let result = (|| {
        let mut f = fs::File::create(&temp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        validate(&temp)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    commit(&temp, target, overwrite)
}

/// Deterministic RFC 9562 version-8 UUID derived from a namespace and key, uppercase.
pub fn stable_uuid(namespace: &str, key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(namespace.as_bytes());
    h.update([0]);
    h.update(key.as_bytes());
    let d = h.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    b[6] = (b[6] & 0x0f) | 0x80;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hx: String = b.iter().map(|x| format!("{x:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &hx[0..8], &hx[8..12], &hx[12..16], &hx[16..20], &hx[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policies() {
        let d = vec![Diagnostic::new(Severity::Info, "i", ""), Diagnostic::new(Severity::Warning, "w", ""), Diagnostic::new(Severity::Lossy, "l", "")];
        assert_eq!(LossPolicy::Strict.violations(&d).len(), 2);
        assert_eq!(LossPolicy::Default.violations(&d).len(), 1);
        assert_eq!(LossPolicy::AllowLossy.violations(&d).len(), 0);
    }

    #[test]
    fn uuids_are_stable_and_well_formed() {
        let a = stable_uuid("show", "abc");
        assert_eq!(a, stable_uuid("show", "abc"));
        assert_ne!(a, stable_uuid("show", "abd"));
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "8");
        assert!(a.chars().all(|c| c == '-' || c.is_ascii_digit() || c.is_ascii_uppercase()));
    }

    #[test]
    fn atomic_write_respects_overwrite_and_validation() {
        let dir = std::env::temp_dir().join(format!("three-du-export-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("out.bin");
        let _ = fs::remove_file(&target);
        write_atomic(&target, b"one", false, |_| Ok(())).unwrap();
        assert!(matches!(write_atomic(&target, b"two", false, |_| Ok(())), Err(ExportError::Exists(_))));
        assert!(write_atomic(&target, b"two", true, |_| Err(ExportError::Validation("no".into()))).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"one");
        write_atomic(&target, b"two", true, |_| Ok(())).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"two");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no temp files left behind");
        fs::remove_dir_all(&dir).unwrap();
    }
}
