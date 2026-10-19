//! The canonical, versioned drill model shared by every importer and exporter.
//!
//! Nothing here is specific to OpenMarch or Fieldwork. Raw source coordinates are kept at
//! source precision (`f32`); canonical coordinates are derived on demand through
//! [`CoordinateTransform`] so the two can never drift apart.

use serde::{Deserialize, Serialize};

use crate::error::{Diagnostic, Severity};
use crate::field::SourceField;
use crate::raw::SectionSpan;
use crate::transform::CoordinateTransform;

pub const MODEL_VERSION: &str = "1.0.0";

/// Where a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Read directly from a decoded source record.
    Source,
    /// Computed exactly from source values by a documented rule.
    Derived,
    /// An assumption backed by evidence, but not stated by the file.
    Inferred,
    /// Supplied by the user (command-line override or page map).
    User,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrillProject {
    pub model_version: String,
    pub metadata: Metadata,
    pub coordinates: CoordinateSystem,
    pub field: Field,
    pub performers: Vec<Performer>,
    pub timeline: Timeline,
    pub sets: Vec<DrillSet>,
    pub measures: Vec<MeasureMarker>,
    /// One track per performer, in performer order.
    pub positions: Vec<PositionTrack>,
    pub annotations: Vec<Annotation>,
    pub diagnostics: Vec<Diagnostic>,
    pub unknown_records: Vec<UnknownRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    pub title: String,
    pub description: Option<String>,
    pub source: SourceInfo,
    pub music: Option<MusicReference>,
    /// Captions of the source's note fields.
    pub note_labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// Always `"3du"` for this importer.
    pub format: String,
    pub magic: String,
    /// Layout revision read from the header.
    pub format_revision: u16,
    /// Version of the writing application, e.g. "11.3.6".
    pub writer_version: String,
    pub file_size: u64,
    /// SHA-256 of the source bytes; used for deterministic output identifiers.
    pub sha256: String,
    pub sections: Vec<SectionSpan>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicReference {
    pub filename: String,
    /// True only when audio bytes are embedded in the source. No `.3du` seen so far embeds audio.
    pub embedded: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoordinateSystem {
    /// Source distance unit (`"yard"` for the verified layout).
    pub source_unit: String,
    pub source_unit_provenance: Provenance,
    pub transform: CoordinateTransform,
    pub orientation_provenance: Provenance,
    /// Human-readable justification for the orientation and scale.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YardLine {
    /// Source x of the line.
    pub position: f64,
    /// Yard number shown on a football field (0 = goal line, 50 = center), when applicable.
    pub yard_number: Option<f64>,
    pub major: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    /// Decoded field records, including uninterpreted ones.
    pub source: SourceField,
    /// Front and back sidelines as source y values.
    pub front_sideline_y: f64,
    pub back_sideline_y: f64,
    /// Goal line to goal line (playing surface) in source x.
    pub playing_x_min: f64,
    pub playing_x_max: f64,
    /// Outermost lines drawn by the source (end lines), in source x.
    pub extent_x_min: f64,
    pub extent_x_max: f64,
    /// Source y of each hash line.
    pub hashes: Vec<f64>,
    pub yard_lines: Vec<YardLine>,
    pub side_labels: [String; 2],
    pub front_label: String,
    pub back_label: String,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformerKind {
    Marcher,
    /// Scenery or a stationary object drawn on the field.
    Prop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Performer {
    pub index: usize,
    /// The stable 8-byte source identifier as 16 hex digits.
    pub source_id: String,
    pub label: String,
    pub section: Option<String>,
    pub symbol: Option<String>,
    pub kind: PerformerKind,
    /// Width, depth, height in source units, when the object has a physical size.
    pub size: Option<[f32; 3]>,
    pub color_rgb: Option<[u16; 3]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TempoChange {
    pub count: u32,
    pub bpm: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    /// Absolute time of each count in seconds; count 0 is the start of the drill.
    pub count_times_seconds: Vec<f64>,
    /// Tempo stored at each count. `tempos_bpm[c]` governs the interval from count `c` to
    /// `c + 1`, so `count_times_seconds[c + 1] - count_times_seconds[c] == 60 / tempos_bpm[c]`.
    pub tempos_bpm: Vec<u32>,
    pub tempo_changes: Vec<TempoChange>,
    /// The count at which the final set is reached. Exports cover counts `0..=drill_end_count`.
    pub drill_end_count: u32,
    /// Counts stored after the final set.
    pub trailing_counts: u32,
}

impl Timeline {
    pub fn count_len(&self) -> usize {
        self.count_times_seconds.len()
    }

    /// Duration of the count that ends at `count` (count ≥ 1).
    pub fn duration_of(&self, count: usize) -> f64 {
        self.count_times_seconds[count] - self.count_times_seconds[count - 1]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrillSet {
    pub index: usize,
    pub source_id: String,
    pub name: String,
    /// Rehearsal mark with surrounding whitespace removed; `None` when blank.
    pub rehearsal_mark: Option<String>,
    /// Measure text as written, e.g. "175-176".
    pub measures_text: String,
    /// Parsed measure range of the move into this set.
    pub measure_range: Option<(u32, u32)>,
    /// Count at which this formation is reached.
    pub start_count: u32,
    /// Counts in the move from the previous set (0 for the first set).
    pub counts: u32,
    pub is_subset: bool,
    pub tab_type: String,
    pub color: Option<String>,
    /// False when at least one performer moves along a non-straight or non-uniform path
    /// into this set (curves, follow-the-leader, holds inside the move).
    pub straight_transition: bool,
    pub nonlinear_performers: usize,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasureMarker {
    /// Measure number as printed in the music.
    pub number: u32,
    /// First count of the measure.
    pub start_count: u32,
    pub rehearsal_mark: Option<String>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionTrack {
    pub performer_index: usize,
    /// Raw source `[x, y]` per count; `None` when the performer has no sample at that count.
    pub xy: Vec<Option<[f32; 2]>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnnotationKind {
    Text { rect: [f32; 4], text: String },
    Polyline { points: Vec<[f32; 2]> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    pub count: u32,
    /// The set reached at `count`, if any.
    pub set_index: Option<usize>,
    #[serde(flatten)]
    pub kind: AnnotationKind,
}

/// Bytes whose meaning is not known, preserved for later research.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnknownRecord {
    pub context: String,
    pub offset: u64,
    pub hex: String,
}

impl DrillProject {
    pub fn position(&self, performer: usize, count: usize) -> Option<[f32; 2]> {
        self.positions.get(performer)?.xy.get(count).copied().flatten()
    }

    /// Canonical position in steps (see [`crate::transform`]).
    pub fn steps(&self, performer: usize, count: usize) -> Option<(f64, f64)> {
        let [x, y] = self.position(performer, count)?;
        Some(self.coordinates.transform.to_steps(x as f64, y as f64))
    }

    /// Canonical position in meters from field center.
    pub fn meters(&self, performer: usize, count: usize) -> Option<(f64, f64)> {
        let [x, y] = self.position(performer, count)?;
        Some(self.coordinates.transform.to_meters_centered(x as f64, y as f64))
    }

    pub fn has_lossy_diagnostics(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == Severity::Lossy)
    }

    /// A summary safe to print: counts and capabilities, no names or titles.
    pub fn summary(&self) -> Summary {
        Summary {
            format: format!("{} (revision {})", self.metadata.source.format, self.metadata.source.format_revision),
            writer_version: self.metadata.source.writer_version.clone(),
            file_size: self.metadata.source.file_size,
            performers: self.performers.iter().filter(|p| p.kind == PerformerKind::Marcher).count(),
            props: self.performers.iter().filter(|p| p.kind == PerformerKind::Prop).count(),
            counts_stored: self.timeline.count_len(),
            drill_end_count: self.timeline.drill_end_count,
            sets: self.sets.iter().filter(|s| !s.is_subset).count(),
            subsets: self.sets.iter().filter(|s| s.is_subset).count(),
            position_samples: self.positions.iter().map(|t| t.xy.iter().filter(|p| p.is_some()).count()).sum(),
            tempo_changes: self.timeline.tempo_changes.len(),
            measure_markers: self.measures.len(),
            nonlinear_transitions: self.sets.iter().filter(|s| !s.straight_transition).count(),
            text_annotations: self.annotations.iter().filter(|a| matches!(a.kind, AnnotationKind::Text { .. })).count(),
            line_annotations: self.annotations.iter().filter(|a| matches!(a.kind, AnnotationKind::Polyline { .. })).count(),
            has_music_reference: self.metadata.music.is_some(),
            embedded_audio: self.metadata.music.as_ref().is_some_and(|m| m.embedded),
            source_unit: self.coordinates.source_unit.clone(),
            step_inches: self.coordinates.transform.step_inches,
            diagnostics: self.diagnostics.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub format: String,
    pub writer_version: String,
    pub file_size: u64,
    pub performers: usize,
    pub props: usize,
    pub counts_stored: usize,
    pub drill_end_count: u32,
    pub sets: usize,
    pub subsets: usize,
    pub position_samples: usize,
    pub tempo_changes: usize,
    pub measure_markers: usize,
    pub nonlinear_transitions: usize,
    pub text_annotations: usize,
    pub line_annotations: usize,
    pub has_music_reference: bool,
    pub embedded_audio: bool,
    pub source_unit: String,
    pub step_inches: f64,
    pub diagnostics: Vec<Diagnostic>,
}
