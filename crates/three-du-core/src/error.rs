use std::fmt;

/// Errors produced while decoding a `.3du` file. Every variant carries enough context
/// (byte offset, record name) to diagnose the input without printing its contents.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("not a .3du drill file (expected magic \"3DUD\", found {found})")]
    NotA3du { found: String },

    #[error("unsupported .3du format revision {header_version} (written by version {app_version}); supported revisions: {supported}")]
    UnsupportedVersion {
        header_version: u16,
        app_version: String,
        supported: String,
    },

    #[error("file truncated in {context} at byte {offset}: needed {needed} more byte(s)")]
    Truncated {
        context: &'static str,
        offset: u64,
        needed: u64,
    },

    #[error("invalid {context} at byte {offset}: {detail}")]
    Invalid {
        context: &'static str,
        offset: u64,
        detail: String,
    },

    #[error("{what} exceeds the safety limit ({value} > {limit})")]
    LimitExceeded {
        what: &'static str,
        value: u64,
        limit: u64,
    },

    #[error("decoding was cancelled")]
    Cancelled,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl DecodeError {
    /// Stable machine-readable code for diagnostics and the FFI layer.
    pub fn code(&self) -> &'static str {
        match self {
            DecodeError::NotA3du { .. } => "not_3du",
            DecodeError::UnsupportedVersion { .. } => "unsupported_version",
            DecodeError::Truncated { .. } => "truncated",
            DecodeError::Invalid { .. } => "invalid_record",
            DecodeError::LimitExceeded { .. } => "limit_exceeded",
            DecodeError::Cancelled => "cancelled",
            DecodeError::Io(_) => "io",
        }
    }
}

/// Severity of a conversion diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational: nothing was lost.
    Info,
    /// Something could not be represented exactly; the output is still faithful to the drill.
    Warning,
    /// The output would contain an approximation of drill data (requires explicit approval).
    Lossy,
}

/// A structured diagnostic attached to a decode or export. Messages never contain
/// performer names, titles or other identifying drill content.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable identifier such as `trailing_static_counts`.
    pub code: String,
    pub message: String,
}

impl Diagnostic {
    pub fn new(severity: Severity, code: &str, message: impl Into<String>) -> Self {
        Diagnostic {
            severity,
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.severity {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Lossy => "lossy",
        };
        write!(f, "{level} [{}]: {}", self.code, self.message)
    }
}
