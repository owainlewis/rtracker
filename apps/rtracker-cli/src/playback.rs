//! `rtracker loop PATTERN.json` — plays a pattern in a loop with file-watch
//! hot-reload. Headless companion to the TUI; useful when you want to edit in
//! your own editor without a UI in the way.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use rtracker_core::Pattern;

use crate::engine::AudioEngine;

pub fn run(input: PathBuf) -> Result<()> {
    let initial = render_pattern_at(&input, 48000)?;
    let engine = AudioEngine::start(initial.0)?;
    // Re-render at the engine's actual device sample rate.
    let re = render_pattern_at(&input, engine.device_sr)?;
    engine.swap_buffer(re.0);

    println!(
        "rtracker loop — {} @ {} Hz on {}. Save to reload, Ctrl-C to stop.",
        input.display(),
        engine.device_sr,
        engine.device_name
    );

    let mut last_mtime = std::fs::metadata(&input).and_then(|m| m.modified()).ok();
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let now = std::fs::metadata(&input).and_then(|m| m.modified()).ok();
        if now.is_some() && now != last_mtime {
            last_mtime = now;
            match render_pattern_at(&input, engine.device_sr) {
                Ok((buf, events)) => {
                    engine.swap_buffer(buf);
                    tracing::info!(events, "reloaded");
                }
                Err(e) => tracing::warn!(error = %e, "reload failed, keeping previous buffer"),
            }
        }
    }
}

/// Returns (rendered stereo buffer, event count).
pub fn render_pattern_at(path: &Path, target_sr: u32) -> Result<(Vec<f32>, usize)> {
    let text = std::fs::read_to_string(path).context("read pattern")?;
    let mut pat: Pattern = serde_json::from_str(&text).context("parse pattern")?;
    pat.sample_rate = target_sr;
    let piece = pat.compile().context("compile pattern")?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let rendered = rtracker_render::render_with_dir(&piece, base).context("render piece")?;
    let n = piece.events.len();
    Ok((rendered, n))
}

pub fn read_pattern(path: &Path) -> Result<Pattern> {
    let text = std::fs::read_to_string(path).context("read pattern")?;
    let pat: Pattern = serde_json::from_str(&text).context("parse pattern")?;
    Ok(pat)
}

pub fn file_mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}
