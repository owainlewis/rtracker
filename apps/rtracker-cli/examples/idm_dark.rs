// Generates examples/idm_dark.json — slow, broken, dissonant IDM in the
// Autechre / Andy Stott / Vladislav Delay neighbourhood. Lower register,
// inharmonic metallic bells, polymeter, lots of negative space.
//
// Run: cargo run -p rtracker-cli --example idm_dark

use std::collections::HashMap;
use std::path::PathBuf;

use rtracker_core::{Envelope, Event, FxNode, Piece, PieceMetadata, VoiceDef};

const SR: u32 = 48000;
const BEAT: u64 = 28800;          // 100 BPM
const STEP: u64 = BEAT / 4;       // 16th-note grid
const BAR: u64 = BEAT * 4;
const TOTAL: u64 = 30 * SR as u64;

// D dorian / minor-with-flatted-5 cluster — dissonant but coherent.
// Hz: D2, Eb2, F2, Ab2, A2, C3, Eb3
const SCALE: &[f32] = &[73.42, 77.78, 87.31, 103.83, 110.00, 130.81, 155.56];

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self { Self(if seed == 0 { 0xA1A1A1A1 } else { seed }) }
    fn next(&mut self) -> u64 {
        let mut x = self.0; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.0 = x; x
    }
    fn f01(&mut self) -> f32 { ((self.next() >> 40) as f32) / ((1u32 << 24) as f32) }
    fn range(&mut self, lo: f32, hi: f32) -> f32 { lo + (hi - lo) * self.f01() }
    fn choose<'a, T>(&mut self, xs: &'a [T]) -> &'a T { &xs[(self.next() as usize) % xs.len()] }
    fn chance(&mut self, p: f32) -> bool { self.f01() < p }
}

fn ev(t: u64, voice: &str, freq: f32, dur: u64, amp: f32, pan: f32,
      env: Envelope, fx: Vec<FxNode>) -> Event {
    Event { t, voice: voice.into(), freq: Some(freq), dur, amp, pan,
            envelope: env, fx, pitch_ratio: None, pitch_env: None }
}

