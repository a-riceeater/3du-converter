//! Exporter for Fieldwork `.fieldwork` documents (container format 1.0, database schema 4,
//! or schema 5 when curved or uneven moves are written as per-performer [`paths`]).
//!
//! The layout follows `fieldwork-file-schema.md` and the reader in Fieldwork's
//! `packages/DrillDocument`. Units are meters from field center (+x audience right,
//! +y toward the back sideline), durations in seconds, IDs uppercase UUIDs.

pub mod paths;
pub mod props;
pub mod zip;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use paths::{path_end, Point, Ratio, Segment};
use rusqlite::types::Value;
use rusqlite::{params, Connection, DatabaseName};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use three_du_core::export::{enforce, stable_uuid, write_atomic, ExportError, LossPolicy};
use three_du_core::{AnnotationKind, Diagnostic, DrillProject, Performer, PerformerKind, Severity};

pub const FORMAT: &str = "org.fieldwork.show";
pub const SCHEMA_VERSION: i32 = 4;
/// Schema written when at least one performer follows a path.
pub const PATHS_SCHEMA_VERSION: i32 = 5;
pub const PATHS_CAPABILITY: &str = "drill.paths.v1";
pub const WRITER: &str = concat!("3du-converter ", env!("CARGO_PKG_VERSION"));
pub const EXTENSION_NAMESPACE: &str = "org.3du-converter.provenance";
const SYMBOLS: [&str; 6] = ["circle", "cross", "square", "triangle", "diamond", "slash"];

const SCHEMA: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE show (id TEXT PRIMARY KEY, title TEXT NOT NULL, max_stride REAL NOT NULL, min_clearance REAL NOT NULL);
CREATE TABLE field (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), name TEXT NOT NULL, preset TEXT NOT NULL,
    length REAL NOT NULL, width REAL NOT NULL, step_size REAL NOT NULL, side_labels TEXT NOT NULL,
    front_label TEXT NOT NULL, back_label TEXT NOT NULL);
CREATE TABLE field_checkpoint (ordinal INTEGER PRIMARY KEY, axis TEXT NOT NULL CHECK (axis IN ('x','y')),
    position REAL NOT NULL, label TEXT NOT NULL, prominence TEXT NOT NULL);
CREATE TABLE media_asset (id TEXT PRIMARY KEY, sha256 TEXT NOT NULL, extension TEXT NOT NULL, original_filename TEXT NOT NULL,
    byte_count INTEGER NOT NULL, sample_rate REAL NOT NULL, frame_count INTEGER NOT NULL, channel_count INTEGER NOT NULL);
CREATE TABLE section (id TEXT PRIMARY KEY, ordinal INTEGER NOT NULL, name TEXT NOT NULL, prefix TEXT NOT NULL,
    symbol TEXT NOT NULL, color_index INTEGER NOT NULL);
CREATE TABLE performer (id TEXT PRIMARY KEY, ordinal INTEGER NOT NULL, label TEXT NOT NULL UNIQUE,
    section_id TEXT NOT NULL REFERENCES section(id), name TEXT, notes TEXT NOT NULL);
CREATE TABLE movement (id TEXT PRIMARY KEY, ordinal INTEGER NOT NULL, name TEXT NOT NULL,
    audio_asset_id TEXT REFERENCES media_asset(id), fit_mode TEXT NOT NULL);
CREATE TABLE measure (id TEXT PRIMARY KEY, movement_id TEXT NOT NULL REFERENCES movement(id), ordinal INTEGER NOT NULL,
    number INTEGER NOT NULL, meter_numerator INTEGER NOT NULL, meter_denominator INTEGER NOT NULL, grouping TEXT,
    tempo_unit_num INTEGER NOT NULL, tempo_unit_den INTEGER NOT NULL, pulse_unit_num INTEGER NOT NULL, pulse_unit_den INTEGER NOT NULL,
    pickup_num INTEGER, pickup_den INTEGER);
CREATE TABLE timing_event (id TEXT PRIMARY KEY, movement_id TEXT NOT NULL REFERENCES movement(id), ordinal INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('beat','hold')), measure_id TEXT NOT NULL REFERENCES measure(id), beat_index INTEGER NOT NULL,
    notated_num INTEGER NOT NULL, notated_den INTEGER NOT NULL, baseline_seconds REAL NOT NULL CHECK (baseline_seconds > 0));
CREATE TABLE audio_anchor (id TEXT PRIMARY KEY, movement_id TEXT NOT NULL REFERENCES movement(id),
    event_id TEXT NOT NULL REFERENCES timing_event(id), fraction_num INTEGER NOT NULL, fraction_den INTEGER NOT NULL,
    asset_id TEXT NOT NULL REFERENCES media_asset(id), sample_frame INTEGER NOT NULL, sample_rate REAL NOT NULL, locked INTEGER NOT NULL);
CREATE TABLE arrival_set (id TEXT PRIMARY KEY, movement_id TEXT NOT NULL REFERENCES movement(id), ordinal INTEGER NOT NULL,
    label TEXT NOT NULL, event_id TEXT NOT NULL REFERENCES timing_event(id), fraction_num INTEGER NOT NULL, fraction_den INTEGER NOT NULL,
    is_subset INTEGER NOT NULL, notes TEXT NOT NULL);
CREATE TABLE transition (id TEXT PRIMARY KEY, to_set_id TEXT NOT NULL UNIQUE REFERENCES arrival_set(id), model TEXT NOT NULL);
CREATE TABLE placement (set_id TEXT NOT NULL REFERENCES arrival_set(id), performer_id TEXT NOT NULL REFERENCES performer(id),
    state TEXT NOT NULL CHECK (state IN ('placed','unresolved','inactive')), x REAL, y REAL, z REAL, facing REAL,
    PRIMARY KEY (set_id, performer_id));
