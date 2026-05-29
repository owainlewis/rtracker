//! Tracker-style pattern authoring layer.
//!
//! A `Pattern` is the unit the editor / TUI manipulates: tempo, a row grid, and
//! one or more `Track`s holding sparse `Cell`s. Patterns compile down to a
//! `Piece` — the renderer never sees patterns, only events. This keeps the
//! authoring shape and the rendering shape decoupled.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::presets;
use crate::{Envelope, Event, FxNode, Piece, PieceMetadata, PitchEnv, SampleRef, VoiceDef};

/// A note value. Accepts either a raw frequency in Hz (`50.0`) or a
/// tracker-style note name (`"C-4"`, `"A#3"`, `"Bb2"`). Note names are
/// resolved against A4 = 440 Hz.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Note {
    Hz(f32),
    Name(String),
}

impl Note {
    pub fn to_hz(&self) -> Result<f32, String> {
        match self {
            Note::Hz(f) => Ok(*f),
            Note::Name(s) => parse_note_name(s),
        }
    }
}

fn parse_note_name(s: &str) -> Result<f32, String> {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Err("empty note name".into());
    }
    let letter_off: i32 = match bytes[0].to_ascii_uppercase() {
        b'C' => 0, b'D' => 2, b'E' => 4, b'F' => 5,
        b'G' => 7, b'A' => 9, b'B' => 11,
        _ => return Err(format!("bad note letter in '{s}'")),
    };
    let mut i = 1;
    let mut accidental: i32 = 0;
    if i < bytes.len() {
        match bytes[i] {
            b'#' => { accidental = 1; i += 1; }
            b'b' => { accidental = -1; i += 1; }
            _ => {}
        }
    }
    // Optional `-` separator between letter+accidental and octave.
    if i < bytes.len() && bytes[i] == b'-' {
        i += 1;
    }
    let rest = std::str::from_utf8(&bytes[i..]).map_err(|e| e.to_string())?;
    let octave: i32 = rest.parse().map_err(|_| format!("bad octave in '{s}'"))?;
    // MIDI: C-1 = 0, C0 = 12, ..., A4 = 69.
    let midi = (octave + 1) * 12 + letter_off + accidental;
    Ok(440.0 * (2.0_f32).powf((midi as f32 - 69.0) / 12.0))
}

/// One tracker pattern. A pattern is `rows` long; at tempo `tempo_bpm` with
/// `lines_per_beat` rows per beat, each row is `samples_per_row()` samples.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    pub sample_rate: u32,
    pub tempo_bpm: f32,
    /// Rows per beat. 4 = 16th-note resolution; 8 = 32nd-note.
    pub lines_per_beat: u32,
    /// Pattern length in rows.
    pub rows: u32,
    #[serde(default)]
    pub voices: HashMap<String, VoiceDef>,
    #[serde(default)]
    pub samples: HashMap<String, SampleRef>,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub metadata: PatternMetadata,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PatternMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
}

/// One track is one synth/sample voice with a set of defaults and a sparse
/// list of cells (only filled rows are stored).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    /// If set, defaults are pulled from `presets::get(instrument)` for any
    /// field not explicitly given. The instrument's `VoiceDef` is auto-injected
    /// into the pattern's voice map (keyed by the instrument name) unless
    /// `voice` is set to override.
    #[serde(default)]
    pub instrument: Option<String>,
    /// Key into the parent pattern's `voices` map. Optional when `instrument`
    /// is set — defaults to the instrument name.
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub default_amp: Option<f32>,
    #[serde(default)]
    pub default_pan: Option<f32>,
    #[serde(default)]
    pub default_envelope: Option<Envelope>,
    #[serde(default)]
    pub default_fx: Option<Vec<FxNode>>,
    /// How long each trigger sounds, in rows. 1.0 = one row.
    #[serde(default)]
    pub default_duration_rows: Option<f32>,
    #[serde(default)]
    pub default_pitch_env: Option<PitchEnv>,
    /// Cells are sparse: only rows that contain triggers appear here.
    #[serde(default)]
    pub cells: Vec<Cell>,
}

