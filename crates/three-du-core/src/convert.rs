//! Builds the canonical [`DrillProject`] from decoded `.3du` records.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{Diagnostic, Severity};
use crate::field::SourceField;
use crate::model::*;
use crate::raw::{RawAnnotation, RawDrill};
use crate::transform::{CoordinateTransform, TransformOverrides, INCHES_PER_YARD};

/// A user-supplied set boundary, used when the file has no set table or it must be replaced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageMapEntry {
    pub name: String,
    pub start_count: u32,
    #[serde(default)]
    pub is_subset: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ConvertOptions {
    pub transform: TransformOverrides,
    /// Replaces the decoded set table.
    pub page_map: Option<Vec<PageMapEntry>>,
}

/// Position tolerance (source units) when testing whether a move is a straight, even-step line.
const LINEAR_TOLERANCE: f64 = 1e-3;

pub fn to_project(raw: &RawDrill, sha256: String, opts: &ConvertOptions) -> DrillProject {
    let mut diags = Vec::new();
    let mut unknown = Vec::new();

    // Performers ----------------------------------------------------------------------
    let mut index_of: HashMap<[u8; 8], usize> = HashMap::new();
    let mut performers = Vec::with_capacity(raw.performers.len());
    for p in &raw.performers {
        if index_of.contains_key(&p.id) {
            diags.push(Diagnostic::new(
                Severity::Warning,
                "duplicate_performer_id",
                format!("performer id {} appears twice in the roster; the second entry is ignored", hex(&p.id)),
            ));
            continue;
        }
        let index = performers.len();
        index_of.insert(p.id, index);
        performers.push(Performer {
            index,
            source_id: hex(&p.id),
            label: p.label.clone(),
            section: non_empty(&p.info),
            symbol: non_empty(&p.symbol),
            kind: PerformerKind::Marcher,
            size: None,
            color_rgb: None,
        });
    }

    // Positions -------------------------------------------------------------------------
    let frames = raw.frames.len();
    let mut tracks: Vec<PositionTrack> = (0..performers.len())
        .map(|i| PositionTrack { performer_index: i, xy: vec![None; frames] })
        .collect();
    let mut kind_votes = vec![[0usize; 2]; performers.len()];
    let mut orphan_samples = 0usize;
    let mut non_finite = 0usize;
    let mut alternate_markers = 0usize;
    for (f, frame) in raw.frames.iter().enumerate() {
        for o in &frame.objects {
            let Some(&i) = index_of.get(&o.id) else {
                orphan_samples += 1;
                continue;
            };
            if !(o.x.is_finite() && o.y.is_finite()) {
                non_finite += 1;
                continue;
            }
            tracks[i].xy[f] = Some([o.x, o.y]);
            kind_votes[i][usize::from(o.kind != 1)] += 1;
            if o.kind != 1 && o.kind != 2 || o.state != 0 {
                alternate_markers += 1;
            }
            let p = &mut performers[i];
            if p.size.is_none() {
                p.size = o.size.map(|s| [s.width, s.depth, s.height]);
            }
            if p.color_rgb.is_none() {
                p.color_rgb = Some(o.rgb);
            }
        }
    }
    for (p, votes) in performers.iter_mut().zip(&kind_votes) {
        if votes[0] > votes[1] {
            p.kind = PerformerKind::Prop;
        }
    }
    if orphan_samples > 0 {
        diags.push(Diagnostic::new(
            Severity::Warning,
            "orphan_samples",
            format!("{orphan_samples} position sample(s) reference ids missing from the roster and were skipped"),
        ));
    }
    if non_finite > 0 {
        diags.push(Diagnostic::new(Severity::Warning, "non_finite_positions", format!("{non_finite} non-finite coordinate(s) were dropped")));
    }
    if alternate_markers > 0 {
        diags.push(Diagnostic::new(
            Severity::Info,
            "alternate_markers",
            format!("{alternate_markers} sample(s) use an alternate marker style (object type 7 or state flags); positions are unaffected"),
        ));
    }
    let missing: usize = tracks.iter().map(|t| t.xy.iter().filter(|p| p.is_none()).count()).sum();
    if missing > 0 {
        diags.push(Diagnostic::new(Severity::Warning, "missing_samples", format!("{missing} performer/count sample(s) are absent from the source")));
    }

    // Timeline --------------------------------------------------------------------------
    let mut counts = frames;
    if raw.count_times.len() != frames || raw.tempos.len() != frames {
        counts = frames.min(raw.count_times.len()).min(raw.tempos.len());
        diags.push(Diagnostic::new(
            Severity::Warning,
            "timeline_length_mismatch",
            format!(
                "frames ({frames}), count times ({}) and tempos ({}) differ; using the first {counts} counts",
                raw.count_times.len(),
                raw.tempos.len()
            ),
        ));
        for t in &mut tracks {
            t.xy.truncate(counts);
        }
    }
    let count_times: Vec<f64> = raw.count_times[..counts].to_vec();
    let tempos: Vec<u32> = raw.tempos[..counts].to_vec();
    if count_times.iter().any(|t| !t.is_finite()) || count_times.windows(2).any(|w| w[1] <= w[0]) {
        diags.push(Diagnostic::new(Severity::Warning, "count_times_not_increasing", "count timestamps are not strictly increasing"));
    }
    let mut tempo_changes = Vec::new();
    for (i, &bpm) in tempos.iter().enumerate() {
        if i == 0 || tempos[i - 1] != bpm {
            tempo_changes.push(TempoChange { count: i as u32, bpm });
        }
    }

    // Sets --------------------------------------------------------------------------------
    let mut sets: Vec<DrillSet> = match &opts.page_map {
        Some(map) => {
            diags.push(Diagnostic::new(Severity::Info, "user_page_map", format!("set boundaries taken from a user-supplied page map ({} entries)", map.len())));
            map.iter()
                .enumerate()
                .map(|(i, e)| DrillSet {
                    index: i,
                    source_id: String::new(),
                    name: e.name.clone(),
                    rehearsal_mark: None,
                    measures_text: String::new(),
                    measure_range: None,
                    start_count: e.start_count,
                    counts: 0,
                    is_subset: e.is_subset,
                    tab_type: String::new(),
                    color: None,
                    straight_transition: true,
                    nonlinear_performers: 0,
                    provenance: Provenance::User,
                })
                .collect()
        }
        None => raw
            .sets
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if s.reserved_a != [0; 10] || s.reserved_b != 0 || s.reserved_c != 0 {
                    unknown.push(UnknownRecord {
                        context: format!("set record {i} reserved fields"),
                        offset: s.offset,
                        hex: format!("{}|{:08x}|{:04x}", hex(&s.reserved_a), s.reserved_b, s.reserved_c),
                    });
                }
                DrillSet {
                    index: i,
                    source_id: hex(&s.id),
                    name: s.name.clone(),
                    rehearsal_mark: non_empty(&s.rehearsal_mark),
                    measures_text: s.measures.clone(),
                    measure_range: parse_measure_range(&s.measures),
                    start_count: s.start_count as u32,
                    counts: 0,
                    is_subset: s.tab_type.eq_ignore_ascii_case("SubTab"),
                    tab_type: s.tab_type.clone(),
                    color: Some(s.color.clone()).filter(|c| c != "."),
                    straight_transition: true,
                    nonlinear_performers: 0,
                    provenance: Provenance::Source,
                }
            })
            .collect(),
    };

    let valid_order = sets.windows(2).all(|w| w[1].start_count > w[0].start_count);
    if !valid_order {
        diags.push(Diagnostic::new(
            Severity::Lossy,
            "sets_out_of_order",
            "set start counts are not strictly increasing; sets were sorted and duplicates dropped",
        ));
        sets.sort_by_key(|s| s.start_count);
        sets.dedup_by_key(|s| s.start_count);
    }
    let before = sets.len();
    sets.retain(|s| (s.start_count as usize) < counts);
    if sets.len() < before {
        diags.push(Diagnostic::new(Severity::Warning, "set_beyond_timeline", format!("{} set(s) start after the last stored count and were dropped", before - sets.len())));
    }
    if sets.is_empty() {
        diags.push(Diagnostic::new(
            Severity::Lossy,
            "no_sets",
            "no set boundaries were found; supply a page map, or allow approximation to split the drill at movement breakpoints",
        ));
    } else if sets[0].start_count != 0 {
        diags.push(Diagnostic::new(Severity::Warning, "first_set_not_at_zero", format!("the first set starts at count {}, not 0", sets[0].start_count)));
    }
    for i in 0..sets.len() {
        sets[i].index = i;
        sets[i].counts = if i == 0 { 0 } else { sets[i].start_count.saturating_sub(sets[i - 1].start_count) };
    }
    for i in 1..sets.len() {
        let (a, b) = (sets[i - 1].start_count as usize, sets[i].start_count as usize);
        let nonlinear = tracks.iter().filter(|t| !is_linear(&t.xy, a, b)).count();
        sets[i].nonlinear_performers = nonlinear;
        sets[i].straight_transition = nonlinear == 0;
    }

    let drill_end_count = sets.last().map(|s| s.start_count).unwrap_or(counts.saturating_sub(1) as u32);
    let trailing_counts = (counts as u32).saturating_sub(drill_end_count + 1);
    if trailing_counts > 0 && !sets.is_empty() {
        let end = drill_end_count as usize;
        let moving = tracks.iter().filter(|t| t.xy[end..].iter().any(|p| *p != t.xy[end])).count();
        if moving == 0 {
            diags.push(Diagnostic::new(
                Severity::Info,
                "trailing_static_counts",
                format!("{trailing_counts} count(s) are stored after the final set; nobody moves during them, so exports end at the final set"),
            ));
        } else {
            diags.push(Diagnostic::new(
                Severity::Lossy,
                "movement_after_final_set",
                format!("{moving} performer(s) move during the {trailing_counts} count(s) after the final set; that motion is not part of any set"),
            ));
        }
    }

    // Measures ------------------------------------------------------------------------------
    let measures = build_measures(&sets, &mut diags);

    // Annotations -----------------------------------------------------------------------------
    let set_at: HashMap<u32, usize> = sets.iter().map(|s| (s.start_count, s.index)).collect();
    let mut annotations = Vec::new();
    for (f, frame) in raw.frames.iter().enumerate().take(counts) {
        for a in &frame.annotations {
            let kind = match a {
                RawAnnotation::Text { rect, text, .. } => AnnotationKind::Text { rect: *rect, text: text.clone() },
                RawAnnotation::Polyline { points, .. } => AnnotationKind::Polyline { points: points.clone() },
            };
            annotations.push(Annotation { count: f as u32, set_index: set_at.get(&(f as u32)).copied(), kind });
        }
    }

    // Field and coordinates ---------------------------------------------------------------------
    let source_field = SourceField::parse(&raw.field_records);
    let (coordinates, field) = build_coordinates(source_field, &opts.transform, &mut diags);

    // Unknown bytes -------------------------------------------------------------------------------
    if raw.header.reserved != 0 {
        unknown.push(UnknownRecord { context: "header reserved word".into(), offset: 0, hex: format!("{:08x}", raw.header.reserved) });
    }
    if !raw.trailer.is_empty() {
        unknown.push(UnknownRecord {
            context: "trailer after frames".into(),
            offset: raw.trailer_offset,
            hex: hex(&raw.trailer[..raw.trailer.len().min(4096)]),
        });
    }

    let music = non_empty(&raw.audio_reference).map(|filename| MusicReference { filename, embedded: false });
    if music.is_some() {
        diags.push(Diagnostic::new(Severity::Info, "audio_reference_only", "the drill names an audio file but does not embed audio; no audio is exported"));
    }

    DrillProject {
        model_version: MODEL_VERSION.to_string(),
        metadata: Metadata {
            title: raw.header.title.clone(),
            description: None,
            source: SourceInfo {
                format: "3du".into(),
                magic: "3DUD".into(),
                format_revision: raw.header.header_version,
                writer_version: raw.header.version_string.clone(),
                file_size: raw.file_size,
                sha256,
                sections: raw.sections.clone(),
            },
            music,
            note_labels: raw.header.note_labels.clone(),
        },
        coordinates,
        field,
        performers,
        timeline: Timeline {
            count_times_seconds: count_times,
            tempos_bpm: tempos,
            tempo_changes,
            drill_end_count,
            trailing_counts,
        },
        sets,
        measures,
        positions: tracks,
        annotations,
        diagnostics: diags,
        unknown_records: unknown,
    }
}

