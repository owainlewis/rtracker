use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
mod engine;
mod mod_import;
mod playback;
mod slice;
mod tui;

use rtracker_core::{Pattern, Piece, Song};

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
    /// Compile a song JSON (an array of patterns, run in order) to a piece JSON.
    CompileSong { input: PathBuf, output: PathBuf },
    /// Compile a song and render it to WAV in one step.
    RenderSong { input: PathBuf, output: PathBuf },
    /// Chop a WAV into slices and emit a tracker pattern (one track per slice).
    Slice {
        input: PathBuf,
        output: PathBuf,
        /// Number of equal slices (ignored with --transient).
        #[arg(long, default_value_t = 16)]
        slices: u32,
        /// Cut on detected onsets (drum hits) instead of even spacing.
        #[arg(long)]
        transient: bool,
        /// Onset sensitivity for --transient (higher = fewer slices).
        #[arg(long, default_value_t = 1.6)]
        threshold: f32,
        /// Minimum spacing between onsets in ms for --transient.
        #[arg(long, default_value_t = 40.0)]
        min_gap_ms: f32,
        /// Force grid tempo (default: slices play back gaplessly).
        #[arg(long)]
        bpm: Option<f32>,
        #[arg(long, default_value_t = 4)]
        lines_per_beat: u32,
        /// Piece sample rate (default: the WAV's native rate).
        #[arg(long)]
        sample_rate: Option<u32>,
    },
    /// Import a simple 4-channel ProTracker MOD as a pattern JSON plus WAV samples.
    ImportMod {
        input: PathBuf,
        output: PathBuf,
        /// Directory to write extracted sample WAVs. Defaults to <output_stem>_samples beside output.
        #[arg(long)]
        samples_dir: Option<PathBuf>,
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
        Cmd::CompileSong { input, output } => compile_song_cmd(input, output),
        Cmd::RenderSong { input, output } => render_song_cmd(input, output),
        Cmd::Slice {
            input, output, slices, transient, threshold, min_gap_ms, bpm, lines_per_beat, sample_rate,
        } => slice_cmd(
            input, output,
            slice::SliceOpts { slices, transient, threshold, min_gap_ms, bpm, lines_per_beat, sample_rate },
        ),
        Cmd::ImportMod { input, output, samples_dir } => import_mod_cmd(input, output, samples_dir),
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

fn load_song(path: &PathBuf) -> Result<Song> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let song: Song = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(song)
}

fn compile_song_cmd(input: PathBuf, output: PathBuf) -> Result<()> {
    let song = load_song(&input)?;
    let piece = song.compile().context("compile song")?;
    let text = serde_json::to_string_pretty(&piece)?;
    fs::write(&output, text)?;
    tracing::info!(
        patterns = song.patterns.len(),
        events = piece.events.len(),
        duration_samples = piece.duration_samples,
        path = %output.display(),
        "wrote piece"
    );
    Ok(())
}

fn render_song_cmd(input: PathBuf, output: PathBuf) -> Result<()> {
    let song = load_song(&input)?;
    let piece = song.compile().context("compile song")?;
    tracing::info!(
        patterns = song.patterns.len(),
        events = piece.events.len(),
        duration_samples = piece.duration_samples,
        "rendering song"
    );
    let base = input.parent().unwrap_or_else(|| std::path::Path::new("."));
    let buf = rtracker_render::render_with_dir(&piece, base)?;
    rtracker_render::write_stereo_f32(&output, piece.sample_rate, &buf)?;
    tracing::info!(path = %output.display(), "wrote wav");
    Ok(())
}

fn slice_cmd(input: PathBuf, output: PathBuf, opts: slice::SliceOpts) -> Result<()> {
    let pat = slice::slice_to_pattern(&input, &output, &opts).context("slice wav")?;
    tracing::info!(
        slices = pat.tracks.len(),
        rows = pat.rows,
        tempo_bpm = pat.tempo_bpm,
        mode = if opts.transient { "transient" } else { "equal" },
        path = %output.display(),
        "wrote sliced pattern"
    );
    Ok(())
}

fn import_mod_cmd(input: PathBuf, output: PathBuf, samples_dir: Option<PathBuf>) -> Result<()> {
    let pat = mod_import::import_mod_to_pattern(&input, &output, samples_dir).context("import mod")?;
    tracing::info!(
        rows = pat.rows,
        tracks = pat.tracks.len(),
        samples = pat.samples.len(),
        path = %output.display(),
        "imported mod"
    );
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