fn main() {
    let mut rng = Rng::new(0xDEAD_C0DE_DEAD_C0DE);
    let mut events: Vec<Event> = Vec::new();

    // ---------- Section A: drone bed (0–6 s) ----------
    // Two detuned low sines, very long fade-in, slight inharmonicity → unease.
    events.push(ev(0, "drone", 36.7, TOTAL, 0.16, -0.3,
        Envelope::Ad { attack: 96000, decay: TOTAL - 96000 }, vec![]));
    events.push(ev(0, "drone", 38.1, TOTAL, 0.14, 0.3,                // ~67¢ flat — beats slowly
        Envelope::Ad { attack: 96000, decay: TOTAL - 96000 }, vec![]));
    // A low whistle that drifts in and out
    events.push(ev(2 * SR as u64, "drone", 311.13, 8 * SR as u64, 0.08, 0.0, // Eb4
        Envelope::Exp { attack: 24000, tau: 60000 },
        vec![FxNode::Bitcrush { bits: 6 }]));

    // Three distant low clicks
    for i in 0..3 {
        let t = 1 * SR as u64 + i * 2 * SR as u64;
        events.push(ev(t, "tick", 1200.0 + i as f32 * 300.0, 1500,
            0.18, if i % 2 == 0 { -0.6 } else { 0.6 },
            Envelope::Ad { attack: 30, decay: 1470 },
            vec![FxNode::CombDelay { delay_samples: 7200, feedback: 0.55 }]));
    }

    // ---------- Section B: slow heavy sub (6–14 s) ----------
    let sec_b_start = 6 * SR as u64;
    let sec_b_end = 14 * SR as u64;

    // Sub kick every 2 beats, very low.
    let mut bt = sec_b_start;
    let mut beat_idx = 0usize;
    while bt < sec_b_end {
        // Occasional double-tap for IDM jitter
        let double = rng.chance(0.25);
        events.push(ev(bt, "sub", 36.7, 12000, 0.85, 0.0,
            Envelope::Exp { attack: 30, tau: 4500 }, vec![]));
        if double {
            events.push(ev(bt + STEP / 2, "sub", 34.6, 8000, 0.55, 0.0,
                Envelope::Exp { attack: 30, tau: 3500 }, vec![]));
        }
        bt += BEAT * 2;
        beat_idx += 1;
    }
    let _ = beat_idx;

    // Low-mid bandpass percussion on irregular steps (placed by rng, sparse)
    for _ in 0..14 {
        let step = (rng.next() % 32) as u64;
        let t = sec_b_start + step * STEP;
        if t >= sec_b_end { continue; }
        let pan = rng.range(-0.85, 0.85);
        events.push(ev(t, "perc", rng.range(180.0, 550.0), 2400, 0.32, pan,
            Envelope::Ad { attack: 20, decay: 2380 },
            vec![FxNode::Bitcrush { bits: 5 }]));
    }

    // One bitcrushed minor-second cluster stab around 10s
    let stab_t = sec_b_start + BEAT * 4;
    events.push(ev(stab_t, "mid", SCALE[0], 12000, 0.30, -0.4,
        Envelope::Exp { attack: 60, tau: 6000 },
        vec![FxNode::Bitcrush { bits: 3 },
             FxNode::CombDelay { delay_samples: 3600, feedback: 0.5 }]));
    events.push(ev(stab_t, "mid", SCALE[1], 12000, 0.28, 0.4,         // a minor 2nd above — cluster
        Envelope::Exp { attack: 60, tau: 6000 },
        vec![FxNode::Bitcrush { bits: 3 },
             FxNode::CombDelay { delay_samples: 3600, feedback: 0.5 }]));

    // ---------- Section C: tense build (14–18 s) ----------
    let sec_c_start = 14 * SR as u64;
    let sec_c_end = 18 * SR as u64;

    // Comb-delayed metallic bell hits, rising in density
    let hits = 7;
    for i in 0..hits {
        let frac = i as f32 / (hits - 1) as f32;
        let t = sec_c_start + ((sec_c_end - sec_c_start) as f32
            * frac.powf(1.6)) as u64;                                 // accelerate
        let pitch = *rng.choose(SCALE);
        events.push(ev(t, "bell", pitch, 8000, 0.32, rng.range(-0.7, 0.7),
            Envelope::Exp { attack: 40, tau: 4000 },
            vec![FxNode::Bitcrush { bits: 5 },
                 FxNode::CombDelay { delay_samples: 1800, feedback: 0.65 }]));
    }

    // Low rumble noise sweep down (high → low) — feels like pressure dropping
    let sweep_steps = 32;
    for s in 0..sweep_steps {
        let frac = s as f32 / (sweep_steps - 1) as f32;
        let f = 4000.0 * (1.0 - frac) + 200.0 * frac;
        let t = sec_c_start + (s as u64) * ((sec_c_end - sec_c_start) / sweep_steps as u64);
        events.push(ev(t, "noise", f, 1500, 0.14, rng.range(-0.4, 0.4),
            Envelope::Ad { attack: 100, decay: 1400 },
            vec![FxNode::Bitcrush { bits: 6 }]));
    }

    // ---------- Section D: broken IDM beat (18–24 s) ----------
    let sec_d_start = 18 * SR as u64;
    let sec_d_end = 24 * SR as u64;

    // Autechre-ish broken kick: hits on irregular 16th-note steps.
    // 16-step pattern per bar; 1 = kick. Notice: not on 1-5-9-13.
    let kick_pat: [u8; 16] = [1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 1];
    // Snare-ish low-mid bandpass on 5 and 13 (typical) but bitcrushed to a "crack"
    let snare_pat: [u8; 16] = [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0];
    // Stuttered hat: dense, with stutter FX applied
    let hat_pat: [u8; 16] = [1, 0, 1, 1, 1, 0, 1, 0, 1, 1, 0, 1, 1, 1, 0, 1];

    let mut bar_t = sec_d_start;
    while bar_t < sec_d_end {
        for (i, &v) in kick_pat.iter().enumerate() {
            if v == 1 {
                let t = bar_t + i as u64 * STEP;
                let detune = rng.range(-1.0, 1.0);                    // ±1 Hz jitter
                events.push(ev(t, "sub", 38.0 + detune, 9000, 0.95, 0.0,
                    Envelope::Exp { attack: 20, tau: 4000 }, vec![]));
            }
        }
        for (i, &v) in snare_pat.iter().enumerate() {
            if v == 1 {
                let t = bar_t + i as u64 * STEP;
                events.push(ev(t, "noise", rng.range(800.0, 1400.0), 5000,
                    0.55, rng.range(-0.2, 0.2),
                    Envelope::Ad { attack: 30, decay: 4970 },
                    vec![FxNode::Bitcrush { bits: 4 },
                         FxNode::CombDelay { delay_samples: 1200, feedback: 0.4 }]));
            }
        }
        for (i, &v) in hat_pat.iter().enumerate() {
            if v == 1 {
                let t = bar_t + i as u64 * STEP;
                let pan = if i % 2 == 0 { -0.75 } else { 0.75 };
                events.push(ev(t, "tick", rng.range(3500.0, 6500.0), 1200,
                    0.20, pan,
                    Envelope::Ad { attack: 6, decay: 1194 },
                    vec![FxNode::Stutter { slice_samples: 200, repeats: 6 }]));
            }
        }
        bar_t += BAR;
    }

    // 3-against-4 polymeter sub layer (every 3 steps independently)
    let mut pt = sec_d_start + STEP * 2;
    while pt < sec_d_end {
        events.push(ev(pt, "sub", 49.0, 6000, 0.45, rng.range(-0.3, 0.3),
            Envelope::Exp { attack: 20, tau: 3000 },
            vec![FxNode::Bitcrush { bits: 5 }]));
        pt += STEP * 3;
    }

    // One huge stutter-stab per bar at step 7 (offbeat)
    let mut sbar = sec_d_start;
    while sbar < sec_d_end {
        let t = sbar + STEP * 7;
        let f = *rng.choose(&[SCALE[0], SCALE[3], SCALE[5]]);          // root / b5 / b7
        events.push(ev(t, "mid", f, 9000, 0.40, rng.range(-0.5, 0.5),
            Envelope::Ad { attack: 4, decay: 8996 },
            vec![FxNode::Stutter { slice_samples: 600, repeats: 12 },
                 FxNode::Bitcrush { bits: 3 },
                 FxNode::CombDelay { delay_samples: 3000, feedback: 0.6 }]));
        sbar += BAR;
    }

    // ---------- Section E: descent (24–28 s) ----------
    let sec_e_start = 24 * SR as u64;
    let sec_e_end = 28 * SR as u64;

    // Reversed bell stabs falling through the scale, heavy comb-delay
    let descent = [SCALE[6], SCALE[5], SCALE[4], SCALE[3], SCALE[2], SCALE[1], SCALE[0]];
    for (i, &f) in descent.iter().enumerate() {
        let frac = i as f32 / (descent.len() - 1) as f32;
        let t = sec_e_start + (frac * (sec_e_end - sec_e_start) as f32) as u64;
        events.push(ev(t, "bell", f, 14000, 0.42, rng.range(-0.7, 0.7),
            Envelope::Exp { attack: 200, tau: 7000 },
            vec![FxNode::Reverse,
                 FxNode::Bitcrush { bits: 4 },
                 FxNode::CombDelay { delay_samples: 6000, feedback: 0.6 }]));
    }

    // One final, very low, very loud sub thud at the end of the descent
    events.push(ev(sec_e_end - 12000, "sub", 32.0, 16000, 0.95, 0.0,
        Envelope::Exp { attack: 40, tau: 8000 }, vec![]));

    // ---------- Section F: tail (28–30 s) ----------
    // Just the drones decaying; one last sample-rate-crushed sigh.
    events.push(ev(28 * SR as u64, "noise", 600.0, 2 * SR as u64, 0.12, 0.0,
        Envelope::Ad { attack: 8000, decay: 88000 },
        vec![FxNode::SampleRateReduce { factor: 12 },
             FxNode::CombDelay { delay_samples: 3600, feedback: 0.55 }]));

    // ---------- Voices ----------
    let mut voices = HashMap::new();
    voices.insert("drone".into(), VoiceDef::Sine { default_pan: 0.0 });
    voices.insert("sub".into(),   VoiceDef::Sine { default_pan: 0.0 });
    voices.insert("mid".into(),   VoiceDef::Sine { default_pan: 0.0 });
    voices.insert("tick".into(),  VoiceDef::NoiseBandpass { q: 20.0, default_pan: 0.0 });
    voices.insert("perc".into(),  VoiceDef::NoiseBandpass { q: 4.0,  default_pan: 0.0 });
    voices.insert("noise".into(), VoiceDef::NoiseBandpass { q: 2.5,  default_pan: 0.0 });
    // Inharmonic metallic bell — sounds like struck metal, not orchestral bell.
    voices.insert("bell".into(),  VoiceDef::SinePartials {
        partials:   vec![1.0, 1.41, 2.13, 2.78, 3.61, 4.92],
        amplitudes: vec![1.0, 0.7,  0.45, 0.30, 0.18, 0.10],
        default_pan: 0.0,
    });

    let piece = Piece {
        sample_rate: SR,
        duration_samples: TOTAL,
        voices,
        samples: Default::default(),
        events,
        metadata: PieceMetadata {
            title: Some("idm_dark".into()),
            seed: Some(0xDEAD_C0DE_DEAD_C0DE),
            generator: Some("handmade/programmatic".into()),
            constraints_ref: None,
        },
    };
    piece.validate().expect("validation");

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().parent().unwrap()
        .join("examples").join("idm_dark.json");
    std::fs::write(&out, serde_json::to_string_pretty(&piece).unwrap()).unwrap();
    println!("wrote {} ({} events)", out.display(), piece.events.len());
}
