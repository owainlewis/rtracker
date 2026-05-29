//! Built-in instrument library. Patterns reference instruments by name; the
//! pattern compiler injects the voice definition and fills any Track fields
//! the pattern itself doesn't override.
//!
//! Add new instruments by writing a function below and registering it in
//! `library()`. Keep names lowercase snake_case.

use std::collections::HashMap;

use crate::{Envelope, FxNode, PitchEnv, PitchShape, VoiceDef};

#[derive(Debug, Clone)]
pub struct Instrument {
    pub name: &'static str,
    pub category: Category,
    pub voice: VoiceDef,
    pub default_envelope: Envelope,
    pub default_fx: Vec<FxNode>,
    pub default_pitch_env: Option<PitchEnv>,
    pub default_duration_rows: f32,
    pub default_amp: f32,
    pub default_pan: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Bell,
    Pad,
    Lead,
    Bass,
    Drum,
    Drone,
    Glitch,
}

pub fn library() -> HashMap<String, Instrument> {
    [
        aphex_pluck(),
        aphex_bell(),
        glassy_bell(),
        metallic_bell(),
        warm_pad(),
        dark_drone(),
        sub_kick(),
        noise_snare(),
        closed_hat(),
        bass_pulse(),
        glitch_zap(),
    ]
    .into_iter()
    .map(|i| (i.name.to_string(), i))
    .collect()
}

pub fn get(name: &str) -> Option<Instrument> {
    library().get(name).cloned()
}

pub fn names() -> Vec<&'static str> {
    vec![
        "aphex_pluck",
        "aphex_bell", "glassy_bell", "metallic_bell",
        "warm_pad", "dark_drone",
        "sub_kick", "noise_snare", "closed_hat",
        "bass_pulse", "glitch_zap",
    ]
}

// ---------- plucks ----------

/// Plucky lead with a long reverb-ish tail built from cascaded comb delays.
/// Aphex Twin "Avril 14th" / "Stone in Focus" territory: a piano-like attack
/// with a spacious bloom that lingers. Not a struck bell — see `aphex_bell`
/// for that.
fn aphex_pluck() -> Instrument {
    Instrument {
        name: "aphex_pluck",
        category: Category::Lead,
        voice: VoiceDef::SinePartials {
            // Mostly harmonic with a hint of detune on upper partials for
            // string-like character.
            partials:   vec![1.0, 2.0, 3.01, 4.05, 5.2],
            amplitudes: vec![1.0, 0.42, 0.20, 0.10, 0.04],
            default_pan: 0.0,
        },
        // Instant attack (1 ms) so it sounds *plucked*, very short body decay
        // (~58 ms tau). The note dies fast; the comb cascade is what you hear
        // afterwards.
        default_envelope: Envelope::Exp { attack: 40, tau: 2800 },
        default_fx: vec![
            FxNode::Bitcrush { bits: 10 },
            // 4 cascaded combs at mutually-non-aligned delays (~31/46/69/104 ms
            // at 48 kHz). Each adds its own echo layer; in series they smear
            // into a diffuse, reverb-ish tail. High feedbacks because each
            // stage attenuates the next.
            FxNode::CombDelay { delay_samples: 1500, feedback: 0.62 },
            FxNode::CombDelay { delay_samples: 2200, feedback: 0.58 },
            FxNode::CombDelay { delay_samples: 3300, feedback: 0.52 },
            FxNode::CombDelay { delay_samples: 5000, feedback: 0.46 },
        ],
        default_pitch_env: None,
        default_duration_rows: 3.0,
        default_amp: 0.5,
        default_pan: 0.0,
    }
}

// ---------- bells ----------

/// Risset-style inharmonic bell with slow bloom + bitcrush + comb tail.
/// Aphex Twin / Boards of Canada territory.
fn aphex_bell() -> Instrument {
    Instrument {
        name: "aphex_bell",
        category: Category::Bell,
        voice: VoiceDef::SinePartials {
            partials:   vec![0.5, 1.0, 2.0, 2.5, 3.0, 4.2, 5.4, 6.8],
            amplitudes: vec![1.0, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.15],
            default_pan: 0.0,
        },
        default_envelope: Envelope::Exp { attack: 8000, tau: 36000 },
        default_fx: vec![
            FxNode::Bitcrush { bits: 8 },
            FxNode::CombDelay { delay_samples: 1800, feedback: 0.55 },
        ],
        default_pitch_env: None,
        default_duration_rows: 4.0,
        default_amp: 0.45,
        default_pan: 0.0,
    }
}

/// Bright glassy bell — fast attack, clearer top end, no FX.
fn glassy_bell() -> Instrument {
    Instrument {
        name: "glassy_bell",
        category: Category::Bell,
        voice: VoiceDef::SinePartials {
            partials:   vec![1.0, 2.0, 3.01, 4.05, 5.1, 7.2],
            amplitudes: vec![1.0, 0.5, 0.3, 0.18, 0.12, 0.06],
            default_pan: 0.0,
        },
        default_envelope: Envelope::Exp { attack: 300, tau: 18000 },
        default_fx: vec![],
        default_pitch_env: None,
        default_duration_rows: 3.0,
        default_amp: 0.4,
        default_pan: 0.0,
    }
}