/// Splits the drill at counts where any performer changes direction or speed.
/// Used only in approximation mode when no real set table exists.
pub fn infer_sets_from_motion(project: &mut DrillProject) {
    let n = project.timeline.count_len();
    if n == 0 {
        return;
    }
    let mut starts = vec![0u32];
    for c in 1..n.saturating_sub(1) {
        let breaks = project.positions.iter().any(|t| match (t.xy[c - 1], t.xy[c], t.xy[c + 1]) {
            (Some(a), Some(b), Some(d)) => {
                let (v1, v2) = ([b[0] - a[0], b[1] - a[1]], [d[0] - b[0], d[1] - b[1]]);
                ((v2[0] - v1[0]) as f64).abs() > LINEAR_TOLERANCE || ((v2[1] - v1[1]) as f64).abs() > LINEAR_TOLERANCE
            }
            _ => false,
        });
        if breaks {
            starts.push(c as u32);
        }
    }
    let last = (n - 1) as u32;
    if *starts.last().unwrap() != last {
        starts.push(last);
    }
    project.sets = starts
        .iter()
        .enumerate()
        .map(|(i, &c)| DrillSet {
            index: i,
            source_id: String::new(),
            name: format!("{}", i + 1),
            rehearsal_mark: None,
            measures_text: String::new(),
            measure_range: None,
            start_count: c,
            counts: if i == 0 { 0 } else { c - starts[i - 1] },
            is_subset: false,
            tab_type: String::new(),
            color: None,
            straight_transition: true,
            nonlinear_performers: 0,
            provenance: Provenance::Inferred,
        })
        .collect();
    project.timeline.drill_end_count = last;
    project.timeline.trailing_counts = 0;
    project.diagnostics.retain(|d| d.code != "no_sets");
    project.diagnostics.push(Diagnostic::new(
        Severity::Lossy,
        "sets_inferred_from_motion",
        format!("{} set boundaries were inferred from movement breakpoints; they are not the designer's pages", project.sets.len()),
    ));
}

