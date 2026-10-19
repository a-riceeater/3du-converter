//! Record-level decoder for the `3DUD` binary layout.
//!
//! The file is a single big-endian stream of length-prefixed sections. Nothing is located
//! by searching: each record is read at the position the previous one ended, so a layout
//! mismatch surfaces as an error at an exact offset instead of silently wrong data.
//! See `docs/3DU_FORMAT.md` for the record tables and confidence levels.

use crate::error::DecodeError;
use crate::reader::Reader;

pub const MAGIC: &[u8; 4] = b"3DUD";

/// Format revisions (the first `u16` after the magic) whose layout has been verified.
pub const SUPPORTED_HEADER_VERSIONS: &[u16] = &[12];

/// Safety limits applied to untrusted input.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_file_bytes: u64,
    /// Upper bound on frames × objects, the dominant allocation.
    pub max_position_samples: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_file_bytes: 1 << 30,
            max_position_samples: 200_000_000,
        }
    }
}

/// Byte span of a top-level section, kept for diagnostics and format research.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SectionSpan {
    pub name: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone)]
pub struct RawHeader {
    /// Layout revision of the file (12 for the verified layout).
    pub header_version: u16,
    /// Writer version as three numbers, e.g. `[11, 3, 6]`.
    pub app_version: [u16; 3],
    pub version_string: String,
    pub title: String,
    /// Four bytes after the title; zero in every sample seen. Meaning unknown.
    pub reserved: u32,
    /// Captions of the five note fields (default "Note 1" … "Note 5").
    pub note_labels: Vec<String>,
}

/// One drill tab: a set (`StandardTab`) or subset (`SubTab`).
#[derive(Debug, Clone)]
pub struct RawSet {
    pub offset: u64,
    /// Count at which the formation of this set is reached.
    pub start_count: u16,
    pub id: [u8; 8],
    pub rehearsal_mark: String,
    pub name: String,
    /// Measure range of the move into this set, as written ("175-176", "177", "").
    pub measures: String,
    /// Ten bytes, zero in every sample seen. Meaning unknown.
    pub reserved_a: [u8; 10],
    pub tab_type: String,
    pub reserved_b: u32,
    /// Tab color as "R,G,B", or "." for the default.
    pub color: String,
    pub reserved_c: u16,
}

#[derive(Debug, Clone)]
pub struct RawPerformer {
    pub id: [u8; 8],
    pub label: String,
    /// Free text stored after the label; empty in every sample seen (likely instrument/section).
    pub info: String,
    /// Symbol drawn for the performer, e.g. "X".
    pub symbol: String,
}

/// Physical size of a prop or 3D object, in source units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawObjectSize {
    pub width: f32,
    pub depth: f32,
    pub height: f32,
    /// Shape code (0 and 2 observed).
    pub shape: u16,
}

/// One performer/object sample in one frame (count).
#[derive(Debug, Clone, Copy)]
pub struct RawObject {
    pub id: [u8; 8],
    pub x: f32,
    pub y: f32,
    pub rgb: [u16; 3],
    /// Symbol character code (e.g. b'X').
    pub symbol: u16,
    /// High half of the object-type word; 0 normally, 0xFF7F observed on some props.
    pub state: u16,
    /// 1 = scenery/prop, 2 = performer, 7 = performer with an alternate marker.
    pub kind: u16,
    /// Rotation in radians.
    pub rotation: f32,
    pub size: Option<RawObjectSize>,
}

#[derive(Debug, Clone)]
pub enum RawAnnotation {
    /// Type 2: a text box. `rect` is (x1, y1, x2, y2) in source units.
    Text {
        id: [u8; 8],
        reserved: u16,
        rect: [f32; 4],
        extra: [f32; 3],
        text: String,
    },
    /// Type 1: a drawn line or polyline with a 43-byte style block.
    Polyline {
        id: [u8; 8],
        reserved: u16,
        style: Vec<u8>,
        points: Vec<[f32; 2]>,
    },
}

#[derive(Debug, Clone)]
pub struct RawFrame {
    pub offset: u64,
    pub objects: Vec<RawObject>,
    pub annotations: Vec<RawAnnotation>,
}

