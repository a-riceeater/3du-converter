//! Decoder tests on invented shows built at runtime. No real drill data is used.

use three_du_core::raw::{decode, Limits, RawAnnotation};
use three_du_core::synth::{encode, sample_show};
use three_du_core::*;

fn sample_bytes() -> Vec<u8> {
    encode(&sample_show())
}

fn project() -> DrillProject {
    read_bytes(&sample_bytes(), ReadOptions::default()).unwrap()
}

#[test]
fn header_and_version() {
    let raw = decode(&sample_bytes(), Limits::default(), None).unwrap();
    assert_eq!(raw.header.header_version, 12);
    assert_eq!(raw.header.app_version, [11, 3, 6]);
    assert_eq!(raw.header.version_string, "11.3.6");
    assert_eq!(detect(&sample_bytes()), FileFormat::ThreeDu);
}

#[test]
fn section_boundaries_are_contiguous() {
    let bytes = sample_bytes();
    let raw = decode(&bytes, Limits::default(), None).unwrap();
    let names: Vec<_> = raw.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["header", "sets", "performers", "audio", "count_times", "tempos", "field", "frames", "trailer"]);
    assert_eq!(raw.sections[0].start, 0);
    for w in raw.sections.windows(2) {
        assert_eq!(w[0].end, w[1].start, "{} and {} are not adjacent", w[0].name, w[1].name);
    }
    assert_eq!(raw.sections.last().unwrap().end, bytes.len() as u64);
    assert_eq!(raw.trailer, vec![0, 0, 0, 2, 0, 0, 0, 0]);
}

#[test]
fn performers_and_ids() {
    let p = project();
    let labels: Vec<_> = p.performers.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(labels, ["T1", "T2", "S1", "unlabeled"]);
    assert_eq!(p.performers[0].source_id, "0009090900000001");
    assert_eq!(p.performers[3].kind, PerformerKind::Prop);
    assert_eq!(p.performers[3].size, Some([5.0, 0.0, 5.0]));
    assert_eq!(p.performers[0].kind, PerformerKind::Marcher);
}

#[test]
fn coordinates_are_exact() {
    let p = project();
    assert_eq!(p.position(0, 0), Some([-10.0, -8.75]));
    assert_eq!(p.position(0, 4), Some([-10.0, -6.25]));
    assert_eq!(p.position(1, 19), Some([0.0, -8.75]));
    // Canonical steps: 10 yards left of center = -16 steps; front hash = 28 steps from front.
    assert_eq!(p.steps(0, 0), Some((-16.0, 28.0)));
}

#[test]
fn timing_and_tempo() {
    let p = project();
    let t = &p.timeline;
    assert_eq!(t.count_len(), 20);
    assert!((t.duration_of(1) - 0.5).abs() < 1e-12);
    assert!((t.duration_of(9) - 0.375).abs() < 1e-12);
    assert_eq!(t.tempo_changes.len(), 2);
    assert_eq!((t.tempo_changes[1].count, t.tempo_changes[1].bpm), (9, 160));
}

#[test]
fn sets_subsets_and_measures() {
    let p = project();
    let names: Vec<_> = p.sets.iter().map(|s| (s.name.as_str(), s.start_count, s.counts, s.is_subset)).collect();
    assert_eq!(names, [("1", 0, 0, false), ("2", 8, 8, false), ("2A", 12, 4, true), ("3", 16, 4, false)]);
    assert_eq!(p.timeline.drill_end_count, 16);
    assert_eq!(p.timeline.trailing_counts, 3);
    assert!(p.diagnostics.iter().any(|d| d.code == "trailing_static_counts"));
    // Set 2 is a straight move for everyone; 2A/3 contain the curve.
    assert!(p.sets[1].straight_transition);
    assert!(!p.sets[2].straight_transition);
    assert_eq!(p.sets[2].nonlinear_performers, 1);
    // "1-2" over counts 1..8, "3-4" over counts 9..16 (spanning the subset).
    let m: Vec<_> = p.measures.iter().map(|m| (m.number, m.start_count, m.provenance)).collect();
    assert_eq!(m, [(1, 1, Provenance::Derived), (2, 5, Provenance::Inferred), (3, 9, Provenance::Derived), (4, 13, Provenance::Inferred)]);
    // Rehearsal mark "A" on the opening set labels the first measure.
    assert_eq!(p.measures[0].rehearsal_mark.as_deref(), Some("A"));
}

#[test]
fn orientation_and_field() {
    let p = project();
    let t = p.coordinates.transform;
    assert_eq!((t.x_sign, t.y_sign, t.front_y), (1.0, 1.0, -26.25));
    assert_eq!(t.step_inches, 22.5);
    assert_eq!(t.field_depth_steps, 84.0);
    assert_eq!((p.field.extent_x_min, p.field.extent_x_max), (-60.0, 60.0));
    let fifty = p.field.yard_lines.iter().find(|l| l.position == 0.0).unwrap();
    assert_eq!(fifty.yard_number, Some(50.0));
    let goal = p.field.yard_lines.iter().find(|l| l.position == -50.0).unwrap();
    assert_eq!(goal.yard_number, Some(0.0));
}

#[test]
fn annotations_and_audio() {
    let p = project();
    assert_eq!(p.annotations.len(), 2);
    assert_eq!(p.annotations[0].set_index, Some(0));
    assert_eq!(p.annotations[1].set_index, Some(3));
    let music = p.metadata.music.as_ref().unwrap();
    assert_eq!(music.filename, "synthetic.ogg");
    assert!(!music.embedded);
}

