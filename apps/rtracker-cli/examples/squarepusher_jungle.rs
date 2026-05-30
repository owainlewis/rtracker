//! Squarepusher remix of the intelligent-jungle bed.
//!
//! Takes the warm Dm7 → G7 → Cmaj7 → Am7 jungle harmony and runs the drums
//! through a Tom Jenkinson meat-grinder: reordered slices, stutter rolls,
//! bitcrush/sample-rate-reduce mangling, reversed slabs, and pitch_ratio fills
//! (octave-up snare flams, pitched-down kicks). The mix is rebalanced from the
//! original demo — drums pulled *down* and pads pushed *up* so the harmony
//! actually breathes — and two new melodic layers sit on top:
//!
//! - **bells** (`glassy_bell`): bright chord-tone arpeggios, panned, ringing.
//! - **lead** (`aphex_pluck`): a wistful melody that follows the changes.
//!
//! Reuses the vendored Amen from the squarepusher demo. Renders from a clean
//! clone via `render-song`.
//!
//! Run with:  cargo run -p rtracker-cli --example squarepusher_jungle

use std::collections::HashMap;
use std::path::Path;

use rtracker_core::{
    Cell, Envelope, FxNode, Note, Pattern, PatternMetadata, SampleLoopMode, SampleRef, Song,
    SongMetadata, Track, VoiceDef,
};
use rtracker_render::{render_with_dir, write_stereo_f32};

const SR: u32 = 48_000;
const BPM: f32 = 168.0; // a touch faster than the bed — more frantic
const LPB: u32 = 4;
const ROWS: u32 = 16; // one bar per pattern
const N_SLICES: u32 = 64; // 4-bar break → 64 sixteenths
const SAMPLE_REL: &str = "../squarepusher/amen_160.wav"; // relative to the song JSON's dir

// ---- chords (lush 7ths) ----------------------------------------------------
const DM7: &[f32] = &[146.83, 174.61, 220.00, 261.63]; // D3 F3 A3 C4
const G7: &[f32] = &[174.61, 196.00, 246.94, 293.66]; // F3 G3 B3 D4
const CMAJ7: &[f32] = &[130.81, 164.81, 196.00, 246.94]; // C3 E3 G3 B3
const AM7: &[f32] = &[130.81, 164.81, 196.00, 220.00]; // C3 E3 G3 A3

const SUB_D: f32 = 73.42; // D2
const SUB_G: f32 = 49.00; // G1
const SUB_C: f32 = 65.41; // C2
const SUB_A: f32 = 55.00; // A1

/// One drum hit (an Amen slice).
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
    /// Reverse (if set) leads the chain so any smear trails the reversed audio.
    fn fx_chain(&self) -> Vec<FxNode> {
        let mut chain = Vec::new();
        if self.reverse {
            chain.push(FxNode::Reverse);
        }
        chain.extend(self.extra_fx.iter().cloned());
        chain
    }
}

/// (row, note Hz, duration rows).
struct Note3 {
    row: u32,
    note: f32,
    dur: f32,
}
fn n3(row: u32, note: f32, dur: f32) -> Note3 {
    Note3 { row, note, dur }
}

/// Everything that can sit in one bar.
struct Bar {
    name: &'static str,
    chord: &'static [f32],
    sub: Vec<Note3>,
    hits: Vec<Hit>,
    bells: Vec<Note3>,
    lead: Vec<Note3>,
}

fn main() -> anyhow::Result<()> {
    let dir = Path::new("examples/squarepusher_jungle");
    std::fs::create_dir_all(dir)?;
    let n_frames = wav_mono_len(&dir.join(SAMPLE_REL))?;
    let bounds = equal_bounds(n_frames, N_SLICES);

    let bars = arrangement();
    let patterns: Vec<Pattern> = bars.iter().map(|b| build_bar(b, &bounds)).collect();

    let song = Song {
        patterns,
        metadata: SongMetadata {
            title: Some("squarepusher jungle (remix)".into()),
            author: Some("rtracker".into()),
        },
    };

    std::fs::write(dir.join("squarepusher_jungle.json"), serde_json::to_string_pretty(&song)?)?;

    let piece = song.compile()?;
    let buf = render_with_dir(&piece, dir)?;
    std::fs::create_dir_all("out").ok();
    write_stereo_f32(Path::new("out/squarepusher_jungle.wav"), SR, &buf)?;
    println!(
        "wrote examples/squarepusher_jungle/squarepusher_jungle.json and \
         out/squarepusher_jungle.wav ({} patterns, {} events, {:.1}s)",
        song.patterns.len(),
        piece.events.len(),
        piece.duration_samples as f32 / SR as f32,
    );
    Ok(())
}