CREATE TABLE form (id TEXT PRIMARY KEY, set_id TEXT NOT NULL REFERENCES arrival_set(id), ordinal INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('line','arc')), x0 REAL NOT NULL, y0 REAL NOT NULL, x1 REAL NOT NULL, y1 REAL NOT NULL, bulge REAL NOT NULL);
CREATE TABLE form_member (form_id TEXT NOT NULL REFERENCES form(id), ordinal INTEGER NOT NULL,
    performer_id TEXT NOT NULL REFERENCES performer(id), PRIMARY KEY (form_id, ordinal));
ALTER TABLE show ADD COLUMN counting_mode TEXT NOT NULL DEFAULT 'measures' CHECK (counting_mode IN ('measures','counts'));
ALTER TABLE arrival_set ADD COLUMN rehearsal_mark TEXT NOT NULL DEFAULT '';
ALTER TABLE arrival_set ADD COLUMN measure_marker TEXT NOT NULL DEFAULT '';
ALTER TABLE performer ADD COLUMN symbol TEXT;
CREATE TABLE field_note (id TEXT PRIMARY KEY, set_id TEXT NOT NULL REFERENCES arrival_set(id), ordinal INTEGER NOT NULL,
    x REAL NOT NULL, y REAL NOT NULL, text TEXT NOT NULL);
PRAGMA user_version = 4;
";

/// Schema 5 adds per-performer paths (`docs/FIELDWORK_SCHEMA5.md` §2).
const PATHS_SCHEMA: &str = "
CREATE TABLE path_segment (transition_id TEXT NOT NULL REFERENCES transition(id), performer_id TEXT NOT NULL REFERENCES performer(id),
    ordinal INTEGER NOT NULL, kind TEXT NOT NULL CHECK (kind IN ('hold','line','arc')),
    counts_num INTEGER NOT NULL, counts_den INTEGER NOT NULL, x1 REAL, y1 REAL, sweep REAL,
    PRIMARY KEY (transition_id, performer_id, ordinal));
PRAGMA user_version = 5;
";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Member {
    pub bytes: u64,
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionEntry {
    pub namespace: String,
    pub required: bool,
}

/// `manifest.json`. Field order is alphabetical so serde writes sorted keys like Fieldwork.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub capabilities: Vec<String>,
    pub extensions: Vec<ExtensionEntry>,
    pub format: String,
    #[serde(rename = "formatVersion")]
    pub format_version: Version,
    pub members: Vec<Member>,
    #[serde(rename = "minReaderVersion")]
    pub min_reader_version: Version,
    #[serde(rename = "requiredCapabilities")]
    pub required_capabilities: Vec<String>,
    #[serde(rename = "showID")]
    pub show_id: String,
    pub writer: String,
}

fn sha_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn db_err(e: rusqlite::Error) -> ExportError {
    ExportError::Database(e.to_string())
}

/// Every diagnostic a `.fieldwork` export of `p` produces.
pub fn plan(p: &DrillProject) -> Vec<Diagnostic> {
    let mut d = p.diagnostics.clone();
    let renamed = unique_labels(p).iter().zip(marchers(p)).filter(|(a, q)| **a != q.label).count();
    if renamed > 0 {
        d.push(Diagnostic::new(Severity::Warning, "labels_made_unique", format!("{renamed} duplicate performer label(s) were suffixed to make them unique")));
    }
    // A fitting failure is reported by `build_database`.
    if let Ok(fit) = fit_paths(p) {
        if !fit.paths.is_empty() {
            let moves = fit.paths.iter().map(|m| m.set).collect::<BTreeSet<_>>().len();
            let rows: usize = fit.paths.iter().map(|m| m.segments.len()).sum();
            d.push(Diagnostic::new(
                Severity::Info,
                "curves_as_paths",
                format!(
                    "{} performer move(s) in {moves} move(s) follow curved or uneven routes; they are written as {rows} path segment(s) (schema 5, {PATHS_CAPABILITY}) that match every count within 1 mm",
                    fit.paths.len()
                ),
            ));
        }
        if fit.missing > 0 {
            d.push(Diagnostic::new(
                Severity::Warning,
                "curves_flattened",
                format!("{} performer move(s) inside curved moves have counts without a stored position; they are written as straight moves", fit.missing),
            ));
        }
    }
    let lines = p.annotations.iter().filter(|a| matches!(a.kind, AnnotationKind::Polyline { .. })).count();
    if lines > 0 {
        d.push(Diagnostic::new(Severity::Info, "line_annotations_not_exported", format!("{lines} drawn line annotation(s) have no Fieldwork equivalent")));
    }
    let moved = p.annotations.iter().filter(|a| matches!(a.kind, AnnotationKind::Text { .. }) && a.set_index.is_none()).count();
    if moved > 0 {
        d.push(Diagnostic::new(Severity::Info, "notes_moved_to_next_set", format!("{moved} text note(s) between sets were attached to the next set")));
    }
    if p.metadata.music.is_some() {
        d.push(Diagnostic::new(
            Severity::Info,
            "audio_reference_only",
            "the audio file name is recorded in the provenance extension; no audio is embedded",
        ));
    }
    let props = p.performers.len() - marchers(p).len();
    if props > 0 {
        d.push(Diagnostic::new(
            Severity::Info,
            "props_in_extension",
            format!("{props} prop(s) are not performers; they are kept in the {} extension until Fieldwork supports props", props::NAMESPACE),
        ));
    }
    d
}

/// Performers written to the roster: everyone except props (see [`props`]).
fn marchers(p: &DrillProject) -> Vec<&Performer> {
    p.performers.iter().filter(|q| q.kind != PerformerKind::Prop).collect()
}

