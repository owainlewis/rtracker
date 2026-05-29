// Generates examples/noto_alphabet.json as a Pattern (so it opens in the TUI).
// Same audible structure as the previous Piece version — 40 s, 120 BPM,
// LPB 4 → 320 rows at 16th-note resolution. All events happen to land on
// the 16th grid.
//
// Run: cargo run -p rtracker-cli --example noto_alphabet

use std::fs;
use std::path::PathBuf;

use rtracker_core::{
    Cell, Envelope, FxNode, Note, Pattern, PatternMetadata, PitchEnv, PitchShape, Track, VoiceDef,
};

const SR: u32 = 48000;
const BPM: f32 = 120.0;
const LPB: u32 = 4;
const ROWS: u32 = 320;          // 40 s / (1/8 s per row) — i.e. 80 beats × 4

fn cell(row: u32, note_hz: f32, dur_rows: Option<f32>, pan: Option<f32>) -> Cell {
    Cell {
        row,
        note: Note::Hz(note_hz),
        pitch_ratio: None,
        velocity: None,
        duration_rows: dur_rows,
        pan,
        fx: vec![],
        pitch_env: None,
    }
}

fn track(
    name: &str,
    voice: &str,
    amp: f32,
    pan: f32,
    envelope: Envelope,
    duration_rows: f32,
    fx: Vec<FxNode>,
    pitch_env: Option<PitchEnv>,
    cells: Vec<Cell>,
) -> Track {
    Track {
        name: name.into(),
        instrument: None,
        voice: Some(voice.into()),
        default_amp: Some(amp),
        default_pan: Some(pan),
        default_envelope: Some(envelope),
        default_fx: Some(fx),
        default_duration_rows: Some(duration_rows),
        default_pitch_env: pitch_env,
        cells,
    }
}

