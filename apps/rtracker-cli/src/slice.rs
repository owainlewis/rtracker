//! WAV slicer — chop one sample into slices and emit a tracker `Pattern`.
//!
//! This is the automated version of cutting a break into pieces by hand. It
//! never touches the audio file: each slice is just a `(start, end)` offset into
//! the source WAV (a `SampleRef`), so it's non-destructive and reversible. Two
//! ways to place the cuts:
//!
//! - **Equal division** (`--slices N`): N evenly-spaced cuts. Ideal for
//!   breakbeats, which are metrically regular — slice an Amen into 16 and each
//!   slice is a 16th note. The grid tempo defaults so the slices play back
//!   gaplessly (one slice == one row).
//! - **Transient detection** (`--transient`): find onsets (drum hits) by
//!   short-time energy and cut there, so slice boundaries land on the attacks.
//!
//! Either way the output is a `Pattern` with one voice + one track per slice,
//! laid out in source order: render it straight back to hear the original, or
//! open it in the TUI and rearrange.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rtracker_core::{
    Cell, Envelope, Note, Pattern, PatternMetadata, SampleLoopMode, SampleRef, Track, VoiceDef,
};

pub struct SliceOpts {
    /// Number of equal slices (ignored when `transient` is set).
    pub slices: u32,
    /// Cut on detected onsets instead of even spacing.
    pub transient: bool,
    /// Onset sensitivity: a hop must exceed this multiple of the local average
    /// energy to count as an attack. Higher = fewer slices.
    pub threshold: f32,
    /// Minimum spacing between onsets, milliseconds (debounces double-triggers).
    pub min_gap_ms: f32,
    /// Force the grid tempo. Default = the natural tempo at which slices play
    /// back-to-back with no gap.
    pub bpm: Option<f32>,
    pub lines_per_beat: u32,
    /// Piece sample rate. Default = the WAV's native rate (so it plays at the
    /// original pitch, since the renderer does not resample).
    pub sample_rate: Option<u32>,
}

impl Default for SliceOpts {
    fn default() -> Self {
        Self {
            slices: 16,
            transient: false,
            threshold: 1.6,
            min_gap_ms: 40.0,
            bpm: None,
            lines_per_beat: 4,
            sample_rate: None,
        }
    }
}

/// Slice `input` and write the resulting pattern JSON to `output`.
pub fn slice_to_pattern(input: &Path, output: &Path, opts: &SliceOpts) -> Result<Pattern> {
    let (mono, native_sr) = read_wav_mono(input)?;
    let n = mono.len() as u64;
    if n == 0 {
        bail!("input WAV is empty");
    }
    let sr = opts.sample_rate.unwrap_or(native_sr);
    let lpb = opts.lines_per_beat.max(1);

    // Slice boundaries as (start, end) offsets into the file's mono frames.
    let bounds = if opts.transient {
        let onsets = detect_onsets(&mono, native_sr, opts.threshold, opts.min_gap_ms);
        bounds_from_onsets(&onsets, n)
    } else {
        equal_bounds(n, opts.slices.max(1))
    };

    // Grid. Without an explicit BPM, pick samples-per-row so slices abut: the
    // mean slice length for equal cuts is exact; for transient cuts it's the
    // average, which keeps the quantised layout close to the real timing.
    let spr = match opts.bpm {
        Some(bpm) if bpm > 0.0 => ((sr as f64 * 60.0) / (bpm as f64 * lpb as f64)).round() as u64,
        _ => {
            let mean = (n as f64 / bounds.len() as f64).round() as u64;
            mean.max(1)
        }
    };
    let spr = spr.max(1);
    let bpm = (sr as f64 * 60.0) / (spr as f64 * lpb as f64);

    // Place each slice on the grid: equal cuts land one-per-row in order;
    // transient cuts quantise to the nearest row of their onset.
    let rows_needed = ((n as f64) / spr as f64).ceil() as u32;
    let rel = relativize(output_dir(output), input);

    let mut voices: HashMap<String, VoiceDef> = HashMap::new();
    let mut samples: HashMap<String, SampleRef> = HashMap::new();
    let mut tracks: Vec<Track> = Vec::with_capacity(bounds.len());

    let mut max_row = 0u32;
    for (i, &(start, end)) in bounds.iter().enumerate() {
        let id = format!("slice_{i:02}");
        voices.insert(
            id.clone(),
            VoiceDef::Sample { sample_id: id.clone(), loop_mode: SampleLoopMode::OneShot },
        );
        samples.insert(
            id.clone(),
            SampleRef { path: rel.clone(), start_sample: start, end_sample: end, label: Some(id.clone()) },
        );

        let row = if opts.transient {
            ((start as f64 / spr as f64).round() as u32).min(rows_needed.saturating_sub(1))
        } else {
            i as u32
        };
        max_row = max_row.max(row);
        let dur_rows = (((end - start) as f64 / spr as f64).round() as f32).max(1.0);

        tracks.push(Track {
            name: format!("slice {i:02}"),
            instrument: None,
            voice: Some(id),
            default_amp: Some(0.9),
            default_pan: Some(0.0),
            default_envelope: Some(Envelope::Gate),
            default_fx: Some(vec![]),
            default_duration_rows: Some(dur_rows),
            default_pitch_env: None,
            cells: vec![Cell {
                row,
                note: Note::Hz(1.0),
                pitch_ratio: Some(1.0),
                velocity: None,
                duration_rows: None,
                pan: None,
                fx: vec![],
                pitch_env: None,
            }],
        });
    }

    let rows = (max_row + 1).max(rows_needed).max(1);
    let pattern = Pattern {
        sample_rate: sr,
        tempo_bpm: bpm as f32,
        lines_per_beat: lpb,
        rows,
        voices,
        samples,
        tracks,
        metadata: PatternMetadata {
            title: input.file_stem().and_then(|s| s.to_str()).map(|s| format!("{s} (sliced)")),
            author: Some("rtracker slice".into()),
        },
    };

    // Compile once so a malformed result fails here, not at render time.
    pattern.compile().context("compiled sliced pattern failed validation")?;

    let text = serde_json::to_string_pretty(&pattern)?;
    std::fs::write(output, text).with_context(|| format!("writing {}", output.display()))?;
    Ok(pattern)
}

