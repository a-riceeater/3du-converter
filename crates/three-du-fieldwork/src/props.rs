//! Props and scenery (objects the source stores in its roster but that never march).
//!
//! Fieldwork format 1.0 has no prop tables, so props are not written as performers.
//! They are kept, complete, in an optional extension member that Fieldwork preserves
//! byte-for-byte on save:
//!
//!   `extensions/org.3du-converter.props/props.json`
//!
//! When Fieldwork gains native props, replace [`extension_member`] with a writer for the
//! new tables; [`collect`] already yields everything that writer needs, in Fieldwork units.

use serde::Serialize;
use three_du_core::{DrillProject, PerformerKind};

pub const NAMESPACE: &str = "org.3du-converter.props";
pub const PATH: &str = "extensions/org.3du-converter.props/props.json";
/// Version of the JSON layout below; bump on incompatible changes.
pub const LAYOUT_VERSION: u32 = 1;

const YARD_M: f64 = 0.9144;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prop {
    /// The source's stable 8-byte id, as 16 hex digits.
    pub source_id: String,
    /// Label as written in the source (often "unlabeled").
    pub label: String,
    /// Width (x), depth (y) and height (z) in meters, when the source gives a size.
    pub size_meters: Option<[f64; 3]>,
    /// Source shape code (0 and 2 observed; meaning not decoded yet).
    pub shape_code: Option<u16>,
    /// Source rotation in radians, unconverted (direction not yet verified).
    pub rotation_radians: Option<f32>,
    /// Source color, components 0–255.
    pub color_rgb: Option<[u16; 3]>,
    /// Position at each set, in set order: meters from field center, Fieldwork axes.
    /// `None` when the source has no sample for the prop at that set.
    pub placements: Vec<Option<PropPlacement>>,
    /// True when the prop is at the same place at every stored count.
    pub stationary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PropPlacement {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PropsDocument<'a> {
    layout_version: u32,
    units: &'static str,
    /// Set labels in order, aligned with every prop's `placements`.
    sets: Vec<&'a str>,
    props: Vec<Prop>,
}

pub fn collect(p: &DrillProject) -> Vec<Prop> {
    // Sizes are in source units; the verified layout uses yards.
    let unit_m = if p.coordinates.source_unit == "yard" { YARD_M } else { p.coordinates.transform.step_meters() * p.coordinates.transform.steps_per_unit_x };
    p.performers
        .iter()
        .filter(|q| q.kind == PerformerKind::Prop)
        .map(|q| {
            let track = &p.positions[q.index].xy;
            let first = track.iter().flatten().next();
            Prop {
                source_id: q.source_id.clone(),
                label: q.label.clone(),
                size_meters: q.size.map(|s| s.map(|v| v as f64 * unit_m)),
                shape_code: q.shape_code,
                rotation_radians: q.rotation_radians,
                color_rgb: q.color_rgb,
                placements: p
                    .sets
                    .iter()
                    .map(|s| p.meters(q.index, s.start_count as usize).map(|(x, y)| PropPlacement { x, y }))
                    .collect(),
                stationary: track.iter().flatten().all(|xy| Some(xy) == first),
            }
        })
        .collect()
}

/// The extension member, or `None` when the drill has no props.
pub fn extension_member(p: &DrillProject) -> Option<(String, Vec<u8>)> {
    let props = collect(p);
    if props.is_empty() {
        return None;
    }
    let doc = PropsDocument { layout_version: LAYOUT_VERSION, units: "meters", sets: p.sets.iter().map(|s| s.name.as_str()).collect(), props };
    Some((PATH.to_string(), serde_json::to_vec_pretty(&doc).expect("props serialize")))
}