fn is_linear(xy: &[Option<[f32; 2]>], a: usize, b: usize) -> bool {
    if b <= a || b >= xy.len() {
        return true;
    }
    let (Some(p0), Some(p1)) = (xy[a], xy[b]) else {
        return true;
    };
    let span = (b - a) as f64;
    (a + 1..b).all(|c| match xy[c] {
        Some(p) => {
            let t = (c - a) as f64 / span;
            let ex = p0[0] as f64 + (p1[0] as f64 - p0[0] as f64) * t;
            let ey = p0[1] as f64 + (p1[1] as f64 - p0[1] as f64) * t;
            (p[0] as f64 - ex).abs() <= LINEAR_TOLERANCE && (p[1] as f64 - ey).abs() <= LINEAR_TOLERANCE
        }
        None => true,
    })
}

/// Measures from the sets' measure ranges. The range of a set covers the counts since the
/// previous set that carried a range, so the first measure starts one count after it
/// (`Derived`). Inner measure lines are only placed when the counts divide evenly (`Inferred`).
fn build_measures(sets: &[DrillSet], diags: &mut Vec<Diagnostic>) -> Vec<MeasureMarker> {
    let mut out: Vec<MeasureMarker> = Vec::new();
    let mut anchor = sets.first().map(|s| s.start_count).unwrap_or(0);
    let mut uneven = 0;
    let mut last_number: Option<u32> = None;
    let mut discontinuities = 0;
    for s in sets.iter().skip(1) {
        let Some((first, last)) = s.measure_range else { continue };
        if let Some(prev) = last_number {
            if first != prev + 1 {
                discontinuities += 1;
            }
        }
        let n = last - first + 1;
        let span = s.start_count - anchor;
        if span > 0 {
            out.push(MeasureMarker { number: first, start_count: anchor + 1, rehearsal_mark: None, provenance: Provenance::Derived });
            if n > 1 && span % n == 0 {
                let per = span / n;
                for k in 1..n {
                    out.push(MeasureMarker { number: first + k, start_count: anchor + 1 + k * per, rehearsal_mark: None, provenance: Provenance::Inferred });
                }
            } else if n > 1 {
                uneven += 1;
            }
        }
        anchor = s.start_count;
        last_number = Some(last);
    }
    // A rehearsal mark on a set labels the music that starts right after the set.
    for s in sets {
        if let Some(mark) = &s.rehearsal_mark {
            if let Some(m) = out.iter_mut().find(|m| m.start_count == s.start_count + 1) {
                m.rehearsal_mark = Some(mark.clone());
            }
        }
    }
    if uneven > 0 {
        diags.push(Diagnostic::new(
            Severity::Info,
            "uneven_measure_spans",
            format!("{uneven} measure range(s) do not divide evenly into their counts; only their first measure line is placed"),
        ));
    }
    if discontinuities > 0 {
        diags.push(Diagnostic::new(Severity::Info, "measure_range_gaps", format!("{discontinuities} measure range(s) do not continue from the previous one")));
    }
    out
}

