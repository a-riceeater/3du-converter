use three_du_core::export::LossPolicy;
use three_du_core::synth::{encode, sample_show};
use three_du_core::{read_bytes, DrillProject, ReadOptions};
use sha2::{Digest, Sha256};
use three_du_fieldwork::paths::{path_point, Ratio, Segment};
use three_du_fieldwork::{build_archive, export_fieldwork, plan, validate_bytes, zip};

fn project() -> DrillProject {
    read_bytes(&encode(&sample_show()), ReadOptions::default()).unwrap()
}

fn open_db(archive: &[u8]) -> rusqlite::Connection {
    let members = zip::read(archive).unwrap();
    let db = &members.iter().find(|m| m.0 == "show.sqlite").unwrap().1;
    let path = std::env::temp_dir().join(format!("three-du-fw-test-{}-{}.sqlite", std::process::id(), db.len()));
    std::fs::write(&path, db).unwrap();
    rusqlite::Connection::open(path).unwrap()
}

#[test]
fn archive_layout_and_manifest() {
    let p = project();
    let bytes = build_archive(&p, &plan(&p)).unwrap();
    let members = zip::read(&bytes).unwrap();
    let paths: Vec<_> = members.iter().map(|m| m.0.as_str()).collect();
    assert_eq!(
        paths,
        ["manifest.json", "show.sqlite", "extensions/org.3du-converter.props/props.json", "extensions/org.3du-converter.provenance/provenance.json"]
    );
    let manifest: serde_json::Value = serde_json::from_slice(&members[0].1).unwrap();
    assert_eq!(manifest["format"], "org.fieldwork.show");
    assert_eq!(manifest["formatVersion"]["major"], 1);
    assert_eq!(manifest["requiredCapabilities"][0], "drill.core.v1");
    assert_eq!(manifest["extensions"][0]["namespace"], "org.3du-converter.props");
    assert_eq!(manifest["extensions"][1]["required"], false);
    let report = validate_bytes(&bytes).unwrap();
    assert_eq!(report.schema_version, 5, "the curved move is written as paths");
    assert_eq!(report.performers, 3, "the prop is not a performer");
    assert_eq!(report.sets, 4);
    assert_eq!(report.events, 16);
    assert!((report.duration_seconds - 7.125).abs() < 1e-9);
}

#[test]
fn deterministic_output() {
    let p = project();
    let d = plan(&p);
    assert_eq!(build_archive(&p, &d).unwrap(), build_archive(&p, &d).unwrap());
}

