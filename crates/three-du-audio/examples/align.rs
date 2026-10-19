//! Prints the alignment of a drill with a recording (statistics only):
//!
//!     cargo run --release -p three-du-audio --example align -- show.3du show.wav

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (drill, wav) = (&args[1], &args[2]);
    let project = three_du_core::read_file(drill, &Default::default()).expect("decode drill");
    let audio = three_du_audio::read_file(wav).expect("read audio");
    eprintln!("audio: {:.3} s, {} Hz, {} channel(s)", audio.duration_seconds(), audio.sample_rate, audio.channels);
    let a = three_du_audio::align_project(&project, &audio);
    println!("{a:#?}");
    let beats = three_du_audio::drill_beats(&project);
    let t = three_du_audio::align::track_beats(&audio, &beats, a.offset_seconds, 2.5);
    println!(
        "beat tracking: onset rate {:.2} (single offset {:.2}, chance {:.2}), tempo deviation median {:.3} max {:.3}, confident {}",
        t.onset_rate, t.constant_offset_onset_rate, t.chance_onset_rate, t.median_tempo_deviation, t.max_tempo_deviation, t.confident
    );
    if let Ok(out) = std::env::var("CLICK_CHECK") {
        let accents: Vec<usize> = project.sets.iter().filter(|s| s.start_count >= 1).map(|s| s.start_count as usize - 1).collect();
        let mix = three_du_audio::align::click_check(&audio, &t.times, &accents);
        std::fs::write(&out, three_du_audio::wav::write_pcm16(&mix, audio.sample_rate)).unwrap();
        eprintln!("wrote click check to {out}");
    }
    for (i, s) in project.sets.iter().enumerate().skip(1).step_by(4) {
        let k = s.start_count as usize - 1;
        println!("  set #{i:>2} beat {:>3}: drill {:7.3}s  audio {:7.3}s  shift {:+.3}s", k + 1, beats[k], t.times[k], t.times[k] - beats[k]);
    }
    if std::env::var("LOCAL").is_ok() {
        let beats = three_du_audio::drill_beats(&project);
        for (start, o, ratio) in three_du_audio::align::local_offsets(&audio, &beats, a.offset_seconds, 1.5, 16) {
            println!("beats {:>3}-{:<3} drill t={:7.2}s  offset {:+.3}s  peak ratio {:.2}", start + 1, start + 16, beats[start], o, ratio);
        }
    }
}
