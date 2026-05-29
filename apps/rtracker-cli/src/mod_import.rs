//! Minimal ProTracker MOD importer.
//!
//! This is intentionally a converter, not a MOD playback engine. It supports
//! simple 4-channel, 31-sample MODs, flattens the order table into one long
//! rtracker pattern, extracts embedded samples to WAV, and ignores effect
//! commands for now.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rtracker_core::{
    Cell, Envelope, Note, Pattern, PatternMetadata, SampleLoopMode, SampleRef, Track, VoiceDef,
};

const CHANNELS: usize = 4;
const SAMPLE_COUNT: usize = 31;
const ROWS_PER_PATTERN: usize = 64;
const HEADER_LEN: usize = 1084;
const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const DEFAULT_SPEED: f32 = 6.0;
const DEFAULT_MOD_BPM: f32 = 125.0;
const PAL_CLOCK_HZ: f32 = 7_093_789.2;

pub fn import_mod_to_pattern(input: &Path, output: &Path, samples_dir: Option<PathBuf>) -> Result<Pattern> {
    let data = fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let output_parent = output.parent().unwrap_or_else(|| Path::new("."));
    let sample_dir = samples_dir.unwrap_or_else(|| {
        let stem = output
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("mod_import");
        output_parent.join(format!("{stem}_samples"))
    });
    fs::create_dir_all(&sample_dir)
        .with_context(|| format!("creating sample directory {}", sample_dir.display()))?;

    let mut imported = parse_mod(&data)?;
    for sample in &imported.samples {
        let full_path = sample_dir.join(&sample.file_name);
        write_mono_wav(&full_path, &sample.pcm)
            .with_context(|| format!("writing sample {}", full_path.display()))?;
        let pattern_path = full_path
            .strip_prefix(output_parent)
            .map(Path::to_path_buf)
            .unwrap_or(full_path);
        imported.pattern.samples.insert(
            sample.id.clone(),
            SampleRef { path: pattern_path, start_sample: 0, end_sample: 0, label: Some(sample.name.clone()) },
        );
    }

    let text = serde_json::to_string_pretty(&imported.pattern)?;
    fs::write(output, text).with_context(|| format!("writing {}", output.display()))?;
    Ok(imported.pattern)
}

struct ImportedMod {
    pattern: Pattern,
    samples: Vec<SampleExport>,
}

struct SampleExport {
    id: String,
    name: String,
    file_name: String,
    pcm: Vec<f32>,
}

#[derive(Debug, Clone)]
struct SampleHeader {
    name: String,
    len_bytes: usize,
    volume: u8,
    repeat_start: usize,
    repeat_len: usize,
}

#[derive(Debug, Clone)]
struct Trigger {
    row: u32,
    channel: usize,
    sample_index: usize,
    period: u16,
}

