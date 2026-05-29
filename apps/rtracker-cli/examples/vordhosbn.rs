// Generates examples/vordhosbn.json — fast drill-n-bass loop in the Aphex
// Twin "Vordhosbn" / "54 Cymru Beats" neighbourhood. Broken kick pattern,
// stuttered hats, bitcrushed reversed snares, plucky modal lead in B minor,
// bass pulse, dark drone. 4 bars at 140 BPM, 16th-note resolution.
//
// Run: cargo run -p rtracker-cli --example vordhosbn

use std::fs;
use std::path::PathBuf;

use rtracker_core::{
    Cell, FxNode, Note, Pattern, PatternMetadata, Track,
};

const SR: u32 = 48000;
const BPM: f32 = 140.0;
const LPB: u32 = 4;
const ROWS: u32 = 64;       // 4 bars × 16 rows

fn cell(row: u32, note: &str, dur: Option<f32>, fx: Vec<FxNode>) -> Cell {
    Cell {
        row,
        note: Note::Name(note.to_string()),
        pitch_ratio: None,
        velocity: None,
        duration_rows: dur,
        pan: None,
        fx,
        pitch_env: None,
    }
}

fn track(name: &str, instrument: &str, amp: Option<f32>, cells: Vec<Cell>) -> Track {
    Track {
        name: name.into(),
        instrument: Some(instrument.into()),
        voice: None,
        default_amp: amp,
        default_pan: None,
        default_envelope: None,
        default_fx: None,
        default_duration_rows: None,
        default_pitch_env: None,
        cells,
    }
}