/// Roster labels made unique by suffixing repeats: "T1", "T1 (2)", …
fn unique_labels(p: &DrillProject) -> Vec<String> {
    let roster = marchers(p);
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut taken: std::collections::HashSet<String> = roster.iter().map(|q| q.label.clone()).collect();
    let mut first = std::collections::HashSet::new();
    roster
        .into_iter()
        .map(|q| {
            if first.insert(q.label.clone()) {
                return q.label.clone();
            }
            let n = seen.entry(q.label.clone()).or_insert(1);
            loop {
                *n += 1;
                let candidate = format!("{} ({})", q.label, n);
                if taken.insert(candidate.clone()) {
                    return candidate;
                }
            }
        })
        .collect()
}

fn label_prefix(label: &str) -> String {
    let prefix: String = label.chars().take_while(|c| !c.is_ascii_digit() && !c.is_whitespace()).collect();
    if prefix.is_empty() {
        "?".into()
    } else {
        prefix
    }
}

/// One performer's path through the move into a set.
struct MovePath {
    set: usize,
    /// Ordinal in the roster (see [`marchers`]).
    performer: usize,
    segments: Vec<paths::Segment>,
}

struct FittedPaths {
    /// Moves that are not straight at even steps, in arrival-set then roster order.
    paths: Vec<MovePath>,
    /// Performer moves inside non-straight moves that cannot be fitted because a count has
    /// no stored position.
    missing: usize,
}

/// Fits a path to every performer's move from the per-count positions (schema 5 §4).
/// Performers placed at both ends who move straight at even steps get no path.
fn fit_paths(p: &DrillProject) -> Result<FittedPaths, ExportError> {
    let roster = marchers(p);
    let mut fit = FittedPaths { paths: Vec::new(), missing: 0 };
    for w in p.sets.windows(2) {
        let (a, b) = (w[0].start_count as usize, w[1].start_count as usize);
        for (ordinal, q) in roster.iter().enumerate() {
            let (Some(_), Some(_)) = (p.meters(q.index, a), p.meters(q.index, b)) else { continue };
            let Some(samples) = (a..=b).map(|c| p.meters(q.index, c)).collect::<Option<Vec<_>>>() else {
                fit.missing += !w[1].straight_transition as usize;
                continue;
            };
            match paths::path_for_move(&samples) {
                Ok(Some(segments)) => fit.paths.push(MovePath { set: w[1].index, performer: ordinal, segments }),
                Ok(None) => {}
                Err(worst) => {
                    return Err(ExportError::Validation(format!(
                        "no path reproduces roster performer #{} in the move into set #{} within 1 mm (worst count off by {:.3} mm)",
                        ordinal + 1,
                        w[1].index,
                        worst * 1000.0
                    )))
                }
            }
        }
    }
    Ok(fit)
}

/// The cursor (event ordinal, fraction) of the instant count `c` is reached.
/// Event `k` spans counts k → k+1; a boundary is written as the next event at fraction 0.
fn cursor(c: u32, events: u32) -> (u32, i64, i64) {
    if c < events {
        (c, 0, 1)
    } else {
        (events - 1, 1, 1)
    }
}

pub fn build_database(p: &DrillProject) -> Result<Vec<u8>, ExportError> {
    database(p).map(|(bytes, _)| bytes)
}

