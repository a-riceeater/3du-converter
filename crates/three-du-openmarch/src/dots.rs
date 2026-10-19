//! Native OpenMarch `.dots` (SQLite) writer and reader-side validation.
//!
//! Coordinates are stored in canvas pixels: `pixelsPerStep = stepSizeInches × 0.5`, x from
//! the canvas's left edge, y downward from the back edge, with the front sideline at
//! `y = height`. These follow OpenMarch's `FieldProperties` class.

use std::path::Path;

use rusqlite::{params, Connection, OpenFlags};
use serde_json::json;
use three_du_core::export::{commit, temp_path_for, ExportError};
use three_du_core::{Diagnostic, DrillProject, Severity};

use crate::dots_schema::{ddl, MIGRATIONS, USER_VERSION};
use crate::field::{layout, Checkpoint, FieldLayout};
use crate::shared::{beat_durations, page_coordinates, performer_section, unique_prefix_orders};

/// OpenMarch draws `PIXELS_PER_INCH` canvas pixels per real inch.
const PIXELS_PER_INCH: f64 = 0.5;
/// Fixed timestamp for created/updated columns so output is reproducible.
const STAMP: &str = "1970-01-01 00:00:00";

fn db_err(e: rusqlite::Error) -> ExportError {
    ExportError::Database(e.to_string())
}

/// Pixel geometry derived from the checkpoints, as `FieldProperties` computes it.
#[derive(Debug, Clone, Copy)]
pub struct Canvas {
    pub pixels_per_step: f64,
    pub center_x: f64,
    pub front_y: f64,
    pub width: f64,
    pub height: f64,
}

impl Canvas {
    pub fn new(lay: &FieldLayout) -> Canvas {
        let pps = lay.step_inches * PIXELS_PER_INCH;
        let (x_min, x_max) = lay.x_range();
        let (y_min, y_max) = lay.y_range();
        let width = (x_max - x_min) * pps;
        let height = (y_max - y_min) * pps;
        Canvas { pixels_per_step: pps, center_x: width / 2.0, front_y: height, width, height }
    }

    /// Steps (x from center, y from the front sideline toward the back) → canvas pixels.
    pub fn to_pixels(&self, xs: f64, ys: f64) -> (f64, f64) {
        (self.center_x + xs * self.pixels_per_step, self.front_y - ys * self.pixels_per_step)
    }

    pub fn to_steps(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.center_x) / self.pixels_per_step, (self.front_y - y) / self.pixels_per_step)
    }
}

fn checkpoint_json(c: &Checkpoint, id: usize, axis: &str) -> serde_json::Value {
    // OpenMarch stores y checkpoints as negative steps toward the back.
    let steps = if axis == "y" { -c.steps } else { c.steps };
    let mut v = json!({
        "id": id, "name": c.name, "axis": axis, "terseName": c.terse_name,
        "stepsFromCenterFront": steps, "useAsReference": c.use_as_reference, "visible": c.visible,
    });
    if let Some(l) = &c.field_label {
        v["fieldLabel"] = json!(l);
    }
    v
}

