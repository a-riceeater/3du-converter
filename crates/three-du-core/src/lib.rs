//! Core of 3du-converter: format detection, a bounds-checked decoder for Pyware/UDB `.3du`
//! drill files, and the canonical [`DrillProject`] model every exporter consumes.
//!
//! ```no_run
//! let project = three_du_core::read_file("show.3du", &Default::default())?;
//! println!("{} sets", project.sets.len());
//! # Ok::<(), three_du_core::DecodeError>(())
//! ```

pub mod convert;
pub mod detect;
pub mod error;
pub mod export;
pub mod field;
pub mod model;
pub mod raw;
pub mod reader;
pub mod synth;
pub mod transform;

use std::path::Path;

use sha2::{Digest, Sha256};

pub use convert::{ConvertOptions, PageMapEntry};
pub use detect::{detect, FileFormat};
pub use error::{DecodeError, Diagnostic, Severity};
pub use model::*;
pub use raw::Limits;
pub use transform::{CoordinateTransform, TransformOverrides};

#[derive(Default)]
pub struct ReadOptions<'a> {
    pub convert: ConvertOptions,
    pub limits: Limits,
    /// Called with `(frames_done, frames_total)`; return `false` to cancel.
    pub progress: Option<raw::ProgressFn<'a>>,
}

/// Decodes `.3du` bytes into the canonical model.
pub fn read_bytes(bytes: &[u8], opts: ReadOptions<'_>) -> Result<DrillProject, DecodeError> {
    let raw = raw::decode(bytes, opts.limits, opts.progress)?;
    let sha = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    Ok(convert::to_project(&raw, sha, &opts.convert))
}

/// Reads and decodes a `.3du` file. The file is opened read-only and never modified.
pub fn read_file(path: impl AsRef<Path>, convert: &ConvertOptions) -> Result<DrillProject, DecodeError> {
    let limits = Limits::default();
    let len = std::fs::metadata(path.as_ref())?.len();
    if len > limits.max_file_bytes {
        return Err(DecodeError::LimitExceeded { what: "file size", value: len, limit: limits.max_file_bytes });
    }
    let bytes = std::fs::read(path)?;
    read_bytes(&bytes, ReadOptions { convert: convert.clone(), limits, progress: None })
}

/// Serializes the model as the intermediate JSON format (see docs/INTERMEDIATE_FORMAT.md).
pub fn to_intermediate_json(project: &DrillProject) -> serde_json::Result<String> {
    serde_json::to_string_pretty(project)
}

pub fn from_intermediate_json(text: &str) -> serde_json::Result<DrillProject> {
    serde_json::from_str(text)
}
