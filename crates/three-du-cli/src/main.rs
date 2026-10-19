//! `3du-converter`: inspect, extract, convert and validate drill files.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::json;
use three_du_core::convert::{infer_sets_from_motion, split_curved_moves};
use three_du_core::export::{write_atomic, ExportError, LossPolicy};
use three_du_core::{detect, read_bytes, ConvertOptions, DecodeError, Diagnostic, DrillProject, FileFormat, PageMapEntry, ReadOptions, Severity, TransformOverrides};

/// Exit codes are part of the interface; scripts may rely on them.
mod exit {
    pub const OK: u8 = 0;
    pub const FAILURE: u8 = 1;
    pub const INVALID_INPUT: u8 = 3;
    pub const REFUSED_LOSSY: u8 = 4;
    pub const OUTPUT_EXISTS: u8 = 5;
    pub const VALIDATION_FAILED: u8 = 6;
    pub const IO: u8 = 7;
}

#[derive(Parser)]
#[command(name = "3du-converter", version, about = "Convert Pyware/UDB .3du drill files to OpenMarch and Fieldwork", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Print every diagnostic, including informational ones.
    #[arg(long, short, global = true)]
    verbose: bool,
    /// Print nothing except errors.
    #[arg(long, short, global = true, conflicts_with = "verbose")]
    quiet: bool,
    /// Machine-readable JSON on stdout.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Report format, version, counts, sets and conversion warnings.
    Inspect {
        input: PathBuf,
        /// Also print the title, set names and performer labels (private by default).
        #[arg(long)]
        reveal: bool,
        #[command(flatten)]
        source: SourceArgs,
    },
    /// Convert a .3du drill to another format.
    Convert {
        input: PathBuf,
        #[arg(long, short, value_enum)]
        format: OutputFormat,
        #[arg(long, short)]
        output: PathBuf,
        #[command(flatten)]
        policy: PolicyArgs,
        #[command(flatten)]
        source: SourceArgs,
        #[command(flatten)]
        audio: AudioArgs,
    },
    /// Find where the drill sits in a recording (writes nothing except --click-check).
    Align {
        input: PathBuf,
        #[command(flatten)]
        source: SourceArgs,
        #[command(flatten)]
        audio: AudioArgs,
    },
    /// Write the decoded drill as intermediate JSON.
    Extract {
        input: PathBuf,
        #[arg(long, short)]
        output: PathBuf,
        #[arg(long)]
        overwrite: bool,
        #[command(flatten)]
        source: SourceArgs,
    },
    /// Check a .dots, .fieldwork, .om, .omz or .3du file.
    Validate { input: PathBuf },
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    /// OpenMarch project (SQLite)
    Dots,
    /// Fieldwork show
    #[value(alias = "fieldworks")]
    Fieldwork,
    /// OpenMarch Universal Standard JSON
    Om,
    /// OpenMarch Universal Standard, gzip-compressed
    Omz,
    /// Intermediate JSON (same as `extract`)
    Json,
    /// MusicXML count track: measures, meters, tempos, rehearsal marks, sets
    #[value(alias = "xml", alias = "mxl")]
    Musicxml,
}

#[derive(Args, Default)]
struct AudioArgs {
    /// The show's recording (.wav). Used to align the count track and checked for length.
    #[arg(long, value_name = "FILE")]
    audio: Option<PathBuf>,
    /// Audio time (s) of count 0, overriding automatic alignment.
    #[arg(long, value_name = "SECONDS", allow_hyphen_values = true)]
    audio_offset: Option<f64>,
    /// Follow the recording beat by beat (live performances drift from the drill's tempo).
    #[arg(long, requires = "audio")]
    follow_audio: bool,
    /// Write the recording with a click on every aligned beat, to check the alignment by ear.
    #[arg(long, value_name = "FILE", requires = "audio")]
    click_check: Option<PathBuf>,
}

#[derive(Args)]
struct PolicyArgs {
    /// Reject any conversion that is not exact (including flattened curves).
    #[arg(long, conflicts_with = "allow_lossy")]
    strict: bool,
    /// Accept approximations, such as set boundaries inferred from movement.
    #[arg(long)]
    allow_lossy: bool,
    /// Replace an existing output file.
    #[arg(long)]
    overwrite: bool,
    /// Keep curved and follow-the-leader moves exact by adding derived subsets at every
    /// count where a performer changes direction (changes the page structure).
    #[arg(long)]
    preserve_curves: bool,
}