pub fn field_properties_json(p: &DrillProject, lay: &FieldLayout, canvas: &Canvas) -> serde_json::Value {
    let c = |r: u8, g: u8, b: u8, a: f64| json!({"r": r, "g": g, "b": b, "a": a});
    json!({
        "name": format!("{} field (from .3du)", if p.coordinates.source_unit == "yard" { "Football" } else { "Source" }),
        "xCheckpoints": lay.x.iter().enumerate().map(|(i, cp)| checkpoint_json(cp, i, "x")).collect::<Vec<_>>(),
        "yCheckpoints": lay.y.iter().enumerate().map(|(i, cp)| checkpoint_json(cp, i, "y")).collect::<Vec<_>>(),
        "sideDescriptions": {
            "verboseLeft": lay.side_labels[0], "verboseRight": lay.side_labels[1],
            "terseLeft": terse(&lay.side_labels[0], "S1"), "terseRight": terse(&lay.side_labels[1], "S2"),
        },
        "halfLineXInterval": lay.half_line_x,
        "halfLineYInterval": lay.half_line_y,
        "stepSizeInches": lay.step_inches,
        "measurementSystem": "imperial",
        "topLabelsVisible": true, "bottomLabelsVisible": true, "leftLabelsVisible": true, "rightLabelsVisible": true,
        "useHashes": lay.use_hashes,
        "isCustom": true,
        "showFieldImage": false,
        "imageFillOrFit": "fit",
        "backgroundImageOpacity": 1.0,
        "theme": {
            "primaryStroke": c(0, 0, 0, 1.0), "secondaryStroke": c(170, 170, 170, 1.0), "tertiaryStroke": c(221, 221, 221, 1.0),
            "background": c(255, 255, 255, 1.0), "fieldLabel": c(136, 136, 136, 1.0), "externalLabel": c(136, 136, 136, 1.0),
            "previousPath": c(0, 0, 0, 1.0), "nextPath": c(0, 175, 13, 1.0), "shape": c(126, 34, 206, 1.0),
            "tempPath": c(192, 132, 252, 1.0),
            "defaultMarcher": {"fill": c(255, 0, 0, 1.0), "outline": c(0, 0, 0, 0.0), "label": c(0, 0, 0, 1.0)},
        },
        // Derived values OpenMarch recomputes; written for readers that do not.
        "width": canvas.width, "height": canvas.height,
        "centerFrontPoint": {"xPixels": canvas.center_x, "yPixels": canvas.front_y},
    })
}

fn terse(label: &str, fallback: &str) -> String {
    match label.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["Side", n] => format!("S{n}"),
        _ => fallback.to_string(),
    }
}

/// OpenMarch names pages `offset, offset+1, …` with letters for subsets. Returns the offset
/// that reproduces the source names, and how many names still differ.
pub fn page_name_offset(p: &DrillProject) -> (i64, usize) {
    let offset = p.sets.first().and_then(|s| s.name.trim().parse::<i64>().ok()).unwrap_or(0);
    let generated = generate_page_names(&p.sets.iter().map(|s| s.is_subset).collect::<Vec<_>>(), offset);
    let differ = generated.iter().zip(&p.sets).filter(|(g, s)| **g != s.name.trim()).count();
    (offset, differ)
}

/// Reimplementation of OpenMarch's page naming (`generatePageNames`).
pub fn generate_page_names(is_subset: &[bool], offset: i64) -> Vec<String> {
    let mut names = Vec::with_capacity(is_subset.len());
    let mut number = offset;
    let mut letters = String::new();
    for (i, &sub) in is_subset.iter().enumerate() {
        if i == 0 {
            names.push(offset.to_string());
            continue;
        }
        if sub {
            letters = next_letters(&letters);
            names.push(format!("{number}{letters}"));
        } else {
            number += 1;
            letters.clear();
            names.push(number.to_string());
        }
    }
    names
}

fn next_letters(s: &str) -> String {
    if s.is_empty() {
        return "A".into();
    }
    let mut chars: Vec<u8> = s.to_ascii_uppercase().into_bytes();
    let mut i = chars.len();
    loop {
        if i == 0 {
            chars.insert(0, b'A');
            break;
        }
        i -= 1;
        if chars[i] == b'Z' {
            chars[i] = b'A';
        } else {
            chars[i] += 1;
            break;
        }
    }
    String::from_utf8(chars).unwrap()
}

/// Measure lines are emitted only when every measure number in the range has a line,
/// because OpenMarch numbers measures sequentially from `measurementOffset`.
fn contiguous_measures(p: &DrillProject) -> bool {
    !p.measures.is_empty() && p.measures.windows(2).all(|w| w[1].number == w[0].number + 1 && w[1].start_count > w[0].start_count)
}