fn main() {
    let mut tracks: Vec<Track> = Vec::new();

    // ───── lead: plucky modal melody ─────────────────────────────────────
    // B minor / B Phrygian feel: B C# D E F# G A. Ascends, breaks, descends.
    let lead: &[(u32, &str)] = &[
        // bar 1 — climb
        (0, "B-3"), (2, "D-4"), (4, "F#4"), (6, "A-4"),
        (8, "B-4"), (10, "A-4"), (11, "F#4"), (14, "D-4"),
        // bar 2 — fall
        (16, "G-4"), (18, "F#4"), (20, "E-4"), (22, "D-4"),
        (24, "B-3"), (25, "A-3"), (27, "B-3"), (30, "D-4"),
        // bar 3 — peak
        (32, "B-3"), (34, "D-4"), (36, "F#4"), (38, "B-4"),
        (40, "C#5"), (41, "B-4"), (43, "A-4"), (46, "F#4"),
        // bar 4 — settle, with a final stutter accent
        (48, "F#4"), (50, "E-4"), (52, "D-4"), (54, "D-4"),
        (56, "B-3"), (58, "A-3"), (60, "F#3"), (63, "B-3"),
    ];
    let lead_cells: Vec<Cell> = lead
        .iter()
        .map(|(r, n)| {
            // Final note of each bar gets a small extra fx hit for variety.
            let last_in_bar = (r + 2) % 16 == 0;
            let fx = if last_in_bar { vec![FxNode::Bitcrush { bits: 6 }] } else { vec![] };
            cell(*r, n, Some(2.0), fx)
        })
        .collect();
    tracks.push(track("lead", "aphex_pluck", None, lead_cells));

    // ───── bass: low pulse on bar starts + offbeat stabs ─────────────────
    let bass: &[(u32, &str)] = &[
        (0, "B-2"),  (7, "B-2"),
        (16, "G-2"), (23, "G-2"),
        (32, "B-2"), (39, "B-2"),
        (48, "D-3"), (55, "F#2"), (60, "B-2"),
    ];
    tracks.push(track(
        "bass",
        "bass_pulse",
        Some(0.4),
        bass.iter().map(|(r, n)| cell(*r, n, Some(2.5), vec![])).collect(),
    ));

    // ───── kick: broken Autechre-style pattern, varies per bar ───────────
    // Each pattern has 16 slots = 1 bar. Different patterns across the 4 bars
    // so the loop doesn't feel static.
    let kick_patterns: [[u8; 16]; 4] = [
        [1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0],
        [1, 0, 0, 0, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 0],
        [1, 0, 0, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0],
        [1, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 1, 0, 1, 0, 1],
    ];
    let mut kick_cells = vec![];
    for (bar, pat) in kick_patterns.iter().enumerate() {
        for (i, &v) in pat.iter().enumerate() {
            if v == 1 {
                let row = bar as u32 * 16 + i as u32;
                kick_cells.push(cell(row, "C-2", Some(1.0), vec![]));
            }
        }
    }
    tracks.push(track("kick", "sub_kick", Some(0.9), kick_cells));

    // ───── snare: 5 / 13 of each bar, alternating glitch FX overrides ────
    let mut snare_cells = vec![];
    for bar in 0..4u32 {
        for &i in &[4u32, 12] {
            let row = bar * 16 + i;
            // Every fourth hit gets a reverse for drill'n'bass flavour.
            let hit_idx = bar * 2 + (i / 8);
            let fx = match hit_idx % 4 {
                0 => vec![],
                1 => vec![FxNode::Bitcrush { bits: 4 }],
                2 => vec![FxNode::Stutter { slice_samples: 600, repeats: 6 },
                          FxNode::Bitcrush { bits: 5 }],
                _ => vec![FxNode::Reverse, FxNode::Bitcrush { bits: 5 }],
            };
            snare_cells.push(cell(row, "C-4", Some(1.0), fx));
        }
    }
    tracks.push(track("snare", "noise_snare", Some(0.55), snare_cells));

    // ───── hat: dense 16ths with a Stutter FX baked in for that fine grit ─
    // Skip a few cells to leave space; pan via cell? we don't have per-cell
    // pan yet, so panning is set by the track default (centre). The track
    // *feels* spread because the stutter creates short rhythmic clicks.
    let mut hat_cells = vec![];
    for bar in 0..4u32 {
        for i in 0..16u32 {
            // Hit on most 16ths but skip a few to leave breathing room
            let skip = matches!((bar, i), (_, 5) | (_, 11) | (1, 9) | (3, 13));
            if !skip {
                let row = bar * 16 + i;
                // Occasionally double-stutter for chops
                let fx = if (bar + i) % 7 == 0 {
                    vec![FxNode::Stutter { slice_samples: 200, repeats: 5 }]
                } else {
                    vec![]
                };
                hat_cells.push(cell(row, "C-7", Some(0.4), fx));
            }
        }
    }
    tracks.push(track("hat", "closed_hat", Some(0.22), hat_cells));

    // ───── glitch_zap accents: rare, on irregular rows ───────────────────
    let zap_rows = [9u32, 21, 35, 47, 59];
    let zap_cells: Vec<Cell> = zap_rows
        .iter()
        .map(|r| cell(*r, "A-4", Some(1.0), vec![]))
        .collect();
    tracks.push(track("zap", "glitch_zap", Some(0.32), zap_cells));

    // ───── dark_drone: held bed underneath ───────────────────────────────
    tracks.push(track(
        "drone",
        "dark_drone",
        Some(0.14),
        vec![cell(0, "B-1", Some(ROWS as f32), vec![])],
    ));

    // ───── final reversed bell hit at the very end for a phrase ending ───
    tracks.push(track(
        "tail",
        "metallic_bell",
        Some(0.35),
        vec![cell(62, "F#5", Some(4.0), vec![FxNode::Reverse])],
    ));

    let pattern = Pattern {
        sample_rate: SR,
        tempo_bpm: BPM,
        lines_per_beat: LPB,
        rows: ROWS,
        voices: Default::default(),
        samples: Default::default(),
        tracks,
        metadata: PatternMetadata {
            title: Some("vordhosbn".into()),
            author: Some("handmade/programmatic".into()),
        },
    };
    let _ = pattern.compile().expect("must compile");

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap().parent().unwrap()
        .join("examples").join("vordhosbn.json");
    fs::write(&out, serde_json::to_string_pretty(&pattern).unwrap()).unwrap();
    println!(
        "wrote {} — {} tracks, {} cells, {:.2}s per loop",
        out.display(),
        pattern.tracks.len(),
        pattern.tracks.iter().map(|t| t.cells.len()).sum::<usize>(),
        pattern.duration_samples() as f32 / SR as f32,
    );
}