#[derive(Args, Default)]
struct SourceArgs {
    /// JSON list of {"name", "start_count", "is_subset"} replacing the file's set table.
    #[arg(long, value_name = "FILE")]
    page_map: Option<PathBuf>,
    /// Mirror the drill side to side (source +x is the audience's left).
    #[arg(long)]
    flip_x: bool,
    /// Mirror the drill front to back (source +y points toward the audience).
    #[arg(long)]
    flip_y: bool,
    /// Override the marching steps per source unit (default from the GRID record, 1.6 for 8-to-5 yards).
    #[arg(long, value_name = "STEPS")]
    steps_per_unit: Option<f64>,
}

struct Ui {
    verbose: bool,
    quiet: bool,
    json: bool,
}

impl Ui {
    fn info(&self, msg: impl AsRef<str>) {
        if !self.quiet && !self.json {
            eprintln!("{}", msg.as_ref());
        }
    }

    fn diagnostics(&self, diags: &[Diagnostic]) {
        if self.quiet || self.json {
            return;
        }
        for d in diags {
            if self.verbose || d.severity >= Severity::Warning {
                eprintln!("  {d}");
            }
        }
    }
}

struct Failure {
    code: u8,
    kind: &'static str,
    message: String,
    diagnostics: Vec<Diagnostic>,
}

impl From<DecodeError> for Failure {
    fn from(e: DecodeError) -> Self {
        let code = match e {
            DecodeError::Io(_) => exit::IO,
            _ => exit::INVALID_INPUT,
        };
        Failure { code, kind: e.code(), message: e.to_string(), diagnostics: vec![] }
    }
}

impl From<ExportError> for Failure {
    fn from(e: ExportError) -> Self {
        let code = match &e {
            ExportError::Refused { .. } => exit::REFUSED_LOSSY,
            ExportError::Exists(_) => exit::OUTPUT_EXISTS,
            ExportError::Validation(_) => exit::VALIDATION_FAILED,
            ExportError::Unsupported(_) => exit::INVALID_INPUT,
            ExportError::Io(_) => exit::IO,
            ExportError::Database(_) => exit::FAILURE,
        };
        let diagnostics = match &e {
            ExportError::Refused { refused, .. } => refused.clone(),
            _ => vec![],
        };
        Failure { code, kind: e.code(), message: e.to_string(), diagnostics }
    }
}

fn io_failure(path: &Path, e: std::io::Error) -> Failure {
    Failure { code: exit::IO, kind: "io", message: format!("{}: {e}", path.display()), diagnostics: vec![] }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ui = Ui { verbose: cli.verbose, quiet: cli.quiet, json: cli.json };
    match run(cli.command, &ui) {
        Ok(value) => {
            if ui.json {
                println!("{}", serde_json::to_string_pretty(&json!({"ok": true, "result": value})).unwrap());
            }
            ExitCode::from(exit::OK)
        }
        Err(f) => {
            if ui.json {
                let out = json!({"ok": false, "error": {"code": f.kind, "message": f.message, "diagnostics": f.diagnostics}});
                println!("{}", serde_json::to_string_pretty(&out).unwrap());
            } else {
                eprintln!("error: {}", f.message);
                for d in &f.diagnostics {
                    eprintln!("  {d}");
                }
                if f.code == exit::REFUSED_LOSSY {
                    eprintln!("hint: review the warnings above; pass --allow-lossy to accept approximations, or drop --strict");
                }
            }
            ExitCode::from(f.code)
        }
    }
}

fn load(input: &Path, args: &SourceArgs, ui: &Ui) -> Result<DrillProject, Failure> {
    let bytes = std::fs::read(input).map_err(|e| io_failure(input, e))?;
    let format = detect(&bytes);
    if format != FileFormat::ThreeDu {
        let hint = match input.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("3dudb") => " (.3dudb databases are a different format and are not supported yet)",
            _ => "",
        };
        return Err(Failure { code: exit::INVALID_INPUT, kind: "not_3du", message: format!("input is {}, not a .3du drill{hint}", format.describe()), diagnostics: vec![] });
    }
    let page_map = match &args.page_map {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| io_failure(path, e))?;
            let map: Vec<PageMapEntry> = serde_json::from_str(&text)
                .map_err(|e| Failure { code: exit::INVALID_INPUT, kind: "bad_page_map", message: format!("page map: {e}"), diagnostics: vec![] })?;
            Some(map)
        }
        None => None,
    };
    let convert = ConvertOptions {
        transform: TransformOverrides { steps_per_unit: args.steps_per_unit, flip_x: args.flip_x, flip_y: args.flip_y, front_y: None, center_x: None },
        page_map,
    };
    let show_progress = !ui.quiet && !ui.json && bytes.len() > 2 << 20 && std::io::stderr().is_terminal();
    let mut last = 0;
    let mut cb = |done: usize, total: usize| {
        let pct = done * 100 / total.max(1);
        if pct >= last + 10 {
            last = pct;
            eprint!("\rdecoding… {pct:3}%");
            if done == total {
                eprintln!();
            }
        }
        true
    };
    let opts = ReadOptions { convert, progress: if show_progress { Some(&mut cb) } else { None }, ..Default::default() };
    Ok(read_bytes(&bytes, opts)?)
}