/// Every diagnostic a `.dots` export of `p` produces, computed without writing anything.
pub fn plan(p: &DrillProject) -> Vec<Diagnostic> {
    let mut diags = p.diagnostics.clone();
    let (_, differ) = page_name_offset(p);
    if differ > 0 {
        diags.push(Diagnostic::new(
            Severity::Warning,
            "page_names_differ",
            format!("{differ} set name(s) cannot be reproduced by OpenMarch's automatic page numbering; source names are kept in page notes"),
        ));
    }
    if !contiguous_measures(p) && !p.measures.is_empty() {
        diags.push(Diagnostic::new(
            Severity::Warning,
            "measures_in_page_notes",
            "measure ranges do not divide into whole measures everywhere, so OpenMarch measure lines would be misnumbered; ranges and rehearsal marks are kept in page notes",
        ));
    }
    let labels: Vec<String> = p.performers.iter().map(|q| q.label.clone()).collect();
    unique_prefix_orders(&labels, &mut diags);
    page_coordinates(p, &mut diags);
    let nonlinear = p.sets.iter().filter(|s| !s.straight_transition).count();
    if nonlinear > 0 {
        diags.push(Diagnostic::new(
            Severity::Warning,
            "curves_flattened",
            format!("{nonlinear} move(s) contain curved or uneven paths; OpenMarch interpolates straight lines between pages"),
        ));
    }
    if !p.annotations.is_empty() {
        diags.push(Diagnostic::new(Severity::Info, "annotations_not_exported", format!("{} field annotation(s) are not exported to .dots", p.annotations.len())));
    }
    if p.metadata.music.is_some() {
        diags.push(Diagnostic::new(Severity::Info, "audio_not_attached", "the source names an audio file but contains no audio; attach the music in OpenMarch"));
    }
    diags
}

pub fn write(p: &DrillProject, target: &Path, overwrite: bool) -> Result<(), ExportError> {
    three_du_core::export::check_target(target, overwrite)?;
    if p.sets.is_empty() {
        return Err(ExportError::Unsupported("the drill has no sets; OpenMarch pages cannot be built".into()));
    }
    let temp = temp_path_for(target);
    let result = write_db(p, &temp).and_then(|_| validate(&temp).map(|_| ()));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    commit(&temp, target, overwrite)
}