/// N evenly-sized (start, end) frame ranges spanning `[0, n)`. The final slice
/// absorbs any rounding remainder so the whole file is covered.
fn equal_bounds(n: u64, slices: u32) -> Vec<(u64, u64)> {
    let slices = slices.max(1) as u64;
    let base = n / slices;
    if base == 0 {
        return vec![(0, n)];
    }
    (0..slices)
        .map(|i| {
            let start = i * base;
            let end = if i == slices - 1 { n } else { (i + 1) * base };
            (start, end)
        })
        .collect()
}

fn bounds_from_onsets(onsets: &[u64], n: u64) -> Vec<(u64, u64)> {
    if onsets.is_empty() {
        return vec![(0, n)];
    }
    let mut out = Vec::with_capacity(onsets.len());
    for i in 0..onsets.len() {
        let start = onsets[i];
        let end = onsets.get(i + 1).copied().unwrap_or(n);
        if end > start {
            out.push((start, end));
        }
    }
    out
}

/// Energy-based onset detector. Returns slice-start frames (always starting at
/// 0). Cheap and robust enough for percussive material: it tracks short-time
/// energy and marks a rising local peak that jumps above the recent average.
fn detect_onsets(mono: &[f32], sr: u32, threshold: f32, min_gap_ms: f32) -> Vec<u64> {
    const HOP: usize = 256;
    const WIN: usize = 512;
    const AVG_HOPS: usize = 16; // moving-average baseline window
    let n = mono.len();
    if n < WIN {
        return vec![0];
    }

    let mut env = Vec::new();
    let mut k = 0;
    while k + WIN <= n {
        let e: f32 = mono[k..k + WIN].iter().map(|x| x * x).sum::<f32>() / WIN as f32;
        env.push(e);
        k += HOP;
    }

    let min_gap_hops = (((min_gap_ms / 1000.0) * sr as f32) / HOP as f32).max(1.0) as isize;
    let mut onsets = vec![0u64];
    let mut last: isize = -min_gap_hops;
    for i in 1..env.len() {
        let lo = i.saturating_sub(AVG_HOPS);
        let base = env[lo..i].iter().sum::<f32>() / (i - lo).max(1) as f32;
        let rising = env[i] > env[i - 1];
        let loud = env[i] > threshold * base.max(1e-9);
        let is_peak = i + 1 >= env.len() || env[i] >= env[i + 1];
        if rising && loud && is_peak && (i as isize - last) >= min_gap_hops {
            onsets.push((i * HOP) as u64);
            last = i as isize;
        }
    }
    onsets.dedup();
    onsets
}

