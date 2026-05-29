//! Song = an ordered array of patterns, compiled into one continuous `Piece`.
//!
//! This is the arrangement layer above `Pattern`. A song just runs its patterns
//! back to back on the timeline — no order-index indirection yet; copy-paste a
//! pattern to repeat it. Each pattern keeps its own tempo and grid; the only
//! cross-pattern requirement is a shared `sample_rate`, since the rendered
//! `Piece` has a single rate. Voice and sample maps are merged across patterns
//! (later definition wins on key collision).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::pattern::PatternError;
use crate::{Pattern, Piece, PieceMetadata};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Song {
    pub patterns: Vec<Pattern>,
    #[serde(default)]
    pub metadata: SongMetadata,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SongMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
}

impl Song {
    /// Compile every pattern in order and concatenate the results onto one
    /// timeline. Each pattern is compiled (and validated) independently; the
    /// merged piece is validated once more at the end.
    pub fn compile(&self) -> Result<Piece, PatternError> {
        if self.patterns.is_empty() {
            return Err(PatternError::EmptySong);
        }

        let mut offset = 0u64;
        let mut events = Vec::new();
        let mut voices = HashMap::new();
        let mut samples = HashMap::new();
        let mut sample_rate: Option<u32> = None;

        for pat in &self.patterns {
            let piece = pat.compile()?;
            match sample_rate {
                None => sample_rate = Some(piece.sample_rate),
                Some(sr) if sr != piece.sample_rate => {
                    return Err(PatternError::SongSampleRateMismatch {
                        expected: sr,
                        found: piece.sample_rate,
                    });
                }
                _ => {}
            }
            voices.extend(piece.voices);
            samples.extend(piece.samples);
            for mut e in piece.events {
                e.t = e.t.checked_add(offset).ok_or(PatternError::GridOverflow)?;
                events.push(e);
            }
            offset = offset
                .checked_add(piece.duration_samples)
                .ok_or(PatternError::GridOverflow)?;
        }

        let piece = Piece {
            sample_rate: sample_rate.expect("non-empty song sets sample_rate"),
            duration_samples: offset,
            voices,
            samples,
            events,
            metadata: PieceMetadata {
                title: self.metadata.title.clone(),
                generator: Some("rtracker-song".into()),
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
    use crate::pattern::{Cell, Note, Track};
    use crate::{Envelope, VoiceDef};

    fn one_kick_pattern(rows: u32, tempo: f32) -> Pattern {
        let mut voices = HashMap::new();
        voices.insert("k".into(), VoiceDef::Sine { default_pan: 0.0 });
        Pattern {
            sample_rate: 48000,
            tempo_bpm: tempo,
            lines_per_beat: 4,
            rows,
            voices,
            samples: HashMap::new(),
            tracks: vec![Track {
                name: "kick".into(),
                instrument: None,
                voice: Some("k".into()),
                default_amp: Some(0.8),
                default_pan: Some(0.0),
                default_envelope: Some(Envelope::Ad { attack: 30, decay: 1000 }),
                default_fx: Some(vec![]),
                default_duration_rows: Some(1.0),
                default_pitch_env: None,
                cells: vec![Cell {
                    row: 0,
                    note: Note::Hz(50.0),
                    pitch_ratio: None,
                    velocity: None,
                    duration_rows: None,
                    pan: None,
                    fx: vec![],
                    pitch_env: None,
                }],
            }],
            metadata: Default::default(),
        }
    }

    #[test]
    fn empty_song_is_rejected() {
        let song = Song { patterns: vec![], metadata: Default::default() };
        assert!(matches!(song.compile(), Err(PatternError::EmptySong)));
    }

    #[test]
    fn patterns_concatenate_on_timeline() {
        // Two 16-row patterns at 120 BPM / lpb 4 → each 96000 samples long.
        let song = Song {
            patterns: vec![one_kick_pattern(16, 120.0), one_kick_pattern(16, 120.0)],
            metadata: Default::default(),
        };
        let piece = song.compile().expect("compile song");
        // One kick per pattern; the second is offset by the first's duration.
        let ts: Vec<u64> = piece.events.iter().map(|e| e.t).collect();
        assert_eq!(ts, vec![0, 96000]);
        assert_eq!(piece.duration_samples, 192000);
    }

    #[test]
    fn mismatched_sample_rate_is_rejected() {
        let mut a = one_kick_pattern(16, 120.0);
        let mut b = one_kick_pattern(16, 120.0);
        a.sample_rate = 48000;
        b.sample_rate = 44100;
        let song = Song { patterns: vec![a, b], metadata: Default::default() };
        assert!(matches!(
            song.compile(),
            Err(PatternError::SongSampleRateMismatch { expected: 48000, found: 44100 })
        ));
    }

    #[test]
    fn song_roundtrips_json() {
        let song = Song {
            patterns: vec![one_kick_pattern(16, 160.0)],
            metadata: SongMetadata { title: Some("t".into()), author: None },
        };
        let s = serde_json::to_string(&song).unwrap();
        let back: Song = serde_json::from_str(&s).unwrap();
        assert_eq!(back.patterns.len(), 1);
        back.compile().unwrap();
    }
}