#[test]
fn positions_timing_and_sets() {
    let p = project();
    let bytes = build_archive(&p, &plan(&p)).unwrap();
    let db = open_db(&bytes);
    // T1 at the opening set: 10 yards left of center, on the front hash (8.75 yd in front of center).
    let (x, y): (f64, f64) = db
        .query_row(
            "SELECT pl.x, pl.y FROM placement pl JOIN performer pe ON pe.id = pl.performer_id JOIN arrival_set s ON s.id = pl.set_id WHERE pe.label = 'T1' AND s.ordinal = 0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!((x + 9.144).abs() < 1e-9, "{x}");
    assert!((y + 8.75 * 0.9144).abs() < 1e-9, "{y}");
    // Sets arrive at the expected cursors: count 8 is event ordinal 8 at fraction 0; the
    // final set (count 16, the last event boundary) is the end of event 15.
    let cursors: Vec<(String, i64, i64, i64)> = db
        .prepare("SELECT s.label, e.ordinal, s.fraction_num, s.is_subset FROM arrival_set s JOIN timing_event e ON e.id = s.event_id ORDER BY s.ordinal")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(cursors, [("1".into(), 0, 0, 0), ("2".into(), 8, 0, 0), ("2A".into(), 12, 0, 1), ("3".into(), 15, 1, 0)]);
    let (mark, marker): (String, String) = db.query_row("SELECT rehearsal_mark, measure_marker FROM arrival_set WHERE ordinal = 3", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((mark.as_str(), marker.as_str()), ("B", "3-4"));
    let first: f64 = db.query_row("SELECT baseline_seconds FROM timing_event WHERE ordinal = 0", [], |r| r.get(0)).unwrap();
    let ninth: f64 = db.query_row("SELECT baseline_seconds FROM timing_event WHERE ordinal = 8", [], |r| r.get(0)).unwrap();
    assert_eq!((first, ninth), (0.5, 0.5));
    let measures: Vec<(i64, i64)> = db.prepare("SELECT number, meter_numerator FROM measure ORDER BY ordinal").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(measures, [(1, 4), (2, 4), (3, 4), (4, 4)]);
    let unique: i64 = db.query_row("SELECT count(DISTINCT label) FROM performer", [], |r| r.get(0)).unwrap();
    assert_eq!(unique, 3);
    let note: String = db.query_row("SELECT text FROM field_note", [], |r| r.get(0)).unwrap();
    assert_eq!(note, "Opening");
    let props: i64 = db.query_row("SELECT count(*) FROM performer WHERE label = 'unlabeled'", [], |r| r.get(0)).unwrap();
    assert_eq!(props, 0);
}

#[test]
fn props_are_preserved_in_an_extension() {
    let p = project();
    let members = zip::read(&build_archive(&p, &plan(&p)).unwrap()).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&members.iter().find(|m| m.0.ends_with("props.json")).unwrap().1).unwrap();
    assert_eq!(doc["layoutVersion"], 1);
    assert_eq!(doc["sets"].as_array().unwrap().len(), 4);
    let prop = &doc["props"][0];
    assert_eq!(prop["label"], "unlabeled");
    assert_eq!(prop["sourceId"], "0009090900000004");
    assert_eq!(prop["stationary"], true);
    assert_eq!(prop["shapeCode"], 2);
    let size: Vec<f64> = prop["sizeMeters"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert!((size[0] - 4.572).abs() < 1e-9 && size[1] == 0.0 && (size[2] - 4.572).abs() < 1e-9);
    // Behind the back sideline (2.5 yd past it): y = (26.25 + 2.5) yd from center.
    let y = prop["placements"][0]["y"].as_f64().unwrap();
    assert!((y - 28.75 * 0.9144).abs() < 1e-6, "{y}");
    assert!(plan(&p).iter().any(|d| d.code == "props_in_extension"));
}

#[test]
fn tampering_is_detected() {
    let p = project();
    let bytes = build_archive(&p, &plan(&p)).unwrap();
    let mut members = zip::read(&bytes).unwrap();
    let last = members[1].1.len() - 1;
    members[1].1[last] ^= 1;
    let tampered = zip::write(&members).unwrap();
    assert!(validate_bytes(&tampered).unwrap_err().to_string().contains("does not match the manifest"));
}

#[test]
fn writes_atomically_and_refuses_overwrite() {
    let p = project();
    let dir = std::env::temp_dir().join(format!("three-du-fw-out-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("show.fieldwork");
    let _ = std::fs::remove_file(&path);
    export_fieldwork(&p, &path, LossPolicy::Default, false).unwrap();
    assert!(export_fieldwork(&p, &path, LossPolicy::Default, false).is_err());
    // Curves are kept exact as paths, so even the strict policy accepts the synthetic show.
    export_fieldwork(&p, &path, LossPolicy::Strict, true).unwrap();
    validate_bytes(&std::fs::read(&path).unwrap()).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

/// Runs our validator over a real Fieldwork-written file: set FIELDWORK_EXAMPLE to its path.
#[test]
fn validator_accepts_a_real_fieldwork_file() {
    let Ok(path) = std::env::var("FIELDWORK_EXAMPLE") else {
        eprintln!("FIELDWORK_EXAMPLE not set; skipping");
        return;
    };
    let report = validate_bytes(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(report.schema_version, 4);
}

/// Path rows of one performer in the move into the set with `ordinal`.
fn path_rows(db: &rusqlite::Connection, ordinal: i64, label: &str) -> Vec<Segment> {
    db.prepare(
        "SELECT ps.kind, ps.counts_num, ps.counts_den, ps.x1, ps.y1, ps.sweep FROM path_segment ps
           JOIN transition t ON t.id = ps.transition_id JOIN arrival_set s ON s.id = t.to_set_id
           JOIN performer pe ON pe.id = ps.performer_id WHERE s.ordinal = ?1 AND pe.label = ?2 ORDER BY ps.ordinal",
    )
    .unwrap()
    .query_map(rusqlite::params![ordinal, label], |r| {
        let counts = Ratio::new(r.get(1)?, r.get(2)?);
        let to = || -> rusqlite::Result<(f64, f64)> { Ok((r.get(3)?, r.get(4)?)) };
        Ok(match r.get::<_, String>(0)?.as_str() {
            "hold" => Segment::Hold { counts },
            "line" => Segment::Line { counts, to: to()? },
            _ => Segment::Arc { counts, to: to()?, sweep: r.get(5)? },
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[test]
fn curved_moves_become_paths_without_subsets() {
    let p = project();
    let diags = plan(&p);
    assert!(diags.iter().any(|d| d.code == "curves_as_paths"));
    assert!(!diags.iter().any(|d| d.code == "curves_flattened" || d.code == "curve_subsets_added"));
    let bytes = build_archive(&p, &diags).unwrap();
    let members = zip::read(&bytes).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&members[0].1).unwrap();
    assert_eq!(manifest["capabilities"], serde_json::json!(["drill.core.v1", "timing.anchors.v1", "drill.paths.v1"]));
    assert_eq!(manifest["requiredCapabilities"], serde_json::json!(["drill.core.v1", "drill.paths.v1"]));
    let report = validate_bytes(&bytes).unwrap();
    assert_eq!((report.sets, report.paths, report.path_segments), (4, 2, 2));

    let db = open_db(&bytes);
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, 5);
    let types: (String, String) = db.query_row("SELECT DISTINCT typeof(counts_num), typeof(counts_den) FROM path_segment", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(types, ("integer".into(), "integer".into()));
    // Only S1's quarter circle needs rows: two 45° arcs of 4 counts, into 2A and into 3. T1's
    // legs are straight at even steps and T2 stands still.
    let straight: i64 = db.query_row("SELECT count(*) FROM path_segment ps JOIN performer pe ON pe.id = ps.performer_id WHERE pe.label != 'S1'", [], |r| r.get(0)).unwrap();
    assert_eq!(straight, 0);
    let s1 = p.performers.iter().find(|q| q.label == "S1").unwrap().index;
    for (ordinal, from) in [(2, 8), (3, 12)] {
        let segments = path_rows(&db, ordinal, "S1");
        assert_eq!(segments.len(), 1);
        let Segment::Arc { counts, to, sweep } = segments[0] else { panic!("{segments:?}") };
        assert_eq!(counts, Ratio::int(4));
        assert!((sweep.abs() - 45.0).abs() < 1e-3, "{sweep}");
        assert_eq!(Some(to), p.meters(s1, from + 4), "ends on the placement's exact doubles");
        let start = p.meters(s1, from).unwrap();
        for u in 0..=4 {
            let (x, y) = path_point(start, &segments, u as f64);
            let (sx, sy) = p.meters(s1, from + u).unwrap();
            assert!((x - sx).hypot(y - sy) < 1e-4, "count {} off", from + u);
        }
    }
}

#[test]
fn straight_drills_stay_schema_4() {
    let mut p = project();
    // Walk S1 straight from count 8 to 16 instead of along the quarter circle.
    let track = &mut p.positions[2].xy;
    let (a, b) = (track[8].unwrap(), track[16].unwrap());
    for c in 9..16 {
        let t = (c - 8) as f32 / 8.0;
        track[c] = Some([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
    }
    let diags = plan(&p);
    assert!(!diags.iter().any(|d| d.code == "curves_as_paths"));
    let bytes = build_archive(&p, &diags).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&zip::read(&bytes).unwrap()[0].1).unwrap();
    assert_eq!(manifest["requiredCapabilities"], serde_json::json!(["drill.core.v1"]));
    assert_eq!(validate_bytes(&bytes).unwrap().schema_version, 4);
    let tables: i64 = open_db(&bytes).query_row("SELECT count(*) FROM sqlite_master WHERE name = 'path_segment'", [], |r| r.get(0)).unwrap();
    assert_eq!(tables, 0);
}

/// The archive with `sql` applied to its database and the manifest hashes recomputed.
fn tamper_db(archive: &[u8], sql: &str, tag: &str) -> Vec<u8> {
    let mut members = zip::read(archive).unwrap();
    let path = std::env::temp_dir().join(format!("three-du-fw-tamper-{}-{tag}.sqlite", std::process::id()));
    std::fs::write(&path, &members[1].1).unwrap();
    rusqlite::Connection::open(&path).unwrap().execute_batch(sql).unwrap();
    members[1].1 = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(&members[0].1).unwrap();
    let db = &members[1].1;
    manifest["members"][0]["bytes"] = db.len().into();
    manifest["members"][0]["sha256"] = Sha256::digest(db).iter().map(|x| format!("{x:02x}")).collect::<String>().into();
    members[0].1 = serde_json::to_vec_pretty(&manifest).unwrap();
    zip::write(&members).unwrap()
}

#[test]
fn damaged_paths_are_rejected() {
    let p = project();
    let bytes = build_archive(&p, &plan(&p)).unwrap();
    for (tag, sql, why) in [
        ("sweep", "UPDATE path_segment SET sweep = 400", "sweep"),
        // INTEGER affinity would store 4.0 as 4, so a non-integral REAL stands in for it.
        ("real", "UPDATE path_segment SET counts_num = 4.5", "positive integers"),
        ("counts", "UPDATE path_segment SET counts_num = 3", "add up"),
        ("end", "UPDATE path_segment SET x1 = x1 + 0.01", "does not end"),
        ("ordinal", "UPDATE path_segment SET ordinal = 1", "ordinals"),
        ("kind", "UPDATE path_segment SET sweep = NULL", "kind"),
        ("unplaced", "UPDATE placement SET state = 'unresolved', x = NULL, y = NULL WHERE set_id IN (SELECT to_set_id FROM transition WHERE id IN (SELECT transition_id FROM path_segment)) AND performer_id IN (SELECT performer_id FROM path_segment)", "placed"),
    ] {
        let err = validate_bytes(&tamper_db(&bytes, sql, tag)).unwrap_err().to_string();
        assert!(err.contains(why), "{tag}: {err}");
    }
    // Paths without the capability in the manifest.
    let mut members = zip::read(&bytes).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(&members[0].1).unwrap();
    manifest["requiredCapabilities"] = serde_json::json!(["drill.core.v1"]);
    members[0].1 = serde_json::to_vec_pretty(&manifest).unwrap();
    assert!(validate_bytes(&zip::write(&members).unwrap()).unwrap_err().to_string().contains("drill.paths.v1"));
}

#[test]
fn mirrored_drills_keep_positive_field_dimensions() {
    for (flip_x, flip_y) in [(true, false), (false, true), (true, true)] {
        let mut opts = ReadOptions::default();
        opts.convert.transform.flip_x = flip_x;
        opts.convert.transform.flip_y = flip_y;
        let p = read_bytes(&encode(&sample_show()), opts).unwrap();
        let bytes = build_archive(&p, &plan(&p)).unwrap();
        validate_bytes(&bytes).unwrap_or_else(|e| panic!("flip_x {flip_x}, flip_y {flip_y}: {e}"));
        let (length, width): (f64, f64) = open_db(&bytes).query_row("SELECT length, width FROM field", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        let plain = project();
        let plain_bytes = build_archive(&plain, &plan(&plain)).unwrap();
        let expected: (f64, f64) = open_db(&plain_bytes).query_row("SELECT length, width FROM field", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert!((length - expected.0).abs() < 1e-9 && (width - expected.1).abs() < 1e-9, "flip_x {flip_x}, flip_y {flip_y}");
    }
}

#[test]
fn mirroring_reflects_positions_about_the_field_center() {
    let plain = project();
    let mut opts = ReadOptions::default();
    opts.convert.transform.flip_x = true;
    opts.convert.transform.flip_y = true;
    let flipped = read_bytes(&encode(&sample_show()), opts).unwrap();
    let (a, b) = (plain.coordinates.transform, flipped.coordinates.transform);
    for track in &plain.positions {
        for [x, y] in track.xy.iter().flatten() {
            let (px, py) = a.to_meters_centered(*x as f64, *y as f64);
            let (qx, qy) = b.to_meters_centered(*x as f64, *y as f64);
            assert!((qx + px).abs() < 1e-9 && (qy + py).abs() < 1e-9, "({px}, {py}) mirrored to ({qx}, {qy})");
        }
    }
}