// ----- pattern construction -------------------------------------------------

fn build_bar(b: &Bar, bounds: &[(u64, u64)]) -> Pattern {
    let mut voices: HashMap<String, VoiceDef> = HashMap::new();
    let mut samples: HashMap<String, SampleRef> = HashMap::new();
    let mut tracks: Vec<Track> = Vec::new();

    // --- drums: one track per Amen slice this bar triggers. Mix DOWN. ---
    let mut by_slice: HashMap<u32, Vec<&Hit>> = HashMap::new();
    for h in &b.hits {
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
            default_amp: Some(0.36), // ↓ from 0.72 — drums are the transient source; keep them low
            default_pan: Some(0.0),
            default_envelope: Some(Envelope::Gate),
            default_fx: Some(vec![]),
            default_duration_rows: Some(1.0),
            default_pitch_env: None,
            cells,
        });
    }

    // --- pads: warm_pad, one track per chord note, held the bar. Mix UP. ---
    let pans = [-0.55, -0.2, 0.2, 0.55];
    for (i, &freq) in b.chord.iter().enumerate() {
        tracks.push(Track {
            name: format!("pad{i}"),
            instrument: Some("warm_pad".into()),
            voice: None,
            default_amp: Some(0.15), // ↑ from 0.10 — pads now audible
            default_pan: Some(pans[i % pans.len()]),
            default_envelope: None,
            default_fx: None,
            default_duration_rows: Some(ROWS as f32),
            default_pitch_env: None,
            cells: vec![cell_hz(0, freq)],
        });
    }

    // --- bells: glassy_bell arpeggio, alternating pan, ringing tails ---
    if !b.bells.is_empty() {
        tracks.push(Track {
            name: "bell".into(),
            instrument: Some("glassy_bell".into()),
            voice: None,
            default_amp: Some(0.22),
            default_pan: Some(0.0),
            default_envelope: None, // glassy_bell's exp bloom
            default_fx: None,
            default_duration_rows: Some(4.0),
            default_pitch_env: None,
            cells: b
                .bells
                .iter()
                .enumerate()
                .map(|(i, n)| Cell {
                    pan: Some(if i % 2 == 0 { -0.45 } else { 0.45 }),
                    ..cell_dur(n.row, n.note, n.dur)
                })
                .collect(),
        });
    }

    // --- lead: aphex_pluck melody, centre, slight bite ---
    if !b.lead.is_empty() {
        tracks.push(Track {
            name: "lead".into(),
            instrument: Some("aphex_pluck".into()),
            voice: None,
            default_amp: Some(0.25),
            default_pan: Some(0.0),
            default_envelope: None, // aphex_pluck's plucked exp + comb tail
            default_fx: None,
            default_duration_rows: Some(2.0),
            default_pitch_env: None,
            cells: b.lead.iter().map(|n| cell_dur(n.row, n.note, n.dur)).collect(),
        });
    }

    // --- sub: clean deep sine ---
    voices.insert("sub".into(), VoiceDef::Sine { default_pan: 0.0 });
    tracks.push(Track {
        name: "sub".into(),
        instrument: None,
        voice: Some("sub".into()),
        default_amp: Some(0.36),
        default_pan: Some(0.0),
        default_envelope: Some(Envelope::Exp { attack: 500, tau: 16000 }),
        default_fx: Some(vec![]),
        default_duration_rows: Some(4.0),
        default_pitch_env: None,
        cells: b.sub.iter().map(|n| cell_dur(n.row, n.note, n.dur)).collect(),
    });

    Pattern {
        sample_rate: SR,
        tempo_bpm: BPM,
        lines_per_beat: LPB,
        rows: ROWS,
        voices,
        samples,
        tracks,
        metadata: PatternMetadata { title: Some(b.name.into()), author: None },
    }
}