fn main() {
    // Row helper: 16ths since LPB=4. 1 beat = 4 rows. Section starts in rows:
    //   A:   0  (0   s)
    //   B:  64  (8   s)   = 8 s × 8 rows/s
    //   C: 160  (20  s)
    //   D: 256  (32  s)
    let sec_b = 64u32;
    let sec_b_end = 160u32;
    let sec_c = 160u32;
    let sec_c_end = 256u32;
    let sec_d = 256u32;

    let mut tracks: Vec<Track> = Vec::new();

    // ───── drones (two detuned sub-sines spanning the whole pattern) ─────
    let drone_env = Envelope::Ad { attack: 96000, decay: ROWS as u64 * 6000 - 96000 };
    tracks.push(track(
        "drone_l", "sine", 0.13, -0.2, drone_env, ROWS as f32,
        vec![], None,
        vec![cell(0, 42.0, Some(ROWS as f32), None)],
    ));
    tracks.push(track(
        "drone_r", "sine", 0.11, 0.2, drone_env, ROWS as f32,
        vec![], None,
        vec![cell(0, 43.4, Some(ROWS as f32), None)],
    ));

    // ───── click_sine: short windowed sine bursts with per-cell pan ──────
    let click_env = Envelope::Ad { attack: 8, decay: 272 };   // 280 samples
    let dur_rows = 280.0 / 6000.0;
    let mut clicks: Vec<Cell> = Vec::new();

    // Section A: five isolated clicks.
    for (r, f, p, a) in [
        (4u32,  9800.0_f32, -0.95, 0.42),
        (16,   12200.0,     0.95,  0.38),
        (22,    8400.0,    -0.55,  0.40),
        (42,   14000.0,     0.00,  0.36),
        (60,   10800.0,     0.60,  0.38),
    ] {
        clicks.push(Cell {
            row: r, note: Note::Hz(f), velocity: Some(a), duration_rows: Some(dur_rows),
            pitch_ratio: None, pan: Some(p), fx: vec![], pitch_env: None,
        });
    }

    // Section B: half-beat clicks (every 2 rows), ping-pong, sweeping freq.
    let mut k = 0usize;
    let mut r = sec_b + 2;  // first click at half-beat offset = 2 rows after section start
    while r < sec_b_end {
        let f = 8000.0 + (k as f32 % 9.0) * 700.0;
        let p = if k % 2 == 0 { -0.85 } else { 0.85 };
        clicks.push(Cell {
            row: r, note: Note::Hz(f), velocity: Some(0.30), duration_rows: Some(dur_rows),
            pitch_ratio: None, pan: Some(p), fx: vec![], pitch_env: None,
        });
        r += 2;
        k += 1;
    }

    // Section C: 16th clicks (every row), alternating L/R full. Every 5th gets
    // shunted to the noise-tick track instead.
    let mut n = 0usize;
    let mut r = sec_c;
    while r < sec_c_end {
        let f = 9000.0 + (n as f32 % 11.0) * 600.0;
        let p = if n % 2 == 0 { -0.95 } else { 0.95 };
        if n % 5 != 4 {
            clicks.push(Cell {
                row: r, note: Note::Hz(f), velocity: Some(0.28), duration_rows: Some(dur_rows),
                pitch_ratio: None, pan: Some(p), fx: vec![], pitch_env: None,
            });
        }
        r += 1;
        n += 1;
    }

    // Section D: thinning trail of clicks.
    let recede_rows: [(u32, f32); 7] = [
        (sec_d,        0.32),
        (sec_d + 4,    0.27),
        (sec_d + 8,    0.23),
        (sec_d + 12,   0.20),
        (sec_d + 20,   0.17),
        (sec_d + 32,   0.14),
        (sec_d + 48,   0.10),
    ];
    let recede_pans: [f32; 7] = [-0.9, 0.9, -0.6, 0.4, -0.3, 0.3, 0.0];
    for (i, (r, a)) in recede_rows.iter().enumerate() {
        if *r < ROWS {
            clicks.push(Cell {
                row: *r, note: Note::Hz(8000.0 + (i as f32) * 400.0),
                velocity: Some(*a), duration_rows: Some(dur_rows),
                pitch_ratio: None, pan: Some(recede_pans[i]), fx: vec![], pitch_env: None,
            });
        }
    }

    tracks.push(track(
        "click", "sine", 0.30, 0.0, click_env, dur_rows,
        vec![], None,
        clicks,
    ));

    // ───── noise_tick: bandpass-noise variant clicks in section C ────────
    let mut noise_ticks: Vec<Cell> = Vec::new();
    let mut n = 0usize;
    let mut r = sec_c;
    while r < sec_c_end {
        let f = 9000.0 + (n as f32 % 11.0) * 600.0;
        let p = if n % 2 == 0 { -0.95 } else { 0.95 };
        if n % 5 == 4 {
            noise_ticks.push(Cell {
                row: r, note: Note::Hz(f), velocity: Some(0.22),
                duration_rows: Some(400.0 / 6000.0),
                pitch_ratio: None, pan: Some(p * 0.7), fx: vec![], pitch_env: None,
            });
        }
        r += 1;
        n += 1;
    }
    tracks.push(track(
        "noise_tick", "click", 0.22, 0.0,
        Envelope::Ad { attack: 4, decay: 396 },
        400.0 / 6000.0,
        vec![], None,
        noise_ticks,
    ));

    // ───── pulse: sine pulses on grid (sections B + C) ───────────────────
    let pulse_freqs = [220.0_f32, 233.08, 220.0, 261.63, 220.0, 233.08, 246.94, 261.63];
    let mut pulse_cells: Vec<Cell> = Vec::new();
    // Section B: every beat (4 rows).
    let mut i = 0;
    let mut r = sec_b;
    while r < sec_b_end {
        pulse_cells.push(cell(r, pulse_freqs[i % pulse_freqs.len()], Some(4800.0 / 6000.0), None));
        r += 4;
        i += 1;
    }
    // Section C: every 8th (2 rows), shorter pulses.
    let mut i = 0;
    let mut r = sec_c;
    while r < sec_c_end {
        pulse_cells.push(cell(r, pulse_freqs[i % pulse_freqs.len()], Some(3200.0 / 6000.0), None));
        r += 2;
        i += 1;
    }
    tracks.push(track(
        "pulse", "sine", 0.28, 0.0,
        Envelope::Ad { attack: 80, decay: 4720 },
        4800.0 / 6000.0,
        vec![], None,
        pulse_cells,
    ));

    // ───── sub: pitched-down sine kicks every 2 beats (B), every beat (C),
    //          and one final at row 272 (= sec_d + 16, ≈ +4 beats).
    let mut sub_cells: Vec<Cell> = Vec::new();
    let mut r = sec_b;
    while r < sec_b_end {
        sub_cells.push(cell(r, 45.0, Some(8000.0 / 6000.0), None));
        r += 8;   // 2 beats
    }
    let mut r = sec_c;
    while r < sec_c_end {
        sub_cells.push(cell(r, 45.0, Some(8000.0 / 6000.0), None));
        r += 4;   // 1 beat
    }
    sub_cells.push(Cell {
        row: sec_d + 16, note: Note::Hz(38.0),
        velocity: Some(0.8), duration_rows: Some(24000.0 / 6000.0),
        pitch_ratio: None, pan: Some(0.0), fx: vec![], pitch_env: None,
    });
    tracks.push(track(
        "sub", "sine", 0.7, 0.0,
        Envelope::Exp { attack: 30, tau: 3000 },
        8000.0 / 6000.0,
        vec![],
        Some(PitchEnv { from_ratio: 3.5, to_ratio: 1.0, time_samples: 2400, shape: PitchShape::Exp }),
        sub_cells,
    ));

    // ───── arp: bitcrushed sine arpeggio in section C (8ths, +1 row offset) ─
    let arp_freqs = [440.0_f32, 660.0, 880.0, 660.0, 440.0, 587.33, 880.0, 587.33];
    let mut arp_cells: Vec<Cell> = Vec::new();
    let mut a = 0;
    let mut r = sec_c + 1;     // offset by 1 row = +6000 samples = +BEAT/4 (8th-note offbeat half)
    while r < sec_c_end {
        let pan = if a % 2 == 0 { -0.4 } else { 0.4 };
        arp_cells.push(Cell {
            row: r, note: Note::Hz(arp_freqs[a % arp_freqs.len()]),
            velocity: Some(0.18), duration_rows: Some(2400.0 / 6000.0),
            pitch_ratio: None, pan: Some(pan), fx: vec![], pitch_env: None,
        });
        r += 2;
        a += 1;
    }
    tracks.push(track(
        "arp", "sine", 0.18, 0.0,
        Envelope::Exp { attack: 20, tau: 1200 },
        2400.0 / 6000.0,
        vec![FxNode::Bitcrush { bits: 4 }],
        None,
        arp_cells,
    ));

    // ───── data: SRR'd sine bursts every 2 beats in C ────────────────────
    let mut data_cells: Vec<Cell> = Vec::new();
    let mut r = sec_c + 4;
    while r < sec_c_end {
        data_cells.push(cell(r, 1500.0, Some(4000.0 / 6000.0), None));
        r += 8;
    }
    tracks.push(track(
        "data", "sine", 0.18, 0.0,
        Envelope::Ad { attack: 40, decay: 3960 },
        4000.0 / 6000.0,
        vec![FxNode::SampleRateReduce { factor: 10 }, FxNode::Bitcrush { bits: 5 }],
        None,
        data_cells,
    ));

    // ───── voices ─────────────────────────────────────────────────────────
    let mut voices = std::collections::HashMap::new();
    voices.insert("sine".into(),  VoiceDef::Sine { default_pan: 0.0 });
    voices.insert("click".into(), VoiceDef::NoiseBandpass { q: 60.0, default_pan: 0.0 });

    let pattern = Pattern {
        sample_rate: SR,
        tempo_bpm: BPM,
        lines_per_beat: LPB,
        rows: ROWS,
        voices,
        samples: Default::default(),
        tracks,
        metadata: PatternMetadata {
            title: Some("noto_alphabet".into()),
            author: Some("handmade/programmatic".into()),
        },
    };
    let piece = pattern.compile().expect("must compile");

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().parent().unwrap()
        .join("examples").join("noto_alphabet.json");
    fs::write(&out, serde_json::to_string_pretty(&pattern).unwrap()).unwrap();
    println!(
        "wrote {} — {} tracks, {} cells, {} compiled events, {:.2}s",
        out.display(),
        pattern.tracks.len(),
        pattern.tracks.iter().map(|t| t.cells.len()).sum::<usize>(),
        piece.events.len(),
        pattern.duration_samples() as f32 / SR as f32,
    );
}
