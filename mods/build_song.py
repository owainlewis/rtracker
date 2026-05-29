#!/usr/bin/env python3
"""Generate a multi-section jungle SONG from the chopped Amen break.

This is the "order list of bars" that rtracker's engine doesn't have yet: we
define a small library of 1-bar break patterns and sub-bass lines, arrange them
into named sections (intro / main / breakdown / drop / outro), then flatten the
whole thing into one big Pattern and render it. Each bar is 16 rows; at
160 BPM / lpb 4 that's 1 bar = 72000 frames = one bar of the source break.

We slice all 4 bars of the break into 64 slices (bar b, step s -> b*16+s) so
later bars can be used as variation. A None in a bar pattern is a rest.
"""
import json
import os

SR = 48000
BPM = 160
LPB = 4
STEP = SR * 60 // (BPM * LPB)          # 4500 frames per 16th
BARS_IN_BREAK = 4
SLICES = BARS_IN_BREAK * 16            # 64
BREAK = "junglemix_samples/i11_s0_amen_break_fancy_160bp.wav"

# ---- 1-bar break patterns (16 steps; int = slice index, None = rest) --------
# Core grooves use bar-0 slices (analysed: kicks 0,2,10; snares 4,12).
_ = None
STRAIGHT  = list(range(16))                                  # bar 0 verbatim
SPARSE    = [0,_,_,_,  4,_,_,_,  _,_,10,_,  12,_,_,_]        # K . . . S . . . . . K . S . . .
DBLKICK   = [0,1,2,0,  4,5,10,11, 4,9,2,3,  12,4,14,15]      # doubled kicks, snare on the 3
ROLL_END  = [0,1,2,3,  4,5,6,7,  8,9,10,11, 12,12,4,4]       # snare roll into the turnaround
VAR_BAR3  = list(range(48, 64))                              # bar 3 of the source, straight (a fill)

# ---- sub-bass lines (per-bar list of (step, hz, dur_rows)) ------------------
SUB_NONE = []
SUB_ROOT = [(0,55.0,3), (6,55.0,2), (10,55.0,3), (14,73.42,2)]
SUB_MOVE = [(0,55.0,3), (3,55.0,1), (6,65.41,2), (10,49.0,3), (13,55.0,2)]

# ---- arrangement: each entry is one bar (break_pattern, sub_line) -----------
SECTIONS = [
    ("intro",     [(SPARSE, SUB_NONE), (SPARSE, SUB_NONE),
                   (STRAIGHT, SUB_ROOT), (ROLL_END, SUB_ROOT)]),
    ("main",      [(STRAIGHT, SUB_ROOT), (DBLKICK, SUB_MOVE),
                   (STRAIGHT, SUB_ROOT), (ROLL_END, SUB_MOVE)]),
    ("breakdown", [(SPARSE, SUB_MOVE), (SPARSE, SUB_MOVE)]),
    ("drop",      [(DBLKICK, SUB_MOVE), (ROLL_END, SUB_MOVE),
                   (VAR_BAR3, SUB_MOVE), (DBLKICK, SUB_ROOT)]),
    ("outro",     [(STRAIGHT, SUB_ROOT), (SPARSE, SUB_NONE)]),
]


def voices_and_samples():
    """Every pattern carries the full voice + sample map; Song::compile merges
    them. 64 slices (4 bars) plus the sub-bass sine."""
    samples, voices = {}, {}
    for k in range(SLICES):
        sid = f"amen_{k:02d}"
        samples[sid] = {"path": f"../mods/{BREAK}",
                        "start_sample": k * STEP, "end_sample": (k + 1) * STEP}
        voices[sid] = {"kind": "sample", "sample_id": sid, "loop_mode": "one_shot"}
    voices["sub"] = {"kind": "sine", "default_pan": 0.0}
    return voices, samples


def make_pattern(bars):
    """Build one Pattern (a section) from a list of (break_pat, sub_line) bars."""
    voices, samples = voices_and_samples()
    rows_for_slice = {}          # slice_idx -> [rows within this pattern]
    sub_cells = []
    for bar_i, (break_pat, sub_line) in enumerate(bars):
        base = bar_i * 16
        for step, slice_idx in enumerate(break_pat):
            if slice_idx is None:
                continue
            rows_for_slice.setdefault(slice_idx, []).append(base + step)
        for (step, hz, dur) in sub_line:
            sub_cells.append({"row": base + step, "note": hz, "duration_rows": dur})

    tracks = []
    for slice_idx in sorted(rows_for_slice):
        tracks.append({
            "name": f"slice {slice_idx:02d}",
            "voice": f"amen_{slice_idx:02d}",
            "default_amp": 0.85,
            "default_pan": 0.0,
            "default_envelope": {"kind": "gate"},
            "default_duration_rows": 1.0,
            "cells": [{"row": r, "note": 440.0, "pitch_ratio": 1.0}
                      for r in rows_for_slice[slice_idx]],
        })
    tracks.append({
        "name": "sub",
        "voice": "sub",
        "default_amp": 0.5,
        "default_pan": 0.0,
        "default_envelope": {"kind": "exp", "attack": 200, "tau": 9000},
        "default_duration_rows": 2.0,
        "cells": sub_cells,
    })
    return {
        "sample_rate": SR, "tempo_bpm": BPM, "lines_per_beat": LPB,
        "rows": len(bars) * 16, "voices": voices, "samples": samples, "tracks": tracks,
    }


def main():
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

    # One Pattern per section; the song is just the array of them, in order.
    song = {
        "patterns": [make_pattern(bars) for _name, bars in SECTIONS],
        "metadata": {"title": "jungle amen (song)", "author": "rtracker"},
    }

    path = os.path.join(repo, "examples", "jungle_song.json")
    with open(path, "w") as f:
        json.dump(song, f, indent=2)

    total_bars = sum(len(bars) for _n, bars in SECTIONS)
    secs = total_bars * 16 * STEP / SR
    layout = "  ".join(f"{n}:{len(s)}b" for n, s in SECTIONS)
    print(f"wrote {path}")
    print(f"  {len(SECTIONS)} patterns / {total_bars} bars / {secs:.1f}s   [{layout}]")


if __name__ == "__main__":
    main()