fn cell_hz(row: u32, freq: f32) -> Cell {
    Cell {
        row,
        note: Note::Hz(freq),
        pitch_ratio: None,
        velocity: None,
        duration_rows: None,
        pan: None,
        fx: vec![],
        pitch_env: None,
    }
}
fn cell_dur(row: u32, freq: f32, dur: f32) -> Cell {
    Cell { duration_rows: Some(dur), ..cell_hz(row, freq) }
}

fn slice_id(i: u32) -> String {
    format!("slice_{i:02}")
}
fn equal_bounds(n: u64, slices: u32) -> Vec<(u64, u64)> {
    let slices = slices.max(1) as u64;
    let base = (n / slices).max(1);
    (0..slices).map(|i| (i * base, if i == slices - 1 { n } else { (i + 1) * base })).collect()
}
fn wav_mono_len(path: &Path) -> anyhow::Result<u64> {
    let reader = hound::WavReader::open(path)?;
    let ch = reader.spec().channels.max(1) as u64;
    Ok(reader.len() as u64 / ch)
}

// ----- breaks: the Squarepusher edits ---------------------------------------

/// Bar 1 stated, but already nudged: a stutter on the one and a reversed pickup.
fn break_intro() -> Vec<Hit> {
    let mut h: Vec<Hit> = (0..14).map(|r| Hit::at(r, r)).collect();
    h[0] = Hit::at(0, 0).fx(FxNode::Stutter { slice_samples: 1500, repeats: 4 });
    h.push(Hit::at(14, 6).rev());
    h.push(Hit::at(15, 12).amp(0.7));
    h
}

/// Hard chop: reordered slices, octave-up flam, bitcrushed snare, reverse ghost.
fn break_chop() -> Vec<Hit> {
    vec![
        Hit::at(0, 0),
        Hit::at(1, 0).rate(0.75).amp(0.7), // pitched-down kick tail
        Hit::at(2, 4).fx(FxNode::Bitcrush { bits: 6 }), // crushed snare
        Hit::at(3, 1).amp(0.6),
        Hit::at(4, 8),
        Hit::at(5, 4).rate(2.0).amp(0.6),
        Hit::at(6, 4).rate(2.0).amp(0.75), // accelerating flam
        Hit::at(7, 12).rev(), // reversed backbeat ghost
        Hit::at(8, 0),
        Hit::at(9, 10).fx(FxNode::SampleRateReduce { factor: 6 }),
        Hit::at(10, 4),
        Hit::at(11, 2).rev().amp(0.7),
        Hit::at(12, 8),
        Hit::at(13, 8).rate(1.5).amp(0.6),
        Hit::at(14, 6),
        Hit::at(15, 12).fx(FxNode::Stutter { slice_samples: 700, repeats: 6 }),
    ]
}

/// Rinse: dense stutter rolls and a glitched tail — the busiest bar.
fn break_rinse() -> Vec<Hit> {
    vec![
        Hit::at(0, 0).fx(FxNode::Stutter { slice_samples: 900, repeats: 8 }),
        Hit::at(2, 4),
        Hit::at(3, 4).rate(2.0).amp(0.6),
        Hit::at(4, 8),
        Hit::at(5, 12).amp(0.7),
        Hit::at(6, 0).amp(0.7),
        Hit::at(7, 6).rate(2.0).amp(0.6),
        Hit::at(8, 4).fx(FxNode::Bitcrush { bits: 5 }),
        Hit::at(9, 10).rate(1.5),
        Hit::at(10, 8).fx(FxNode::Stutter { slice_samples: 500, repeats: 8 }),
        Hit::at(12, 12),
        Hit::at(13, 6).rate(2.0).amp(0.7),
        Hit::at(14, 6).rate(2.0).amp(0.8),
        Hit::at(15, 15).rev().fx(FxNode::SampleRateReduce { factor: 8 }),
    ]
}

/// Breakdown: sparse, reversed, comb-smeared slabs over the held sub.
fn break_down() -> Vec<Hit> {
    let smear = || FxNode::CombDelay { delay_samples: 3200, feedback: 0.55 };
    vec![
        Hit::at(0, 0).amp(0.7),
        Hit::at(5, 8).rev().dur(2.0).fx(smear()),
        Hit::at(10, 4).rev().dur(2.0).fx(smear()),
        Hit::at(14, 12).rev().fx(FxNode::Bitcrush { bits: 4 }),
    ]
}