pub fn parse_measure_range(text: &str) -> Option<(u32, u32)> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let (a, b) = match t.split_once('-') {
        Some((a, b)) => (a.trim().parse().ok()?, b.trim().parse().ok()?),
        None => {
            let v = t.parse().ok()?;
            (v, v)
        }
    };
    (a <= b).then_some((a, b))
}

fn build_coordinates(src: SourceField, overrides: &TransformOverrides, diags: &mut Vec<Diagnostic>) -> (CoordinateSystem, Field) {
    let mut notes = Vec::new();
    let mut field_prov = Provenance::Source;

    let grid = src.grid.unwrap_or_else(|| {
        diags.push(Diagnostic::new(Severity::Warning, "missing_grid", "no GRID record; assuming 8 steps per 5 yards"));
        field_prov = Provenance::Inferred;
        crate::field::Grid { steps_x: 8.0, units_x: 5.0, steps_y: 8.0, units_y: 5.0 }
    });
    let border = src.border.unwrap_or_else(|| {
        diags.push(Diagnostic::new(Severity::Warning, "missing_border", "no BORD record; assuming a 100 × 52.5 yard field centered on the origin"));
        field_prov = Provenance::Inferred;
        crate::field::Bounds { x1: -50.0, y1: -26.25, x2: 50.0, y2: 26.25 }
    });

    match src.unit_code.as_deref() {
        Some("0 0") => notes.push("UNIT 0 0 with yard lines every 5 units: source unit read as yards".into()),
        other => diags.push(Diagnostic::new(
            Severity::Warning,
            "unknown_unit_code",
            format!("unrecognized UNIT record {other:?}; assuming yards (override with --steps-per-unit)"),
        )),
    }

    // Orientation: captions name the top/bottom of the source view, and the verified sample
    // places tall scenery at +y. +y is therefore toward the back sideline unless the captions
    // say otherwise.
    let top = src.caption("TOP").unwrap_or("").to_ascii_lowercase();
    let bottom = src.caption("BOTTOM").unwrap_or("").to_ascii_lowercase();
    let y_back = !(top.contains("front") || bottom.contains("back"));
    notes.push(format!(
        "+y points toward the {} sideline (TOP caption {:?}, BOTTOM caption {:?})",
        if y_back { "back" } else { "front" },
        src.caption("TOP").unwrap_or(""),
        src.caption("BOTTOM").unwrap_or("")
    ));
    let left = src.caption("LEFT").unwrap_or("Side 1").to_string();
    let right = src.caption("RIGHT").unwrap_or("Side 2").to_string();
    notes.push(format!("-x is {left:?} and +x is {right:?}, audience left to right"));

    let (front_y, back_y) = if y_back { (border.y1, border.y2) } else { (border.y2, border.y1) };
    let center_x = (border.x1 + border.x2) / 2.0;
    let step_inches = grid.units_x / grid.steps_x * INCHES_PER_YARD;
    let base = CoordinateTransform {
        steps_per_unit_x: grid.steps_per_unit_x(),
        steps_per_unit_y: grid.steps_per_unit_y(),
        center_x,
        front_y,
        x_sign: 1.0,
        y_sign: if y_back { 1.0 } else { -1.0 },
        step_inches,
        field_depth_steps: (back_y - front_y).abs() * grid.steps_per_unit_y(),
    };
    let mut transform = overrides.apply(base);
    let user = !overrides.is_empty();
    if user {
        notes.push("transform adjusted by user overrides".into());
    }
    if overrides.steps_per_unit.is_some() {
        transform.step_inches = INCHES_PER_YARD / transform.steps_per_unit_x;
        transform.field_depth_steps = (back_y - front_y).abs() * transform.steps_per_unit_y;
    }
    if let Err(e) = transform.check() {
        diags.push(Diagnostic::new(Severity::Lossy, "invalid_transform", e));
    }
    finish(src, transform, front_y, back_y, border, left, right, notes, field_prov, user)
}