fn run(command: Command, ui: &Ui) -> Result<serde_json::Value, Failure> {
    match command {
        Command::Inspect { input, reveal, source } => {
            let p = load(&input, &source, ui)?;
            let summary = p.summary();
            let mut value = serde_json::to_value(&summary).unwrap();
            if reveal {
                value["title"] = json!(p.metadata.title);
                value["sets"] = json!(p.sets.iter().map(|s| json!({"name": s.name, "start_count": s.start_count, "subset": s.is_subset, "measures": s.measures_text})).collect::<Vec<_>>());
                value["labels"] = json!(p.performers.iter().map(|q| &q.label).collect::<Vec<_>>());
                value["music"] = json!(p.metadata.music.as_ref().map(|m| &m.filename));
            }
            if !ui.json && !ui.quiet {
                let s = &summary;
                println!("format:           {}", s.format);
                println!("written by:       version {}", s.writer_version);
                println!("size:             {} bytes", s.file_size);
                println!("performers:       {} (+ {} props)", s.performers, s.props);
                println!("counts:           {} stored, drill ends at count {}", s.counts_stored, s.drill_end_count);
                println!("sets:             {} sets + {} subsets", s.sets, s.subsets);
                println!("positions:        {} samples", s.position_samples);
                println!("tempo changes:    {}", s.tempo_changes);
                println!("measure markers:  {}", s.measure_markers);
                println!("curved moves:     {}", s.nonlinear_transitions);
                println!("annotations:      {} text, {} lines", s.text_annotations, s.line_annotations);
                println!("audio:            {}", if s.embedded_audio { "embedded" } else if s.has_music_reference { "file name only (no audio data)" } else { "none" });
                println!("units:            {} ({} in per step)", s.source_unit, s.step_inches);
                if reveal {
                    println!("title:            {}", p.metadata.title);
                    for set in &p.sets {
                        println!("  set {:>5} at count {:>4}{}  {}", set.name, set.start_count, if set.is_subset { " (subset)" } else { "" }, set.measures_text);
                    }
                }
                if !s.diagnostics.is_empty() {
                    println!("diagnostics:");
                    for d in &s.diagnostics {
                        println!("  {d}");
                    }
                }
            }
            Ok(value)
        }
        Command::Extract { input, output, overwrite, source } => {
            let p = load(&input, &source, ui)?;
            write_json(&p, &output, overwrite)?;
            ui.info(format!("wrote intermediate JSON to {}", output.display()));
            ui.diagnostics(&p.diagnostics);
            Ok(json!({"output": output, "diagnostics": p.diagnostics}))
        }
        Command::Align { input, source, audio } => {
            let p = load(&input, &source, ui)?;
            if audio.audio.is_none() {
                return Err(Failure { code: exit::INVALID_INPUT, kind: "missing_audio", message: "align needs --audio FILE".into(), diagnostics: vec![] });
            }
            let sync = audio_sync(&p, &audio, ui, true)?;
            if !ui.json && !ui.quiet {
                println!("audio:            {:.3} s", sync.audio_seconds);
                for (k, v) in &sync.notes {
                    println!("{:<18}{v}", format!("{k}:"));
                }
                if let Some(times) = &sync.tracked {
                    println!("tracked beats vs. the drill clock at each set:");
                    let beats = three_du_audio::drill_beats(&p);
                    for (i, s) in p.sets.iter().enumerate().skip(1) {
                        let k = s.start_count as usize - 1;
                        println!("  set #{i:<3} count {:>4}  drill {:8.3} s  audio {:8.3} s  shift {:+.3} s", s.start_count, beats[k], times[k], times[k] - beats[k]);
                    }
                }
            }
            ui.diagnostics(&sync.diagnostics);
            Ok(json!({"notes": sync.notes, "tracked_beats": sync.tracked, "diagnostics": sync.diagnostics}))
        }
        Command::Convert { input, format, output, policy, source, audio } => {
            let mut p = load(&input, &source, ui)?;
            let loss = if policy.strict {
                LossPolicy::Strict
            } else if policy.allow_lossy {
                LossPolicy::AllowLossy
            } else {
                LossPolicy::Default
            };
            if p.sets.is_empty() && policy.allow_lossy && !matches!(format, OutputFormat::Json) {
                infer_sets_from_motion(&mut p);
            }
            if policy.preserve_curves {
                let added = split_curved_moves(&mut p);
                ui.info(format!("added {added} derived subsets to keep curved moves exact"));
            }
            ui.info(format!("converting {} performers, {} sets…", p.performers.len(), p.sets.len()));
            let diags = match format {
                OutputFormat::Dots => three_du_openmarch::export_dots(&p, &output, loss, policy.overwrite)?,
                OutputFormat::Om => three_du_openmarch::export_om(&p, &output, false, loss, policy.overwrite)?,
                OutputFormat::Omz => three_du_openmarch::export_om(&p, &output, true, loss, policy.overwrite)?,
                OutputFormat::Fieldwork => three_du_fieldwork::export_fieldwork(&p, &output, loss, policy.overwrite)?,
                OutputFormat::Json => {
                    three_du_core::export::enforce(loss, &p.diagnostics)?;
                    write_json(&p, &output, policy.overwrite)?;
                    p.diagnostics.clone()
                }
                OutputFormat::Musicxml => {
                    let sync = audio_sync(&p, &audio, ui, false)?;
                    let opts = three_du_musicxml::MusicXmlOptions {
                        audio: audio.audio.as_ref().map(|a| three_du_musicxml::AudioInfo {
                            file_name: a.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                            duration_seconds: sync.audio_seconds,
                        }),
                        offset_seconds: sync.offset,
                        beat_times: if audio.follow_audio { sync.tracked.clone() } else { None },
                        notes: sync.notes.clone(),
                    };
                    let mut diags = p.diagnostics.clone();
                    diags.extend(sync.diagnostics.iter().cloned());
                    let xml = three_du_musicxml::build(&p, &opts, &mut diags).map_err(ExportError::Unsupported)?;
                    three_du_core::export::enforce(loss, &diags)?;
                    write_atomic(&output, xml.as_bytes(), policy.overwrite, |tmp| {
                        three_du_musicxml::check_well_formed(&std::fs::read_to_string(tmp)?).map_err(ExportError::Validation)
                    })?;
                    diags
                }
            };
            ui.info(format!("wrote and validated {}", output.display()));
            ui.diagnostics(&diags);
            Ok(json!({"output": output, "diagnostics": diags}))
        }
        Command::Validate { input } => {
            let bytes = std::fs::read(&input).map_err(|e| io_failure(&input, e))?;
            let format = detect(&bytes);
            let report = match format {
                FileFormat::Sqlite => serde_json::to_value(three_du_openmarch::dots::validate(&input)?).unwrap(),
                FileFormat::Fieldwork => serde_json::to_value(three_du_fieldwork::validate_bytes(&bytes)?).unwrap(),
                FileFormat::OpenMarchJson | FileFormat::Gzip => {
                    let show = three_du_openmarch::om::parse(&bytes).map_err(ExportError::Validation)?;
                    three_du_openmarch::om::validate(&show).map_err(ExportError::Validation)?;
                    json!({"omSchemaVersion": show.om_schema_version, "performers": show.performers.len(), "pages": show.pages.len(), "coordinates": show.coordinates.len()})
                }
                FileFormat::ThreeDu => {
                    let p = read_bytes(&bytes, ReadOptions::default())?;
                    serde_json::to_value(p.summary()).unwrap()
                }
                other => {
                    return Err(Failure { code: exit::INVALID_INPUT, kind: "unsupported", message: format!("cannot validate {}", other.describe()), diagnostics: vec![] });
                }
            };
            if !ui.json && !ui.quiet {
                println!("valid {}", format.describe());
                if let Some(obj) = report.as_object() {
                    for (k, v) in obj {
                        if !v.is_array() {
                            println!("  {k}: {v}");
                        }
                    }
                }
            }
            Ok(json!({"format": format, "report": report}))
        }
    }
}