fn write_db(p: &DrillProject, path: &Path) -> Result<(), ExportError> {
    // Diagnostics are reported by `plan`; the helpers below also emit them, so discard here.
    let diags = &mut Vec::new();
    let lay = layout(p);
    let canvas = Canvas::new(&lay);
    let mut conn = Connection::open(path).map_err(db_err)?;
    conn.execute_batch("PRAGMA journal_mode = DELETE; PRAGMA foreign_keys = ON;").map_err(db_err)?;
    conn.execute_batch(&ddl()).map_err(db_err)?;

    let tx = conn.transaction().map_err(db_err)?;
    {
        let mut st = tx.prepare("INSERT INTO __drizzle_migrations (hash, created_at) VALUES (?1, ?2)").map_err(db_err)?;
        for (hash, when) in MIGRATIONS {
            st.execute(params![hash, when]).map_err(db_err)?;
        }
    }
    tx.execute("INSERT INTO history_stats (id, cur_undo_group, cur_redo_group, group_limit) VALUES (1, 1, 1, 500)", []).map_err(db_err)?;

    let (offset, _) = page_name_offset(p);
    let emit_measures = contiguous_measures(p);
    let measurement_offset = p.measures.first().map(|m| m.number).unwrap_or(1);
    let settings = json!({
        "defaultBeatsPerMeasure": 4, "defaultTempo": 120, "defaultNewPageCounts": 16,
        "audioOffsetSeconds": 0, "pageNumberOffset": offset, "measurementOffset": measurement_offset,
        "projectName": p.metadata.title,
    });
    tx.execute("INSERT INTO workspace_settings (id, json_data, created_at, updated_at) VALUES (1, ?1, ?2, ?2)", params![settings.to_string(), STAMP])
        .map_err(db_err)?;
    tx.execute(
        "INSERT INTO field_properties (id, json_data) VALUES (1, ?1)",
        params![field_properties_json(p, &lay, &canvas).to_string()],
    )
    .map_err(db_err)?;
    let last_counts = p.sets.last().map(|s| s.counts.max(1)).unwrap_or(8);
    tx.execute(
        "INSERT INTO utility (id, last_page_counts, default_beat_duration, updated_at) VALUES (0, ?1, 0.5, ?2)",
        params![last_counts, STAMP],
    )
    .map_err(db_err)?;

    // Beats: id = position = count. Beat 0 is OpenMarch's zero-length first beat.
    let durations = beat_durations(p);
    {
        let mut st = tx
            .prepare("INSERT INTO beats (id, duration, position, include_in_measure, created_at, updated_at) VALUES (?1, ?2, ?1, 1, ?3, ?3)")
            .map_err(db_err)?;
        for (c, d) in durations.iter().enumerate() {
            st.execute(params![c as i64, d, STAMP]).map_err(db_err)?;
        }
    }

    // Pages: page 0 is the opening set on beat 0; page i starts on the beat after set i-1.
    {
        let mut st = tx
            .prepare("INSERT INTO pages (id, is_subset, notes, start_beat, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)")
            .map_err(db_err)?;
        let generated = generate_page_names(&p.sets.iter().map(|s| s.is_subset).collect::<Vec<_>>(), offset);
        for (i, s) in p.sets.iter().enumerate() {
            let start = if i == 0 { 0 } else { p.sets[i - 1].start_count as i64 + 1 };
            let mut notes = Vec::new();
            if generated[i] != s.name.trim() {
                notes.push(format!("Set {}", s.name));
            }
            if !s.measures_text.trim().is_empty() && !emit_measures {
                notes.push(format!("Measures {}", s.measures_text.trim()));
            }
            if let Some(m) = &s.rehearsal_mark {
                notes.push(format!("Rehearsal {m}"));
            }
            let notes = (!notes.is_empty()).then(|| notes.join(" · "));
            st.execute(params![i as i64, s.is_subset as i64, notes, start, STAMP]).map_err(db_err)?;
        }
    }

    if emit_measures {
        let mut st = tx
            .prepare("INSERT INTO measures (start_beat, rehearsal_mark, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)")
            .map_err(db_err)?;
        for m in p.measures.iter().filter(|m| m.start_count <= p.timeline.drill_end_count) {
            st.execute(params![m.start_count as i64, m.rehearsal_mark, STAMP]).map_err(db_err)?;
        }
    }

    let labels: Vec<String> = p.performers.iter().map(|q| q.label.clone()).collect();
    let orders = unique_prefix_orders(&labels, diags);
    {
        let mut st = tx
            .prepare("INSERT INTO marchers (id, name, section, notes, drill_prefix, drill_order, created_at, updated_at) VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?6)")
            .map_err(db_err)?;
        for (q, (prefix, order)) in p.performers.iter().zip(&orders) {
            let renamed = format!("{prefix}{order}") != q.label;
            let notes = if renamed { Some(format!("Source label {}", q.label)) } else { None };
            st.execute(params![q.index as i64 + 1, performer_section(q), notes, prefix, order, STAMP]).map_err(db_err)?;
        }
    }
    {
        let mut st = tx
            .prepare("INSERT INTO marcher_pages (marcher_id, page_id, x, y, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)")
            .map_err(db_err)?;
        for (page, rows) in page_coordinates(p, diags).into_iter().enumerate() {
            for (performer, (xs, ys)) in rows.into_iter().enumerate() {
                let (x, y) = canvas.to_pixels(xs, ys);
                st.execute(params![performer as i64 + 1, page as i64, x, y, STAMP]).map_err(db_err)?;
            }
        }
    }

    tx.execute_batch(TRIGGERS).map_err(db_err)?;
    tx.commit().map_err(db_err)?;
    conn.pragma_update(None, "user_version", USER_VERSION).map_err(db_err)?;

    conn.close().map_err(|(_, e)| db_err(e))
}

