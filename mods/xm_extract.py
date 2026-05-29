#!/usr/bin/env python3
"""Minimal FastTracker II (.xm) sample extractor.

Pulls every instrument's PCM samples out of an XM module and writes them as
16-bit mono WAVs. We only need the raw audio (break loops / drum hits) so we
ignore patterns, envelopes, and effects entirely. Sample data in XM is stored
delta-encoded; we integrate it back to PCM here.

Usage: python3 xm_extract.py <module.xm> <out_dir>
"""
import struct
import sys
import os
import wave


def u16(b, o):
    return struct.unpack_from("<H", b, o)[0]


def u32(b, o):
    return struct.unpack_from("<I", b, o)[0]


def s8(b, o):
    return struct.unpack_from("<b", b, o)[0]


def ascii_clean(raw):
    s = raw.split(b"\x00")[0]
    return "".join(chr(c) if 32 <= c < 127 else "_" for c in s).strip()


def main():
    path, out_dir = sys.argv[1], sys.argv[2]
    os.makedirs(out_dir, exist_ok=True)
    data = open(path, "rb").read()

    if data[:17] != b"Extended Module: ":
        sys.exit("not an XM file")

    module_name = ascii_clean(data[17:37])
    # Header at offset 60: size includes itself.
    header_size = u32(data, 60)
    song_len = u16(data, 64)
    n_channels = u16(data, 68)
    n_patterns = u16(data, 70)
    n_instruments = u16(data, 72)
    bpm = u16(data, 78)
    print(f"module: {module_name!r}  chans={n_channels} pats={n_patterns} "
          f"insts={n_instruments} bpm={bpm}")

    # Skip the pattern blocks to reach the instruments.
    pos = 60 + header_size
    for _ in range(n_patterns):
        ph_len = u32(data, pos)
        packed = u16(data, pos + 7)
        pos += ph_len + packed

    manifest = []
    for inst_i in range(n_instruments):
        inst_start = pos
        inst_hdr_size = u32(data, pos)
        inst_name = ascii_clean(data[pos + 4:pos + 26])
        n_samples = u16(data, pos + 27)
        pos += inst_hdr_size  # jump to first sample header (hdr size covers extra fields)

        if n_samples == 0:
            continue

        # Sample headers are 40 bytes each, back-to-back.
        sample_hdrs = []
        for s in range(n_samples):
            o = inst_start + inst_hdr_size + s * 40
            length = u32(data, o)
            typ = data[o + 14]
            rel_note = s8(data, o + 16)
            name = ascii_clean(data[o + 18:o + 40])
            sample_hdrs.append((length, typ, rel_note, name))
        pos = inst_start + inst_hdr_size + n_samples * 40

        # Sample data follows all headers, in order, delta-encoded.
        for s, (length, typ, rel_note, name) in enumerate(sample_hdrs):
            raw = data[pos:pos + length]
            pos += length
            if length == 0:
                continue
            is16 = bool(typ & 0x10)
            pcm = decode_delta(raw, is16)
            if not pcm:
                continue
            label = name or inst_name or f"inst{inst_i:02d}"
            fname = f"i{inst_i:02d}_s{s}_{sanitize(label)}.wav"
            write_wav(os.path.join(out_dir, fname), pcm)
            secs = len(pcm) / 48000.0
            manifest.append((fname, len(pcm), rel_note, secs, label))

    manifest.sort(key=lambda m: -m[1])
    print("\nextracted samples (largest first):")
    print(f"  {'file':40s} {'frames':>9s} {'relnote':>7s} {'~sec@48k':>9s}  name")
    for fname, n, rel, secs, label in manifest:
        print(f"  {fname:40s} {n:9d} {rel:7d} {secs:9.3f}  {label}")


def decode_delta(raw, is16):
    """XM sample data is stored as deltas; integrate to absolute PCM."""
    out = []
    if is16:
        old = 0
        for i in range(0, len(raw) - 1, 2):
            d = struct.unpack_from("<h", raw, i)[0]
            old = (old + d) & 0xFFFF
            v = old - 0x10000 if old >= 0x8000 else old
            out.append(v)
    else:
        old = 0
        for byte in raw:
            d = byte - 256 if byte >= 128 else byte
            old = (old + d) & 0xFF
            v = old - 256 if old >= 128 else old
            out.append(v << 8)  # scale 8-bit to 16-bit
    return out


def sanitize(name):
    s = "".join(c.lower() if c.isalnum() else "_" for c in name).strip("_")
    return s or "sample"


def write_wav(path, pcm16):
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(48000)
        w.writeframes(struct.pack(f"<{len(pcm16)}h",
                                  *[max(-32768, min(32767, v)) for v in pcm16]))


if __name__ == "__main__":
    main()