#[derive(Debug, Clone)]
pub struct RawDrill {
    pub file_size: u64,
    pub header: RawHeader,
    pub sets: Vec<RawSet>,
    pub performers: Vec<RawPerformer>,
    pub audio_reference: String,
    pub count_times: Vec<f64>,
    pub tempos: Vec<u32>,
    pub field_records: Vec<String>,
    pub frames: Vec<RawFrame>,
    /// Bytes after the frame section (33 in the verified layout; meaning partly unknown).
    pub trailer: Vec<u8>,
    pub trailer_offset: u64,
    pub sections: Vec<SectionSpan>,
}

/// Progress callback: receives `(frames_done, frames_total)`, returns `false` to cancel.
pub type ProgressFn<'a> = &'a mut dyn FnMut(usize, usize) -> bool;

/// Returns true when `bytes` starts with the `3DUD` signature.
pub fn is_3du(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[..4] == MAGIC
}

pub fn decode(bytes: &[u8], limits: Limits, mut progress: Option<ProgressFn<'_>>) -> Result<RawDrill, DecodeError> {
    if bytes.len() as u64 > limits.max_file_bytes {
        return Err(DecodeError::LimitExceeded {
            what: "file size",
            value: bytes.len() as u64,
            limit: limits.max_file_bytes,
        });
    }
    if !is_3du(bytes) {
        let found = bytes.iter().take(4).map(|b| format!("{b:02x}")).collect::<String>();
        return Err(DecodeError::NotA3du {
            found: if found.is_empty() { "empty file".into() } else { format!("0x{found}") },
        });
    }

    let mut r = Reader::new(bytes);
    let mut sections = Vec::new();
    let mut mark = |name: &str, start: u64, end: u64| {
        sections.push(SectionSpan { name: name.to_string(), start, end })
    };

    // Header ------------------------------------------------------------------------
    let start = r.position();
    r.take(4, "magic")?;
    let header_version = r.u16("header")?;
    let app_version = [r.u16("header")?, r.u16("header")?, r.u16("header")?];
    if !SUPPORTED_HEADER_VERSIONS.contains(&header_version) {
        return Err(DecodeError::UnsupportedVersion {
            header_version,
            app_version: format!("{}.{}.{}", app_version[0], app_version[1], app_version[2]),
            supported: format!("{SUPPORTED_HEADER_VERSIONS:?}"),
        });
    }
    let version_string = r.string("version string")?;
    let title = r.string("title")?;
    let reserved = r.u32("header")?;
    let mut note_labels = Vec::with_capacity(5);
    for _ in 0..5 {
        note_labels.push(r.string("note labels")?);
    }
    let header = RawHeader {
        header_version,
        app_version,
        version_string,
        title,
        reserved,
        note_labels,
    };
    mark("header", start, r.position());

    // Sets (drill tabs) -----------------------------------------------------------------
    let start = r.position();
    let n = r.count("set table", 37)?;
    let mut sets = Vec::with_capacity(n);
    for _ in 0..n {
        let offset = r.position();
        sets.push(RawSet {
            offset,
            start_count: r.u16("set record")?,
            id: r.array("set record")?,
            rehearsal_mark: r.string("set rehearsal mark")?,
            name: r.string("set name")?,
            measures: r.string("set measures")?,
            reserved_a: r.array("set record")?,
            tab_type: r.string("set tab type")?,
            reserved_b: r.u32("set record")?,
            color: r.string("set color")?,
            reserved_c: r.u16("set record")?,
        });
    }
    mark("sets", start, r.position());

    // Performers ------------------------------------------------------------------------
    let start = r.position();
    let n = r.count("performer table", 14)?;
    let mut performers = Vec::with_capacity(n);
    for _ in 0..n {
        performers.push(RawPerformer {
            id: r.array("performer record")?,
            label: r.string("performer label")?,
            info: r.string("performer info")?,
            symbol: r.string("performer symbol")?,
        });
    }
    mark("performers", start, r.position());

    // Audio, timing, tempo ----------------------------------------------------------------
    let start = r.position();
    let audio_reference = r.string("audio reference")?;
    mark("audio", start, r.position());

    let start = r.position();
    let n = r.count("count times", 8)?;
    let mut count_times = Vec::with_capacity(n);
    for _ in 0..n {
        count_times.push(r.f64("count times")?);
    }
    mark("count_times", start, r.position());

    let start = r.position();
    let n = r.count("tempos", 4)?;
    let mut tempos = Vec::with_capacity(n);
    for _ in 0..n {
        tempos.push(r.u32("tempos")?);
    }
    mark("tempos", start, r.position());

    // Field configuration ---------------------------------------------------------------
    let start = r.position();
    let n = r.count("field configuration", 2)?;
    let mut field_records = Vec::with_capacity(n);
    for _ in 0..n {
        field_records.push(r.string("field record")?);
    }
    mark("field", start, r.position());

    // Frames: one per count ---------------------------------------------------------------
    let start = r.position();
    let frame_count = r.count("frame table", 4)?;
    let mut frames = Vec::with_capacity(frame_count);
    let mut samples: u64 = 0;
    for f in 0..frame_count {
        let offset = r.position();
        let n_objects = r.count("frame objects", 33)?;
        samples += n_objects as u64;
        if samples > limits.max_position_samples {
            return Err(DecodeError::LimitExceeded {
                what: "position samples",
                value: samples,
                limit: limits.max_position_samples,
            });
        }
        let mut objects = Vec::with_capacity(n_objects);
        for _ in 0..n_objects {
            objects.push(read_object(&mut r)?);
        }
        let n_ann = r.count("frame annotations", 12)?;
        let mut annotations = Vec::with_capacity(n_ann);
        for _ in 0..n_ann {
            annotations.push(read_annotation(&mut r)?);
        }
        frames.push(RawFrame { offset, objects, annotations });

        if let Some(cb) = progress.as_mut() {
            if !cb(f + 1, frame_count) {
                return Err(DecodeError::Cancelled);
            }
        }
    }
    mark("frames", start, r.position());

    let trailer_offset = r.position();
    let trailer = r.take(r.remaining(), "trailer")?.to_vec();
    mark("trailer", trailer_offset, trailer_offset + trailer.len() as u64);

    Ok(RawDrill {
        file_size: bytes.len() as u64,
        header,
        sets,
        performers,
        audio_reference,
        count_times,
        tempos,
        field_records,
        frames,
        trailer,
        trailer_offset,
        sections,
    })
}

