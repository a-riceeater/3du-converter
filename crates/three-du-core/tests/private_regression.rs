//! Opt-in regression check against a real, private drill. Never runs in CI.
//!
//! Set `THREE_DU_PRIVATE_SAMPLE` to a `.3du` path and, optionally, `THREE_DU_PRIVATE_ORACLE`
//! to the previously extracted JSON. Only aggregate numbers are printed; nothing from the
//! drill is written to disk or to snapshots.

use std::collections::HashSet;

use three_du_core::*;

#[test]
fn private_sample_regression() {
    let Ok(path) = std::env::var("THREE_DU_PRIVATE_SAMPLE") else {
        eprintln!("THREE_DU_PRIVATE_SAMPLE not set; skipping");
        return;
    };
    let p = read_file(&path, &ConvertOptions::default()).expect("private sample decodes");

    let ids: HashSet<_> = p.performers.iter().map(|q| q.source_id.clone()).collect();
    assert_eq!(ids.len(), p.performers.len(), "performer ids are unique");
    let samples: usize = p.positions.iter().map(|t| t.xy.iter().flatten().count()).sum();
    assert!(p.positions.iter().flat_map(|t| t.xy.iter().flatten()).all(|xy| xy[0].is_finite() && xy[1].is_finite()));
    assert!(p.sets.windows(2).all(|w| w[1].start_count > w[0].start_count));
    eprintln!(
        "performers={} counts={} samples={} sets={} (subsets {}) tempo_changes={} measures={} nonlinear={}",
        p.performers.len(),
        p.timeline.count_len(),
        samples,
        p.sets.len(),
        p.sets.iter().filter(|s| s.is_subset).count(),
        p.timeline.tempo_changes.len(),
        p.measures.len(),
        p.sets.iter().filter(|s| !s.straight_transition).count()
    );

    let Ok(oracle_path) = std::env::var("THREE_DU_PRIVATE_ORACLE") else { return };
    let oracle: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(oracle_path).unwrap()).unwrap();
    assert_eq!(oracle["source_version"].as_str(), Some(p.metadata.source.writer_version.as_str()));
    assert_eq!(oracle["title"].as_str(), Some(p.metadata.title.as_str()));
    assert_eq!(oracle["field_configuration"].as_array().unwrap().len(), p.field.source.other_records.len() + count_interpreted(&p));

    let times = oracle["count_times_seconds"].as_array().unwrap();
    assert_eq!(times.len(), p.timeline.count_len());
    for (a, b) in times.iter().zip(&p.timeline.count_times_seconds) {
        assert!((a.as_f64().unwrap() - b).abs() < 1e-9);
    }
    let tempos: Vec<u32> = oracle["tempos_bpm"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
    assert_eq!(tempos, p.timeline.tempos_bpm);

    let performers = oracle["performers"].as_array().unwrap();
    assert_eq!(performers.len(), p.performers.len());
    let mut compared = 0usize;
    let mut max_err = 0f64;
    for (o, q) in performers.iter().zip(&p.performers) {
        assert_eq!(o["id"].as_str(), Some(q.source_id.as_str()), "performer order and identity");
        assert_eq!(o["label"].as_str(), Some(q.label.as_str()));
        let track = &p.positions[q.index];
        for (c, xy) in o["positions_xy"].as_array().unwrap().iter().enumerate() {
            let ours = track.xy[c].expect("sample present");
            for k in 0..2 {
                // The oracle printed f32 values with 7 significant digits.
                let err = (xy[k].as_f64().unwrap() - ours[k] as f64).abs();
                max_err = max_err.max(err);
                assert!(err <= 1e-5 * (1.0 + ours[k].abs() as f64), "count {c} differs by {err}");
            }
            compared += 1;
        }
    }
    eprintln!("oracle: {compared} positions match (max abs diff {max_err:.2e})");
}

fn count_interpreted(p: &DrillProject) -> usize {
    let f = &p.field.source;
    usize::from(f.unit_code.is_some())
        + usize::from(f.grid.is_some())
        + usize::from(f.border.is_some())
        + f.x_lines.len()
        + f.y_lines.len()
        + f.hashes.len()
        + f.captions.len()
}
