//! Identifies drill-related files by content, never by extension alone.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileFormat {
    /// Pyware/UDB `3DUD` drill file.
    ThreeDu,
    /// SQLite database (OpenMarch `.dots`, or Fieldwork's inner `show.sqlite`).
    Sqlite,
    /// ZIP archive containing a Fieldwork manifest.
    Fieldwork,
    /// Other ZIP archive.
    Zip,
    /// gzip-compressed data (OpenMarch `.omz`).
    Gzip,
    /// JSON with an `omSchemaVersion` key (OpenMarch `.om`).
    OpenMarchJson,
    /// JSON produced by `extract` (this project's intermediate model).
    IntermediateJson,
    Unknown,
}

impl FileFormat {
    pub fn describe(self) -> &'static str {
        match self {
            FileFormat::ThreeDu => ".3du drill (3DUD)",
            FileFormat::Sqlite => "SQLite database (.dots)",
            FileFormat::Fieldwork => "Fieldwork show (.fieldwork)",
            FileFormat::Zip => "ZIP archive",
            FileFormat::Gzip => "gzip data (.omz)",
            FileFormat::OpenMarchJson => "OpenMarch JSON (.om)",
            FileFormat::IntermediateJson => "3du-converter intermediate JSON",
            FileFormat::Unknown => "unknown",
        }
    }
}

pub fn detect(bytes: &[u8]) -> FileFormat {
    if crate::raw::is_3du(bytes) {
        return FileFormat::ThreeDu;
    }
    if bytes.starts_with(b"SQLite format 3\0") {
        return FileFormat::Sqlite;
    }
    if bytes.starts_with(b"PK\x03\x04") {
        let head = &bytes[..bytes.len().min(64 * 1024)];
        let is_fw = head.windows(13).any(|w| w == b"manifest.json") && bytes.windows(18).any(|w| w == b"org.fieldwork.show");
        return if is_fw { FileFormat::Fieldwork } else { FileFormat::Zip };
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return FileFormat::Gzip;
    }
    let text_start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    if bytes.get(text_start) == Some(&b'{') {
        let head = &bytes[..bytes.len().min(4096)];
        if head.windows(17).any(|w| w == b"\"omSchemaVersion\"") {
            return FileFormat::OpenMarchJson;
        }
        if head.windows(15).any(|w| w == b"\"model_version\"") {
            return FileFormat::IntermediateJson;
        }
    }
    FileFormat::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_by_content() {
        assert_eq!(detect(b"3DUD\x00\x0c"), FileFormat::ThreeDu);
        assert_eq!(detect(b"SQLite format 3\0rest"), FileFormat::Sqlite);
        assert_eq!(detect(&[0x1f, 0x8b, 8, 0]), FileFormat::Gzip);
        assert_eq!(detect(b"  {\"omSchemaVersion\":\"0.2.0\"}"), FileFormat::OpenMarchJson);
        assert_eq!(detect(b"{\"model_version\":\"1.0.0\"}"), FileFormat::IntermediateJson);
        assert_eq!(detect(b"3DU"), FileFormat::Unknown);
        assert_eq!(detect(b""), FileFormat::Unknown);
    }
}