/// Inharmonic struck-metal — clangorous, industrial.
fn metallic_bell() -> Instrument {
    Instrument {
        name: "metallic_bell",
        category: Category::Bell,
        voice: VoiceDef::SinePartials {
            partials:   vec![1.0, 1.41, 2.13, 2.78, 3.61, 4.92],
            amplitudes: vec![1.0, 0.7, 0.45, 0.30, 0.18, 0.10],
            default_pan: 0.0,
        },
        default_envelope: Envelope::Exp { attack: 40, tau: 9000 },
        default_fx: vec![
            FxNode::Bitcrush { bits: 5 },
            FxNode::CombDelay { delay_samples: 2400, feedback: 0.5 },
        ],
        default_pitch_env: None,
        default_duration_rows: 2.0,
        default_amp: 0.42,
        default_pan: 0.0,
    }
}

// ---------- pads + drones ----------

/// Warm slow pad — gentle bloom, lush partials, soft bitcrush.
fn warm_pad() -> Instrument {
    Instrument {
        name: "warm_pad",
        category: Category::Pad,
        voice: VoiceDef::SinePartials {
            partials:   vec![1.0, 2.0, 3.0, 4.0, 5.0],
            amplitudes: vec![1.0, 0.5, 0.3, 0.15, 0.08],
            default_pan: 0.0,
        },
        default_envelope: Envelope::Adsr {
            attack: 24000, decay: 6000, sustain: 0.7, release: 24000,
        },
        default_fx: vec![FxNode::Bitcrush { bits: 9 }],
        default_pitch_env: None,
        default_duration_rows: 8.0,
        default_amp: 0.35,
        default_pan: 0.0,
    }
}

/// Long low sine drone — for beds, builds, atmosphere.
fn dark_drone() -> Instrument {
    Instrument {
        name: "dark_drone",
        category: Category::Drone,
        voice: VoiceDef::Sine { default_pan: 0.0 },
        default_envelope: Envelope::Adsr {
            attack: 48000, decay: 12000, sustain: 0.6, release: 96000,
        },
        default_fx: vec![],
        default_pitch_env: None,
        default_duration_rows: 16.0,
        default_amp: 0.22,
        default_pan: 0.0,
    }
}

// ---------- drums ----------

/// Sine-based kick with hard pitch sweep — the real "boof".
fn sub_kick() -> Instrument {
    Instrument {
        name: "sub_kick",
        category: Category::Drum,
        voice: VoiceDef::Sine { default_pan: 0.0 },
        default_envelope: Envelope::Exp { attack: 30, tau: 3500 },
        default_fx: vec![],
        default_pitch_env: Some(PitchEnv {
            from_ratio: 4.0, to_ratio: 1.0,
            time_samples: 2400, shape: PitchShape::Exp,
        }),
        default_duration_rows: 1.5,
        default_amp: 0.95,
        default_pan: 0.0,
    }
}

/// Wide bandpass snare crack with bitcrush bite.
fn noise_snare() -> Instrument {
    Instrument {
        name: "noise_snare",
        category: Category::Drum,
        voice: VoiceDef::NoiseBandpass { q: 2.0, default_pan: 0.0 },
        default_envelope: Envelope::Ad { attack: 60, decay: 6000 },
        default_fx: vec![FxNode::Bitcrush { bits: 7 }],
        default_pitch_env: None,
        default_duration_rows: 1.0,
        default_amp: 0.6,
        default_pan: 0.0,
    }
}

/// Tight high-Q noise click for closed hat / tick.
fn closed_hat() -> Instrument {
    Instrument {
        name: "closed_hat",
        category: Category::Drum,
        voice: VoiceDef::NoiseBandpass { q: 6.0, default_pan: 0.0 },
        default_envelope: Envelope::Ad { attack: 20, decay: 1800 },
        default_fx: vec![],
        default_pitch_env: None,
        default_duration_rows: 0.4,
        default_amp: 0.28,
        default_pan: 0.0,
    }
}

// ---------- bass + glitch ----------

/// Bitcrushed sine bass — chunky and digital.
fn bass_pulse() -> Instrument {
    Instrument {
        name: "bass_pulse",
        category: Category::Bass,
        voice: VoiceDef::Sine { default_pan: 0.0 },
        default_envelope: Envelope::Exp { attack: 30, tau: 4500 },
        default_fx: vec![FxNode::Bitcrush { bits: 5 }],
        default_pitch_env: None,
        default_duration_rows: 1.0,
        default_amp: 0.38,
        default_pan: 0.0,
    }
}

/// Glitch zap — short sine with extreme pitch sweep + stutter + bitcrush.
fn glitch_zap() -> Instrument {
    Instrument {
        name: "glitch_zap",
        category: Category::Glitch,
        voice: VoiceDef::Sine { default_pan: 0.0 },
        default_envelope: Envelope::Ad { attack: 20, decay: 3980 },
        default_fx: vec![
            FxNode::Stutter { slice_samples: 400, repeats: 10 },
            FxNode::Bitcrush { bits: 3 },
            FxNode::CombDelay { delay_samples: 1500, feedback: 0.5 },
        ],
        default_pitch_env: Some(PitchEnv {
            from_ratio: 8.0, to_ratio: 0.5,
            time_samples: 1200, shape: PitchShape::Exp,
        }),
        default_duration_rows: 1.0,
        default_amp: 0.4,
        default_pan: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_is_populated() {
        let lib = library();
        assert!(lib.len() >= 10);
        assert!(lib.contains_key("aphex_bell"));
    }

    #[test]
    fn names_match_library() {
        let lib = library();
        for n in names() {
            assert!(lib.contains_key(n), "name '{n}' missing from library");
        }
    }
}
