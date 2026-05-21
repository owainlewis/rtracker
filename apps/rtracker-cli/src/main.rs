use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
mod engine;
mod playback;
mod tui;

use rtracker_core::{Pattern, Piece};

#[derive(Parser)]
#[command(name = "rtracker", about = "Constraint-driven generative music renderer")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Render a piece JSON to a stereo WAV.
    Render {
        input: PathBuf,
        output: PathBuf,
        /// Override the piece's sample rate (advisory; the piece's own rate is used unless this is set).
        #[arg(long)]
        sample_rate: Option<u32>,
    },
    /// Validate a piece JSON without rendering.
    Validate { input: PathBuf },
    /// Compile a pattern JSON to a piece JSON (no audio output).
    CompilePattern {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, default_value_t = 1)]
        loops: u32,
    },
    /// Compile a pattern and render it to WAV in one step.
    RenderPattern {
        input: PathBuf,
        output: PathBuf,
        #[arg(long, default_value_t = 1)]
        loops: u32,
    },
    /// Play a pattern in a loop, hot-reloading on file change.
    Loop { input: PathBuf },
    /// Open the tracker TUI for the given pattern.
    Tui { input: PathBuf },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Render { input, output, sample_rate } => render_cmd(input, output, sample_rate),
        Cmd::Validate { input } => validate_cmd(input),
        Cmd::CompilePattern { input, output, loops } => compile_pattern_cmd(input, output, loops),
        Cmd::RenderPattern { input, output, loops } => render_pattern_cmd(input, output, loops),
        Cmd::Loop { input } => playback::run(input),
        Cmd::Tui { input } => tui::run(input),
    }
}

fn load_pattern(path: &PathBuf) -> Result<Pattern> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let pat: Pattern = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(pat)
}

fn compile_pattern_cmd(input: PathBuf, output: PathBuf, loops: u32) -> Result<()> {
    let pat = load_pattern(&input)?;
    let piece = pat.compile_repeated(loops).context("compile pattern")?;
    let text = serde_json::to_string_pretty(&piece)?;
    fs::write(&output, text)?;
    tracing::info!(
        rows = pat.rows,
        tempo_bpm = pat.tempo_bpm,
        events = piece.events.len(),
        loops,
        path = %output.display(),
        "wrote piece"
    );
    Ok(())
}

fn render_pattern_cmd(input: PathBuf, output: PathBuf, loops: u32) -> Result<()> {
    let pat = load_pattern(&input)?;
    let piece = pat.compile_repeated(loops).context("compile pattern")?;
    tracing::info!(
        rows = pat.rows,
        tempo_bpm = pat.tempo_bpm,
        events = piece.events.len(),
        loops,
        "rendering pattern"
    );
    let base = input.parent().unwrap_or_else(|| std::path::Path::new("."));
    let buf = rtracker_render::render_with_dir(&piece, base)?;
    rtracker_render::write_stereo_f32(&output, piece.sample_rate, &buf)?;
    tracing::info!(path = %output.display(), "wrote wav");
    Ok(())
}

fn load_piece(path: &PathBuf) -> Result<Piece> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let piece: Piece = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(piece)
}

fn render_cmd(input: PathBuf, output: PathBuf, sample_rate: Option<u32>) -> Result<()> {
    let mut piece = load_piece(&input)?;
    if let Some(sr) = sample_rate {
        piece.sample_rate = sr;
    }
    piece.validate().context("piece failed validation")?;
    tracing::info!(
        sample_rate = piece.sample_rate,
        duration_samples = piece.duration_samples,
        events = piece.events.len(),
        "rendering"
    );
    let base = input.parent().unwrap_or_else(|| std::path::Path::new("."));
    let buf = rtracker_render::render_with_dir(&piece, base)?;
    rtracker_render::write_stereo_f32(&output, piece.sample_rate, &buf)?;
    tracing::info!(path = %output.display(), "wrote wav");
    Ok(())
}

fn validate_cmd(input: PathBuf) -> Result<()> {
    let piece = load_piece(&input)?;
    piece.validate()?;
    println!("ok: {} events, {} samples", piece.events.len(), piece.duration_samples);
    Ok(())
}
