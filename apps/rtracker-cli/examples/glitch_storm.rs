// Generates examples/glitch_storm.json — a dense, polyrhythmic, bitcrushed mess
// designed to push the Phase 1 voice palette as hard as it can go.
//
// Run: cargo run -p rtracker-cli --example glitch_storm

use std::collections::HashMap;
use std::path::PathBuf;

use rtracker_core::{Envelope, Event, FxNode, Piece, PieceMetadata, VoiceDef};

const SR: u32 = 48000;
const BEAT: u64 = 18000; // 160 BPM
const BAR: u64 = BEAT * 4;
const TOTAL: u64 = 30 * SR as u64; // 30 s

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0xDEAD_BEEF_CAFE_BABE } else { seed })
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn f01(&mut self) -> f32 {
        ((self.next() >> 40) as f32) / ((1u32 << 24) as f32)
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f01()
    }
    fn choose<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next() as usize) % xs.len()]
    }
}

fn ev(
    t: u64,
    voice: &str,
    freq: f32,
    dur: u64,
    amp: f32,
    pan: f32,
    env: Envelope,
    fx: Vec<FxNode>,
) -> Event {
    Event {
        t,
        voice: voice.into(),
        freq: Some(freq),
        dur,
        amp,
        pan,
        envelope: env,
        fx,
        pitch_ratio: None,
        pitch_env: None,
    }
}