// ----- bed parts ------------------------------------------------------------

fn sub_roll(root: f32) -> Vec<Note3> {
    vec![n3(0, root, 6.0), n3(6, root, 4.0), n3(10, root, 4.0), n3(14, root * 2.0, 2.0)]
}
fn sub_hold(root: f32) -> Vec<Note3> {
    vec![n3(0, root, 8.0), n3(8, root, 8.0)]
}

/// A chord-tone arpeggio for the bell (top three notes of the chord, climbing).
fn bell_arp(chord: &[f32]) -> Vec<Note3> {
    let hi = |i: usize| chord[i.min(chord.len() - 1)] * 2.0; // up an octave, brighter
    vec![n3(0, hi(1), 4.0), n3(4, hi(2), 4.0), n3(8, hi(3), 4.0), n3(12, hi(2), 4.0)]
}

// Lead melody fragments (one octave above the pads, ~290–590 Hz), phrased so
// the four bars answer each other.
fn lead_a() -> Vec<Note3> {
    vec![n3(0, 293.66, 2.0), n3(4, 349.23, 2.0), n3(8, 440.0, 3.0), n3(12, 392.0, 2.0)] // D E A G
}
fn lead_b() -> Vec<Note3> {
    vec![n3(2, 392.0, 2.0), n3(6, 493.88, 2.0), n3(10, 440.0, 4.0)] // G B A
}
fn lead_c() -> Vec<Note3> {
    vec![n3(0, 523.25, 2.0), n3(4, 493.88, 2.0), n3(8, 392.0, 3.0), n3(12, 329.63, 2.0)] // C B G E
}
fn lead_d() -> Vec<Note3> {
    vec![n3(0, 440.0, 3.0), n3(6, 392.0, 2.0), n3(10, 329.63, 4.0)] // A G E
}

// ----- arrangement ----------------------------------------------------------

fn arrangement() -> Vec<Bar> {
    vec![
        // Intro: pads + bells, no lead, gentle break.
        Bar { name: "intro Dm7", chord: DM7, sub: sub_hold(SUB_D), hits: break_intro(),
              bells: bell_arp(DM7), lead: vec![] },
        Bar { name: "intro Am7", chord: AM7, sub: sub_hold(SUB_A), hits: break_intro(),
              bells: bell_arp(AM7), lead: vec![] },
        // First pass: chop, lead enters.
        Bar { name: "Dm7", chord: DM7, sub: sub_roll(SUB_D), hits: break_chop(),
              bells: bell_arp(DM7), lead: lead_a() },
        Bar { name: "G7", chord: G7, sub: sub_roll(SUB_G), hits: break_chop(),
              bells: bell_arp(G7), lead: lead_b() },
        Bar { name: "Cmaj7", chord: CMAJ7, sub: sub_roll(SUB_C), hits: break_rinse(),
              bells: bell_arp(CMAJ7), lead: lead_c() },
        Bar { name: "Am7", chord: AM7, sub: sub_roll(SUB_A), hits: break_chop(),
              bells: bell_arp(AM7), lead: lead_d() },
        // Second pass: rinse-heavy.
        Bar { name: "Dm7 b", chord: DM7, sub: sub_roll(SUB_D), hits: break_rinse(),
              bells: bell_arp(DM7), lead: lead_a() },
        Bar { name: "G7 b", chord: G7, sub: sub_roll(SUB_G), hits: break_chop(),
              bells: bell_arp(G7), lead: lead_b() },
        Bar { name: "Cmaj7 b", chord: CMAJ7, sub: sub_roll(SUB_C), hits: break_rinse(),
              bells: bell_arp(CMAJ7), lead: lead_c() },
        Bar { name: "Am7 b", chord: AM7, sub: sub_roll(SUB_A), hits: break_rinse(),
              bells: bell_arp(AM7), lead: lead_d() },
        // Breakdown then outro.
        Bar { name: "breakdown", chord: CMAJ7, sub: sub_hold(SUB_C), hits: break_down(),
              bells: bell_arp(CMAJ7), lead: lead_c() },
        Bar { name: "outro Am7", chord: AM7, sub: sub_hold(SUB_A), hits: break_down(),
              bells: bell_arp(AM7), lead: vec![] },
    ]
}
