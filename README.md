# rtracker

Tracker-style generative audio in Rust. Author music as data — patterns of
sparse cells over a row grid — and render it offline to WAV, or edit it live in
a terminal UI.

## Layout

```
crates/rtracker-core     Data model + authoring layer (no audio)
crates/rtracker-render    DSP: voices, envelopes, FX, sample bank, WAV out
apps/rtracker-cli         CLI + TUI editor + MOD importer
```

## The pipeline

Authoring and rendering are deliberately decoupled. Three shapes, compiled
top-down:

```
Song  ──┐  ordered list of patterns on one timeline
        ├─▶ Pattern  tempo + row grid + sparse Tracks of Cells
        │
        └─▶ Piece    flat list of timed Events — the only thing the renderer sees
                     └─▶ Vec<f32>  stereo-interleaved samples ─▶ WAV
```

A `Pattern` compiles to a `Piece`; a `Song` compiles every pattern and
concatenates them. The renderer never sees patterns or songs — only `Piece`
events. Each `Event` names a voice, a start time and duration in samples, a
frequency, an envelope, and a chain of FX.

## CLI

```sh
# Render a piece JSON to stereo WAV
cargo run -p rtracker-cli -- render piece.json out.wav

# Validate without rendering
cargo run -p rtracker-cli -- validate piece.json

# Compile / render a pattern (--loops N to bake repeats)
cargo run -p rtracker-cli -- render-pattern pattern.json out.wav --loops 4

# Compile / render a song (array of patterns)
cargo run -p rtracker-cli -- render-song song.json out.wav

# Chop a WAV into slices → a pattern (one track per slice, laid out in order)
cargo run -p rtracker-cli -- slice break.wav break.json --slices 16
cargo run -p rtracker-cli -- slice break.wav break.json --transient   # cut on onsets

# Import a 4-channel ProTracker MOD to a pattern + extracted sample WAVs
cargo run -p rtracker-cli -- import-mod tune.mod tune.json

# Play a pattern in a loop, hot-reloading on save
cargo run -p rtracker-cli -- loop pattern.json

# Open the TUI editor
cargo run -p rtracker-cli -- tui pattern.json
```

See `examples/` for sample pattern, song, and piece JSON.

## Chopping samples

Rather than cutting a break into separate files, `slice` stores slice points as
`(start, end)` offsets into one WAV — non-destructive and reversible. Two modes:

- **Equal division** (`--slices N`): N evenly-spaced cuts. The default grid
  tempo makes the slices play back gaplessly (one slice per row), so rendering
  the result straight back reproduces the source. Ideal for breakbeats.
- **Transient detection** (`--transient`): finds onsets by short-time energy and
  cuts on the attacks; `--threshold` / `--min-gap-ms` tune sensitivity.

Open the sliced pattern in the TUI and reorder the rows to chop the break. Every
slice boundary is edge-faded automatically (see "declick" in `render/mixer.rs`)
so reordered cuts don't click.

The `squarepusher` example builds a full chopped-Amen drill'n'bass song on top
of the slicer — reordered 16th-note hits, retriggers, reversed tails, comb-
smeared breakdown, and sped-up fills over a fast jazzy sub:

```sh
cargo run -p rtracker-cli --example squarepusher   # → out/squarepusher.wav
```

## TUI keys

| Key        | Action                              |
|------------|-------------------------------------|
| `a`–`g`    | enter note (uppercase = sharp)      |
| `1`–`9`    | set octave                          |
| `Del`/`⌫`  | clear cell                          |
| `[` `]`    | previous / next pattern             |
| `n` / `N`  | new blank / clone pattern           |
| `x`        | delete pattern                      |
| `^←` `^→`  | reorder pattern                     |
| `-` `=`    | shrink / grow pattern (by one beat) |
| `,` `.`    | tempo ∓1 BPM (`<` `>` = ∓10)        |
| `m`        | scope: loop pattern ↔ play song     |
| `f`        | toggle playhead-follow              |
| `Space`    | play / pause                        |
| `R`        | rewind                              |
| `s`        | save (song if >1 pattern, else bare pattern) |
| `q`/`Esc`  | quit                                |

## Voices, envelopes, FX

**Voices:** `sine`, `sine_partials` (additive), `noise_bandpass`, `fm`
(2-operator), `sample` (one-shot / loop / ping-pong). Built-in instrument
presets live in `rtracker-core::presets`.

**Envelopes:** `ad`, `adsr`, `gate`, `exp`. Optional per-event pitch envelope
(linear or exponential) for kick drops and zaps.

**FX:** `bitcrush`, `sample_rate_reduce`, `reverse`, `stutter`, `comb_delay`,
and `lowpass` / `highpass` / `bandpass` (RBJ biquads).

## Notes / limits

- Samples are **not** resampled to the piece rate; a 44.1 kHz sample in a 48 kHz
  piece plays ~8.8% sharp (tracker convention). Use `pitch_ratio` to compensate.
- Multi-channel sample WAVs are folded to mono by averaging.

## Develop

```sh
cargo test --workspace
cargo build --workspace --release
```
