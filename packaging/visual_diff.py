#!/usr/bin/env python3
"""Compare self-test screenshots against the design baseline.

Usage:
  packaging/visual_diff.py CANDIDATE_DIR [--baseline design/baseline]
                           [--threshold 1.0] [--tolerance 8] [--diff-dir DIR]

For every PNG in the baseline directory, the PNG with the same name in
CANDIDATE_DIR is decoded and compared pixel by pixel. A pixel differs when any
channel (RGBA) differs by more than --tolerance (0-255). The script prints the
percentage of differing pixels per image and exits 1 if any image is above
--threshold percent, or if a baseline image has no candidate. Images of
different sizes count as 100% different. With --diff-dir, a mask PNG (red =
differing pixel, dimmed grey = unchanged) is written for each image that
differs.

Standard library only: PNGs are decoded with zlib (8- and 16-bit, all colour
types, non-interlaced and Adam7-interlaced).
"""
import argparse
import struct
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PNG_SIG = b"\x89PNG\r\n\x1a\n"
CHANNELS = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}


class PngError(Exception):
    pass


def _unfilter(raw: bytes, width: int, height: int, bpp: int, bits_per_pixel: int):
    """Undo PNG scanline filters. Returns a list of bytearray rows."""
    stride = (width * bits_per_pixel + 7) // 8
    rows = []
    prev = bytearray(stride)
    pos = 0
    for _ in range(height):
        ftype = raw[pos]
        line = bytearray(raw[pos + 1 : pos + 1 + stride])
        pos += 1 + stride
        if len(line) != stride:
            raise PngError("truncated image data")
        if ftype == 0:
            pass
        elif ftype == 1:  # Sub
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 0xFF
        elif ftype == 2:  # Up
            line = bytearray((a + b) & 0xFF for a, b in zip(line, prev))
        elif ftype == 3:  # Average
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif ftype == 4:  # Paeth
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                if pa <= pb and pa <= pc:
                    pred = a
                elif pb <= pc:
                    pred = b
                else:
                    pred = c
                line[i] = (line[i] + pred) & 0xFF
        else:
            raise PngError(f"bad filter type {ftype}")
        rows.append(line)
        prev = line
    return rows, pos


def _row_to_rgba(line: bytearray, width: int, ctype: int, depth: int, palette, trns) -> list:
    """Convert one unfiltered scanline to a flat list of 8-bit RGBA values."""
    n = CHANNELS[ctype]
    if depth == 16:
        samples = [line[i] for i in range(0, len(line), 2)]  # keep the high byte
    elif depth == 8:
        samples = line
    else:  # 1, 2, 4 bits: unpack, then scale (palette indices are not scaled)
        mask = (1 << depth) - 1
        samples = []
        for byte in line:
            for shift in range(8 - depth, -1, -depth):
                samples.append((byte >> shift) & mask)
        samples = samples[: width * n]
        if ctype != 3:
            scale = 255 // mask
            samples = [s * scale for s in samples]
    out = []
    if ctype == 6:
        out = list(samples[: width * 4])
    elif ctype == 2:
        for i in range(0, width * 3, 3):
            r, g, b = samples[i], samples[i + 1], samples[i + 2]
            out += (r, g, b, 0 if trns == (r, g, b) else 255)
    elif ctype == 0:
        for v in samples[:width]:
            out += (v, v, v, 0 if trns == v else 255)
    elif ctype == 4:
        for i in range(0, width * 2, 2):
            v = samples[i]
            out += (v, v, v, samples[i + 1])
    elif ctype == 3:
        for idx in samples[:width]:
            r, g, b = palette[idx] if idx < len(palette) else (0, 0, 0)
            a = trns[idx] if trns and idx < len(trns) else 255
            out += (r, g, b, a)
    return out


