use three_du_core::export::LossPolicy;
use three_du_core::synth::{encode, sample_show};
use three_du_core::{read_bytes, DrillProject, ReadOptions};
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
    assert_eq!(report.schema_version, 4);
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
    assert_eq!(export_fieldwork(&p, &path, LossPolicy::Strict, true).unwrap_err().code(), "refused_lossy");
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