/// Read a WAV as mono f32 (channels averaged) plus its native sample rate.
fn read_wav_mono(path: &Path) -> Result<(Vec<f32>, u32)> {
    let reader = hound::WavReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    rtracker_render::decode_wav_mono(reader)
        .map_err(|e| anyhow::anyhow!("{e} in {}", path.display()))
}

fn output_dir(output: &Path) -> &Path {
    output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."))
}

/// Path to `to` expressed relative to directory `from_dir`, so the written
/// pattern resolves its sample the same way the renderer will (relative to the
/// JSON's own directory). Falls back to the input path verbatim if either side
/// can't be canonicalised.
fn relativize(from_dir: &Path, to: &Path) -> PathBuf {
    let (Ok(from), Ok(to_abs)) = (from_dir.canonicalize(), to.canonicalize()) else {
        return to.to_path_buf();
    };
    let from_c: Vec<_> = from.components().collect();
    let to_c: Vec<_> = to_abs.components().collect();
    let mut i = 0;
    while i < from_c.len() && i < to_c.len() && from_c[i] == to_c[i] {
        i += 1;
    }
    let mut res = PathBuf::new();
    for _ in i..from_c.len() {
        res.push("..");
    }
    for c in &to_c[i..] {
        res.push(c.as_os_str());
    }
    if res.as_os_str().is_empty() { to.to_path_buf() } else { res }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_bounds_cover_whole_file_without_gaps() {
        let b = equal_bounds(1000, 4);
        assert_eq!(b, vec![(0, 250), (250, 500), (500, 750), (750, 1000)]);
        // Remainder lands in the final slice.
        let b = equal_bounds(1003, 4);
        assert_eq!(b.first().unwrap().0, 0);
        assert_eq!(b.last().unwrap().1, 1003);
        for w in b.windows(2) {
            assert_eq!(w[0].1, w[1].0, "slices must abut");
        }
    }

    #[test]
    fn onset_detection_finds_spaced_impulses() {
        // Four loud bursts on a quiet bed, 12000 frames apart at 48k.
        let sr = 48_000;
        let mut buf = vec![0.0f32; 48_000];
        for hit in 0..4 {
            let at = hit * 12_000;
            for j in 0..2000 {
                // decaying noise-ish burst
                let phase = (j as f32) * 0.7;
                buf[at + j] = phase.sin() * (1.0 - j as f32 / 2000.0);
            }
        }
        let onsets = detect_onsets(&buf, sr, 1.6, 40.0);
        // Starts at 0, then roughly one per burst (allow the leading 0 to double
        // up with the first burst).
        assert!(onsets.len() >= 4, "expected ~4 onsets, got {onsets:?}");
        // Detected onsets should sit near burst starts (within ~one hop block).
        for hit in 1..4u64 {
            let want = hit * 12_000;
            assert!(
                onsets.iter().any(|&o| (o as i64 - want as i64).abs() < 1024),
                "no onset near {want} in {onsets:?}"
            );
        }
    }

    #[test]
    fn slice_to_pattern_roundtrips_and_compiles() {
        let dir = std::env::temp_dir().join(format!("rtracker_slice_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("src.wav");
        // 8000-frame mono ramp.
        let spec = hound::WavSpec {
            channels: 1, sample_rate: 48_000, bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&wav, spec).unwrap();
        for i in 0..8000 {
            w.write_sample(((i % 256) as i16 - 128) * 100).unwrap();
        }
        w.finalize().unwrap();

        let out = dir.join("sliced.json");
        let opts = SliceOpts { slices: 8, ..SliceOpts::default() };
        let pat = slice_to_pattern(&wav, &out, &opts).expect("slice");

        assert_eq!(pat.tracks.len(), 8);
        assert_eq!(pat.rows, 8);
        // Eight equal slices of an 8000-frame file → 1000 frames each → at 48k
        // the gapless tempo is 48000*60/(1000*4) = 720 BPM.
        assert!((pat.tempo_bpm - 720.0).abs() < 1.0, "bpm {}", pat.tempo_bpm);
        assert!(out.exists());
        // Slices abut and cover the file.
        let mut ranges: Vec<(u64, u64)> =
            pat.samples.values().map(|s| (s.start_sample, s.end_sample)).collect();
        ranges.sort();
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(ranges.last().unwrap().1, 8000);

        let _ = std::fs::remove_dir_all(dir);
    }
}