/// Guards OpenMarch installs on a new file; equivalent definitions keep a fresh file safe
/// even before the app first opens it.
const TRIGGERS: &str = "
CREATE TRIGGER prevent_first_beat_modification BEFORE UPDATE ON beats FOR EACH ROW WHEN OLD.id = 0
BEGIN SELECT RAISE(FAIL, 'Modification not allowed for the first beat.'); END;
CREATE TRIGGER prevent_first_beat_deletion BEFORE DELETE ON beats FOR EACH ROW WHEN OLD.id = 0
BEGIN SELECT RAISE(FAIL, 'Deletion not allowed for the first beat.'); END;
CREATE TRIGGER prevent_first_page_modification BEFORE UPDATE ON pages FOR EACH ROW
WHEN OLD.id = 0 AND (NEW.is_subset != OLD.is_subset OR NEW.start_beat != OLD.start_beat)
BEGIN SELECT RAISE(FAIL, 'Modification not allowed for the first page.'); END;
CREATE TRIGGER prevent_first_page_deletion BEFORE DELETE ON pages FOR EACH ROW WHEN OLD.id = 0
BEGIN SELECT RAISE(FAIL, 'Deletion not allowed for the first page.'); END;
CREATE TRIGGER prevent_utility_deletion BEFORE DELETE ON utility FOR EACH ROW WHEN OLD.id = 0
BEGIN SELECT RAISE(FAIL, 'Deletion not allowed for the utility record.'); END;
";

/// What `validate` found in a `.dots` file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DotsReport {
    pub user_version: i64,
    pub migrations: i64,
    pub latest_migration: i64,
    pub marchers: i64,
    pub pages: i64,
    pub beats: i64,
    pub marcher_pages: i64,
    pub measures: i64,
    pub duration_seconds: f64,
}

/// Opens a `.dots` read-only and checks everything OpenMarch relies on when loading it.
pub fn validate(path: &Path) -> Result<DotsReport, ExportError> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(db_err)?;
    let fail = |m: String| Err(ExportError::Validation(m));
    let one = |sql: &str| -> Result<i64, ExportError> { conn.query_row(sql, [], |r| r.get(0)).map_err(db_err) };

    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).map_err(db_err)?;
    if integrity != "ok" {
        return fail(format!("SQLite integrity check failed: {integrity}"));
    }
    let fk_violations = conn.prepare("PRAGMA foreign_key_check").map_err(db_err)?.query_map([], |_| Ok(())).map_err(db_err)?.count();
    if fk_violations > 0 {
        return fail(format!("{fk_violations} foreign key violation(s)"));
    }
    let user_version = one("PRAGMA user_version")?;
    if user_version != USER_VERSION as i64 {
        return fail(format!("user_version is {user_version}; OpenMarch requires {USER_VERSION}"));
    }
    let migrations = one("SELECT count(*) FROM __drizzle_migrations")?;
    let latest_migration = one("SELECT CAST(max(created_at) AS INTEGER) FROM __drizzle_migrations")?;
    let known_latest = MIGRATIONS.last().unwrap().1;
    if latest_migration < known_latest {
        return fail(format!("database predates migration {known_latest}; OpenMarch would migrate it"));
    }
    for (table, sql) in [
        ("first beat", "SELECT count(*) FROM beats WHERE id = 0 AND position = 0 AND duration = 0"),
        ("first page", "SELECT count(*) FROM pages WHERE id = 0 AND start_beat = 0"),
        ("utility row", "SELECT count(*) FROM utility WHERE id = 0"),
        ("field properties", "SELECT count(*) FROM field_properties WHERE id = 1"),
    ] {
        if one(sql)? != 1 {
            return fail(format!("missing {table}"));
        }
    }
    let json: String = conn.query_row("SELECT json_data FROM field_properties WHERE id = 1", [], |r| r.get(0)).map_err(db_err)?;
    let fp: serde_json::Value = serde_json::from_str(&json).map_err(|e| ExportError::Validation(format!("field_properties JSON: {e}")))?;
    for key in ["name", "xCheckpoints", "yCheckpoints", "stepSizeInches"] {
        if fp.get(key).is_none() {
            return fail(format!("field_properties lacks {key}"));
        }
    }
    let marchers = one("SELECT count(*) FROM marchers")?;
    let pages = one("SELECT count(*) FROM pages")?;
    let marcher_pages = one("SELECT count(*) FROM marcher_pages")?;
    if marcher_pages != marchers * pages {
        return fail(format!("{marcher_pages} marcher positions for {marchers} marchers × {pages} pages"));
    }
    let bad_coords = one("SELECT count(*) FROM marcher_pages WHERE x IS NULL OR y IS NULL OR x != x OR y != y OR abs(x) > 1e9 OR abs(y) > 1e9")?;
    if bad_coords > 0 {
        return fail(format!("{bad_coords} invalid coordinate(s)"));
    }
    let gaps = one("SELECT count(*) FROM (SELECT position, row_number() OVER (ORDER BY position) - 1 AS expected FROM beats) WHERE position != expected")?;
    if gaps > 0 {
        return fail("beat positions are not contiguous from 0".into());
    }
    let beats = one("SELECT count(*) FROM beats")?;
    let measures = one("SELECT count(*) FROM measures")?;
    let duration_seconds: f64 = conn.query_row("SELECT total(duration) FROM beats", [], |r| r.get(0)).map_err(db_err)?;
    Ok(DotsReport { user_version, migrations, latest_migration, marchers, pages, beats, marcher_pages, measures, duration_seconds })
}

