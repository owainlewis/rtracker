// One-shot tool that bakes a small set of drum samples into `samples/`.
// We synthesise them inside rtracker so the demo is self-contained — in real
// use you'd drop your own WAVs in.
//
// Run: cargo run -p rtracker-cli --example bake_samples

use std::collections::HashMap;
use std::path::PathBuf;

use rtracker_core::{Envelope, Event, FxNode, Piece, PieceMetadata, PitchEnv, PitchShape, VoiceDef};

const SR: u32 = 48000;

fn bake_one(name: &str, voice: VoiceDef, base_freq: f32, dur_samples: u64,
            env: Envelope, fx: Vec<FxNode>, pitch_env: Option<PitchEnv>) {
    let mut voices = HashMap::new();
    voices.insert("v".into(), voice);
    let piece = Piece {
        sample_rate: SR,
        duration_samples: dur_samples,
        voices,
        samples: Default::default(),
        events: vec![Event {
            t: 0,
            voice: "v".into(),
            freq: Some(base_freq),
            dur: dur_samples,
            amp: 0.95,
            pan: 0.0,
            envelope: env,
            fx,
            pitch_ratio: None,
            pitch_env,
        }],
        metadata: PieceMetadata { title: Some(name.into()), ..Default::default() },
    };
    let buf = rtracker_render::render(&piece).expect("render");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().parent().unwrap()
        .join("samples").join(format!("{name}.wav"));
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    rtracker_render::write_stereo_f32(&out, SR, &buf).expect("wav write");
    println!("wrote {} ({} samples)", out.display(), dur_samples);
}

fn main() {
    // Kick: 50 Hz sine with a sharp pitch sweep down from 4x.
    bake_one(
        "kick",
        VoiceDef::Sine { default_pan: 0.0 },
        50.0, 9600,
        Envelope::Exp { attack: 30, tau: 3000 },
        vec![],
        Some(PitchEnv { from_ratio: 4.0, to_ratio: 1.0, time_samples: 2400, shape: PitchShape::Exp }),
    );

    // Snare: noise bandpass blast.
    bake_one(
        "snare",
        VoiceDef::NoiseBandpass { q: 2.0, default_pan: 0.0 },
        2200.0, 7200,
        Envelope::Ad { attack: 60, decay: 7140 },
        vec![FxNode::Bitcrush { bits: 8 }],
        None,
    );

    // Hat: tight high-Q noise click.
    bake_one(
        "hat",
        VoiceDef::NoiseBandpass { q: 6.0, default_pan: 0.0 },
        9000.0, 2400,
        Envelope::Ad { attack: 20, decay: 2380 },
        vec![],
        None,
    );

    // Clap: two short noise bursts back to back via Stutter.
    bake_one(
        "clap",
        VoiceDef::NoiseBandpass { q: 1.5, default_pan: 0.0 },
        1500.0, 6000,
        Envelope::Ad { attack: 30, decay: 5970 },
        vec![FxNode::Stutter { slice_samples: 1200, repeats: 3 }],
        None,
    );
}
