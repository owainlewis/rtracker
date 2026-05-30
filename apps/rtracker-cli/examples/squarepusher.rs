//! Squarepusher-esque chopped-Amen drill'n'bass — built on top of the slicer.
//!
//! Slices the Amen break into sixteenth-notes (the same cut `rtracker slice`
//! makes), then hand-arranges a frantic, edit-heavy drum'n'bass workout over a
//! fast jazzy sub: reordered hits, 16th-note retriggers, reversed tails, and
//! sped-up (pitch_ratio 2) fills — the hallmarks of Tom Jenkinson's amen
//! butchery. Writes the song JSON next to the sample and renders a WAV.
//!
//! The bundled break (examples/squarepusher/amen_160.wav) is 48 kHz mono, 6 s =
//! four bars at 160 BPM, so 64 even slices are sixteenth-notes and slices 0..15
//! are the sixteenths of bar 1.
//!
//! Run with:  cargo run -p rtracker-cli --example squarepusher

use std::collections::HashMap;
use std::path::Path;

use rtracker_core::{
    Cell, Envelope, FxNode, Note, Pattern, PatternMetadata, SampleLoopMode, SampleRef, Song,
    SongMetadata, Track, VoiceDef,
};
use rtracker_render::{render_with_dir, write_stereo_f32};

const SR: u32 = 48_000; // the break's native rate — no resampling, true pitch
const BPM: f32 = 160.0; // the break's own tempo; at this grid one 16th-slice == one row
const LPB: u32 = 4; // 16th-note grid
const ROWS: u32 = 16; // one bar per pattern
const N_SLICES: u32 = 64; // 4-bar break → 64 sixteenths; slices 0..15 are bar 1
const SAMPLE_REL: &str = "amen_160.wav"; // relative to the song JSON's dir

/// One drum hit placed on the grid.
struct Hit {
    row: u32,
    slice: u32,
    ratio: f32,   // playback rate: 1.0 = native, 2.0 = up an octave / sped up
    reverse: bool,
    amp: f32,
    dur: f32,     // duration in rows
    extra_fx: Vec<FxNode>,
}

impl Hit {
    fn at(row: u32, slice: u32) -> Self {
        Hit { row, slice, ratio: 1.0, reverse: false, amp: 0.9, dur: 1.0, extra_fx: vec![] }
    }
    fn rev(mut self) -> Self {
        self.reverse = true;
        self
    }
    fn rate(mut self, r: f32) -> Self {
        self.ratio = r;
        self
    }
    fn amp(mut self, a: f32) -> Self {
        self.amp = a;
        self
    }
    fn dur(mut self, d: f32) -> Self {
        self.dur = d;
        self
    }
    fn fx(mut self, fx: FxNode) -> Self {
        self.extra_fx.push(fx);
        self
    }

    /// FX chain for this hit: reverse first (so a smear trails the reversed
    /// audio), then any extras.
    fn fx_chain(&self) -> Vec<FxNode> {
        let mut chain = Vec::new();
        if self.reverse {
            chain.push(FxNode::Reverse);
        }
        chain.extend(self.extra_fx.iter().cloned());
        chain
    }
}

/// One sub-bass note.
struct Sub {
    row: u32,
    note: f32,
    dur: f32,
}

fn sub(row: u32, note: f32, dur: f32) -> Sub {
    Sub { row, note, dur }
}

// Note frequencies — E natural-minor-ish, the jazz-tinged Squarepusher register.
const E1: f32 = 41.20;
const G1: f32 = 49.00;
const A1: f32 = 55.00;
const B1: f32 = 61.74;
const C2: f32 = 65.41;
const D2: f32 = 73.42;
const E2: f32 = 82.41;

fn main() -> anyhow::Result<()> {
    let dir = Path::new("examples/squarepusher");
    let sample_path = dir.join(SAMPLE_REL);
    let n_frames = wav_mono_len(&sample_path)?;
    let bounds = equal_bounds(n_frames, N_SLICES);

    let patterns = vec![
        build_pattern("head", head_hits(), head_sub(), &bounds),
        build_pattern("chop", chop_hits(), chop_sub(), &bounds),
        build_pattern("rinse", rinse_hits(), rinse_sub(), &bounds),
        build_pattern("break", break_hits(), break_sub(), &bounds),
    ];
    // Arrange: state the break, edit it, restate, go off, edit, rinse, drop,
    // restate.
    let order = [0, 1, 0, 2, 1, 2, 3, 0];
    let song = Song {
        patterns: order.iter().map(|&i| patterns[i].clone()).collect(),
        metadata: SongMetadata {
            title: Some("amen drill (squarepusher-esque)".into()),
            author: Some("rtracker".into()),
        },
    };

    let json = serde_json::to_string_pretty(&song)?;
    std::fs::write(dir.join("squarepusher.json"), json)?;

    let piece = song.compile()?;
    let buf = render_with_dir(&piece, dir)?;
    std::fs::create_dir_all("out").ok();
    write_stereo_f32(Path::new("out/squarepusher.wav"), SR, &buf)?;
    println!(
        "wrote examples/squarepusher/squarepusher.json and out/squarepusher.wav \
         ({} patterns, {} events, {:.1}s)",
        song.patterns.len(),
        piece.events.len(),
        piece.duration_samples as f32 / SR as f32,
    );
    Ok(())
}