/// Fully-resolved track defaults after preset merging. Used internally during
/// compile.
struct ResolvedTrack<'a> {
    voice_key: String,
    amp: f32,
    pan: f32,
    envelope: Envelope,
    fx: Vec<FxNode>,
    duration_rows: f32,
    pitch_env: Option<PitchEnv>,
    cells: &'a [Cell],
    track_name: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub row: u32,
    /// Pitch as Hz (`440.0`) or note name (`"A-4"`).
    pub note: Note,
    /// Optional playback-rate multiplier for sample voices.
    #[serde(default)]
    pub pitch_ratio: Option<f32>,
    #[serde(default)]
    pub velocity: Option<f32>,
    #[serde(default)]
    pub duration_rows: Option<f32>,
    /// Pan override (-1.0 = L, +1.0 = R). Overrides the track's default_pan.
    #[serde(default)]
    pub pan: Option<f32>,
    /// If non-empty, overrides the track's `default_fx` for this trigger.
    #[serde(default)]
    pub fx: Vec<FxNode>,
    /// Overrides the track's `default_pitch_env` for this trigger.
    #[serde(default)]
    pub pitch_env: Option<PitchEnv>,
}

#[derive(Debug, Error)]
pub enum PatternError {
    #[error("track '{0}' references unknown voice '{1}'")]
    UnknownVoice(String, String),
    #[error("cell in track '{track}' has row {row} >= pattern rows {rows}")]
    CellOutOfRange { track: String, row: u32, rows: u32 },
    #[error("pattern has zero rows or zero lines_per_beat")]
    DegenerateGrid,
    #[error("pattern has invalid grid field {field}: {value}")]
    InvalidGrid { field: &'static str, value: f64 },
    #[error("pattern duration overflows u64 samples")]
    GridOverflow,
    #[error("compiled piece failed validation: {0}")]
    Validation(#[from] crate::ValidationError),
    #[error("track '{0}' has bad note: {1}")]
    BadNote(String, String),
    #[error("unknown instrument '{0}'")]
    UnknownInstrument(String),
    #[error("track '{0}' has no voice and no instrument to default from")]
    TrackMissingVoice(String),
    #[error("track '{0}' is missing required field '{1}' (and no instrument provides it)")]
    TrackMissingField(String, &'static str),
    #[error("song has no patterns")]
    EmptySong,
    #[error("song patterns disagree on sample_rate: {expected} vs {found}")]
    SongSampleRateMismatch { expected: u32, found: u32 },
}

impl Pattern {
    /// Sample count per row, rounded to nearest integer.
    pub fn samples_per_row(&self) -> u64 {
        let sr = self.sample_rate as f64;
        let bpm = self.tempo_bpm as f64;
        let lpb = self.lines_per_beat as f64;
        // samples / row = (sr * 60) / (bpm * lpb)
        ((sr * 60.0) / (bpm * lpb)).round() as u64
    }

    /// Total length of one pattern instance in samples.
    pub fn duration_samples(&self) -> u64 {
        self.samples_per_row() * self.rows as u64
    }

    /// Compile to a renderable `Piece`. The piece's duration matches one
    /// pattern instance — call `compile_repeated(n)` to bake N loops into one
    /// piece for offline rendering.
    pub fn compile(&self) -> Result<Piece, PatternError> {
        self.compile_repeated(1)
    }

    pub fn compile_repeated(&self, loops: u32) -> Result<Piece, PatternError> {
        let spr = self.checked_samples_per_row()?;
        let one_loop = spr
            .checked_mul(self.rows as u64)
            .ok_or(PatternError::GridOverflow)?;
        let total = one_loop
            .checked_mul(loops as u64)
            .ok_or(PatternError::GridOverflow)?;

        // Resolve every track against its instrument preset and auto-register
        // any voices that come from presets.
        let mut voices = self.voices.clone();
        let mut resolved: Vec<ResolvedTrack> = Vec::with_capacity(self.tracks.len());
        for track in &self.tracks {
            let preset = track
                .instrument
                .as_ref()
                .map(|n| presets::get(n).ok_or_else(|| PatternError::UnknownInstrument(n.clone())))
                .transpose()?;

            let voice_key = match (&track.voice, &preset) {
                (Some(v), _) => v.clone(),
                (None, Some(p)) => p.name.to_string(),
                (None, None) => {
                    return Err(PatternError::TrackMissingVoice(track.name.clone()));
                }
            };

            // Auto-inject the preset's voice into the pattern voice map if the
            // pattern doesn't already define one under that key.
            if let Some(p) = &preset {
                voices.entry(voice_key.clone()).or_insert_with(|| p.voice.clone());
            }

            if !voices.contains_key(&voice_key) {
                return Err(PatternError::UnknownVoice(track.name.clone(), voice_key));
            }

            for c in &track.cells {
                if c.row >= self.rows {
                    return Err(PatternError::CellOutOfRange {
                        track: track.name.clone(),
                        row: c.row,
                        rows: self.rows,
                    });
                }
            }

            let amp = track.default_amp
                .or(preset.as_ref().map(|p| p.default_amp))
                .ok_or_else(|| PatternError::TrackMissingField(track.name.clone(), "default_amp"))?;
            let pan = track.default_pan
                .or(preset.as_ref().map(|p| p.default_pan))
                .unwrap_or(0.0);
            let envelope = track.default_envelope
                .or(preset.as_ref().map(|p| p.default_envelope))
                .ok_or_else(|| PatternError::TrackMissingField(track.name.clone(), "default_envelope"))?;
            let fx = track.default_fx.clone()
                .or(preset.as_ref().map(|p| p.default_fx.clone()))
                .unwrap_or_default();
            let duration_rows = track.default_duration_rows
                .or(preset.as_ref().map(|p| p.default_duration_rows))
                .ok_or_else(|| PatternError::TrackMissingField(track.name.clone(), "default_duration_rows"))?;
            let pitch_env = track.default_pitch_env
                .or(preset.as_ref().and_then(|p| p.default_pitch_env));

            resolved.push(ResolvedTrack {
                voice_key,
                amp,
                pan,
                envelope,
                fx,
                duration_rows,
                pitch_env,
                cells: &track.cells,
                track_name: &track.name,
            });
        }

        let mut events = Vec::new();
        for loop_i in 0..loops as u64 {
            let loop_offset = loop_i * one_loop;
            for rt in &resolved {
                for cell in rt.cells {
                    let t = loop_offset + cell.row as u64 * spr;
                    let dur_rows = cell.duration_rows.unwrap_or(rt.duration_rows);
                    let dur = ((spr as f64) * dur_rows.max(0.0) as f64).round() as u64;
                    let dur = dur.min(total.saturating_sub(t));
                    let amp = cell.velocity.unwrap_or(rt.amp);
                    let fx = if cell.fx.is_empty() {
                        rt.fx.clone()
                    } else {
                        cell.fx.clone()
                    };
                    events.push(Event {
                        t,
                        voice: rt.voice_key.clone(),
                        freq: Some(cell.note.to_hz()
                            .map_err(|e| PatternError::BadNote(rt.track_name.to_string(), e))?),
                        dur,
                        amp,
                        pan: cell.pan.unwrap_or(rt.pan),
                        envelope: rt.envelope,
                        fx,
                        pitch_ratio: cell.pitch_ratio,
                        pitch_env: cell.pitch_env.or(rt.pitch_env),
                    });
                }
            }
        }

        let piece = Piece {
            sample_rate: self.sample_rate,
            duration_samples: total,
            voices,
            samples: self.samples.clone(),
            events,
            metadata: PieceMetadata {
                title: self.metadata.title.clone(),
                generator: Some("rtracker-pattern".into()),
                ..Default::default()
            },
        };
        piece.validate()?;
        Ok(piece)
    }

    fn checked_samples_per_row(&self) -> Result<u64, PatternError> {
        if self.rows == 0 || self.lines_per_beat == 0 {
            return Err(PatternError::DegenerateGrid);
        }
        if self.sample_rate == 0 {
            return Err(PatternError::InvalidGrid {
                field: "sample_rate",
                value: self.sample_rate as f64,
            });
        }
        if !self.tempo_bpm.is_finite() || self.tempo_bpm <= 0.0 {
            return Err(PatternError::InvalidGrid {
                field: "tempo_bpm",
                value: self.tempo_bpm as f64,
            });
        }

        let row_samples = ((self.sample_rate as f64 * 60.0)
            / (self.tempo_bpm as f64 * self.lines_per_beat as f64))
            .round();
        if !row_samples.is_finite() || row_samples < 1.0 {
            return Err(PatternError::InvalidGrid {
                field: "samples_per_row",
                value: row_samples,
            });
        }
        if row_samples > u64::MAX as f64 {
            return Err(PatternError::GridOverflow);
        }
        Ok(row_samples as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_pattern() -> Pattern {
        let mut voices = HashMap::new();
        voices.insert("k".into(), VoiceDef::Sine { default_pan: 0.0 });
        Pattern {
            sample_rate: 48000,
            tempo_bpm: 120.0,
            lines_per_beat: 4,
            rows: 16,
            voices,
            samples: HashMap::new(),
            tracks: vec![],
            metadata: PatternMetadata::default(),
        }
    }

    fn kick_track(name: &str, voice: &str, cells: Vec<Cell>) -> Track {
        Track {
            name: name.into(),
            instrument: None,
            voice: Some(voice.into()),
            default_amp: Some(0.8),
            default_pan: Some(0.0),
            default_envelope: Some(Envelope::Ad { attack: 30, decay: 5970 }),
            default_fx: Some(vec![]),
            default_duration_rows: Some(1.0),
            default_pitch_env: None,
            cells,
        }
    }

    fn cell(row: u32, note: f32) -> Cell {
        Cell { row, note: Note::Hz(note), velocity: None, duration_rows: None,
               pitch_ratio: None, pan: None, fx: vec![], pitch_env: None }
    }

    #[test]
    fn samples_per_row_at_120bpm_lpb4() {
        // 120 BPM, LPB 4 → beat = 0.5 s → row = 0.125 s → 6000 samples @ 48k
        let p = empty_pattern();
        assert_eq!(p.samples_per_row(), 6000);
        assert_eq!(p.duration_samples(), 96000);
    }

    #[test]
    fn compile_places_events_on_grid() {
        let mut p = empty_pattern();
        p.tracks.push(kick_track("kick", "k", vec![
            cell(0, 50.0), cell(4, 50.0), cell(8, 50.0), cell(12, 50.0),
        ]));
        let piece = p.compile().expect("compile");
        assert_eq!(piece.events.len(), 4);
        let ts: Vec<u64> = piece.events.iter().map(|e| e.t).collect();
        assert_eq!(ts, vec![0, 24000, 48000, 72000]);
    }

    #[test]
    fn compile_repeated_concatenates() {
        let mut p = empty_pattern();
        p.tracks.push(kick_track("kick", "k", vec![cell(0, 50.0)]));
        let piece = p.compile_repeated(3).expect("compile");
        let ts: Vec<u64> = piece.events.iter().map(|e| e.t).collect();
        assert_eq!(ts, vec![0, 96000, 192000]);
        assert_eq!(piece.duration_samples, 96000 * 3);
    }

    #[test]
    fn instrument_provides_voice_and_defaults() {
        let mut p = empty_pattern();
        p.voices.clear();   // start with no voices defined
        p.tracks.push(Track {
            name: "lead".into(),
            instrument: Some("aphex_bell".into()),
            voice: None,
            default_amp: None,
            default_pan: None,
            default_envelope: None,
            default_fx: None,
            default_duration_rows: None,
            default_pitch_env: None,
            cells: vec![cell(0, 440.0)],
        });
        let piece = p.compile().expect("compile with instrument");
        assert_eq!(piece.events.len(), 1);
        // Voice key defaults to instrument name; voice was auto-registered.
        assert_eq!(piece.events[0].voice, "aphex_bell");
        assert!(piece.voices.contains_key("aphex_bell"));
        // Amp came from the preset (0.45).
        assert!((piece.events[0].amp - 0.45).abs() < 1e-6);
    }

    #[test]
    fn unknown_instrument_is_rejected() {
        let mut p = empty_pattern();
        p.tracks.push(Track {
            name: "lead".into(),
            instrument: Some("not_a_real_preset".into()),
            voice: None,
            default_amp: None, default_pan: None,
            default_envelope: None, default_fx: None,
            default_duration_rows: None, default_pitch_env: None,
            cells: vec![],
        });
        assert!(matches!(p.compile(), Err(PatternError::UnknownInstrument(_))));
    }

    #[test]
    fn note_name_parses_a4_440() {
        assert!((Note::Name("A-4".into()).to_hz().unwrap() - 440.0).abs() < 0.01);
        assert!((Note::Name("A4".into()).to_hz().unwrap() - 440.0).abs() < 0.01);
    }

    #[test]
    fn note_name_octaves_and_accidentals() {
        let middle_c = Note::Name("C-4".into()).to_hz().unwrap();
        assert!((middle_c - 261.626).abs() < 0.01);
        let c_sharp = Note::Name("C#4".into()).to_hz().unwrap();
        assert!((c_sharp - 277.183).abs() < 0.01);
        let d_flat = Note::Name("Db4".into()).to_hz().unwrap();
        assert!((d_flat - c_sharp).abs() < 0.01);          // enharmonic
        let low = Note::Name("A-1".into()).to_hz().unwrap();
        assert!((low - 55.0).abs() < 0.01);
        let high = Note::Name("A-6".into()).to_hz().unwrap();
        assert!((high - 1760.0).abs() < 0.1);
    }

    #[test]
    fn note_hz_passthrough() {
        assert_eq!(Note::Hz(123.45).to_hz().unwrap(), 123.45);
    }

    #[test]
    fn unknown_voice_is_rejected() {
        let mut p = empty_pattern();
        p.tracks.push(kick_track("bad", "nope", vec![]));
        assert!(matches!(p.compile(), Err(PatternError::UnknownVoice(_, _))));
    }

    #[test]
    fn out_of_range_row_is_rejected() {
        let mut p = empty_pattern();
        p.tracks.push(kick_track("k", "k", vec![cell(99, 50.0)]));
        assert!(matches!(p.compile(), Err(PatternError::CellOutOfRange { .. })));
    }

    #[test]
    fn zero_tempo_is_rejected_without_panicking() {
        let mut p = empty_pattern();
        p.tempo_bpm = 0.0;
        p.tracks.push(kick_track("kick", "k", vec![cell(0, 50.0)]));
        assert!(matches!(
            p.compile(),
            Err(PatternError::InvalidGrid { field: "tempo_bpm", .. })
        ));
    }

    #[test]
    fn non_finite_tempo_is_rejected_without_panicking() {
        let mut p = empty_pattern();
        p.tempo_bpm = f32::NAN;
        p.tracks.push(kick_track("kick", "k", vec![cell(0, 50.0)]));
        assert!(matches!(
            p.compile(),
            Err(PatternError::InvalidGrid { field: "tempo_bpm", .. })
        ));
    }

    #[test]
    fn zero_sample_rate_is_rejected_without_panicking() {
        let mut p = empty_pattern();
        p.sample_rate = 0;
        p.tracks.push(kick_track("kick", "k", vec![cell(0, 50.0)]));
        assert!(matches!(
            p.compile(),
            Err(PatternError::InvalidGrid { field: "sample_rate", .. })
        ));
    }

    #[test]
    fn overflowing_grid_is_rejected_without_panicking() {
        let mut p = empty_pattern();
        p.sample_rate = u32::MAX;
        p.tempo_bpm = f32::MIN_POSITIVE;
        p.rows = u32::MAX;
        p.tracks.push(kick_track("kick", "k", vec![cell(0, 50.0)]));
        assert!(matches!(p.compile_repeated(u32::MAX), Err(PatternError::GridOverflow)));
    }
}
