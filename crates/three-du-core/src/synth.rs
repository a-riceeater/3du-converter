//! Encoder for the `3DUD` layout, used to build synthetic test fixtures at runtime.
//!
//! Committed tests never contain real drill files: they describe a small invented show
//! here and encode it. Encoding is the exact inverse of [`crate::raw::decode`].

use crate::raw::*;

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u16(out, s.len() as u16);
    out.extend_from_slice(s.as_bytes());
}

pub fn encode(d: &RawDrill) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(MAGIC);
    put_u16(&mut o, d.header.header_version);
    for v in d.header.app_version {
        put_u16(&mut o, v);
    }
    put_str(&mut o, &d.header.version_string);
    put_str(&mut o, &d.header.title);
    put_u32(&mut o, d.header.reserved);
    for n in &d.header.note_labels {
        put_str(&mut o, n);
    }

    put_u16(&mut o, d.sets.len() as u16);
    for s in &d.sets {
        put_u16(&mut o, s.start_count);
        o.extend_from_slice(&s.id);
        put_str(&mut o, &s.rehearsal_mark);
        put_str(&mut o, &s.name);
        put_str(&mut o, &s.measures);
        o.extend_from_slice(&s.reserved_a);
        put_str(&mut o, &s.tab_type);
        put_u32(&mut o, s.reserved_b);
        put_str(&mut o, &s.color);
        put_u16(&mut o, s.reserved_c);
    }

    put_u16(&mut o, d.performers.len() as u16);
    for p in &d.performers {
        o.extend_from_slice(&p.id);
        put_str(&mut o, &p.label);
        put_str(&mut o, &p.info);
        put_str(&mut o, &p.symbol);
    }

    put_str(&mut o, &d.audio_reference);
    put_u16(&mut o, d.count_times.len() as u16);
    for t in &d.count_times {
        o.extend_from_slice(&t.to_be_bytes());
    }
    put_u16(&mut o, d.tempos.len() as u16);
    for t in &d.tempos {
        put_u32(&mut o, *t);
    }
    put_u16(&mut o, d.field_records.len() as u16);
    for r in &d.field_records {
        put_str(&mut o, r);
    }

    put_u16(&mut o, d.frames.len() as u16);
    for f in &d.frames {
        put_u16(&mut o, f.objects.len() as u16);
        for ob in &f.objects {
            o.extend_from_slice(&ob.id);
            put_f32(&mut o, ob.x);
            put_f32(&mut o, ob.y);
            for c in ob.rgb {
                put_u16(&mut o, c);
            }
            put_u16(&mut o, ob.symbol);
            put_u16(&mut o, ob.state);
            put_u16(&mut o, ob.kind);
            put_f32(&mut o, ob.rotation);
            match ob.size {
                None => o.push(0),
                Some(s) => {
                    o.push(1);
                    put_f32(&mut o, s.width);
                    put_f32(&mut o, s.depth);
                    put_f32(&mut o, s.height);
                    put_u16(&mut o, s.shape);
                }
            }
        }
        put_u16(&mut o, f.annotations.len() as u16);
        for a in &f.annotations {
            match a {
                RawAnnotation::Text { id, reserved, rect, extra, text } => {
                    put_u16(&mut o, 2);
                    o.extend_from_slice(id);
                    put_u16(&mut o, *reserved);
                    for v in rect.iter().chain(extra) {
                        put_f32(&mut o, *v);
                    }
                    put_str(&mut o, text);
                }
                RawAnnotation::Polyline { id, reserved, style, points } => {
                    put_u16(&mut o, 1);
                    o.extend_from_slice(id);
                    put_u16(&mut o, *reserved);
                    let mut st = style.clone();
                    st.resize(43, 0);
                    o.extend_from_slice(&st);
                    put_u16(&mut o, points.len() as u16);
                    for p in points {
                        put_f32(&mut o, p[0]);
                        put_f32(&mut o, p[1]);
                    }
                }
            }
        }
    }
    o.extend_from_slice(&d.trailer);
    o
}

/// The field records of a standard 8-to-5 football field in yards, as the source writes them.
pub fn standard_field_records() -> Vec<String> {
    let mut r = vec![
        "UNIT 0 0".to_string(),
        "GRID 8 5 8 5 1 1 247 187 50".to_string(),
        "BORD -50.0 -26.25 50.0 26.25 255 255 255".to_string(),
        "HZMJ -26.25 0 `".to_string(),
        "HZMJ 26.25 0 `".to_string(),
    ];
    for i in 0..=20 {
        r.push(format!("VTMJ {:.1} 1 `", -50.0 + 5.0 * i as f64));
    }
    for x in [-60.0, -55.0, 55.0, 60.0] {
        r.push(format!("VTMN {x:.1}"));
    }
    r.extend(["HZHS -8.75 `", "HZHS 8.75 `", "LEFT Side_1", "RIGHT Side_2", "TOP Back", "BOTTOM Front"].map(String::from));
    r
}