fn parse_mod(data: &[u8]) -> Result<ImportedMod> {
    if data.len() < HEADER_LEN {
        bail!("file is too small to be a 31-sample ProTracker MOD");
    }

    let signature = bytes_to_ascii(&data[1080..1084]);
    if !matches!(signature.as_str(), "M.K." | "M!K!" | "FLT4" | "4CHN") {
        bail!("unsupported MOD signature '{signature}' (expected a 4-channel ProTracker MOD)");
    }

    let title = bytes_to_ascii(&data[0..20]);
    let mut headers = Vec::with_capacity(SAMPLE_COUNT);
    for i in 0..SAMPLE_COUNT {
        let off = 20 + i * 30;
        let name = bytes_to_ascii(&data[off..off + 22]);
        let len_bytes = be_u16(data, off + 22) as usize * 2;
        let volume = data[off + 25].min(64);
        let repeat_start = be_u16(data, off + 26) as usize * 2;
        let repeat_len = be_u16(data, off + 28) as usize * 2;
        headers.push(SampleHeader { name, len_bytes, volume, repeat_start, repeat_len });
    }

    let song_len = data[950] as usize;
    if song_len == 0 || song_len > 128 {
        bail!("invalid MOD song length {song_len}");
    }
    let orders = &data[952..952 + song_len];
    let pattern_count = orders.iter().copied().max().unwrap_or(0) as usize + 1;
    let pattern_bytes = pattern_count * ROWS_PER_PATTERN * CHANNELS * 4;
    let sample_data_start = HEADER_LEN + pattern_bytes;
    if data.len() < sample_data_start {
        bail!("file ended before pattern data completed");
    }

    let mut voices = HashMap::new();
    let mut sample_exports = Vec::new();
    let mut sample_offsets = Vec::with_capacity(SAMPLE_COUNT);
    let mut pos = sample_data_start;
    for (i, h) in headers.iter().enumerate() {
        sample_offsets.push(pos);
        pos = pos.saturating_add(h.len_bytes);
        if h.len_bytes == 0 {
            continue;
        }
        if data.len() < pos {
            bail!("file ended inside sample {}", i + 1);
        }
        let id = sample_id(i);
        let name = if h.name.is_empty() { id.clone() } else { h.name.clone() };
        let file_name = format!("{}_{}.wav", id, sanitize_name(&name));
        voices.insert(
            id.clone(),
            VoiceDef::Sample {
                sample_id: id.clone(),
                loop_mode: loop_mode_for(h),
            },
        );
        sample_exports.push(SampleExport {
            id,
            name,
            file_name,
            pcm: data[sample_offsets[i]..pos]
                .iter()
                .map(|b| (*b as i8) as f32 / 128.0)
                .collect(),
        });
    }

    let triggers = collect_triggers(data, HEADER_LEN, orders, &headers)?;
    let durations = trigger_durations(&triggers, (song_len * ROWS_PER_PATTERN) as u32);
    let mut by_track: BTreeMap<(usize, usize), Vec<Cell>> = BTreeMap::new();
    for (i, trigger) in triggers.iter().enumerate() {
        if headers[trigger.sample_index].len_bytes == 0 {
            continue;
        }
        let hz = period_to_hz(trigger.period);
        by_track
            .entry((trigger.channel, trigger.sample_index))
            .or_default()
            .push(Cell {
                row: trigger.row,
                note: Note::Hz(hz),
                pitch_ratio: Some((hz / DEFAULT_SAMPLE_RATE as f32).max(0.001)),
                velocity: None,
                duration_rows: Some(durations[i].max(1) as f32),
                pan: None,
                fx: vec![],
                pitch_env: None,
            });
    }

    let mut tracks = Vec::new();
    for ((channel, sample_index), cells) in by_track {
        let h = &headers[sample_index];
        let id = sample_id(sample_index);
        let label = if h.name.is_empty() { id.clone() } else { h.name.clone() };
        tracks.push(Track {
            name: format!("ch{} {}", channel + 1, label),
            instrument: None,
            voice: Some(id),
            default_amp: Some((h.volume as f32 / 64.0).clamp(0.0, 1.0)),
            default_pan: Some(channel_pan(channel)),
            default_envelope: Some(Envelope::Gate),
            default_fx: Some(vec![]),
            default_duration_rows: Some(1.0),
            default_pitch_env: None,
            cells,
        });
    }

    let pattern = Pattern {
        sample_rate: DEFAULT_SAMPLE_RATE,
        tempo_bpm: 6.0 * DEFAULT_MOD_BPM / DEFAULT_SPEED,
        lines_per_beat: 4,
        rows: (song_len * ROWS_PER_PATTERN) as u32,
        voices,
        samples: HashMap::new(),
        tracks,
        metadata: PatternMetadata {
            title: if title.is_empty() { None } else { Some(title) },
            author: Some("imported from ProTracker MOD".into()),
        },
    };

    Ok(ImportedMod { pattern, samples: sample_exports })
}

fn collect_triggers(
    data: &[u8],
    pattern_start: usize,
    orders: &[u8],
    headers: &[SampleHeader],
) -> Result<Vec<Trigger>> {
    let mut active_sample: [Option<usize>; CHANNELS] = [None; CHANNELS];
    let mut triggers = Vec::new();
    for (order_index, pattern_number) in orders.iter().copied().enumerate() {
        let pat_off = pattern_start + pattern_number as usize * ROWS_PER_PATTERN * CHANNELS * 4;
        for row in 0..ROWS_PER_PATTERN {
            for channel in 0..CHANNELS {
                let off = pat_off + (row * CHANNELS + channel) * 4;
                if off + 4 > data.len() {
                    bail!("pattern {} row {} channel {} is truncated", pattern_number, row, channel + 1);
                }
                let b0 = data[off];
                let b1 = data[off + 1];
                let b2 = data[off + 2];
                let period = (((b0 & 0x0f) as u16) << 8) | b1 as u16;
                let sample_number = ((b0 & 0xf0) as usize) | ((b2 >> 4) as usize);
                if (1..=SAMPLE_COUNT).contains(&sample_number) {
                    let idx = sample_number - 1;
                    if headers[idx].len_bytes > 0 {
                        active_sample[channel] = Some(idx);
                    }
                }
                if period > 0 {
                    if let Some(sample_index) = active_sample[channel] {
                        triggers.push(Trigger {
                            row: (order_index * ROWS_PER_PATTERN + row) as u32,
                            channel,
                            sample_index,
                            period,
                        });
                    }
                }
            }
        }
    }
    Ok(triggers)
}

