use std::path::PathBuf;

use three_du_core::export::LossPolicy;
use three_du_core::synth::{encode, sample_show};
use three_du_core::{read_bytes, DrillProject, ReadOptions};
use three_du_openmarch::{dots, export_dots, export_om, om};

fn project() -> DrillProject {
    read_bytes(&encode(&sample_show()), ReadOptions::default()).unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("three-du-om-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn om_structure_matches_the_universal_standard() {
    let p = project();
    let mut d = vec![];
    let show = om::build(&p, &mut d);
    om::validate(&show).unwrap();
    assert_eq!(show.om_schema_version, "0.2.0");
    assert_eq!(show.pages.len(), 4);
    // Page 0 is the zero-length opening page; later pages start the beat after the previous set.
    let starts: Vec<_> = show.pages.iter().map(|p| (p.name.as_str(), p.start_beat_index, p.is_subset)).collect();
    assert_eq!(starts, [("1", 0, None), ("2", 1, None), ("2A", 9, Some(true)), ("3", 13, None)]);
    assert!((show.pages[1].duration - 4.0).abs() < 1e-12, "8 counts at 120 BPM");
    assert!((show.pages[2].duration - 1.625).abs() < 1e-12, "one count at 120 BPM, three at 160");
    // Tempo: the mandatory 0-tempo first beat, 8 beats at 120, 8 at 160 (ends at the final set).
    let tempos: Vec<_> = show.tempo_sections.iter().map(|t| (t.tempo, t.number_of_beats)).collect();
    assert_eq!(tempos, [(0.0, 1), (120.0, 9), (160.0, 7)]);
    // T1 starts 10 yd left of center on the front hash: -16 steps, 28 steps from the front.
    let c = show.coordinates.iter().find(|c| c.marcher_id == "1" && c.page_id == "0").unwrap();
    assert_eq!((c.x_steps, c.y_steps), (-16.0, 28.0));
    assert_eq!(show.metadata.performance_area.y_origin, "front");
    assert_eq!(show.metadata.performance_area.inches_per_step, 22.5);
    let names: Vec<_> = show.measures.as_ref().unwrap().iter().map(|m| (m.name.as_str(), m.start_beat_index)).collect();
    assert_eq!(names, [("1", 1), ("2", 5), ("3", 9), ("4", 13)]);
    let json: serde_json::Value = serde_json::from_slice(&om::to_json(&show)).unwrap();
    for key in ["omSchemaVersion", "metadata", "performers", "pages", "tempoSections", "coordinates"] {
        assert!(json.get(key).is_some(), "missing {key}");
    }
    assert!(json["coordinates"][0].get("marcherId").is_some());
    assert!(json["metadata"]["performanceArea"]["xCheckpoints"][0].get("stepsFromCenter").is_some());
}

#[test]
fn om_and_omz_files_round_trip_deterministically() {
    let p = project();
    let a = scratch("a.om");
    let b = scratch("b.omz");
    let c = scratch("c.omz");
    export_om(&p, &a, false, LossPolicy::Default, false).unwrap();
    export_om(&p, &b, true, LossPolicy::Default, false).unwrap();
    export_om(&p, &c, true, LossPolicy::Default, false).unwrap();
    let from_om = om::parse(&std::fs::read(&a).unwrap()).unwrap();
    let from_omz = om::parse(&std::fs::read(&b).unwrap()).unwrap();
    assert_eq!(from_om, from_omz);
    assert_eq!(std::fs::read(&b).unwrap(), std::fs::read(&c).unwrap(), "omz output is reproducible");
    assert_eq!(&std::fs::read(&b).unwrap()[..2], &[0x1f, 0x8b]);
    // No overwrite without permission.
    assert!(export_om(&p, &a, false, LossPolicy::Default, false).is_err());
}

#[test]
fn strict_policy_refuses_flattened_curves() {
    let p = project();
    let err = export_om(&p, &scratch("strict.om"), false, LossPolicy::Strict, false).unwrap_err();
    assert_eq!(err.code(), "refused_lossy");
    assert!(err.to_string().contains("curves_flattened"));
    assert!(!scratch("strict.om").exists());
}

#[test]
fn dots_database_is_valid_and_positions_survive() {
    let p = project();
    let path = scratch("show.dots");
    let diags = export_dots(&p, &path, LossPolicy::Default, false).unwrap();
    assert!(diags.iter().any(|d| d.code == "curves_flattened"));
    let report = dots::validate(&path).unwrap();
    assert_eq!(report.user_version, 7);
    assert_eq!(report.migrations, 17);
    assert_eq!(report.marchers, 4);
    assert_eq!(report.pages, 4);
    assert_eq!(report.beats, 17, "beat 0 plus counts 1..=16");
    assert!((report.duration_seconds - (4.5 + 7.0 * 0.375)).abs() < 1e-9);

    let back = dots::read_coordinates(&path).unwrap();
    for (s, set) in p.sets.iter().enumerate() {
        for i in 0..p.performers.len() {
            let (x, y) = p.steps(i, set.start_count as usize).unwrap();
            let (bx, by) = back[s][i];
            assert!((x - bx).abs() < 1e-9 && (y - by).abs() < 1e-9, "set {s} performer {i}: {:?} vs {:?}", (x, y), (bx, by));
        }
    }

    let conn = rusqlite::Connection::open(&path).unwrap();
    let starts: Vec<i64> = conn.prepare("SELECT start_beat FROM pages ORDER BY start_beat").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(starts, [0, 1, 9, 13]);
    let subset: i64 = conn.query_row("SELECT is_subset FROM pages WHERE start_beat = 9", [], |r| r.get(0)).unwrap();
    assert_eq!(subset, 1);
    let labels: Vec<String> = conn
        .prepare("SELECT drill_prefix || drill_order FROM marchers ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(labels, ["T1", "T2", "S1", "unlabeled1"]);
    let settings: String = conn.query_row("SELECT json_data FROM workspace_settings", [], |r| r.get(0)).unwrap();
    assert!(settings.contains("\"pageNumberOffset\":1"));
    // The guard triggers OpenMarch relies on are present.
    let triggers: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'trigger'", [], |r| r.get(0)).unwrap();
    assert_eq!(triggers, 5);
    assert!(conn.execute("DELETE FROM pages WHERE id = 0", []).is_err());
}

#[test]
fn dots_output_is_deterministic() {
    let p = project();
    let (a, b) = (scratch("d1.dots"), scratch("d2.dots"));
    export_dots(&p, &a, LossPolicy::Default, false).unwrap();
    export_dots(&p, &b, LossPolicy::Default, false).unwrap();
    assert_eq!(std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
}

/// Compares our schema with the one OpenMarch's own migrations build.
/// Set OPENMARCH_MIGRATIONS_DIR to `apps/desktop/electron/database/migrations` of an OpenMarch checkout.
#[test]
fn schema_matches_openmarch_migrations() {
    let Ok(dir) = std::env::var("OPENMARCH_MIGRATIONS_DIR") else {
        eprintln!("OPENMARCH_MIGRATIONS_DIR not set; skipping");
        return;
    };
    let reference = rusqlite::Connection::open_in_memory().unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "sql")).collect();
    files.sort();
    assert_eq!(files.len(), three_du_openmarch::dots_schema::MIGRATIONS.len(), "migration count changed: update dots_schema");
    for f in files {
        let sql = std::fs::read_to_string(f).unwrap().replace("PRAGMA foreign_keys=ON;", "").replace("PRAGMA foreign_keys = ON;", "");
        reference.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        for stmt in sql.split("--> statement-breakpoint") {
            if !stmt.trim().is_empty() {
                reference.execute_batch(stmt).unwrap();
            }
        }
    }
    let ours = rusqlite::Connection::open_in_memory().unwrap();
    ours.execute_batch(&three_du_openmarch::dots_schema::ddl()).unwrap();

    let describe = |c: &rusqlite::Connection| -> Vec<String> {
        let names: Vec<(String, String)> = c
            .prepare("SELECT type, name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND name != '__drizzle_migrations' ORDER BY type, name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let mut out = Vec::new();
        for (kind, name) in names {
            if kind == "table" {
                let cols: Vec<String> = c
                    .prepare(&format!("PRAGMA table_xinfo(\"{name}\")"))
                    .unwrap()
                    .query_map([], |r| Ok(format!("{}:{}:{}:{:?}:{}", r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, i64>(5)?)))
                    .unwrap()
                    .map(Result::unwrap)
                    .collect();
                let fks: Vec<String> = c
                    .prepare(&format!("PRAGMA foreign_key_list(\"{name}\")"))
                    .unwrap()
                    .query_map([], |r| Ok(format!("{}->{}.{} {} {}", r.get::<_, String>(3)?, r.get::<_, String>(2)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?)))
                    .unwrap()
                    .map(Result::unwrap)
                    .collect();
                out.push(format!("table {name} [{}] fk [{}]", cols.join(", "), fks.join(", ")));
            } else if kind == "index" {
                let cols: Vec<String> = c
                    .prepare(&format!("PRAGMA index_info(\"{name}\")"))
                    .unwrap()
                    .query_map([], |r| r.get::<_, String>(2))
                    .unwrap()
                    .map(Result::unwrap)
                    .collect();
                out.push(format!("index {name} ({})", cols.join(",")));
            } else {
                out.push(format!("{kind} {name}"));
            }
        }
        out
    };
    assert_eq!(describe(&ours), describe(&reference));
}