/// Reads canonical step coordinates back out of a `.dots`: `[page][marcher] = (x, y)`.
pub fn read_coordinates(path: &Path) -> Result<Vec<Vec<(f64, f64)>>, ExportError> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(db_err)?;
    let json: String = conn.query_row("SELECT json_data FROM field_properties WHERE id = 1", [], |r| r.get(0)).map_err(db_err)?;
    let fp: serde_json::Value = serde_json::from_str(&json).map_err(|e| ExportError::Validation(e.to_string()))?;
    let steps = |axis: &str| -> Vec<f64> {
        fp[axis].as_array().map(|a| a.iter().filter_map(|c| c["stepsFromCenterFront"].as_f64()).collect()).unwrap_or_default()
    };
    let (xs, ys) = (steps("xCheckpoints"), steps("yCheckpoints"));
    let range = |v: &[f64]| (v.iter().copied().fold(0.0, f64::min), v.iter().copied().fold(0.0, f64::max));
    let pps = fp["stepSizeInches"].as_f64().unwrap_or(22.5) * PIXELS_PER_INCH;
    let ((x0, x1), (y0, y1)) = (range(&xs), range(&ys));
    let canvas = Canvas { pixels_per_step: pps, center_x: (x1 - x0) * pps / 2.0, front_y: (y1 - y0) * pps, width: (x1 - x0) * pps, height: (y1 - y0) * pps };
    let mut st = conn
        .prepare("SELECT mp.page_id, mp.marcher_id, mp.x, mp.y FROM marcher_pages mp JOIN pages p ON p.id = mp.page_id JOIN beats b ON b.id = p.start_beat ORDER BY b.position, mp.marcher_id")
        .map_err(db_err)?;
    let mut out: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut last_page = None;
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(2)?, r.get::<_, f64>(3)?))).map_err(db_err)?;
    for row in rows {
        let (page, x, y) = row.map_err(db_err)?;
        if last_page != Some(page) {
            out.push(Vec::new());
            last_page = Some(page);
        }
        out.last_mut().unwrap().push(canvas.to_steps(x, y));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_names_follow_openmarch() {
        assert_eq!(generate_page_names(&[false, false, true, true, false], 0), ["0", "1", "1A", "1B", "2"]);
        assert_eq!(generate_page_names(&[false, false, true, false], 59), ["59", "60", "60A", "61"]);
        assert_eq!(next_letters("Z"), "AA");
        assert_eq!(next_letters("AZ"), "BA");
    }
}