fn trigger_durations(triggers: &[Trigger], total_rows: u32) -> Vec<u32> {
    let mut durations = vec![1; triggers.len()];
    for channel in 0..CHANNELS {
        let mut indices: Vec<usize> = triggers
            .iter()
            .enumerate()
            .filter_map(|(i, t)| (t.channel == channel).then_some(i))
            .collect();
        indices.sort_by_key(|i| triggers[*i].row);
        for pair in indices.windows(2) {
            durations[pair[0]] = triggers[pair[1]].row.saturating_sub(triggers[pair[0]].row).max(1);
        }
        if let Some(last) = indices.last().copied() {
            durations[last] = total_rows.saturating_sub(triggers[last].row).max(1);
        }
    }
    durations
}

fn write_mono_wav(path: &Path, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: DEFAULT_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

fn be_u16(data: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([data[off], data[off + 1]])
}

fn bytes_to_ascii(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    bytes[..end]
        .iter()
        .map(|b| if b.is_ascii_graphic() || *b == b' ' { *b as char } else { ' ' })
        .collect::<String>()
        .trim()
        .to_string()
}

fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    let cleaned = cleaned.trim_matches('_');
    if cleaned.is_empty() { "sample".into() } else { cleaned.into() }
}

fn sample_id(index: usize) -> String {
    format!("sample_{:02}", index + 1)
}

fn loop_mode_for(header: &SampleHeader) -> SampleLoopMode {
    if header.repeat_start == 0 && header.repeat_len >= 4 && header.repeat_len >= header.len_bytes {
        SampleLoopMode::Loop
    } else {
        SampleLoopMode::OneShot
    }
}

fn period_to_hz(period: u16) -> f32 {
    PAL_CLOCK_HZ / (2.0 * period.max(1) as f32)
}

fn channel_pan(channel: usize) -> f32 {
    match channel {
        0 | 3 => -0.65,
        1 | 2 => 0.65,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_four_channel_mod() {
        let data = tiny_mod();
        let imported = parse_mod(&data).expect("parse mod");

        assert_eq!(imported.pattern.rows, 64);
        assert_eq!(imported.pattern.tracks.len(), 1);
        assert_eq!(imported.pattern.voices.len(), 1);
        assert_eq!(imported.samples.len(), 1);

        let cell = &imported.pattern.tracks[0].cells[0];
        assert_eq!(cell.row, 0);
        assert!(matches!(cell.note, Note::Hz(f) if (f - 8287.1).abs() < 1.0));
        assert!(cell.pitch_ratio.unwrap() > 0.17 && cell.pitch_ratio.unwrap() < 0.18);
    }

    #[test]
    fn import_writes_pattern_and_samples() {
        let dir = temp_test_dir();
        fs::create_dir_all(&dir).expect("create temp dir");
        let input = dir.join("tiny.mod");
        let output = dir.join("tiny.json");
        fs::write(&input, tiny_mod()).expect("write tiny mod");

        let pattern = import_mod_to_pattern(&input, &output, None).expect("import mod");

        assert_eq!(pattern.samples.len(), 1);
        assert!(output.exists());
        assert!(dir.join("tiny_samples").join("sample_01_beep.wav").exists());

        let _ = fs::remove_dir_all(dir);
    }

    fn tiny_mod() -> Vec<u8> {
        let mut data = vec![0u8; HEADER_LEN + ROWS_PER_PATTERN * CHANNELS * 4 + 4];
        data[0..8].copy_from_slice(b"tiny mod");

        let sample = 20;
        data[sample..sample + 4].copy_from_slice(b"beep");
        data[sample + 23] = 2; // two words = four bytes
        data[sample + 25] = 64;

        data[950] = 1; // song length
        data[1080..1084].copy_from_slice(b"M.K.");

        let period = 428u16;
        data[HEADER_LEN] = ((period >> 8) as u8) & 0x0f;
        data[HEADER_LEN + 1] = period as u8;
        data[HEADER_LEN + 2] = 0x10; // sample 1, no effect
        data[HEADER_LEN + 3] = 0;

        let sample_data = HEADER_LEN + ROWS_PER_PATTERN * CHANNELS * 4;
        data[sample_data..sample_data + 4].copy_from_slice(&[0, 64, 192, 0]);
        data
    }

    fn temp_test_dir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("rtracker_mod_import_{}_{}", std::process::id(), nanos))
    }
}
