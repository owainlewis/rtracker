//! `rtracker loop PATTERN.json` — plays a pattern in a loop with file-watch
//! hot-reload. Headless companion to the TUI; useful when you want to edit in
//! your own editor without a UI in the way.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use rtracker_core::{Pattern, Song, SongMetadata};

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

/// Load a file as a `Song`, transparently wrapping a bare single-pattern file.
/// Returns `(song, was_song_format)` so the editor knows which format to save
/// back. Detection is unambiguous: a `Song` requires a `patterns` array, a
/// `Pattern` requires `tempo_bpm`/`rows`/etc — neither parses as the other.
pub fn read_song(path: &Path) -> Result<(Song, bool)> {
    let text = std::fs::read_to_string(path).context("read song")?;
    if let Ok(song) = serde_json::from_str::<Song>(&text) {
        if !song.patterns.is_empty() {
            return Ok((song, true));
        }
    }
    let pat: Pattern = serde_json::from_str(&text)
        .context("file is neither a song (patterns array) nor a pattern")?;
    Ok((Song { patterns: vec![pat], metadata: SongMetadata::default() }, false))
}

pub fn file_mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str, body: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("rtracker_pb_{}_{}_{}", std::process::id(), nanos, name));
        std::fs::write(&p, body).unwrap();
        p
    }

    const PATTERN_JSON: &str = r#"{
        "sample_rate": 48000, "tempo_bpm": 160, "lines_per_beat": 4, "rows": 16,
        "voices": {"k": {"kind": "sine", "default_pan": 0.0}},
        "tracks": [{"name":"k","voice":"k","default_amp":0.8,"default_pan":0.0,
            "default_envelope":{"kind":"gate"},"default_duration_rows":1.0,
            "cells":[{"row":0,"note":50.0}]}]
    }"#;

    #[test]
    fn read_song_wraps_bare_pattern() {
        let p = tmp("pat.json", PATTERN_JSON);
        let (song, was_song) = read_song(&p).expect("read");
        assert!(!was_song, "bare pattern must not report song format");
        assert_eq!(song.patterns.len(), 1);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn read_song_reads_song_format() {
        let body = format!(r#"{{"patterns": [{}, {}]}}"#, PATTERN_JSON, PATTERN_JSON);
        let p = tmp("song.json", &body);
        let (song, was_song) = read_song(&p).expect("read");
        assert!(was_song, "patterns array must report song format");
        assert_eq!(song.patterns.len(), 2);
        let _ = std::fs::remove_file(p);
    }
}