#[test]
fn truncation_at_every_offset_is_an_error_not_a_panic() {
    let bytes = sample_bytes();
    // The trailer is an opaque tail, so only cuts before it are detectable.
    let trailer_start = decode(&bytes, Limits::default(), None).unwrap().trailer_offset as usize;
    for len in (0..trailer_start).step_by(7) {
        let r = decode(&bytes[..len], Limits::default(), None);
        assert!(r.is_err(), "truncated to {len} bytes decoded successfully");
    }
}

#[test]
fn corrupted_bytes_never_panic() {
    let bytes = sample_bytes();
    let mut seed = 0x2545F491u32;
    for _ in 0..300 {
        let mut b = bytes.clone();
        for _ in 0..4 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let i = 4 + (seed as usize % (b.len() - 4));
            b[i] = (seed >> 8) as u8;
        }
        let _ = read_bytes(&b, ReadOptions::default());
    }
}

#[test]
fn unsupported_version_is_structured() {
    let mut raw = sample_show();
    raw.header.header_version = 13;
    let err = decode(&encode(&raw), Limits::default(), None).unwrap_err();
    assert_eq!(err.code(), "unsupported_version");
    assert!(err.to_string().contains("13"));
}

#[test]
fn not_a_3du() {
    let err = decode(b"SQLite format 3\0", Limits::default(), None).unwrap_err();
    assert_eq!(err.code(), "not_3du");
}

#[test]
fn unknown_annotation_type_fails_safely() {
    let mut raw = sample_show();
    raw.frames[0].annotations.push(RawAnnotation::Polyline { id: [1; 8], reserved: 0, style: vec![0; 43], points: vec![] });
    let mut bytes = encode(&raw);
    // Rewrite the new polyline's type word (1) to an unknown type (9).
    let pos = bytes.windows(10).position(|w| w == [0, 1, 1, 1, 1, 1, 1, 1, 1, 1]).unwrap();
    bytes[pos + 1] = 9;
    let err = decode(&bytes, Limits::default(), None).unwrap_err();
    assert_eq!(err.code(), "invalid_record");
}

#[test]
fn limits_are_enforced() {
    let limits = Limits { max_file_bytes: 64, ..Limits::default() };
    assert_eq!(decode(&sample_bytes(), limits, None).unwrap_err().code(), "limit_exceeded");
    let limits = Limits { max_position_samples: 10, ..Limits::default() };
    assert_eq!(decode(&sample_bytes(), limits, None).unwrap_err().code(), "limit_exceeded");
}

#[test]
fn cancellation() {
    let mut cb = |done: usize, _total: usize| done < 5;
    let err = decode(&sample_bytes(), Limits::default(), Some(&mut cb)).unwrap_err();
    assert_eq!(err.code(), "cancelled");
}

#[test]
fn deterministic_and_round_trips_through_json() {
    let a = project();
    let b = project();
    assert_eq!(a, b);
    let json = to_intermediate_json(&a).unwrap();
    assert_eq!(json, to_intermediate_json(&b).unwrap());
    let back = from_intermediate_json(&json).unwrap();
    assert_eq!(back, a);
}

#[test]
fn curved_moves_can_be_split_exactly() {
    let mut p = project();
    let added = three_du_core::convert::split_curved_moves(&mut p);
    // The quarter circle runs over counts 9..16 and bends at every count.
    assert_eq!(added, 6);
    assert!(p.sets.iter().all(|s| s.straight_transition));
    let derived: Vec<_> = p.sets.iter().filter(|s| s.provenance == Provenance::Derived).map(|s| s.name.as_str()).collect();
    assert_eq!(derived, ["2A~1", "2A~2", "2A~3", "3~1", "3~2", "3~3"]);
    // Every stored count now lies on a straight segment between consecutive sets.
    for w in p.sets.windows(2) {
        let (a, b) = (w[0].start_count as usize, w[1].start_count as usize);
        for t in &p.positions {
            for c in a + 1..b {
                let (p0, p1, pc) = (t.xy[a].unwrap(), t.xy[b].unwrap(), t.xy[c].unwrap());
                let f = (c - a) as f32 / (b - a) as f32;
                assert!((p0[0] + (p1[0] - p0[0]) * f - pc[0]).abs() < 1e-3);
                assert!((p0[1] + (p1[1] - p0[1]) * f - pc[1]).abs() < 1e-3);
            }
        }
    }
    assert_eq!(p.annotations[1].set_index, Some(p.sets.len() - 1));
}

#[test]
fn page_map_and_motion_inference() {
    let raw = sample_show();
    let mut no_sets = raw.clone();
    no_sets.sets.clear();
    let mut p = read_bytes(&encode(&no_sets), ReadOptions::default()).unwrap();
    assert!(p.has_lossy_diagnostics());
    three_du_core::convert::infer_sets_from_motion(&mut p);
    let starts: Vec<_> = p.sets.iter().map(|s| s.start_count).collect();
    assert_eq!(starts.first(), Some(&0));
    assert!(starts.contains(&8) && starts.contains(&16));
    assert!(p.sets.iter().all(|s| s.provenance == Provenance::Inferred));

    let map = vec![
        PageMapEntry { name: "A".into(), start_count: 0, is_subset: false },
        PageMapEntry { name: "B".into(), start_count: 16, is_subset: false },
    ];
    let opts = ReadOptions { convert: ConvertOptions { page_map: Some(map), ..Default::default() }, ..Default::default() };
    let p = read_bytes(&encode(&raw), opts).unwrap();
    assert_eq!(p.sets.len(), 2);
    assert_eq!(p.sets[1].provenance, Provenance::User);
    assert!(!p.sets[1].straight_transition);
}