/// The serialized database, and whether it holds paths (schema 5).
fn database(p: &DrillProject) -> Result<(Vec<u8>, bool), ExportError> {
    if p.sets.is_empty() {
        return Err(ExportError::Unsupported("the drill has no sets; Fieldwork arrival sets cannot be built".into()));
    }
    let end = p.timeline.drill_end_count;
    if end == 0 {
        return Err(ExportError::Unsupported("the drill has no counts after the opening set".into()));
    }
    let key = &p.metadata.source.sha256;
    let uid = |kind: &str, k: &str| stable_uuid(&format!("fieldwork/{kind}"), &format!("{key}/{k}"));
    let show_id = uid("show", "");
    let t = p.coordinates.transform;
    let meters = |x: f64, y: f64| t.to_meters_centered(x, y);

    let mut conn = Connection::open_in_memory().map_err(db_err)?;
    conn.execute_batch("PRAGMA foreign_keys = ON;").map_err(db_err)?;
    conn.execute_batch(SCHEMA).map_err(db_err)?;
    let fit = fit_paths(p)?;
    if !fit.paths.is_empty() {
        conn.execute_batch(PATHS_SCHEMA).map_err(db_err)?;
    }
    let tx = conn.transaction().map_err(db_err)?;

    tx.execute("INSERT INTO meta (key, value) VALUES ('writer', ?1), ('units', 'meters')", params![WRITER]).map_err(db_err)?;
    tx.execute(
        "INSERT INTO show (id, title, max_stride, min_clearance, counting_mode) VALUES (?1, ?2, 0.762, 0.5, 'counts')",
        params![show_id, p.metadata.title],
    )
    .map_err(db_err)?;

    // Field: the source field exactly, not a resized template.
    let f = &p.field;
    let (x0, _) = meters(f.extent_x_min, f.front_sideline_y);
    let (x1, _) = meters(f.extent_x_max, f.front_sideline_y);
    let (_, y_front) = meters(0.0, f.front_sideline_y);
    let (_, y_back) = meters(0.0, f.back_sideline_y);
    let terse = |s: &str, d: &str| match s.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["Side", n] => format!("S{n}"),
        _ => d.to_string(),
    };
    let side_labels = serde_json::to_string(&[terse(&f.side_labels[0], "S1"), terse(&f.side_labels[1], "S2")]).unwrap();
    tx.execute(
        "INSERT INTO field (singleton, name, preset, length, width, step_size, side_labels, front_label, back_label) VALUES (1, ?1, 'custom', ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            "Football field (from .3du)",
            x1 - x0,
            y_back - y_front,
            t.step_meters(),
            side_labels,
            f.front_label.to_lowercase(),
            f.back_label.to_lowercase()
        ],
    )
    .map_err(db_err)?;
    {
        let mut st = tx.prepare("INSERT INTO field_checkpoint (ordinal, axis, position, label, prominence) VALUES (?1, ?2, ?3, ?4, ?5)").map_err(db_err)?;
        let mut ordinal = 0;
        let mut put = |axis: &str, pos: f64, label: String, prominence: &str| -> Result<(), ExportError> {
            st.execute(params![ordinal, axis, pos, label, prominence]).map_err(db_err)?;
            ordinal += 1;
            Ok(())
        };
        for l in f.yard_lines.iter().filter(|l| l.major) {
            let x = meters(l.position, 0.0).0;
            let (label, prominence) = match l.yard_number {
                Some(n) if n == 0.0 => ("G".to_string(), "major"),
                Some(n) => (format!("{}", n.round() as i64), if (n / 10.0).fract() == 0.0 { "major" } else { "minor" }),
                None => (format!("{x:.2} m"), "minor"),
            };
            put("x", x, label, prominence)?;
        }
        if f.extent_x_min < f.playing_x_min {
            put("x", x0, format!("{} end line", terse(&f.side_labels[0], "S1")), "major")?;
        }
        if f.extent_x_max > f.playing_x_max {
            put("x", x1, format!("{} end line", terse(&f.side_labels[1], "S2")), "major")?;
        }
        put("y", y_front, "front sideline".into(), "major")?;
        let mut hashes: Vec<f64> = f.hashes.iter().map(|&h| meters(0.0, h).1).collect();
        hashes.sort_by(f64::total_cmp);
        let n = hashes.len();
        for (i, h) in hashes.into_iter().enumerate() {
            let label = match (n, i) {
                (2, 0) => "front hash".to_string(),
                (2, 1) => "back hash".to_string(),
                _ => format!("hash {}", i + 1),
            };
            put("y", h, label, "hash")?;
        }
        put("y", y_back, "back sideline".into(), "major")?;
    }

    // Roster: one section per label prefix. Props are handled by `props`, not here.
    let roster = marchers(p);
    let labels = unique_labels(p);
    let mut sections: Vec<(String, String)> = Vec::new();
    let section_of: Vec<usize> = roster
        .iter()
        .map(|q| {
            let (name, prefix) = match &q.section {
                Some(s) => (s.clone(), label_prefix(&q.label)),
                None => (label_prefix(&q.label), label_prefix(&q.label)),
            };
            match sections.iter().position(|s| s.0 == name) {
                Some(i) => i,
                None => {
                    sections.push((name, prefix));
                    sections.len() - 1
                }
            }
        })
        .collect();
    {
        let mut st = tx.prepare("INSERT INTO section (id, ordinal, name, prefix, symbol, color_index) VALUES (?1, ?2, ?3, ?4, ?5, ?6)").map_err(db_err)?;
        for (i, (name, prefix)) in sections.iter().enumerate() {
            st.execute(params![uid("section", name), i as i64, name, prefix, SYMBOLS[i % SYMBOLS.len()], (i % 10) as i64]).map_err(db_err)?;
        }
        let mut st = tx
            .prepare("INSERT INTO performer (id, ordinal, label, section_id, name, notes, symbol) VALUES (?1, ?2, ?3, ?4, NULL, '', NULL)")
            .map_err(db_err)?;
        for (ordinal, (q, label)) in roster.iter().zip(&labels).enumerate() {
            st.execute(params![uid("performer", &q.source_id), ordinal as i64, label, uid("section", &sections[section_of[ordinal]].0)]).map_err(db_err)?;
        }
    }

    // Timing: one movement, one beat event per count, durations taken from the source clock.
    let movement = uid("movement", "1");
    tx.execute("INSERT INTO movement (id, ordinal, name, audio_asset_id, fit_mode) VALUES (?1, 0, 'Movement 1', NULL, 'preserveAudioPins')", params![movement])
        .map_err(db_err)?;
    // Measure boundaries: count 1 plus every known measure start.
    let mut bounds: BTreeMap<u32, Option<u32>> = BTreeMap::new();
    bounds.insert(1, None);
    for m in p.measures.iter().filter(|m| m.start_count >= 1 && m.start_count <= end) {
        bounds.insert(m.start_count, Some(m.number));
    }
    let starts: Vec<(u32, Option<u32>)> = bounds.into_iter().collect();
    let mut event_ids = Vec::with_capacity(end as usize);
    {
        let mut ms = tx
            .prepare(
                "INSERT INTO measure (id, movement_id, ordinal, number, meter_numerator, meter_denominator, grouping, tempo_unit_num, tempo_unit_den, pulse_unit_num, pulse_unit_den, pickup_num, pickup_den)
                 VALUES (?1, ?2, ?3, ?4, ?5, 4, NULL, 1, 4, 1, 4, NULL, NULL)",
            )
            .map_err(db_err)?;
        let mut es = tx
            .prepare(
                "INSERT INTO timing_event (id, movement_id, ordinal, kind, measure_id, beat_index, notated_num, notated_den, baseline_seconds)
                 VALUES (?1, ?2, ?3, 'beat', ?4, ?5, 1, 4, ?6)",
            )
            .map_err(db_err)?;
        let mut number = 0u32;
        for (i, &(start, num)) in starts.iter().enumerate() {
            let stop = starts.get(i + 1).map(|s| s.0).unwrap_or(end + 1);
            number = num.unwrap_or(number + 1);
            let measure_id = uid("measure", &i.to_string());
            ms.execute(params![measure_id, movement, i as i64, number, (stop - start) as i64]).map_err(db_err)?;
            for (beat, count) in (start..stop).enumerate() {
                let seconds = p.timeline.duration_of(count as usize);
                if !(seconds.is_finite() && seconds > 0.0) {
                    return Err(ExportError::Unsupported(format!("count {count} has a non-positive duration")));
                }
                let id = uid("event", &count.to_string());
                es.execute(params![id, movement, (count - 1) as i64, measure_id, beat as i64, seconds]).map_err(db_err)?;
                event_ids.push(id);
            }
        }
    }

    // Sets, transitions, placements.
    {
        let mut ss = tx
            .prepare(
                "INSERT INTO arrival_set (id, movement_id, ordinal, label, event_id, fraction_num, fraction_den, is_subset, notes, rehearsal_mark, measure_marker)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '', ?9, ?10)",
            )
            .map_err(db_err)?;
        let mut ts = tx.prepare("INSERT INTO transition (id, to_set_id, model) VALUES (?1, ?2, 'straight')").map_err(db_err)?;
        let mut ps = tx
            .prepare("INSERT INTO placement (set_id, performer_id, state, x, y, z, facing) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
            .map_err(db_err)?;
        for s in &p.sets {
            let set_id = uid("set", &s.index.to_string());
            let (event, num, den) = cursor(s.start_count, end);
            ss.execute(params![
                set_id,
                movement,
                s.index as i64,
                s.name,
                event_ids[event as usize],
                num,
                den,
                s.is_subset as i64,
                s.rehearsal_mark.clone().unwrap_or_default(),
                s.measures_text.trim()
            ])
            .map_err(db_err)?;
            if s.index > 0 {
                ts.execute(params![uid("transition", &s.index.to_string()), set_id]).map_err(db_err)?;
            }
            for q in &roster {
                let pid = uid("performer", &q.source_id);
                match p.meters(q.index, s.start_count as usize) {
                    Some((x, y)) => ps.execute(params![set_id, pid, "placed", x, y, 0.0, 0.0]),
                    None => ps.execute(params![set_id, pid, "unresolved", None::<f64>, None::<f64>, None::<f64>, None::<f64>]),
                }
                .map_err(db_err)?;
            }
        }
    }

    // Paths, by arrival set, then roster order, then segment order. Each ends on the
    // placement's exact doubles, which `fit_paths` sampled with the same `p.meters`.
    if !fit.paths.is_empty() {
        let mut st = tx.prepare("INSERT INTO path_segment (transition_id, performer_id, ordinal, kind, counts_num, counts_den, x1, y1, sweep) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)").map_err(db_err)?;
        for m in &fit.paths {
            let (transition, performer) = (uid("transition", &m.set.to_string()), uid("performer", &roster[m.performer].source_id));
            for (ordinal, seg) in m.segments.iter().enumerate() {
                let (x1, y1) = seg.end().unzip();
                let counts = seg.counts();
                st.execute(params![transition, performer, ordinal as i64, seg.kind(), counts.num, counts.den, x1, y1, seg.sweep()]).map_err(db_err)?;
            }
        }
    }

    // Text annotations become field notes on their set (or the next set).
    {
        let mut st = tx.prepare("INSERT INTO field_note (id, set_id, ordinal, x, y, text) VALUES (?1, ?2, ?3, ?4, ?5, ?6)").map_err(db_err)?;
        let mut ordinal = 0i64;
        for (i, a) in p.annotations.iter().enumerate() {
            let AnnotationKind::Text { rect, text } = &a.kind else { continue };
            let set = a.set_index.or_else(|| p.sets.iter().find(|s| s.start_count >= a.count).map(|s| s.index));
            let Some(set) = set else { continue };
            let left = rect[0].min(rect[2]) as f64;
            let top = if t.y_sign > 0.0 { rect[1].max(rect[3]) } else { rect[1].min(rect[3]) } as f64;
            let (x, y) = meters(left, top);
            let text: String = text.trim_end().chars().take(2000).collect();
            if text.is_empty() {
                continue;
            }
            st.execute(params![uid("note", &i.to_string()), uid("set", &set.to_string()), ordinal, x, y, text]).map_err(db_err)?;
            ordinal += 1;
        }
    }
    tx.commit().map_err(db_err)?;

    let fk: i64 = conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).map_err(db_err)?;
    if fk != 0 {
        return Err(ExportError::Validation(format!("{fk} foreign key violation(s)")));
    }
    let bytes = conn.serialize(DatabaseName::Main).map_err(db_err)?;
    Ok((bytes.to_vec(), !fit.paths.is_empty()))
}

