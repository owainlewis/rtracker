//! Old-school "intelligent" jungle — LTJ Bukem / Good Looking territory.
//!
//! The opposite of the squarepusher demo: instead of frantic stutter edits, the
//! Amen just *rolls* (slices played mostly in order, the natural break) inside a
//! lush bed of warm 7th-chord pads and a deep, clean sub. A jazzy
//! Dm7 → G7 → Cmaj7 → Am7 progression, one chord per bar; pads + sub intro,
//! drums roll in, a reversed-break breakdown, then a pads-only tail.
//!
//! Pads use the built-in `warm_pad` instrument (slow ADSR bloom, soft partials)
//! — the pattern compiler auto-injects its voice. The sub is a plain sine with a
//! soft attack and a long tail: round and clean, no bitcrush (Bukem subs are
//! smooth, not the gritty drill'n'bass sub).
//!
//! Reuses the vendored break from the squarepusher demo, so it renders from a
//! clean clone via `render-song` too.
//!
//! Run with:  cargo run -p rtracker-cli --example intelligent_jungle

use std::collections::HashMap;
use std::path::Path;

use rtracker_core::{
    Cell, Envelope, FxNode, Note, Pattern, PatternMetadata, SampleLoopMode, SampleRef, Song,
    SongMetadata, Track, VoiceDef,
};
use rtracker_render::{render_with_dir, write_stereo_f32};

const SR: u32 = 48_000;
const BPM: f32 = 160.0; // the break's own tempo → one 16th-slice == one row
const LPB: u32 = 4;
const ROWS: u32 = 16; // one bar per pattern
const N_SLICES: u32 = 64; // 4-bar break → 64 sixteenths; slices 0..15 are bar 1
const SAMPLE_REL: &str = "../squarepusher/amen_160.wav"; // relative to the song JSON's dir

// ---- chords (lush 7ths, voiced in the mid register ~130–350 Hz) ------------
// Each is four pad notes held across the whole bar.
const DM7: &[f32] = &[146.83, 174.61, 220.00, 261.63]; // D3 F3 A3 C4
const G7: &[f32] = &[174.61, 196.00, 246.94, 293.66]; // F3 G3 B3 D4
const CMAJ7: &[f32] = &[130.81, 164.81, 196.00, 246.94]; // C3 E3 G3 B3
const AM7: &[f32] = &[130.81, 164.81, 196.00, 220.00]; // C3 E3 G3 A3

// Deep sub roots, one per chord (round and low).
const SUB_D: f32 = 73.42; // D2
const SUB_G: f32 = 49.00; // G1
const SUB_C: f32 = 65.41; // C2
const SUB_A: f32 = 55.00; // A1

/// One drum hit (an Amen slice) placed on the grid.
struct Hit {
    row: u32,
    slice: u32,
    ratio: f32,
    reverse: bool,
    amp: f32,
    dur: f32,
    extra_fx: Vec<FxNode>,
}