/// A small invented show exercising every feature the decoder knows about:
/// static positions, straight moves, a curved move, a subset, tempo changes, a prop,
/// text and line annotations, and trailing counts after the final set.
pub fn sample_show() -> RawDrill {
    let ids: Vec<[u8; 8]> = (0..4u8).map(|i| [0, 9, 9, 9, 0, 0, 0, i + 1]).collect();
    let labels = ["T1", "T2", "S1", "unlabeled"];
    let sets = [(0u16, "1", "A", "", "StandardTab"), (8, "2", "", "1-2", "StandardTab"), (12, "2A", "", "", "SubTab"), (16, "3", "B", "3-4", "StandardTab")];
    let frame_count = 20usize;

    // Tempo: 120 BPM for counts 0..=8, then 160 BPM. As in real files, the tempo stored at a
    // count governs the interval that starts there.
    let tempos: Vec<u32> = (0..frame_count).map(|c| if c <= 8 { 120 } else { 160 }).collect();
    let mut count_times = vec![0.0f64];
    for c in 1..frame_count {
        count_times.push(count_times[c - 1] + 60.0 / tempos[c - 1] as f64);
    }

    let position = |p: usize, c: usize| -> (f32, f32) {
        let c = c.min(16) as f32;
        match p {
            // Source y grows toward the front sideline (screen-style axes).
            // Straight line 8 counts toward the back 5 yards, hold, then 8 counts across 5 yards.
            0 => {
                if c <= 8.0 {
                    (-10.0, 8.75 - 5.0 * c / 8.0)
                } else {
                    (-10.0 + 5.0 * (c - 8.0) / 8.0, 3.75)
                }
            }
            // Stays on the 50, front hash.
            1 => (0.0, 8.75),
            // Quarter circle of radius 5 around (10, 0) for counts 8..16.
            2 => {
                if c <= 8.0 {
                    (15.0, 0.0)
                } else {
                    let a = (c - 8.0) / 8.0 * std::f32::consts::FRAC_PI_2;
                    (10.0 + 5.0 * a.cos(), -5.0 * a.sin())
                }
            }
            // A prop behind the back sideline.
            _ => (0.0, -28.75),
        }
    };

    let frames = (0..frame_count)
        .map(|c| {
            let objects = (0..4)
                .map(|p| {
                    let (x, y) = position(p, c);
                    RawObject {
                        id: ids[p],
                        x,
                        y,
                        rgb: [255, 0, 0],
                        symbol: b'X' as u16,
                        state: 0,
                        kind: if p == 3 { 1 } else { 2 },
                        rotation: 0.0,
                        size: (p == 3).then_some(RawObjectSize { width: 5.0, depth: 0.0, height: 5.0, shape: 2 }),
                    }
                })
                .collect();
            let annotations = match c {
                0 => vec![RawAnnotation::Text { id: [7; 8], reserved: 0, rect: [-60.0, 18.75, -47.5, 26.25], extra: [0.0, 0.0, 1.0], text: "Opening".into() }],
                16 => vec![RawAnnotation::Polyline { id: [8; 8], reserved: 0, style: vec![0; 43], points: vec![[0.0, 0.0], [5.0, 5.0]] }],
                _ => vec![],
            };
            RawFrame { offset: 0, objects, annotations }
        })
        .collect();

    RawDrill {
        file_size: 0,
        header: RawHeader {
            header_version: 12,
            app_version: [11, 3, 6],
            version_string: "11.3.6".into(),
            title: "Synthetic Test Show".into(),
            reserved: 0,
            note_labels: (1..=5).map(|i| format!("Note {i}")).collect(),
        },
        sets: sets
            .iter()
            .enumerate()
            .map(|(i, &(count, name, mark, measures, tab))| RawSet {
                offset: 0,
                start_count: count,
                id: [0, 1, 2, 3, 4, 5, 6, i as u8],
                rehearsal_mark: mark.into(),
                name: name.into(),
                measures: measures.into(),
                reserved_a: [0; 10],
                tab_type: tab.into(),
                reserved_b: 0,
                color: ".".into(),
                reserved_c: 0,
            })
            .collect(),
        performers: ids
            .iter()
            .zip(labels)
            .map(|(id, label)| RawPerformer { id: *id, label: label.into(), info: String::new(), symbol: "X".into() })
            .collect(),
        audio_reference: "synthetic.ogg".into(),
        count_times,
        tempos,
        field_records: standard_field_records(),
        frames,
        trailer: vec![0, 0, 0, 2, 0, 0, 0, 0],
        trailer_offset: 0,
        sections: vec![],
    }
}