def decode_png(path: Path):
    """Return (width, height, pixels) with pixels a flat bytearray of RGBA."""
    data = path.read_bytes()
    if not data.startswith(PNG_SIG):
        raise PngError("not a PNG file")
    pos = len(PNG_SIG)
    idat = []
    palette, trns_raw, ihdr = [], None, None
    while pos + 8 <= len(data):
        length, ctype_chunk = struct.unpack(">I4s", data[pos : pos + 8])
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if ctype_chunk == b"IHDR":
            ihdr = struct.unpack(">IIBBBBB", body)
        elif ctype_chunk == b"PLTE":
            palette = [tuple(body[i : i + 3]) for i in range(0, len(body), 3)]
        elif ctype_chunk == b"tRNS":
            trns_raw = body
        elif ctype_chunk == b"IDAT":
            idat.append(body)
        elif ctype_chunk == b"IEND":
            break
    if ihdr is None:
        raise PngError("missing IHDR")
    width, height, depth, ctype, _comp, _filt, interlace = ihdr
    if ctype not in CHANNELS or depth not in (1, 2, 4, 8, 16):
        raise PngError(f"unsupported colour type {ctype} / depth {depth}")
    trns = None
    if trns_raw is not None:
        if ctype == 3:
            trns = list(trns_raw)
        elif ctype == 0:
            trns = struct.unpack(">H", trns_raw[:2])[0] >> (8 if depth == 16 else 0)
        elif ctype == 2:
            vals = struct.unpack(">HHH", trns_raw[:6])
            trns = tuple(v >> 8 for v in vals) if depth == 16 else vals
    raw = zlib.decompress(b"".join(idat))
    bits_per_pixel = CHANNELS[ctype] * depth
    bpp = max(1, bits_per_pixel // 8)
    pixels = bytearray(width * height * 4)

    if interlace == 0:
        rows, _ = _unfilter(raw, width, height, bpp, bits_per_pixel)
        for y, line in enumerate(rows):
            pixels[y * width * 4 : (y + 1) * width * 4] = bytes(_row_to_rgba(line, width, ctype, depth, palette, trns))
        return width, height, pixels

    # Adam7: seven reduced images, each placed on its own grid.
    passes = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]
    offset = 0
    for x0, y0, dx, dy in passes:
        pw = (width - x0 + dx - 1) // dx
        ph = (height - y0 + dy - 1) // dy
        if pw <= 0 or ph <= 0:
            continue
        rows, used = _unfilter(raw[offset:], pw, ph, bpp, bits_per_pixel)
        offset += used
        for j, line in enumerate(rows):
            rgba = _row_to_rgba(line, pw, ctype, depth, palette, trns)
            y = y0 + j * dy
            for i in range(pw):
                x = x0 + i * dx
                pixels[(y * width + x) * 4 : (y * width + x) * 4 + 4] = bytes(rgba[i * 4 : i * 4 + 4])
    return width, height, pixels


def encode_png(path: Path, width: int, height: int, rgba: bytes) -> None:
    def chunk(kind: bytes, body: bytes) -> bytes:
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)

    stride = width * 4
    raw = b"".join(b"\x00" + rgba[y * stride : (y + 1) * stride] for y in range(height))
    path.write_bytes(
        PNG_SIG
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 6))
        + chunk(b"IEND", b"")
    )


def compare(a, b, tolerance: int):
    """Return (percent differing, mask RGBA bytes or None)."""
    (wa, ha, pa), (wb, hb, pb) = a, b
    if (wa, ha) != (wb, hb):
        return 100.0, None
    total = wa * ha
    if total == 0:
        return 0.0, None
    if pa == pb:
        return 0.0, None
    differing = 0
    mask = bytearray(total * 4)
    for i in range(0, total * 4, 4):
        if (
            abs(pa[i] - pb[i]) > tolerance
            or abs(pa[i + 1] - pb[i + 1]) > tolerance
            or abs(pa[i + 2] - pb[i + 2]) > tolerance
            or abs(pa[i + 3] - pb[i + 3]) > tolerance
        ):
            differing += 1
            mask[i : i + 4] = b"\xff\x00\x00\xff"
        else:
            g = (pa[i] + pa[i + 1] + pa[i + 2]) // 12
            mask[i : i + 4] = bytes((g, g, g, 255))
    return 100.0 * differing / total, (mask if differing else None)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("candidate", type=Path, help="directory of self-test PNGs")
    ap.add_argument("--baseline", type=Path, default=ROOT / "design" / "baseline")
    ap.add_argument("--threshold", type=float, default=1.0, help="max %% of differing pixels per image (default 1.0)")
    ap.add_argument("--tolerance", type=int, default=8, help="per-channel difference ignored (0-255, default 8)")
    ap.add_argument("--diff-dir", type=Path, help="write a mask PNG for each differing image here")
    args = ap.parse_args(argv)

    baselines = sorted(args.baseline.glob("*.png"))
    if not baselines:
        print(f"visual diff: no baseline PNGs in {args.baseline}; nothing to compare")
        return 0
    if args.diff_dir:
        args.diff_dir.mkdir(parents=True, exist_ok=True)

    failures = 0
    print(f"visual diff: {len(baselines)} baseline image(s), threshold {args.threshold}%, tolerance {args.tolerance}")
    for base in baselines:
        cand = args.candidate / base.name
        if not cand.is_file():
            print(f"  MISSING  {base.name}: no candidate image")
            failures += 1
            continue
        try:
            a, b = decode_png(base), decode_png(cand)
        except (PngError, zlib.error, struct.error, OSError) as e:
            print(f"  ERROR    {base.name}: {e}")
            failures += 1
            continue
        pct, mask = compare(a, b, args.tolerance)
        size_note = "" if a[:2] == b[:2] else f" (size {a[0]}x{a[1]} vs {b[0]}x{b[1]})"
        status = "FAIL" if pct > args.threshold else "ok"
        failures += status == "FAIL"
        print(f"  {status:<8} {base.name}: {pct:.3f}% differing{size_note}")
        if args.diff_dir and mask is not None:
            encode_png(args.diff_dir / base.name, a[0], a[1], bytes(mask))
    if failures:
        print(f"visual diff: {failures} image(s) above threshold or missing", file=sys.stderr)
        return 1
    print("visual diff: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