fn provenance_json(p: &DrillProject, diags: &[Diagnostic]) -> Vec<u8> {
    let v = serde_json::json!({
        "converter": WRITER,
        "source": {
            "format": p.metadata.source.format,
            "formatRevision": p.metadata.source.format_revision,
            "writerVersion": p.metadata.source.writer_version,
            "sha256": p.metadata.source.sha256,
        },
        "audioReference": p.metadata.music.as_ref().map(|m| &m.filename),
        "coordinateNotes": p.coordinates.notes,
        "sourceStepInches": p.coordinates.transform.step_inches,
        "diagnostics": diags,
    });
    serde_json::to_vec_pretty(&v).unwrap()
}

/// Builds the complete `.fieldwork` archive in memory.
pub fn build_archive(p: &DrillProject, diags: &[Diagnostic]) -> Result<Vec<u8>, ExportError> {
    let (db, has_paths) = database(p)?;
    let ext_path = format!("extensions/{EXTENSION_NAMESPACE}/provenance.json");
    let ext = provenance_json(p, diags);
    // Extension members are written sorted by path, after the database.
    let mut extensions = vec![(EXTENSION_NAMESPACE, ext_path, ext)];
    if let Some((path, bytes)) = props::extension_member(p) {
        extensions.push((props::NAMESPACE, path, bytes));
    }
    extensions.sort_by(|a, b| a.1.cmp(&b.1));
    let mut members = vec![Member { bytes: db.len() as u64, path: "show.sqlite".into(), sha256: sha_hex(&db) }];
    members.extend(extensions.iter().map(|(_, path, bytes)| Member { bytes: bytes.len() as u64, path: path.clone(), sha256: sha_hex(bytes) }));
    let mut capabilities = vec!["drill.core.v1".to_string(), "timing.anchors.v1".into()];
    let mut required_capabilities = vec!["drill.core.v1".to_string()];
    if has_paths {
        capabilities.push(PATHS_CAPABILITY.into());
        required_capabilities.push(PATHS_CAPABILITY.into());
    }
    let manifest = Manifest {
        capabilities,
        extensions: extensions.iter().map(|(ns, _, _)| ExtensionEntry { namespace: ns.to_string(), required: false }).collect(),
        format: FORMAT.into(),
        format_version: Version { major: 1, minor: 0 },
        members,
        min_reader_version: Version { major: 1, minor: 0 },
        required_capabilities,
        show_id: stable_uuid("fieldwork/show", &format!("{}/", p.metadata.source.sha256)),
        writer: WRITER.into(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let mut files = vec![("manifest.json".to_string(), manifest_bytes), ("show.sqlite".to_string(), db)];
    files.extend(extensions.into_iter().map(|(_, path, bytes)| (path, bytes)));
    zip::write(&files).map_err(ExportError::Validation)
}

/// Opens serialized database bytes read-only, entirely in memory.
fn open_serialized(bytes: &[u8]) -> Result<Connection, ExportError> {
    let mut conn = Connection::open_in_memory().map_err(db_err)?;
    if bytes.is_empty() {
        return Err(ExportError::Validation("show.sqlite is empty".into()));
    }
    // SAFETY: sqlite3_malloc64 returns a buffer of the requested size (checked for null);
    // ownership passes to SQLite, which frees it when the connection closes.
    let data = unsafe {
        let ptr = rusqlite::ffi::sqlite3_malloc64(bytes.len() as u64) as *mut u8;
        let ptr = std::ptr::NonNull::new(ptr).ok_or_else(|| ExportError::Database("out of memory".into()))?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.as_ptr(), bytes.len());
        rusqlite::serialize::OwnedData::from_raw_nonnull(ptr, bytes.len())
    };
    conn.deserialize(DatabaseName::Main, data, true).map_err(db_err)?;
    Ok(conn)
}

/// Summary of a validated `.fieldwork` file.
#[derive(Debug, Clone, Serialize)]
pub struct FieldworkReport {
    pub format_version: String,
    pub schema_version: i64,
    pub performers: i64,
    pub sets: i64,
    pub events: i64,
    pub placements: i64,
    pub field_notes: i64,
    /// Performer moves that follow a path, and their segment rows (schema 5).
    pub paths: i64,
    pub path_segments: i64,
    pub duration_seconds: f64,
    pub members: usize,
}

/// Validates an archive the way Fieldwork's reader does (§2, §3, §7 of the format document).
pub fn validate_bytes(bytes: &[u8]) -> Result<FieldworkReport, ExportError> {
    let v = |m: String| ExportError::Validation(m);
    let members = zip::read(bytes).map_err(v)?;
    if members.first().map(|m| m.0.as_str()) != Some("manifest.json") {
        return Err(v("manifest.json is not the first member".into()));
    }
    let manifest: Manifest = serde_json::from_slice(&members[0].1).map_err(|e| v(format!("manifest: {e}")))?;
    if manifest.format != FORMAT {
        return Err(v("not a Fieldwork show".into()));
    }
    if manifest.format_version.major > 1 || manifest.min_reader_version.major > 1 {
        return Err(v("format version is newer than 1.x".into()));
    }
    for m in &manifest.members {
        let data = members.iter().find(|(p, _)| *p == m.path).map(|(_, d)| d).ok_or_else(|| v(format!("{} is missing", m.path)))?;
        if data.len() as u64 != m.bytes || sha_hex(data) != m.sha256 {
            return Err(v(format!("{} does not match the manifest", m.path)));
        }
    }
    for (path, _) in members.iter().skip(1) {
        if !manifest.members.iter().any(|m| &m.path == path) {
            return Err(v(format!("{path} is not listed in the manifest")));
        }
    }
    let db = &members.iter().find(|(p, _)| p == "show.sqlite").ok_or_else(|| v("show.sqlite is missing".into()))?.1;
    let conn = open_serialized(db)?;
    let one = |sql: &str| -> Result<i64, ExportError> { conn.query_row(sql, [], |r| r.get(0)).map_err(db_err) };
    let schema_version = one("PRAGMA user_version")?;
    if schema_version != SCHEMA_VERSION as i64 && schema_version != PATHS_SCHEMA_VERSION as i64 {
        return Err(v(format!("schema {schema_version}, expected {SCHEMA_VERSION} or {PATHS_SCHEMA_VERSION}")));
    }
    if one("SELECT count(*) FROM pragma_foreign_key_check")? != 0 {
        return Err(v("foreign key violations".into()));
    }
    if one("SELECT count(*) FROM show")? != 1 || one("SELECT count(*) FROM field")? != 1 {
        return Err(v("show and field must have exactly one row".into()));
    }
    let show_id: String = conn.query_row("SELECT id FROM show", [], |r| r.get(0)).map_err(db_err)?;
    if show_id != manifest.show_id {
        return Err(v("manifest showID differs from show.id".into()));
    }
    if one("SELECT count(*) FROM field WHERE length < 2 OR length > 1000 OR width < 2 OR width > 1000 OR step_size <= 0.05 OR step_size >= 5")? != 0 {
        return Err(v("field dimensions out of range".into()));
    }
    if one("SELECT count(*) FROM performer")? != one("SELECT count(DISTINCT label) FROM performer")? {
        return Err(v("performer labels are not unique".into()));
    }
    if one("SELECT count(*) FROM timing_event WHERE NOT (baseline_seconds > 0) OR (kind = 'beat' AND notated_num <= 0)")? != 0 {
        return Err(v("invalid timing event".into()));
    }
    if one("SELECT count(*) FROM measure WHERE meter_numerator <= 0 OR meter_denominator <= 0 OR (meter_denominator & (meter_denominator - 1)) != 0")? != 0 {
        return Err(v("invalid meter".into()));
    }
    if one("SELECT count(*) FROM arrival_set WHERE fraction_den <= 0 OR fraction_num < 0 OR fraction_num > fraction_den")? != 0 {
        return Err(v("set cursor out of range".into()));
    }
    // Sets arrive at strictly increasing positions (event ordinal + fraction).
    let order: i64 = one(
        "SELECT count(*) FROM (SELECT e.ordinal + CAST(s.fraction_num AS REAL) / s.fraction_den AS t,
                LAG(e.ordinal + CAST(s.fraction_num AS REAL) / s.fraction_den) OVER (ORDER BY s.ordinal) AS prev
           FROM arrival_set s JOIN timing_event e ON e.id = s.event_id) WHERE prev IS NOT NULL AND t <= prev",
    )?;
    if order != 0 {
        return Err(v("sets do not arrive at strictly increasing counts".into()));
    }
    let sets = one("SELECT count(*) FROM arrival_set")?;
    if one("SELECT count(*) FROM transition")? != sets - 1 {
        return Err(v("every set after the preset needs exactly one transition".into()));
    }
    if one("SELECT count(*) FROM transition t JOIN arrival_set s ON s.id = t.to_set_id WHERE s.ordinal = 0")? != 0 {
        return Err(v("the preset has a transition".into()));
    }
    if one("SELECT count(*) FROM placement WHERE state = 'placed' AND (x IS NULL OR y IS NULL OR x != x OR y != y)")? != 0 {
        return Err(v("placed positions must have finite coordinates".into()));
    }
    if one("SELECT count(*) FROM field_note WHERE length(text) > 2000")? != 0 {
        return Err(v("field note longer than 2000 characters".into()));
    }
    let (paths, path_segments) = if schema_version >= PATHS_SCHEMA_VERSION as i64 { check_paths(&conn)? } else { (0, 0) };
    let declared = |list: &[String]| list.iter().any(|c| c == PATHS_CAPABILITY);
    if path_segments > 0 && !(declared(&manifest.capabilities) && declared(&manifest.required_capabilities)) {
        return Err(v(format!("paths exist but {PATHS_CAPABILITY} is not a required capability")));
    }
    let duration_seconds: f64 = conn.query_row("SELECT total(baseline_seconds) FROM timing_event", [], |r| r.get(0)).map_err(db_err)?;
    Ok(FieldworkReport {
        format_version: format!("{}.{}", manifest.format_version.major, manifest.format_version.minor),
        schema_version,
        performers: one("SELECT count(*) FROM performer")?,
        sets,
        events: one("SELECT count(*) FROM timing_event")?,
        placements: one("SELECT count(*) FROM placement")?,
        field_notes: one("SELECT count(*) FROM field_note")?,
        paths,
        path_segments,
        duration_seconds,
        members: members.len(),
    })
}

