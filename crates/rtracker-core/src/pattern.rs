//! Tracker-style pattern authoring layer.
//!
//! A `Pattern` is the unit the editor / TUI manipulates: tempo, a row grid, and
//! one or more `Track`s holding sparse `Cell`s. Patterns compile down to a
//! `Piece` — the renderer never sees patterns, only events. This keeps the
//! authoring shape and the rendering shape decoupled.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
    /// Key into the parent pattern's `voices` map.
    pub voice: String,
    pub default_amp: f32,
    #[serde(default)]
    pub default_pan: f32,
    pub default_envelope: Envelope,
    #[serde(default)]
    pub default_fx: Vec<FxNode>,
    /// How long each trigger sounds, expressed in rows. 1.0 = one row,
    /// 0.9 = mostly fill the row with a small gap, 4.0 = four rows long.
    pub default_duration_rows: f32,
    /// Optional pitch envelope applied to every trigger in this track unless
    /// overridden by the cell. Use this for kick drops, zaps, risers.
    #[serde(default)]
    pub default_pitch_env: Option<PitchEnv>,
    /// Cells are sparse: only rows that contain triggers appear here.
    #[serde(default)]
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub row: u32,
    /// Pitch as Hz (`440.0`) or note name (`"A-4"`).
    pub note: Note,
    #[serde(default)]
    pub velocity: Option<f32>,
    #[serde(default)]
    pub duration_rows: Option<f32>,
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
    #[error("compiled piece failed validation: {0}")]
    Validation(#[from] crate::ValidationError),
    #[error("track '{0}' has bad note: {1}")]
    BadNote(String, String),
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
        if self.rows == 0 || self.lines_per_beat == 0 {
            return Err(PatternError::DegenerateGrid);
        }
        for track in &self.tracks {
            if !self.voices.contains_key(&track.voice) {
                return Err(PatternError::UnknownVoice(track.name.clone(), track.voice.clone()));
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
        }

        let spr = self.samples_per_row();
        let one_loop = spr * self.rows as u64;
        let total = one_loop * loops as u64;

        let mut events = Vec::new();
        for loop_i in 0..loops as u64 {
            let loop_offset = loop_i * one_loop;
            for track in &self.tracks {
                for cell in &track.cells {
                    let t = loop_offset + cell.row as u64 * spr;
                    let dur_rows = cell.duration_rows.unwrap_or(track.default_duration_rows);
                    let dur = ((spr as f64) * dur_rows.max(0.0) as f64).round() as u64;
                    let dur = dur.min(total.saturating_sub(t));
                    let amp = cell.velocity.unwrap_or(track.default_amp);
                    let fx = if cell.fx.is_empty() {
                        track.default_fx.clone()
                    } else {
                        cell.fx.clone()
                    };
                    events.push(Event {
                        t,
                        voice: track.voice.clone(),
                        freq: Some(cell.note.to_hz()
                            .map_err(|e| PatternError::BadNote(track.name.clone(), e))?),
                        dur,
                        amp,
                        pan: track.default_pan,
                        envelope: track.default_envelope,
                        fx,
                        pitch_ratio: None,
                        pitch_env: cell.pitch_env.or(track.default_pitch_env),
                    });
                }
            }
        }

        let piece = Piece {
            sample_rate: self.sample_rate,
            duration_samples: total,
            voices: self.voices.clone(),
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
        p.tracks.push(Track {
            name: "kick".into(),
            voice: "k".into(),
            default_amp: 0.8,
            default_pan: 0.0,
            default_envelope: Envelope::Ad { attack: 30, decay: 5970 },
            default_fx: vec![],
            default_duration_rows: 1.0,
            default_pitch_env: None,
            cells: vec![
                Cell { row: 0,  note: Note::Hz(50.0), velocity: None, duration_rows: None, fx: vec![], pitch_env: None },
                Cell { row: 4,  note: Note::Hz(50.0), velocity: None, duration_rows: None, fx: vec![], pitch_env: None },
                Cell { row: 8,  note: Note::Hz(50.0), velocity: None, duration_rows: None, fx: vec![], pitch_env: None },
                Cell { row: 12, note: Note::Hz(50.0), velocity: None, duration_rows: None, fx: vec![], pitch_env: None },
            ],
        });
        let piece = p.compile().expect("compile");
        assert_eq!(piece.events.len(), 4);
        let ts: Vec<u64> = piece.events.iter().map(|e| e.t).collect();
        assert_eq!(ts, vec![0, 24000, 48000, 72000]);
    }

    #[test]
    fn compile_repeated_concatenates() {
        let mut p = empty_pattern();
        p.tracks.push(Track {
            name: "kick".into(),
            voice: "k".into(),
            default_amp: 0.8,
            default_pan: 0.0,
            default_envelope: Envelope::Ad { attack: 30, decay: 5970 },
            default_fx: vec![],
            default_duration_rows: 1.0,
            default_pitch_env: None,
            cells: vec![Cell { row: 0, note: Note::Hz(50.0), velocity: None, duration_rows: None, fx: vec![], pitch_env: None }],
        });
        let piece = p.compile_repeated(3).expect("compile");
        let ts: Vec<u64> = piece.events.iter().map(|e| e.t).collect();
        assert_eq!(ts, vec![0, 96000, 192000]);
        assert_eq!(piece.duration_samples, 96000 * 3);
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
        p.tracks.push(Track {
            name: "bad".into(),
            voice: "nope".into(),
            default_amp: 0.5,
            default_pan: 0.0,
            default_envelope: Envelope::Gate,
            default_fx: vec![],
            default_duration_rows: 1.0,
            default_pitch_env: None,
            cells: vec![],
        });
        assert!(matches!(p.compile(), Err(PatternError::UnknownVoice(_, _))));
    }

    #[test]
    fn out_of_range_row_is_rejected() {
        let mut p = empty_pattern();
        p.tracks.push(Track {
            name: "k".into(),
            voice: "k".into(),
            default_amp: 0.5,
            default_pan: 0.0,
            default_envelope: Envelope::Gate,
            default_fx: vec![],
            default_duration_rows: 1.0,
            default_pitch_env: None,
            cells: vec![Cell { row: 99, note: Note::Hz(50.0), velocity: None, duration_rows: None, fx: vec![], pitch_env: None }],
        });
        assert!(matches!(p.compile(), Err(PatternError::CellOutOfRange { .. })));
    }
}