// ----- pattern construction -------------------------------------------------

fn build_pattern(name: &str, hits: Vec<Hit>, subs: Vec<Sub>, bounds: &[(u64, u64)]) -> Pattern {
    // Group hits into one track per slice voice.
    let mut by_slice: HashMap<u32, Vec<&Hit>> = HashMap::new();
    for h in &hits {
        by_slice.entry(h.slice).or_default().push(h);
    }
    let mut used: Vec<u32> = by_slice.keys().copied().collect();
    used.sort();

    // Register only the slices this pattern actually triggers (plus the sub).
    let mut voices: HashMap<String, VoiceDef> = HashMap::new();
    let mut samples: HashMap<String, SampleRef> = HashMap::new();
    for &slice in &used {
        let (start, end) = bounds[slice as usize];
        let id = slice_id(slice);
        voices.insert(
            id.clone(),
            VoiceDef::Sample { sample_id: id.clone(), loop_mode: SampleLoopMode::OneShot },
        );
        samples.insert(
            id.clone(),
            SampleRef { path: SAMPLE_REL.into(), start_sample: start, end_sample: end, label: Some(id) },
        );
    }
    voices.insert("sub".into(), VoiceDef::Sine { default_pan: 0.0 });

    let mut tracks: Vec<Track> = Vec::new();
    for slice in used {
        let cells = by_slice[&slice]
            .iter()
            .map(|h| Cell {
                row: h.row,
                note: Note::Hz(1.0),
                pitch_ratio: Some(h.ratio),
                velocity: Some(h.amp),
                duration_rows: Some(h.dur),
                pan: None,
                fx: h.fx_chain(),
                pitch_env: None,
            })
            .collect();
        tracks.push(Track {
            name: format!("s{slice:02}"),
            instrument: None,
            voice: Some(slice_id(slice)),
            default_amp: Some(0.9),
            default_pan: Some(0.0),
            default_envelope: Some(Envelope::Gate),
            default_fx: Some(vec![]),
            default_duration_rows: Some(1.0),
            default_pitch_env: None,
            cells,
        });
    }

    // Sub-bass track: bitcrushed sine, snappy exp decay.
    tracks.push(Track {
        name: "sub".into(),
        instrument: None,
        voice: Some("sub".into()),
        default_amp: Some(0.42),
        default_pan: Some(0.0),
        default_envelope: Some(Envelope::Exp { attack: 60, tau: 5200 }),
        default_fx: Some(vec![FxNode::Bitcrush { bits: 6 }]),
        default_duration_rows: Some(1.0),
        default_pitch_env: None,
        cells: subs
            .iter()
            .map(|s| Cell {
                row: s.row,
                note: Note::Hz(s.note),
                pitch_ratio: None,
                velocity: None,
                duration_rows: Some(s.dur),
                pan: None,
                fx: vec![],
                pitch_env: None,
            })
            .collect(),
    });

    Pattern {
        sample_rate: SR,
        tempo_bpm: BPM,
        lines_per_beat: LPB,
        rows: ROWS,
        voices,
        samples,
        tracks,
        metadata: PatternMetadata { title: Some(name.into()), author: None },
    }
}

fn slice_id(i: u32) -> String {
    format!("slice_{i:02}")
}

/// N even (start, end) frame ranges over [0, n) — the same cut the CLI makes.
fn equal_bounds(n: u64, slices: u32) -> Vec<(u64, u64)> {
    let slices = slices.max(1) as u64;
    let base = (n / slices).max(1);
    (0..slices)
        .map(|i| (i * base, if i == slices - 1 { n } else { (i + 1) * base }))
        .collect()
}

fn wav_mono_len(path: &Path) -> anyhow::Result<u64> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let total = reader.len() as u64; // total samples across channels
    Ok(total / spec.channels.max(1) as u64)
}

