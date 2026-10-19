//! MusicXML 4.0 export of a drill's musical timeline.
//!
//! The score is one percussion staff with a note on every count, so it imports as a click
//! track with the drill's structure:
//! * measures numbered as in the source, with meters derived from counts per measure;
//! * tempo marks wherever the drill's tempo changes;
//! * rehearsal marks, set names (as text) and the count within each move (as lyrics);
//! * an optional lead-in bar so that bar 1 starts where the music does in a recording.
//!
//! Count `c` (c ≥ 1) sounds at drill time `count_times[c]` and lasts until `count_times[c+1]`.

use std::fmt::Write as _;

use three_du_core::{Diagnostic, DrillProject, Severity};

#[derive(Debug, Clone, Default)]
pub struct AudioInfo {
    pub file_name: String,
    pub duration_seconds: f64,
}

#[derive(Debug, Clone, Default)]
pub struct MusicXmlOptions {
    pub audio: Option<AudioInfo>,
    /// Audio time of count 0 when the drill's own clock is used.
    pub offset_seconds: f64,
    /// Audio time of every musical beat (counts 1..=drill end). When set, playback tempos
    /// follow these times instead of the drill's clock.
    pub beat_times: Option<Vec<f64>>,
    /// Free-form `name = value` notes recorded in `<miscellaneous>` (e.g. alignment stats).
    pub notes: Vec<(String, String)>,
}

/// A bar of the count track.
#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    pub number: String,
    pub first_count: u32,
    pub counts: u32,
    /// Bar lines inside this measure range were guessed (counts did not divide evenly).
    pub estimated: bool,
    /// Not a numbered measure in the source (counts no measure range covers).
    pub implicit: bool,
}

/// Measures from the sets' measure ranges. A range covers the counts since the previous set
/// that had one. Missing measure numbers just before a range are filled in when that makes
/// the counts divide evenly; otherwise uneven ranges are split as evenly as possible.
pub fn bars(p: &DrillProject, diags: &mut Vec<Diagnostic>) -> Vec<Bar> {
    let end = p.timeline.drill_end_count;
    let mut out = Vec::new();
    let mut anchor = p.sets.first().map(|s| s.start_count).unwrap_or(0);
    let mut last_number: Option<u32> = None;
    let (mut filled, mut estimated) = (Vec::new(), Vec::new());
    for s in p.sets.iter().skip(1) {
        let Some((mut first, last)) = s.measure_range else { continue };
        let span = s.start_count.saturating_sub(anchor);
        if span == 0 {
            continue;
        }
        let mut n = last - first + 1;
        if let Some(prev) = last_number {
            if first > prev + 1 {
                let gap = first - prev - 1;
                if span % (n + gap) == 0 && span % n != 0 {
                    filled.push(if gap == 1 { (prev + 1).to_string() } else { format!("{}-{}", prev + 1, first - 1) });
                    first = prev + 1;
                    n += gap;
                }
            }
        }
        let n = n.min(span);
        let even = span % n == 0;
        if !even {
            estimated.push(format!("{first}-{last}"));
        }
        let (base, extra) = (span / n, span % n);
        let mut count = anchor + 1;
        for i in 0..n {
            let len = base + u32::from(i < extra);
            out.push(Bar { number: (first + i).to_string(), first_count: count, counts: len, estimated: !even, implicit: false });
            count += len;
        }
        anchor = s.start_count;
        last_number = Some(last);
    }
    // Counts after the last measure range: one unnumbered bar per remaining move.
    let rest: Vec<u32> = p.sets.iter().map(|s| s.start_count).filter(|&c| c > anchor && c <= end).collect();
    for c in rest {
        out.push(Bar { number: format!("X{}", out.len() + 1), first_count: anchor + 1, counts: c - anchor, estimated: false, implicit: true });
        anchor = c;
    }
    if !filled.is_empty() {
        diags.push(Diagnostic::new(
            Severity::Warning,
            "measure_numbers_filled",
            format!("measure(s) {} were missing from every set and were inferred so the counts divide evenly", filled.join(", ")),
        ));
    }
    if !estimated.is_empty() {
        diags.push(Diagnostic::new(
            Severity::Warning,
            "bar_lines_estimated",
            format!("measures {} do not divide evenly into their counts; bar lines inside them are estimated", estimated.join(", ")),
        ));
    }
    out
}