impl Hit {
    fn at(row: u32, slice: u32) -> Self {
        Hit { row, slice, ratio: 1.0, reverse: false, amp: 0.8, dur: 1.0, extra_fx: vec![] }
    }
    fn rev(mut self) -> Self {
        self.reverse = true;
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
    fn fx_chain(&self) -> Vec<FxNode> {
        let mut chain = Vec::new();
        if self.reverse {
            chain.push(FxNode::Reverse);
        }
        chain.extend(self.extra_fx.iter().cloned());
        chain
    }
}

/// One sub-bass note: (row, note Hz, duration in rows).
struct Sub {
    row: u32,
    note: f32,
    dur: f32,
}
fn sub(row: u32, note: f32, dur: f32) -> Sub {
    Sub { row, note, dur }
}

fn main() -> anyhow::Result<()> {
    let dir = Path::new("examples/intelligent_jungle");
    std::fs::create_dir_all(dir)?;
    let sample_path = dir.join(SAMPLE_REL);
    let n_frames = wav_mono_len(&sample_path)?;
    let bounds = equal_bounds(n_frames, N_SLICES);

    // The arrangement: pads+sub intro, the roll comes in, two passes through the
    // progression, a reversed breakdown, then a pads-only outro.
    let bars: Vec<Pattern> = vec![
        bar("intro Dm7", DM7, sub_hold(SUB_D), vec![], &bounds),
        bar("intro Am7", AM7, sub_hold(SUB_A), vec![], &bounds),
        bar("Dm7", DM7, sub_roll(SUB_D), roll(), &bounds),
        bar("G7", G7, sub_roll(SUB_G), roll(), &bounds),
        bar("Cmaj7", CMAJ7, sub_roll(SUB_C), roll(), &bounds),
        bar("Am7", AM7, sub_roll(SUB_A), roll_vary(), &bounds),
        bar("Dm7 b", DM7, sub_roll(SUB_D), roll(), &bounds),
        bar("G7 b", G7, sub_roll(SUB_G), roll(), &bounds),
        bar("Cmaj7 b", CMAJ7, sub_roll(SUB_C), roll(), &bounds),
        bar("Am7 b", AM7, sub_roll(SUB_A), roll_vary(), &bounds),
        bar("breakdown", CMAJ7, sub_hold(SUB_C), breakdown(), &bounds),
        bar("outro Am7", AM7, sub_hold(SUB_A), vec![], &bounds),
    ];

    let song = Song {
        patterns: bars,
        metadata: SongMetadata {
            title: Some("intelligent jungle (bukem-esque)".into()),
            author: Some("rtracker".into()),
        },
    };

    std::fs::write(dir.join("intelligent_jungle.json"), serde_json::to_string_pretty(&song)?)?;

    let piece = song.compile()?;
    let buf = render_with_dir(&piece, dir)?;
    std::fs::create_dir_all("out").ok();
    write_stereo_f32(Path::new("out/intelligent_jungle.wav"), SR, &buf)?;
    println!(
        "wrote examples/intelligent_jungle/intelligent_jungle.json and \
         out/intelligent_jungle.wav ({} patterns, {} events, {:.1}s)",
        song.patterns.len(),
        piece.events.len(),
        piece.duration_samples as f32 / SR as f32,
    );
    Ok(())
}

// ----- pattern construction -------------------------------------------------

fn bar(name: &str, chord: &[f32], subs: Vec<Sub>, hits: Vec<Hit>, bounds: &[(u64, u64)]) -> Pattern {
    let mut voices: HashMap<String, VoiceDef> = HashMap::new();
    let mut samples: HashMap<String, SampleRef> = HashMap::new();
    let mut tracks: Vec<Track> = Vec::new();

    // --- drums: one track per Amen slice this bar triggers ---
    let mut by_slice: HashMap<u32, Vec<&Hit>> = HashMap::new();
    for h in &hits {
        by_slice.entry(h.slice).or_default().push(h);
    }
    let mut used: Vec<u32> = by_slice.keys().copied().collect();
    used.sort();
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
            default_amp: Some(0.72),
            default_pan: Some(0.0),
            default_envelope: Some(Envelope::Gate),
            default_fx: Some(vec![]),
            default_duration_rows: Some(1.0),
            default_pitch_env: None,
            cells,
        });
    }

    // --- pads: warm_pad instrument, one track per chord note, held the bar ---
    // The compiler injects the `warm_pad` voice from presets. Slight stereo
    // spread across the chord tones widens the bed.
    let pans = [-0.5, -0.18, 0.18, 0.5];
    for (i, &freq) in chord.iter().enumerate() {
        tracks.push(Track {
            name: format!("pad{i}"),
            instrument: Some("warm_pad".into()),
            voice: None, // defaults to the instrument's voice ("warm_pad")
            // Four notes stack and the slow release rings into the next bar's
            // attack, so keep each pad voice quiet to leave master headroom.
            default_amp: Some(0.10),
            default_pan: Some(pans[i % pans.len()]),
            default_envelope: None, // warm_pad's slow ADSR bloom
            default_fx: None,
            default_duration_rows: Some(ROWS as f32), // hold the whole bar; release rings over
            default_pitch_env: None,
            cells: vec![Cell {
                row: 0,
                note: Note::Hz(freq),
                pitch_ratio: None,
                velocity: None,
                duration_rows: None,
                pan: None,
                fx: vec![],
                pitch_env: None,
            }],
        });
    }

    // --- sub: clean deep sine, soft attack, long tail ---
    voices.insert("sub".into(), VoiceDef::Sine { default_pan: 0.0 });
    tracks.push(Track {
        name: "sub".into(),
        instrument: None,
        voice: Some("sub".into()),
        default_amp: Some(0.42),
        default_pan: Some(0.0),
        default_envelope: Some(Envelope::Exp { attack: 500, tau: 16000 }),
        default_fx: Some(vec![]), // clean — no bitcrush
        default_duration_rows: Some(4.0),
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
    Ok(reader.len() as u64 / spec.channels.max(1) as u64)
}

// ----- the parts ------------------------------------------------------------

/// The natural rolling break: the 16 sixteenth-slices of bar 1 played in order.
fn roll() -> Vec<Hit> {
    (0..16).map(|r| Hit::at(r, r)).collect()
}

/// A rolling bar with a tasteful turnaround: a ghost retrigger mid-bar and a
/// reversed slice at the phrase end leading into the next chord.
fn roll_vary() -> Vec<Hit> {
    let mut h: Vec<Hit> = (0..15).map(|r| Hit::at(r, r)).collect();
    h.push(Hit::at(7, 7).amp(0.5)); // ghost double on the mid snare
    h.push(Hit::at(15, 15).rev().amp(0.7)); // reversed pickup into the turnaround
    h
}

/// Sparse, reversed, comb-smeared hits for the breakdown bar.
fn breakdown() -> Vec<Hit> {
    let smear = || FxNode::CombDelay { delay_samples: 3200, feedback: 0.55 };
    vec![
        Hit::at(0, 0).amp(0.8),
        Hit::at(6, 8).rev().dur(2.0).fx(smear()),
        Hit::at(12, 4).rev().dur(2.0).fx(smear()),
    ]
}

/// Pad-bed sub: two long half-bar holds — round and sustained.
fn sub_hold(root: f32) -> Vec<Sub> {
    vec![sub(0, root, 8.0), sub(8, root, 8.0)]
}

/// Rolling sub that pulses with the break: root on the one, a mid-bar re-pluck,
/// and an octave lift on the last beat for a touch of movement.
fn sub_roll(root: f32) -> Vec<Sub> {
    vec![sub(0, root, 6.0), sub(6, root, 4.0), sub(10, root, 4.0), sub(14, root * 2.0, 2.0)]
}