fn main() {
    let mut rng = Rng::new(0xC0FF_EE_BE_EF);
    let mut events: Vec<Event> = Vec::new();

    // ---------- Section A: build  (0–4 s = 0..192000) ----------
    // Sub on every beat; sparse high clicks ping-ponging.
    let mut t = 0u64;
    while t < 4 * SR as u64 {
        events.push(ev(t, "sub", 48.0, 6000, 0.75, 0.0,
            Envelope::Ad { attack: 40, decay: 5960 }, vec![]));
        t += BEAT;
    }
    for i in 0..8 {
        let t = i * BEAT / 2 + BEAT / 4;
        let pan = if i % 2 == 0 { -0.75 } else { 0.75 };
        events.push(ev(t, "click", 11000.0 + i as f32 * 250.0, 500,
            0.42, pan,
            Envelope::Ad { attack: 6, decay: 494 }, vec![]));
    }

    // ---------- Section B: chaos #1  (4–12 s = 192000..576000) ----------
    let sec_b_start = 4 * SR as u64;
    let sec_b_end = 12 * SR as u64;

    // 1) Sub kicks on 1 and 3 (every half-bar)
    let mut bt = sec_b_start;
    while bt < sec_b_end {
        events.push(ev(bt, "sub", 45.0, 8000, 0.9, 0.0,
            Envelope::Ad { attack: 30, decay: 7970 }, vec![]));
        bt += BAR / 2;
    }

    // 2) 32nd-note click storm with rotating pan and rising frequency micro-sweeps
    let thirty_second = BEAT / 8;
    let mut cnt = 0u64;
    let mut ct = sec_b_start;
    while ct < sec_b_end {
        if cnt % 5 != 4 {                    // 4-of-5 hit rate, gives a swung feel
            let freq = 8000.0 + (cnt % 11) as f32 * 600.0;
            let pan = ((cnt as f32 * 0.37).sin()).clamp(-0.95, 0.95);
            let bits = (4 + (cnt % 5)) as u8;
            events.push(ev(ct, "click", freq, 400, 0.30, pan,
                Envelope::Ad { attack: 4, decay: 396 },
                vec![FxNode::Bitcrush { bits }]));
        }
        ct += thirty_second;
        cnt += 1;
    }

    // 3) Bitcrushed sine arpeggio on 16ths, semi-randomly walking
    let arp_notes = [220.0, 277.18, 329.63, 415.30, 554.37, 659.25];
    let mut at = sec_b_start;
    let mut idx = 0usize;
    while at < sec_b_end {
        let freq = arp_notes[idx % arp_notes.len()];
        let pan = rng.range(-0.6, 0.6);
        events.push(ev(at, "mid", freq, 3500, 0.32, pan,
            Envelope::Exp { attack: 30, tau: 1800 },
            vec![FxNode::Bitcrush { bits: 3 }, FxNode::SampleRateReduce { factor: 6 }]));
        // walk: ±1 with occasional ±2 jump
        let step = if rng.f01() < 0.2 { 2isize } else { 1isize };
        let sign = if rng.f01() < 0.5 { -1isize } else { 1isize };
        idx = ((idx as isize) + sign * step).rem_euclid(arp_notes.len() as isize) as usize;
        at += BEAT / 4;
    }

    // ---------- Section C: BREAK / dropout  (12–14 s) ----------
    // Just one long bitcrushed pad chord + a single reversed bell stab.
    let sec_c_start = 12 * SR as u64;
    events.push(ev(sec_c_start, "mid", 110.0, 2 * SR as u64, 0.45, -0.3,
        Envelope::Exp { attack: 8000, tau: 60000 },
        vec![FxNode::Bitcrush { bits: 2 }]));
    events.push(ev(sec_c_start, "mid", 138.59, 2 * SR as u64, 0.40, 0.3,
        Envelope::Exp { attack: 8000, tau: 60000 },
        vec![FxNode::Bitcrush { bits: 2 }]));
    events.push(ev(sec_c_start + SR as u64, "bell", 880.0, 1 * SR as u64, 0.5, 0.0,
        Envelope::Exp { attack: 200, tau: 24000 },
        vec![FxNode::Reverse, FxNode::Bitcrush { bits: 6 }]));

    // ---------- Section D: chaos #2 — polyrhythm + sweeps  (14–22 s) ----------
    let sec_d_start = 14 * SR as u64;
    let sec_d_end = 22 * SR as u64;

    // 1) Sub: 4-on-the-floor, harder
    let mut bt = sec_d_start;
    while bt < sec_d_end {
        events.push(ev(bt, "sub", 50.0, 7000, 0.95, 0.0,
            Envelope::Ad { attack: 20, decay: 6980 }, vec![]));
        bt += BEAT;
    }

    // 2) Polyrhythm: 5-against-7 micro-clicks
    let span = sec_d_end - sec_d_start;
    for k in 0..40 {
        let t = sec_d_start + (k as u64 * span) / 40;       // every (span/40) ≈ 1/5 beat
        events.push(ev(t, "click", 9000.0 + (k % 7) as f32 * 700.0, 350,
            0.28, -0.85,
            Envelope::Ad { attack: 4, decay: 346 }, vec![]));
    }
    for k in 0..56 {
        let t = sec_d_start + (k as u64 * span) / 56;       // every (span/56) ≈ 1/7 beat
        events.push(ev(t, "click", 12000.0 + (k % 5) as f32 * 900.0, 280,
            0.24, 0.85,
            Envelope::Ad { attack: 4, decay: 276 },
            vec![FxNode::Bitcrush { bits: 5 }]));
    }

    // 3) Real Stutter FX: one event per bar offbeat, stutter chops a 800-sample slice
    //    out of a longer hit and repeats it 8x, with a comb-delay tail for resonance.
    let mut sb = sec_d_start + BEAT / 2;
    while sb < sec_d_end {
        let burst_freq = *rng.choose(&[440.0f32, 554.0, 660.0, 880.0]);
        events.push(ev(sb, "mid", burst_freq, 6400, 0.45, rng.range(-0.6, 0.6),
            Envelope::Ad { attack: 4, decay: 6396 },
            vec![
                FxNode::Stutter { slice_samples: 800, repeats: 8 },
                FxNode::Bitcrush { bits: 4 },
                FxNode::CombDelay { delay_samples: 2400, feedback: 0.55 },
            ]));
        sb += BAR;
    }

    // 4) Pitched-noise sweeps via many bandpass events at sliding freqs
    let sweep_start = sec_d_start + BAR;
    let n_steps = 64;
    for s in 0..n_steps {
        let frac = s as f32 / (n_steps - 1) as f32;
        let f = 500.0 * (1.0 - frac) + 10000.0 * frac;     // 500 → 10000 Hz
        let t = sweep_start + (s as u64) * 800;
        events.push(ev(t, "noise", f, 1200, 0.22, rng.range(-0.5, 0.5),
            Envelope::Ad { attack: 80, decay: 1120 },
            vec![
                FxNode::Bitcrush { bits: 6 },
                FxNode::CombDelay { delay_samples: 900, feedback: 0.45 },
            ]));
    }

    // ---------- Section E: full storm + descending bells  (22–28 s) ----------
    let sec_e_start = 22 * SR as u64;
    let sec_e_end = 28 * SR as u64;

    // Continue 4-on-the-floor sub
    let mut bt = sec_e_start;
    while bt < sec_e_end {
        events.push(ev(bt, "sub", 50.0, 7000, 0.95, 0.0,
            Envelope::Ad { attack: 20, decay: 6980 }, vec![]));
        // Add a snare-like noise burst on 2 & 4 (offset by half beat)
        events.push(ev(bt + BEAT, "noise", 2500.0, 5000, 0.55, 0.0,
            Envelope::Ad { attack: 40, decay: 4960 }, vec![]));
        bt += BEAT * 2;
    }

    // Descending bell stabs every quarter
    let bell_pitches = [1320.0f32, 1100.0, 880.0, 660.0, 554.0, 440.0, 330.0, 220.0];
    let mut bt = sec_e_start;
    let mut bi = 0usize;
    while bt < sec_e_end {
        let f = bell_pitches[bi % bell_pitches.len()];
        events.push(ev(bt, "bell", f, 9000, 0.42, rng.range(-0.7, 0.7),
            Envelope::Exp { attack: 60, tau: 4500 },
            vec![
                FxNode::Bitcrush { bits: (4 + (bi as u8 % 6)).min(12) },
                FxNode::CombDelay { delay_samples: 4500, feedback: 0.5 },
            ]));
        bt += BEAT / 2;
        bi += 1;
    }

    // 16th-note click hi-hats running through whole section
    let mut ct = sec_e_start;
    let mut k = 0u64;
    while ct < sec_e_end {
        let pan = if k % 2 == 0 { -0.7 } else { 0.7 };
        events.push(ev(ct, "click", 9000.0 + (k % 9) as f32 * 500.0, 380, 0.22,
            pan, Envelope::Ad { attack: 4, decay: 376 }, vec![]));
        ct += BEAT / 4;
        k += 1;
    }

    // ---------- Section F: tail  (28–30 s) ----------
    // Sub drone with sample-rate-reduce making it crackle, plus a last reversed swell.
    let sec_f_start = 28 * SR as u64;
    events.push(ev(sec_f_start, "sub", 42.0, TOTAL - sec_f_start, 0.55, 0.0,
        Envelope::Exp { attack: 200, tau: 36000 },
        vec![FxNode::SampleRateReduce { factor: 8 }]));
    events.push(ev(sec_f_start, "bell", 660.0, 1 * SR as u64 + SR as u64 / 2,
        0.5, 0.0,
        Envelope::Exp { attack: 200, tau: 30000 },
        vec![FxNode::Reverse]));

    // ---------- Assemble piece ----------
    let mut voices = HashMap::new();
    voices.insert("sub".to_string(),   VoiceDef::Sine { default_pan: 0.0 });
    voices.insert("mid".to_string(),   VoiceDef::Sine { default_pan: 0.0 });
    voices.insert("click".to_string(), VoiceDef::NoiseBandpass { q: 20.0, default_pan: 0.0 });
    voices.insert("noise".to_string(), VoiceDef::NoiseBandpass { q: 3.0, default_pan: 0.0 });
    voices.insert("bell".to_string(),  VoiceDef::SinePartials {
        partials: vec![1.0, 2.76, 5.4, 8.93],
        amplitudes: vec![1.0, 0.6, 0.35, 0.2],
        default_pan: 0.0,
    });

    let piece = Piece {
        sample_rate: SR,
        duration_samples: TOTAL,
        voices,
        samples: Default::default(),
        events,
        metadata: PieceMetadata {
            title: Some("glitch_storm".into()),
            seed: Some(0xC0FF_EE_BE_EF),
            generator: Some("handmade/programmatic".into()),
            constraints_ref: None,
        },
    };

    piece.validate().expect("piece validation failed");

    let out_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().parent().unwrap()
        .join("examples").join("glitch_storm.json");
    let text = serde_json::to_string_pretty(&piece).expect("serialize");
    std::fs::write(&out_path, text).expect("write");
    println!("wrote {} ({} events, {} s)", out_path.display(), piece.events.len(),
        piece.duration_samples / SR as u64);
}
