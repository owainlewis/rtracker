use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod pattern;
pub use pattern::{Cell, Note, Pattern, PatternError, PatternMetadata, Track};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Piece {
    pub sample_rate: u32,
    pub duration_samples: u64,
    #[serde(default)]
    pub voices: HashMap<String, VoiceDef>,
    #[serde(default)]
    pub samples: HashMap<String, SampleRef>,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub metadata: PieceMetadata,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PieceMetadata {
    pub title: Option<String>,
    pub seed: Option<u64>,
    pub generator: Option<String>,
    pub constraints_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VoiceDef {
    Sine {
        #[serde(default)]
        default_pan: f32,
    },
    SinePartials {
        partials: Vec<f32>,
        amplitudes: Vec<f32>,
        #[serde(default)]
        default_pan: f32,
    },
    NoiseBandpass {
        q: f32,
        #[serde(default)]
        default_pan: f32,
    },
    // Phase 2 placeholders kept in the type so JSON round-trips don't break,
    // but the renderer rejects them in Phase 1.
    Fm {
        modulator_ratio: f32,
        modulation_index: f32,
        #[serde(default)]
        default_pan: f32,
    },
    Sample {
        sample_id: String,
        loop_mode: SampleLoopMode,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleLoopMode {
    OneShot,
    Loop,
    PingPong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleRef {
    pub path: PathBuf,
    /// Slice start within the file. Defaults to 0.
    #[serde(default)]
    pub start_sample: u64,
    /// Slice end (exclusive). 0 = play to end of file.
    #[serde(default)]
    pub end_sample: u64,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub t: u64,
    pub voice: String,
    #[serde(default)]
    pub freq: Option<f32>,
    pub dur: u64,
    pub amp: f32,
    #[serde(default)]
    pub pan: f32,
    pub envelope: Envelope,
    #[serde(default)]
    pub fx: Vec<FxNode>,
    #[serde(default)]
    pub pitch_ratio: Option<f32>,
    /// Optional per-event pitch envelope. Modulates `freq` over the first
    /// `time_samples` of the event by a multiplier swept from `from_ratio`
    /// to `to_ratio`. After that, freq stays at `freq * to_ratio`.
    #[serde(default)]
    pub pitch_env: Option<PitchEnv>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PitchEnv {
    pub from_ratio: f32,
    pub to_ratio: f32,
    pub time_samples: u64,
    #[serde(default = "default_pitch_shape")]
    pub shape: PitchShape,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PitchShape {
    Linear,
    /// Exponential interpolation in log-frequency space — natural for kick drops.
    #[default]
    Exp,
}

fn default_pitch_shape() -> PitchShape { PitchShape::Exp }

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Envelope {
    Ad { attack: u64, decay: u64 },
    Adsr { attack: u64, decay: u64, sustain: f32, release: u64 },
    Gate,
    Exp { attack: u64, tau: u64 },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FxNode {
    Bitcrush { bits: u8 },
    SampleRateReduce { factor: u32 },
    Reverse,
    Stutter { slice_samples: u64, repeats: u32 },
    CombDelay { delay_samples: u64, feedback: f32 },
    Highpass { cutoff_hz: f32, q: f32 },
    Lowpass { cutoff_hz: f32, q: f32 },
    Bandpass { center_hz: f32, q: f32 },
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("event {index} references unknown voice '{voice}'")]
    UnknownVoice { index: usize, voice: String },
    #[error("event {index} ends at {end} which is past piece duration {duration}")]
    EventPastEnd { index: usize, end: u64, duration: u64 },
    #[error("event {index} has invalid amp {amp} (must be 0..=1)")]
    BadAmp { index: usize, amp: f32 },
    #[error("event {index} has invalid pan {pan} (must be -1..=1)")]
    BadPan { index: usize, pan: f32 },
    #[error("event {index} field {field} is not finite")]
    NotFinite { index: usize, field: &'static str },
    #[error("voice '{voice}' references unknown sample_id '{sample_id}'")]
    UnknownSample { voice: String, sample_id: String },
}

impl Piece {
    pub fn validate(&self) -> Result<(), ValidationError> {
        for (name, v) in &self.voices {
            if let VoiceDef::Sample { sample_id, .. } = v {
                if !self.samples.contains_key(sample_id) {
                    return Err(ValidationError::UnknownSample {
                        voice: name.clone(),
                        sample_id: sample_id.clone(),
                    });
                }
            }
        }
        for (i, e) in self.events.iter().enumerate() {
            if !self.voices.contains_key(&e.voice) {
                return Err(ValidationError::UnknownVoice { index: i, voice: e.voice.clone() });
            }
            let end = e.t.saturating_add(e.dur);
            if end > self.duration_samples {
                return Err(ValidationError::EventPastEnd { index: i, end, duration: self.duration_samples });
            }
            if !e.amp.is_finite() {
                return Err(ValidationError::NotFinite { index: i, field: "amp" });
            }
            if e.amp < 0.0 || e.amp > 1.0 {
                return Err(ValidationError::BadAmp { index: i, amp: e.amp });
            }
            if !e.pan.is_finite() {
                return Err(ValidationError::NotFinite { index: i, field: "pan" });
            }
            if e.pan < -1.0 || e.pan > 1.0 {
                return Err(ValidationError::BadPan { index: i, pan: e.pan });
            }
            if let Some(f) = e.freq {
                if !f.is_finite() {
                    return Err(ValidationError::NotFinite { index: i, field: "freq" });
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piece_roundtrips_json() {
        let mut voices = HashMap::new();
        voices.insert("sin".to_string(), VoiceDef::Sine { default_pan: 0.0 });
        let p = Piece {
            sample_rate: 48000,
            duration_samples: 48000,
            voices,
            samples: HashMap::new(),
            events: vec![Event {
                t: 0,
                voice: "sin".into(),
                freq: Some(440.0),
                dur: 24000,
                amp: 0.5,
                pan: 0.0,
                envelope: Envelope::Ad { attack: 100, decay: 23900 },
                fx: vec![],
                pitch_ratio: None,
                pitch_env: None,
            }],
            metadata: PieceMetadata::default(),
        };
        let s = serde_json::to_string(&p).unwrap();
        let p2: Piece = serde_json::from_str(&s).unwrap();
        assert_eq!(p2.events.len(), 1);
        p2.validate().unwrap();
    }

    #[test]
    fn validation_catches_unknown_voice() {
        let p = Piece {
            sample_rate: 48000,
            duration_samples: 1000,
            voices: HashMap::new(),
            samples: HashMap::new(),
            events: vec![Event {
                t: 0,
                voice: "nope".into(),
                freq: Some(440.0),
                dur: 100,
                amp: 0.5,
                pan: 0.0,
                envelope: Envelope::Gate,
                fx: vec![],
                pitch_ratio: None,
                pitch_env: None,
            }],
            metadata: PieceMetadata::default(),
        };
        assert!(p.validate().is_err());
    }
}