fn read_object(r: &mut Reader<'_>) -> Result<RawObject, DecodeError> {
    let id = r.array("frame object")?;
    let x = r.f32("frame object")?;
    let y = r.f32("frame object")?;
    let rgb = [r.u16("frame object")?, r.u16("frame object")?, r.u16("frame object")?];
    let symbol = r.u16("frame object")?;
    let state = r.u16("frame object")?;
    let kind = r.u16("frame object")?;
    let rotation = r.f32("frame object")?;
    let flag_offset = r.position();
    let size = match r.u8("frame object")? {
        0 => None,
        1 => Some(RawObjectSize {
            width: r.f32("object size")?,
            depth: r.f32("object size")?,
            height: r.f32("object size")?,
            shape: r.u16("object size")?,
        }),
        other => {
            return Err(DecodeError::Invalid {
                context: "frame object",
                offset: flag_offset,
                detail: format!("size flag must be 0 or 1, found {other}"),
            })
        }
    };
    Ok(RawObject {
        id,
        x,
        y,
        rgb,
        symbol,
        state,
        kind,
        rotation,
        size,
    })
}

fn read_annotation(r: &mut Reader<'_>) -> Result<RawAnnotation, DecodeError> {
    let offset = r.position();
    let kind = r.u16("annotation")?;
    let id = r.array("annotation")?;
    let reserved = r.u16("annotation")?;
    match kind {
        2 => {
            let rect = [r.f32("text annotation")?, r.f32("text annotation")?, r.f32("text annotation")?, r.f32("text annotation")?];
            let extra = [r.f32("text annotation")?, r.f32("text annotation")?, r.f32("text annotation")?];
            let text = r.string("text annotation")?;
            Ok(RawAnnotation::Text { id, reserved, rect, extra, text })
        }
        1 => {
            let style = r.take(43, "line annotation style")?.to_vec();
            let n = r.count("line annotation points", 8)?;
            let mut points = Vec::with_capacity(n);
            for _ in 0..n {
                points.push([r.f32("line annotation")?, r.f32("line annotation")?]);
            }
            Ok(RawAnnotation::Polyline { id, reserved, style, points })
        }
        other => Err(DecodeError::Invalid {
            context: "annotation",
            offset,
            detail: format!("unknown annotation type {other}; the record length is unknown so decoding cannot continue safely"),
        }),
    }
}
