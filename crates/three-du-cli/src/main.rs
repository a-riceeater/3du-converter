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
        Command::Convert { input, format, output, policy, source } => {
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

fn write_json(p: &DrillProject, output: &Path, overwrite: bool) -> Result<(), Failure> {
    let text = three_du_core::to_intermediate_json(p).expect("model serializes");
    write_atomic(output, text.as_bytes(), overwrite, |tmp| {
        let back = std::fs::read_to_string(tmp)?;
        three_du_core::from_intermediate_json(&back).map(|_| ()).map_err(|e| ExportError::Validation(e.to_string()))
    })?;
    Ok(())
}