struct AudioSync {
    audio_seconds: f64,
    /// Audio time of count 0 used for drill-clock timing.
    offset: f64,
    /// Audio time of every beat when beat tracking ran.
    tracked: Option<Vec<f64>>,
    notes: Vec<(String, String)>,
    diagnostics: Vec<Diagnostic>,
}

/// Reads the recording (if any) and decides how the drill clock maps onto it. An offset
/// found automatically is applied only when the alignment is confident.
fn audio_sync(p: &DrillProject, args: &AudioArgs, ui: &Ui, always_track: bool) -> Result<AudioSync, Failure> {
    let mut sync = AudioSync { audio_seconds: 0.0, offset: args.audio_offset.unwrap_or(0.0), tracked: None, notes: vec![], diagnostics: vec![] };
    let Some(path) = &args.audio else {
        if args.audio_offset.is_none() {
            sync.diagnostics.push(Diagnostic::new(Severity::Info, "no_audio", "no recording given; count 0 is placed at time 0"));
        }
        return Ok(sync);
    };
    let audio = three_du_audio::read_file(path)
        .map_err(|e| Failure { code: exit::INVALID_INPUT, kind: "bad_audio", message: format!("{}: {e}", path.display()), diagnostics: vec![] })?;
    sync.audio_seconds = audio.duration_seconds();
    ui.info(format!("aligning with {:.1} s of audio…", sync.audio_seconds));
    let global = three_du_audio::align_project(p, &audio);
    sync.notes.push(("alignment-estimate-seconds".into(), format!("{:.4}", global.offset_seconds)));
    sync.notes.push(("alignment-confident".into(), global.confident.to_string()));
    sync.notes.push(("alignment-beat-hits".into(), format!("{:.2} (chance {:.2})", global.beat_hit_rate, global.chance_hit_rate)));
    match args.audio_offset {
        Some(o) => sync.notes.push(("audio-offset-seconds".into(), format!("{o:.4} (given)"))),
        None if global.confident => {
            sync.offset = global.offset_seconds;
            sync.notes.push(("audio-offset-seconds".into(), format!("{:.4} (automatic)", global.offset_seconds)));
        }
        None => {
            sync.notes.push(("audio-offset-seconds".into(), "0 (estimate not confident, not applied)".into()));
            sync.diagnostics.push(Diagnostic::new(
                Severity::Warning,
                "alignment_not_confident",
                format!(
                    "automatic alignment is not confident (best offset {:+.3} s); count 0 is placed at the start of the audio. Check with --click-check, then pass --audio-offset or --follow-audio",
                    global.offset_seconds
                ),
            ));
        }
    }
    if args.follow_audio || always_track {
        let beats = three_du_audio::drill_beats(p);
        let track = three_du_audio::align::track_beats(&audio, &beats, args.audio_offset.unwrap_or(global.offset_seconds), 2.5);
        sync.notes.push((
            "beat-tracking".into(),
            format!(
                "onset rate {:.2} (chance {:.2}), median tempo deviation {:.1}%, confident {}",
                track.onset_rate,
                track.chance_onset_rate,
                (track.median_tempo_deviation - 1.0) * 100.0,
                track.confident
            ),
        ));
        if args.follow_audio && !track.confident {
            sync.diagnostics.push(Diagnostic::new(
                Severity::Lossy,
                "beat_tracking_not_confident",
                "beat tracking could not be confirmed against chance; the followed tempos may be wrong. Verify with --click-check",
            ));
        }
        sync.tracked = Some(track.times);
    }
    if let Some(out) = &args.click_check {
        let times: Vec<f64> = match &sync.tracked {
            Some(t) if args.follow_audio || always_track => t.clone(),
            _ => three_du_audio::drill_beats(p).iter().map(|b| b + sync.offset).collect(),
        };
        let accents: Vec<usize> = p.sets.iter().filter(|s| s.start_count >= 1).map(|s| s.start_count as usize - 1).collect();
        let mix = three_du_audio::align::click_check(&audio, &times, &accents);
        write_atomic(out, &three_du_audio::wav::write_pcm16(&mix, audio.sample_rate), true, |_| Ok(()))?;
        ui.info(format!("wrote click check to {} (accented clicks mark sets)", out.display()));
    }
    Ok(sync)
}

fn write_json(p: &DrillProject, output: &Path, overwrite: bool) -> Result<(), Failure> {
    let text = three_du_core::to_intermediate_json(p).expect("model serializes");
    write_atomic(output, text.as_bytes(), overwrite, |tmp| {
        let back = std::fs::read_to_string(tmp)?;
        three_du_core::from_intermediate_json(&back).map(|_| ()).map_err(|e| ExportError::Validation(e.to_string()))
    })?;
    Ok(())
}
