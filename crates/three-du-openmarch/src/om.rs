//! OpenMarch Universal Standard (`.om` / `.omz`), schema version 0.2.0.
//!
//! Field names follow `src/schema.ts` of <https://github.com/OpenMarch/schema> exactly.

use std::io::{Read, Write};

use flate2::{read::GzDecoder, write::GzEncoder, Compression, GzBuilder};
use serde::{Deserialize, Serialize};
use three_du_core::{Diagnostic, DrillProject, Severity};

use crate::field::layout;
use crate::shared::{beat_durations, page_coordinates, performer_section};

pub const SCHEMA_VERSION: &str = "0.2.0";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OmShow {
    pub om_schema_version: String,
    pub metadata: OmMetadata,
    pub performers: Vec<OmPerformer>,
    pub pages: Vec<OmPage>,
    pub tempo_sections: Vec<OmTempoSection>,
    pub coordinates: Vec<OmCoordinate>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub measures: Option<Vec<OmMeasure>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub performer_appearance: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OmMetadata {
    pub created_at_utc: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub audio_offset_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub info: Option<OmInfo>,
    pub performance_area: OmPerformanceArea,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OmInfo {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OmPerformanceArea {
    pub y_origin: String,
    pub inches_per_step: f64,
    pub x_checkpoints: Vec<OmXCheckpoint>,
    pub y_checkpoints: Vec<OmYCheckpoint>,
    pub width_feet: f64,
    pub height_feet: f64,
    pub use_hashes: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OmXCheckpoint {
    pub name: String,
    pub terse_name: String,
    pub use_as_reference: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub field_label: Option<String>,
    pub visible: bool,
    pub steps_from_center: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OmYCheckpoint {
    pub name: String,
    pub terse_name: String,
    pub use_as_reference: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub field_label: Option<String>,
    pub visible: bool,
    pub steps_from_y_origin: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OmPerformer {
    pub id: u32,
    pub label: String,
    pub section: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OmPage {
    pub id: String,
    pub duration: f64,
    pub start_beat_index: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub is_subset: Option<bool>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OmTempoSection {
    pub tempo: f64,
    pub number_of_beats: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OmCoordinate {
    #[serde(rename = "marcherId")]
    pub marcher_id: String,
    #[serde(rename = "pageId")]
    pub page_id: String,
    #[serde(rename = "xSteps")]
    pub x_steps: f64,
    #[serde(rename = "ySteps")]
    pub y_steps: f64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rotation_degrees: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OmMeasure {
    pub start_beat_index: u32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rehearsal_mark: Option<String>,
}

/// Builds the Universal Standard document. Requires at least one set.
pub fn build(p: &DrillProject, diags: &mut Vec<Diagnostic>) -> OmShow {
    let lay = layout(p);
    let (x_min, x_max) = lay.x_range();
    let (y_min, y_max) = lay.y_range();

    let performers: Vec<OmPerformer> = p
        .performers
        .iter()
        .map(|q| OmPerformer { id: q.index as u32 + 1, label: q.label.clone(), section: performer_section(q), name: None, notes: None })
        .collect();

    let durations = beat_durations(p);
    let mut pages = Vec::with_capacity(p.sets.len());
    for (i, s) in p.sets.iter().enumerate() {
        let (start, duration) = if i == 0 {
            (0, 0.0)
        } else {
            let a = p.sets[i - 1].start_count as usize;
            let b = s.start_count as usize;
            (a as u32 + 1, durations[a + 1..=b].iter().sum())
        };
        pages.push(OmPage { id: i.to_string(), duration, start_beat_index: start, is_subset: s.is_subset.then_some(true), name: s.name.clone() });
    }

    // The first beat has no duration; the Universal Standard writes it as tempo 0 × 1 beat.
    let mut tempo_sections = vec![OmTempoSection { tempo: 0.0, number_of_beats: 1 }];
    for (c, &d) in durations.iter().enumerate().skip(1) {
        let source_bpm = p.timeline.tempos_bpm[c - 1] as f64;
        let tempo = if source_bpm > 0.0 && (60.0 / source_bpm - d).abs() < 1e-9 { source_bpm } else { 60.0 / d };
        match tempo_sections.last_mut() {
            Some(last) if c > 1 && (last.tempo - tempo).abs() < 1e-9 => last.number_of_beats += 1,
            _ => tempo_sections.push(OmTempoSection { tempo, number_of_beats: 1 }),
        }
    }

    let mut coordinates = Vec::with_capacity(p.sets.len() * p.performers.len());
    for (page, rows) in page_coordinates(p, diags).into_iter().enumerate() {
        for (performer, (xs, ys)) in rows.into_iter().enumerate() {
            coordinates.push(OmCoordinate {
                marcher_id: (performer + 1).to_string(),
                page_id: page.to_string(),
                x_steps: xs,
                y_steps: ys,
                rotation_degrees: None,
            });
        }
    }

    let measures: Vec<OmMeasure> = p
        .measures
        .iter()
        .filter(|m| m.start_count <= p.timeline.drill_end_count)
        .map(|m| OmMeasure { start_beat_index: m.start_count, name: m.number.to_string(), rehearsal_mark: m.rehearsal_mark.clone() })
        .collect();

    let nonlinear = p.sets.iter().filter(|s| !s.straight_transition).count();
    if nonlinear > 0 {
        diags.push(Diagnostic::new(
            Severity::Warning,
            "curves_flattened",
            format!("{nonlinear} move(s) contain curved or uneven paths; OpenMarch interpolates straight lines between pages"),
        ));
    }
    let annotations = p.annotations.len();
    if annotations > 0 {
        diags.push(Diagnostic::new(Severity::Info, "annotations_not_exported", format!("{annotations} field annotation(s) have no Universal Standard equivalent")));
    }

    OmShow {
        om_schema_version: SCHEMA_VERSION.into(),
        metadata: OmMetadata {
            // Fixed so identical input produces identical output.
            created_at_utc: "1970-01-01T00:00:00.000Z".into(),
            audio_offset_seconds: None,
            info: Some(OmInfo { title: Some(p.metadata.title.clone()) }),
            performance_area: OmPerformanceArea {
                y_origin: "front".into(),
                inches_per_step: lay.step_inches,
                x_checkpoints: lay
                    .x
                    .iter()
                    .map(|c| OmXCheckpoint {
                        name: c.name.clone(),
                        terse_name: c.terse_name.clone(),
                        use_as_reference: c.use_as_reference,
                        field_label: c.field_label.clone(),
                        visible: c.visible,
                        steps_from_center: c.steps,
                    })
                    .collect(),
                y_checkpoints: lay
                    .y
                    .iter()
                    .map(|c| OmYCheckpoint {
                        name: c.name.clone(),
                        terse_name: c.terse_name.clone(),
                        use_as_reference: c.use_as_reference,
                        field_label: c.field_label.clone(),
                        visible: c.visible,
                        steps_from_y_origin: c.steps,
                    })
                    .collect(),
                width_feet: (x_max - x_min) * lay.step_inches / 12.0,
                height_feet: (y_max - y_min) * lay.step_inches / 12.0,
                use_hashes: lay.use_hashes,
            },
        },
        performers,
        pages,
        tempo_sections,
        coordinates,
        measures: (!measures.is_empty()).then_some(measures),
        performer_appearance: None,
    }
}

pub fn to_json(show: &OmShow) -> Vec<u8> {
    serde_json::to_vec_pretty(show).expect("OmShow always serializes")
}

/// gzip with a zero timestamp and no file name, so output is byte-for-byte reproducible.
pub fn to_gzip(json: &[u8]) -> Vec<u8> {
    let mut enc: GzEncoder<Vec<u8>> = GzBuilder::new().mtime(0).write(Vec::new(), Compression::best());
    enc.write_all(json).expect("in-memory write");
    enc.finish().expect("in-memory write")
}

/// Parses `.om` or `.omz` bytes (gzip detected by magic number).
pub fn parse(bytes: &[u8]) -> Result<OmShow, String> {
    let json = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        GzDecoder::new(bytes).take(1 << 31).read_to_end(&mut out).map_err(|e| format!("gzip: {e}"))?;
        out
    } else {
        bytes.to_vec()
    };
    serde_json::from_slice(&json).map_err(|e| format!("JSON does not match the Universal Standard: {e}"))
}

/// Structural checks beyond the JSON shape: references resolve, beats line up, values are finite.
pub fn validate(show: &OmShow) -> Result<(), String> {
    if show.om_schema_version != SCHEMA_VERSION {
        return Err(format!("omSchemaVersion {} (expected {SCHEMA_VERSION})", show.om_schema_version));
    }
    let area = &show.metadata.performance_area;
    if area.y_origin != "front" && area.y_origin != "center" {
        return Err(format!("yOrigin must be front or center, found {}", area.y_origin));
    }
    let beats: u32 = show.tempo_sections.iter().map(|t| t.number_of_beats).sum();
    let first = show.tempo_sections.first().ok_or("no tempo sections")?;
    if first.tempo != 0.0 || first.number_of_beats != 1 {
        return Err("the first tempo section must be tempo 0 with 1 beat".into());
    }
    if show.tempo_sections.iter().skip(1).any(|t| !(t.tempo.is_finite() && t.tempo > 0.0) || t.number_of_beats == 0) {
        return Err("tempo sections after the first must have a positive tempo and at least one beat".into());
    }
    let page_ids: std::collections::HashSet<&str> = show.pages.iter().map(|p| p.id.as_str()).collect();
    if page_ids.len() != show.pages.len() {
        return Err("page ids are not unique".into());
    }
    if show.pages.first().map(|p| p.start_beat_index) != Some(0) {
        return Err("the first page must start at beat 0".into());
    }
    if show.pages.windows(2).any(|w| w[1].start_beat_index <= w[0].start_beat_index) {
        return Err("page start beats must increase".into());
    }
    if show.pages.iter().any(|p| p.start_beat_index >= beats) {
        return Err("a page starts after the last beat".into());
    }
    let performer_ids: std::collections::HashSet<String> = show.performers.iter().map(|p| p.id.to_string()).collect();
    for c in &show.coordinates {
        if !performer_ids.contains(&c.marcher_id) || !page_ids.contains(c.page_id.as_str()) {
            return Err(format!("coordinate references unknown marcher {} or page {}", c.marcher_id, c.page_id));
        }
        if !(c.x_steps.is_finite() && c.y_steps.is_finite()) {
            return Err("non-finite coordinate".into());
        }
    }
    if show.coordinates.len() != show.pages.len() * show.performers.len() {
        return Err(format!("expected {} coordinates, found {}", show.pages.len() * show.performers.len(), show.coordinates.len()));
    }
    Ok(())
}