#[allow(clippy::too_many_arguments)]
fn finish(
    src: SourceField,
    transform: CoordinateTransform,
    front_y: f64,
    back_y: f64,
    border: crate::field::Bounds,
    left: String,
    right: String,
    notes: Vec<String>,
    field_prov: Provenance,
    user: bool,
) -> (CoordinateSystem, Field) {
    let center = transform.center_x;
    let half = (border.x2 - border.x1) / 2.0;
    let yard_lines: Vec<YardLine> = src
        .x_lines
        .iter()
        .map(|l| {
            let from_goal = half - (l.position - center).abs();
            let yard_number = (from_goal >= -1e-9 && (from_goal / 5.0 - (from_goal / 5.0).round()).abs() < 1e-9).then_some(from_goal.max(0.0));
            YardLine { position: l.position, yard_number, major: l.major }
        })
        .collect();
    let extent_x_min = src.x_lines.iter().map(|l| l.position).fold(border.x1, f64::min);
    let extent_x_max = src.x_lines.iter().map(|l| l.position).fold(border.x2, f64::max);
    let hashes = src.hashes.clone();
    let front_label = src.caption("BOTTOM").map(str::to_string).unwrap_or_else(|| "Front".into());
    let back_label = src.caption("TOP").map(str::to_string).unwrap_or_else(|| "Back".into());
    let coordinates = CoordinateSystem {
        source_unit: "yard".into(),
        source_unit_provenance: if user { Provenance::User } else { Provenance::Inferred },
        transform,
        orientation_provenance: if user { Provenance::User } else { Provenance::Inferred },
        notes,
    };
    let field = Field {
        source: src,
        front_sideline_y: front_y,
        back_sideline_y: back_y,
        playing_x_min: border.x1,
        playing_x_max: border.x2,
        extent_x_min,
        extent_x_max,
        hashes,
        yard_lines,
        side_labels: [left, right],
        front_label,
        back_label,
        provenance: field_prov,
    };
    (coordinates, field)
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_ranges() {
        assert_eq!(parse_measure_range("175-176"), Some((175, 176)));
        assert_eq!(parse_measure_range(" 177 "), Some((177, 177)));
        assert_eq!(parse_measure_range(""), None);
        assert_eq!(parse_measure_range("9-3"), None);
        assert_eq!(parse_measure_range("abc"), None);
    }

    #[test]
    fn linearity() {
        let straight = vec![Some([0.0, 0.0]), Some([1.0, 0.5]), Some([2.0, 1.0])];
        assert!(is_linear(&straight, 0, 2));
        let curved = vec![Some([0.0, 0.0]), Some([1.0, 1.0]), Some([2.0, 0.0])];
        assert!(!is_linear(&curved, 0, 2));
        // A hold then a fast move is not an even-step line either.
        let uneven = vec![Some([0.0, 0.0]), Some([0.0, 0.0]), Some([2.0, 0.0])];
        assert!(!is_linear(&uneven, 0, 2));
    }
}