// ----- the arrangement ------------------------------------------------------
// Slice indices are the 16 sixteenths of bar 1 of the break, in order.
// Reordering them on the grid is the whole game; exact slice content matters
// less than the rhythm the edits spell out.

/// Bar 1: the break stated straight so the ear locks on before the edits.
fn head_hits() -> Vec<Hit> {
    (0..16).map(|r| Hit::at(r, r)).collect()
}
fn head_sub() -> Vec<Sub> {
    vec![
        sub(0, E1, 2.0), sub(3, E1, 1.0), sub(6, G1, 1.5), sub(8, A1, 1.0),
        sub(10, E1, 2.0), sub(13, B1, 1.0), sub(14, D2, 1.5),
    ]
}

/// Bar 2: classic amen edit — kick stutter on the one, snare pulled early,
/// reversed ghost into the backbeat, retriggered fill at the end.
fn chop_hits() -> Vec<Hit> {
    vec![
        Hit::at(0, 0), Hit::at(1, 0).amp(0.7),            // kick double
        Hit::at(2, 4),                                     // snare early
        Hit::at(3, 1).amp(0.6),
        Hit::at(4, 8), Hit::at(5, 8).amp(0.7),
        Hit::at(6, 2).rev(),                               // reversed ghost
        Hit::at(7, 12),                                    // backbeat snare
        Hit::at(8, 0),
        Hit::at(9, 10),
        Hit::at(10, 4), Hit::at(11, 4).amp(0.6),
        Hit::at(12, 8),
        Hit::at(13, 14).rev(),
        Hit::at(14, 6), Hit::at(15, 12).amp(0.8),
    ]
}
fn chop_sub() -> Vec<Sub> {
    vec![
        sub(0, E1, 1.0), sub(2, E1, 0.5), sub(4, A1, 1.0), sub(6, G1, 1.0),
        sub(8, E1, 1.0), sub(9, B1, 0.5), sub(11, C2, 1.0), sub(13, A1, 1.0),
        sub(15, E2, 1.0),
    ]
}

/// Bar 3: rinse — fast 16th rolls, a sped-up (octave-up) snare flam, and a
/// reversed tail. Busiest sub.
fn rinse_hits() -> Vec<Hit> {
    vec![
        Hit::at(0, 0),
        Hit::at(1, 4).rate(2.0).amp(0.6), Hit::at(2, 4).rate(2.0).amp(0.7),
        Hit::at(3, 4).rate(2.0).amp(0.85),                 // accelerating flam
        Hit::at(4, 8),
        Hit::at(5, 12).amp(0.7),
        Hit::at(6, 0), Hit::at(7, 0).amp(0.6),
        Hit::at(8, 4),
        Hit::at(9, 10).rate(1.5),
        Hit::at(10, 8), Hit::at(11, 8).amp(0.6),
        Hit::at(12, 12),
        Hit::at(13, 6).rate(2.0).amp(0.7), Hit::at(14, 6).rate(2.0).amp(0.8),
        Hit::at(15, 15).rev(),                             // reversed tail into next bar
    ]
}
fn rinse_sub() -> Vec<Sub> {
    vec![
        sub(0, E1, 0.5), sub(1, E1, 0.5), sub(2, G1, 0.5), sub(3, A1, 0.5),
        sub(4, E1, 1.0), sub(6, B1, 0.5), sub(7, C2, 0.5), sub(8, E1, 1.0),
        sub(10, D2, 0.5), sub(11, B1, 0.5), sub(12, A1, 1.0), sub(14, G1, 0.5),
        sub(15, E2, 0.5),
    ]
}

/// Bar 4: breakdown — sparse, reversed slices smeared with comb delay over a
/// long low sub, then a sped-up pickup to kick the loop back in.
fn break_hits() -> Vec<Hit> {
    let smear = || FxNode::CombDelay { delay_samples: 2600, feedback: 0.6 };
    vec![
        Hit::at(0, 0).amp(0.85),
        Hit::at(4, 8).rev().dur(2.0).fx(smear()),
        Hit::at(8, 6).rev().dur(2.0).fx(smear()),
        Hit::at(10, 12).rev().dur(2.0).fx(smear()),
        Hit::at(14, 0).amp(0.8),
        Hit::at(15, 4).rate(2.0).amp(0.7),
    ]
}
fn break_sub() -> Vec<Sub> {
    vec![sub(0, E1, 8.0), sub(8, G1, 6.0), sub(14, A1, 2.0)]
}
