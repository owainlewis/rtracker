#!/usr/bin/env python3
"""Generate a chopped-Amen jungle pattern JSON for rtracker.

The break `i11_s0_amen_break_fancy_160bp.wav` is 4 bars at 160 BPM. One bar is
72000 frames @ 48k, so a 16th note is 4500 frames. We build the pattern at
160 BPM / lpb 4, where samples_per_row == 4500 == one slice. Reordering rows
therefore chops and rearranges the break.

We slice bar 1 into 16 pieces (amen_00..amen_15), make one Sample voice per
slice, and emit one track per slice index holding cells on the rows where that
slice fires. A sub-bass sine track is layered underneath for low end.
"""
import json
import os

SR = 48000
BPM = 160
LPB = 4
STEP = SR * 60 // (BPM * LPB)          # 4500 frames per 16th
SLICES = 16
BREAK = "junglemix_samples/i11_s0_amen_break_fancy_160bp.wav"

# Strong hits in bar 1 (from RMS analysis): kicks @0,2,10 ; snares @4,12.
# Bar A: play the break straight (validates the chop).
BAR_A = list(range(16))
# Bar B: a jungle edit — doubled kicks, snare moved onto the 3, a stutter tail.
BAR_B = [0, 1, 2, 0,   4, 5, 10, 11,   4, 9, 2, 3,   12, 4, 14, 15]
ORDER = BAR_A + BAR_B                   # 32 rows = 2 bars
ROWS = len(ORDER)

# Sub bass: root-locked jungle sub. Notes as Hz (low sine). A1=55, C2=65.4, etc.
# (row, hz, duration_rows)
SUB = [
    (0, 55.0, 3), (3, 55.0, 1), (6, 65.41, 2), (10, 55.0, 3),
    (16, 55.0, 3), (19, 55.0, 1), (22, 73.42, 2), (26, 49.0, 3), (29, 55.0, 2),
]


def main():
    out_dir = os.path.dirname(os.path.abspath(__file__))
    repo = os.path.dirname(out_dir)

    # SampleRefs: 16 slices of bar 1, all pointing at the same WAV.
    samples = {}
    voices = {}
    for k in range(SLICES):
        sid = f"amen_{k:02d}"
        samples[sid] = {
            "path": f"../mods/{BREAK}",
            "start_sample": k * STEP,
            "end_sample": (k + 1) * STEP,
        }
        voices[sid] = {"kind": "sample", "sample_id": sid, "loop_mode": "one_shot"}
    voices["sub"] = {"kind": "sine", "default_pan": 0.0}

    # One track per slice index actually used; cells on the rows it fires.
    rows_for = {}
    for row, slice_idx in enumerate(ORDER):
        rows_for.setdefault(slice_idx, []).append(row)

    tracks = []
    for slice_idx in sorted(rows_for):
        sid = f"amen_{slice_idx:02d}"
        cells = [{
            "row": row,
            "note": 440.0,          # ignored by sample voices
            "pitch_ratio": 1.0,
        } for row in rows_for[slice_idx]]
        tracks.append({
            "name": f"slice {slice_idx:02d}",
            "voice": sid,
            "default_amp": 0.85,
            "default_pan": 0.0,
            "default_envelope": {"kind": "gate"},
            "default_duration_rows": 1.0,
            "cells": cells,
        })

    # Sub-bass track.
    tracks.append({
        "name": "sub",
        "voice": "sub",
        "default_amp": 0.5,
        "default_pan": 0.0,
        "default_envelope": {"kind": "exp", "attack": 200, "tau": 9000},
        "default_duration_rows": 2.0,
        "cells": [{"row": r, "note": hz, "duration_rows": d} for (r, hz, d) in SUB],
    })

    pattern = {
        "sample_rate": SR,
        "tempo_bpm": BPM,
        "lines_per_beat": LPB,
        "rows": ROWS,
        "voices": voices,
        "samples": samples,
        "tracks": tracks,
        "metadata": {"title": "jungle amen (chopped)", "author": "rtracker"},
    }

    path = os.path.join(repo, "examples", "jungle_amen.json")
    with open(path, "w") as f:
        json.dump(pattern, f, indent=2)
    print(f"wrote {path}  ({ROWS} rows, {len(tracks)} tracks, step={STEP})")


if __name__ == "__main__":
    main()