fn esc(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\n' => " ".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// Shortest decimal for a tempo, with at most 4 decimals.
fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn build(p: &DrillProject, opts: &MusicXmlOptions, diags: &mut Vec<Diagnostic>) -> Result<String, String> {
    let end = p.timeline.drill_end_count as usize;
    if end == 0 || p.timeline.count_len() <= end {
        return Err("the drill has no counts to write".into());
    }
    let times = &p.timeline.count_times_seconds;
    let tempos = &p.timeline.tempos_bpm;
    // Audio time at which each beat (count 1..=end) starts, and how long it lasts.
    let starts: Vec<f64> = match &opts.beat_times {
        Some(t) if t.len() == end => t.clone(),
        Some(_) => return Err("beat_times must have one entry per count".into()),
        None => (1..=end).map(|c| times[c] + opts.offset_seconds).collect(),
    };
    let follow = opts.beat_times.is_some();
    let lengths: Vec<f64> = (0..end)
        .map(|k| if k + 1 < end { starts[k + 1] - starts[k] } else { 60.0 / tempos[end].max(1) as f64 })
        .collect();
    if lengths.iter().any(|l| !(l.is_finite() && *l > 0.0)) {
        return Err("beat times must increase".into());
    }

    let bars = bars(p, diags);
    let set_at: std::collections::HashMap<u32, &three_du_core::DrillSet> = p.sets.iter().map(|s| (s.start_count, s)).collect();
    let rehearsal_at: std::collections::HashMap<u32, &str> = p
        .measures
        .iter()
        .filter_map(|m| m.rehearsal_mark.as_deref().map(|r| (m.start_count, r)))
        .collect();
    let move_start = |c: u32| p.sets.iter().rev().find(|s| s.start_count < c).map(|s| s.start_count).unwrap_or(0);

    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n");
    x.push_str("<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\">\n");
    x.push_str("<score-partwise version=\"4.0\">\n");
    let _ = writeln!(x, "  <work><work-title>{}</work-title></work>", esc(&p.metadata.title));
    x.push_str("  <identification>\n    <encoding>\n");
    let _ = writeln!(x, "      <software>3du-converter {}</software>", env!("CARGO_PKG_VERSION"));
    x.push_str("      <supports element=\"accidental\" type=\"no\"/>\n      <supports element=\"beam\" type=\"no\"/>\n      <supports element=\"stem\" type=\"no\"/>\n");
    x.push_str("    </encoding>\n");
    let _ = writeln!(x, "    <source>{} drill, writer version {}</source>", esc(&p.metadata.source.format), esc(&p.metadata.source.writer_version));
    x.push_str("    <miscellaneous>\n");
    let mut misc: Vec<(String, String)> = vec![
        ("timing".into(), if follow { "follows the recording beat by beat".into() } else { "drill clock".into() }),
        ("drill-counts".into(), end.to_string()),
    ];
    if let Some(a) = &opts.audio {
        misc.push(("audio-file".into(), a.file_name.clone()));
        misc.push(("audio-duration-seconds".into(), num(a.duration_seconds)));
    }
    misc.push(("audio-time-of-count-1-seconds".into(), num(starts[0])));
    misc.extend(opts.notes.iter().cloned());
    for (k, v) in &misc {
        let _ = writeln!(x, "      <miscellaneous-field name=\"{}\">{}</miscellaneous-field>", esc(k), esc(v));
    }
    x.push_str("    </miscellaneous>\n  </identification>\n");
    x.push_str("  <part-list>\n    <score-part id=\"P1\">\n      <part-name>Drill counts</part-name>\n");
    x.push_str("      <score-instrument id=\"P1-I1\"><instrument-name>Wood Block</instrument-name></score-instrument>\n");
    x.push_str("      <midi-instrument id=\"P1-I1\"><midi-channel>10</midi-channel><midi-unpitched>77</midi-unpitched></midi-instrument>\n");
    x.push_str("    </score-part>\n  </part-list>\n  <part id=\"P1\">\n");

    let attributes = |x: &mut String, beats: u32, first: bool| {
        x.push_str("      <attributes>\n");
        if first {
            x.push_str("        <divisions>1</divisions>\n        <key><fifths>0</fifths></key>\n");
        }
        let _ = writeln!(x, "        <time><beats>{beats}</beats><beat-type>4</beat-type></time>");
        if first {
            x.push_str("        <clef><sign>percussion</sign></clef>\n        <staff-details><staff-lines>1</staff-lines></staff-details>\n");
        }
        x.push_str("      </attributes>\n");
    };
    let words = |x: &mut String, text: &str| {
        let _ = writeln!(x, "      <direction placement=\"above\"><direction-type><words>{}</words></direction-type></direction>", esc(text));
    };
    let sound = |x: &mut String, tempo: f64| {
        let _ = writeln!(x, "      <sound tempo=\"{}\"/>", num(tempo));
    };

    // Lead-in: the stretch of audio before the first beat, as one quarter rest.
    let lead = starts[0];
    let first_meter = bars.first().map(|b| b.counts).unwrap_or(4);
    let mut prev_meter = 0;
    let preset = p.sets.first().filter(|s| s.start_count == 0);
    if lead > 0.01 {
        x.push_str("    <measure number=\"0\" implicit=\"yes\">\n");
        attributes(&mut x, first_meter, true);
        prev_meter = first_meter;
        words(&mut x, "Audio lead-in");
        if let Some(s) = preset {
            words(&mut x, &format!("Set {}", s.name));
        }
        sound(&mut x, 60.0 / lead);
        x.push_str("      <note><rest/><duration>1</duration><voice>1</voice><type>quarter</type></note>\n    </measure>\n");
    } else if lead < -0.01 {
        diags.push(Diagnostic::new(
            Severity::Warning,
            "audio_starts_late",
            format!("the recording starts {:.3} s after the first beat; bar 1 cannot be aligned with it", -lead),
        ));
    }

    let mut last_tempo: Option<f64> = None;
    let mut last_nominal: Option<u32> = None;
    for (i, bar) in bars.iter().enumerate() {
        let _ = writeln!(x, "    <measure number=\"{}\"{}>", esc(&bar.number), if bar.implicit { " implicit=\"yes\"" } else { "" });
        if bar.counts != prev_meter || (i == 0 && prev_meter == 0) {
            attributes(&mut x, bar.counts, prev_meter == 0);
            prev_meter = bar.counts;
        }
        if let Some(r) = rehearsal_at.get(&bar.first_count) {
            let _ = writeln!(x, "      <direction placement=\"above\"><direction-type><rehearsal>{}</rehearsal></direction-type></direction>", esc(r));
        }
        if i == 0 && lead <= 0.01 {
            if let Some(s) = preset {
                words(&mut x, &format!("Set {}", s.name));
            }
        }
        if bar.estimated && bars.get(i.wrapping_sub(1)).is_none_or(|b| !b.estimated) {
            words(&mut x, "bar lines estimated");
        }
        for c in bar.first_count..bar.first_count + bar.counts {
            let k = c as usize - 1;
            let nominal = tempos[c as usize];
            // Playback tempo of this beat: the drill's, or the recording's when followed.
            let tempo = if follow { 60.0 / lengths[k] } else { nominal as f64 };
            if last_nominal != Some(nominal) {
                // The visible mark shows the drill's tempo. Its <sound> carries the playback
                // tempo: some importers (MuseScore) play a mark's number unless <sound> says otherwise.
                let _ = writeln!(
                    x,
                    "      <direction placement=\"above\"><direction-type><metronome><beat-unit>quarter</beat-unit><per-minute>{nominal}</per-minute></metronome></direction-type><sound tempo=\"{}\"/></direction>",
                    num(tempo)
                );
                last_nominal = Some(nominal);
                last_tempo = Some(tempo);
            } else if last_tempo.is_none_or(|t| (60.0 / t - 60.0 / tempo).abs() > 1e-4) {
                // Compare beat lengths, not BPM, so rounding never accumulates past 0.1 ms.
                sound(&mut x, tempo);
                last_tempo = Some(tempo);
            }
            if let Some(s) = set_at.get(&c) {
                words(&mut x, &format!("Set {}{}", s.name, if s.is_subset { " (subset)" } else { "" }));
            }
            let in_move = c - move_start(c);
            let _ = writeln!(
                x,
                "      <note><unpitched><display-step>B</display-step><display-octave>4</display-octave></unpitched><duration>1</duration><instrument id=\"P1-I1\"/><voice>1</voice><type>quarter</type><lyric number=\"1\"><syllabic>single</syllabic><text>{in_move}</text></lyric></note>"
            );
        }
        if i + 1 == bars.len() {
            x.push_str("      <barline location=\"right\"><bar-style>light-heavy</bar-style></barline>\n");
        }
        x.push_str("    </measure>\n");
    }
    x.push_str("  </part>\n</score-partwise>\n");

    let covered: u32 = bars.iter().map(|b| b.counts).sum();
    let first = bars.first().map(|b| b.first_count).unwrap_or(1);
    if first != 1 || covered as usize != end {
        return Err(format!("internal: bars cover counts {first}..{} but the drill has 1..={end}", first as u64 + covered as u64 - 1));
    }
    if let Some(a) = &opts.audio {
        let music_end = starts[end - 1] + lengths[end - 1];
        if music_end > a.duration_seconds + 0.05 {
            diags.push(Diagnostic::new(Severity::Warning, "audio_shorter_than_drill", format!("the drill ends {:.2} s after the recording does", music_end - a.duration_seconds)));
        }
    }
    Ok(x)
}

/// Checks that the document's elements nest correctly (the writer's output contains no
/// comments or CDATA; the XML declaration and DOCTYPE are skipped).
pub fn check_well_formed(xml: &str) -> Result<(), String> {
    let mut stack: Vec<&str> = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let end = rest.find('>').ok_or("unterminated tag")?;
        let tag = &rest[..end];
        rest = &rest[end + 1..];
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            match stack.pop() {
                Some(open) if open == name.trim() => {}
                other => return Err(format!("</{}> closes <{}>", name.trim(), other.unwrap_or("nothing"))),
            }
        } else if !tag.ends_with('/') {
            stack.push(tag.split_whitespace().next().ok_or("empty tag")?);
        }
    }
    match stack.as_slice() {
        [] => Ok(()),
        open => Err(format!("unclosed <{}>", open.join(">, <"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use three_du_core::synth::{encode, sample_show};
    use three_du_core::{read_bytes, ReadOptions};

    fn project() -> DrillProject {
        read_bytes(&encode(&sample_show()), ReadOptions::default()).unwrap()
    }

    #[test]
    fn bars_follow_measure_ranges() {
        let mut d = vec![];
        let b = bars(&project(), &mut d);
        let got: Vec<_> = b.iter().map(|b| (b.number.as_str(), b.first_count, b.counts, b.estimated)).collect();
        assert_eq!(got, [("1", 1, 4, false), ("2", 5, 4, false), ("3", 9, 4, false), ("4", 13, 4, false)]);
        assert!(d.is_empty());
    }

    #[test]
    fn fills_gaps_and_estimates_uneven_ranges() {
        let mut p = project();
        // Ranges "1-2" then "4-5" over 8 + 9 counts: measure 3 is missing, 9 counts / 3 = 3 each.
        p.sets[1].measure_range = Some((1, 2));
        p.sets[2].measure_range = None;
        p.sets[3].measure_range = Some((4, 5));
        p.sets[3].start_count = 17;
        p.timeline.drill_end_count = 17;
        let mut d = vec![];
        let b = bars(&p, &mut d);
        let got: Vec<_> = b.iter().map(|b| (b.number.as_str(), b.counts)).collect();
        assert_eq!(got, [("1", 4), ("2", 4), ("3", 3), ("4", 3), ("5", 3)]);
        assert!(d.iter().any(|x| x.code == "measure_numbers_filled"));

        // "3-4" over 9 counts with no gap: split 5 + 4 and flagged.
        p.sets[3].measure_range = Some((3, 4));
        let mut d = vec![];
        let got: Vec<_> = bars(&p, &mut d).iter().map(|b| (b.number.clone(), b.counts, b.estimated)).collect();
        assert_eq!(got[2..], [("3".to_string(), 5, true), ("4".to_string(), 4, true)]);
        assert!(d.iter().any(|x| x.code == "bar_lines_estimated"));
    }

    #[test]
    fn score_contents() {
        let p = project();
        let mut d = vec![];
        let xml = build(&p, &MusicXmlOptions::default(), &mut d).unwrap();
        assert!(xml.starts_with("<?xml") && xml.contains("<score-partwise version=\"4.0\">"));
        assert_eq!(xml.matches("<unpitched>").count(), 16, "one note per count");
        assert_eq!(xml.matches("<measure ").count(), 5, "lead-in plus four bars");
        assert!(xml.contains("<per-minute>120</per-minute>") && xml.contains("<per-minute>160</per-minute>"));
        assert!(xml.contains("<sound tempo=\"120\"/>"));
        assert!(xml.contains("<rehearsal>A</rehearsal>"));
        assert!(xml.contains("<words>Set 2A (subset)</words>"));
        // Lead-in: count 1 sounds 0.5 s into the drill clock, so the rest lasts 0.5 s (120 BPM).
        assert!(xml.contains("<measure number=\"0\" implicit=\"yes\">"));
        assert!(xml.contains("<sound tempo=\"120\"/>\n      <note><rest/>"));
        // The count within the move restarts after each set.
        assert!(xml.contains("<text>8</text>") && !xml.contains("<text>9</text>"));
        assert_eq!(xml, build(&p, &MusicXmlOptions::default(), &mut vec![]).unwrap(), "deterministic");
        check_well_formed(&xml).unwrap();
        assert!(check_well_formed("<a><b></a>").is_err());
    }

    #[test]
    fn follows_recorded_beat_times() {
        let p = project();
        let nominal: Vec<f64> = (1..=16).map(|c| p.timeline.count_times_seconds[c]).collect();
        let mut performed = nominal.clone();
        for t in performed.iter_mut().skip(4) {
            *t += 0.1; // the band holds beat 4 a little longer
        }
        let opts = MusicXmlOptions { beat_times: Some(performed), ..Default::default() };
        let xml = build(&p, &opts, &mut vec![]).unwrap();
        // Beat 4 lasts 0.6 s (100 BPM) then the tempo returns to 120.
        assert!(xml.contains("<sound tempo=\"100\"/>"));
        // The visible mark still shows the drill tempo while playback follows the beats.
        assert!(xml.contains("<per-minute>120</per-minute></metronome></direction-type><sound tempo=\"120\"/>"));
    }
}