/// Rows of a query as dynamically typed values.
fn query_values(conn: &Connection, sql: &str) -> Result<Vec<Vec<Value>>, ExportError> {
    let mut st = conn.prepare(sql).map_err(db_err)?;
    let n = st.column_count();
    let rows = st.query_map([], |r| (0..n).map(|i| r.get(i)).collect()).map_err(db_err)?;
    rows.collect::<Result<_, _>>().map_err(db_err)
}

fn text(x: &Value) -> String {
    if let Value::Text(t) = x {
        t.clone()
    } else {
        String::new()
    }
}

/// An INTEGER column; REAL and other types are `None`, as Fieldwork requires.
fn integer(x: &Value) -> Option<i64> {
    if let Value::Integer(i) = x {
        Some(*i)
    } else {
        None
    }
}

fn finite(x: &Value) -> Option<f64> {
    match x {
        Value::Real(f) if f.is_finite() => Some(*f),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

/// Each set's position in count pulses, keyed by set id. `None` for timelines with
/// groupings or pickups, whose counts fall unevenly; this converter never writes them.
fn set_pulses(conn: &Connection) -> Result<Option<HashMap<String, Ratio>>, ExportError> {
    let mut unit: HashMap<String, Ratio> = HashMap::new();
    for r in query_values(conn, "SELECT id, pulse_unit_num, pulse_unit_den, grouping, pickup_num FROM measure")? {
        match (integer(&r[1]), integer(&r[2])) {
            (Some(n), Some(d)) if n > 0 && d > 0 && r[3] == Value::Null && r[4] == Value::Null => unit.insert(text(&r[0]), Ratio::new(n, d)),
            _ => return Ok(None),
        };
    }
    let mut events: HashMap<String, (Ratio, Ratio)> = HashMap::new();
    let mut next: HashMap<String, Ratio> = HashMap::new();
    for r in query_values(conn, "SELECT id, movement_id, kind, measure_id, notated_num, notated_den FROM timing_event ORDER BY movement_id, ordinal")? {
        let at = next.get(&text(&r[1])).copied().unwrap_or(Ratio::ZERO);
        let length = match (text(&r[2]).as_str(), unit.get(&text(&r[3])), integer(&r[4]), integer(&r[5])) {
            ("hold", ..) => Ratio::ZERO,
            (_, Some(u), Some(n), Some(d)) if d > 0 => Ratio::new(n * u.den, d * u.num),
            _ => return Ok(None),
        };
        events.insert(text(&r[0]), (at, length));
        next.insert(text(&r[1]), at + length);
    }
    let mut out = HashMap::new();
    for r in query_values(conn, "SELECT id, event_id, fraction_num, fraction_den FROM arrival_set")? {
        let (Some(&(at, length)), Some(n), Some(d)) = (events.get(&text(&r[1])), integer(&r[2]), integer(&r[3])) else { return Ok(None) };
        out.insert(text(&r[0]), at + Ratio::new(length.num * n, length.den * d));
    }
    Ok(Some(out))
}

/// Checks every path the way Fieldwork's reader does (schema 5 §7). Returns the number of
/// paths and of segment rows.
fn check_paths(conn: &Connection) -> Result<(i64, i64), ExportError> {
    let pulses = set_pulses(conn)?;
    // Each transition's move starts at the previous set in the same movement.
    let mut from: HashMap<String, String> = HashMap::new();
    let mut previous: HashMap<String, String> = HashMap::new();
    for r in query_values(conn, "SELECT id, movement_id FROM arrival_set ORDER BY movement_id, ordinal")? {
        if let Some(a) = previous.insert(text(&r[1]), text(&r[0])) {
            from.insert(text(&r[0]), a);
        }
    }
    let to_set: HashMap<String, String> = query_values(conn, "SELECT id, to_set_id FROM transition")?.iter().map(|r| (text(&r[0]), text(&r[1]))).collect();
    let placed: HashMap<(String, String), Point> = query_values(conn, "SELECT set_id, performer_id, x, y FROM placement WHERE state = 'placed'")?
        .iter()
        .filter_map(|r| Some(((text(&r[0]), text(&r[1])), (finite(&r[2])?, finite(&r[3])?))))
        .collect();

    let rows = query_values(conn, "SELECT transition_id, performer_id, ordinal, kind, counts_num, counts_den, x1, y1, sweep FROM path_segment ORDER BY transition_id, performer_id, ordinal")?;
    let mut groups: BTreeMap<(String, String), Vec<&Vec<Value>>> = BTreeMap::new();
    for r in &rows {
        groups.entry((text(&r[0]), text(&r[1]))).or_default().push(r);
    }
    for ((transition, performer), group) in &groups {
        let damaged = |why: &str| ExportError::Validation(format!("path in transition {transition}: {why}"));
        let b = to_set.get(transition).ok_or_else(|| damaged("no such transition"))?;
        let a = from.get(b).ok_or_else(|| damaged("the move has no previous set"))?;
        let (Some(&start), Some(&end)) = (placed.get(&(a.clone(), performer.clone())), placed.get(&(b.clone(), performer.clone()))) else {
            return Err(damaged("the performer is not placed at both ends"));
        };
        let mut segments = Vec::with_capacity(group.len());
        let mut point = start;
        for (k, r) in group.iter().enumerate() {
            if integer(&r[2]) != Some(k as i64) {
                return Err(damaged("ordinals are not 0…n−1"));
            }
            let counts = match (integer(&r[4]), integer(&r[5])) {
                (Some(n), Some(d)) if n > 0 && d > 0 => Ratio::new(n, d),
                _ => return Err(damaged("counts are not positive integers")),
            };
            let to = match (&r[6], &r[7]) {
                (Value::Null, Value::Null) => None,
                (x, y) => Some(finite(x).zip(finite(y)).ok_or_else(|| damaged("a point is not finite"))?),
            };
            let sweep = match &r[8] {
                Value::Null => None,
                w => Some(finite(w).filter(|w| *w != 0.0 && w.abs() < 360.0).ok_or_else(|| damaged("a sweep is out of range"))?),
            };
            let segment = match (text(&r[3]).as_str(), to, sweep) {
                ("hold", None, None) => Segment::Hold { counts },
                ("line", Some(to), None) => Segment::Line { counts, to },
                ("arc", Some(to), Some(sweep)) if (to.0 - point.0).hypot(to.1 - point.1) > 1e-6 => Segment::Arc { counts, to, sweep },
                _ => return Err(damaged("a segment's kind does not match its point and sweep, or an arc's chord is too short")),
            };
            point = segment.end().unwrap_or(point);
            segments.push(segment);
        }
        if let Some(pulses) = &pulses {
            let total = segments.iter().fold(Ratio::ZERO, |t, s| t + s.counts());
            if pulses.get(b).zip(pulses.get(a)).map(|(b, a)| *b - *a) != Some(total) {
                return Err(damaged("durations do not add up to the move's length"));
            }
        }
        let last = path_end(start, &segments);
        if (last.0 - end.0).hypot(last.1 - end.1) > 1e-6 {
            return Err(damaged("the path does not end at the arrival placement"));
        }
    }
    Ok((groups.len() as i64, rows.len() as i64))
}

/// Writes a `.fieldwork` file atomically after validating the encoded archive.
pub fn export_fieldwork(p: &DrillProject, target: &Path, policy: LossPolicy, overwrite: bool) -> Result<Vec<Diagnostic>, ExportError> {
    let diags = plan(p);
    enforce(policy, &diags)?;
    let bytes = build_archive(p, &diags)?;
    validate_bytes(&bytes)?;
    write_atomic(target, &bytes, overwrite, |tmp| validate_bytes(&std::fs::read(tmp)?).map(|_| ()))?;
    Ok(diags)
}
