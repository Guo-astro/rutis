#!/usr/bin/env python3
"""Custom monoline letterforms for the rutis wordmark.

Only the five letters the name needs are drawn: r u t i s. They are monoline
(a single stroke weight) with round terminals, so the wordmark is literally the
same material as the woven ribbon mark.

Glyph space: baseline at y=0, x-height XH, ascender for t / i-dot above it.
Emitting happens in SVG coordinates, so glyphs are wrapped in scale(1,-1).
"""

import math

S = 100.0                 # stroke weight in glyph units
XH = 460.0                # x-height
K = 0.5522847498          # circle-to-cubic constant
DOT_R = S * 0.62


def _q(cx, cy, r, a0, a1):
    """Cubic approximation of a circular arc, y-up, sweeping a0 -> a1 degrees."""
    a0, a1 = math.radians(a0), math.radians(a1)
    x0, y0 = cx + r * math.cos(a0), cy + r * math.sin(a0)
    x1, y1 = cx + r * math.cos(a1), cy + r * math.sin(a1)
    sweep = a1 - a0
    handle = 4.0 / 3.0 * math.tan(sweep / 4.0)
    c1 = (x0 - handle * r * math.sin(a0), y0 + handle * r * math.cos(a0))
    c2 = (x1 + handle * r * math.sin(a1), y1 - handle * r * math.cos(a1))
    return (x0, y0), c1, c2, (x1, y1)


def _p(pts):
    return " ".join(f"{x:.2f} {y:.2f}" for x, y in pts)


# each glyph: (advance width, list of stroke paths, list of filled dots)

def g_r():
    stem = f"M 50 0 L 50 {XH:.0f}"
    # arm leaves the stem top, turns right, terminal drops to ~65% x-height
    arm = (f"M 50 {XH:.0f} "
           f"C 150 {XH + 18:.0f} 262 {XH - 4:.0f} 276 300")
    return 330, [stem, arm], []


def g_u():
    (x0, y0), c1, c2, (x1, y1) = _q(190, 170, 140, 180, 270)
    (a0, b0), d1, d2, (a1, b1) = _q(190, 170, 140, 270, 360)
    path = (f"M 50 {XH:.0f} L 50 {y0:.2f} "
            f"C {_p([c1])} {_p([c2])} {x1:.2f} {y1:.2f} "
            f"C {_p([d1])} {_p([d2])} {a1:.2f} {b1:.2f} L 330 {XH:.0f}")
    return 380, [path], []


def g_t():
    stem = f"M 190 660 L 190 108"
    tail = "C 190 18 258 -16 332 14"
    bar = "M 40 402 L 344 402"
    return 372, [stem + " " + tail, bar], []


def g_i():
    stem = f"M 60 0 L 60 {XH:.0f}"
    return 180, [stem], [(60.0, XH + 128.0, S * 0.58)]


def g_s():
    # two stacked bowls; the spine descends through the middle to the right
    path = ("M 292 372 "
            "C 292 452 208 486 128 458 "
            "C 48 430 22 356 62 300 "
            "C 102 244 214 232 246 176 "
            "C 278 120 250 44 168 34 "
            "C 96 25 46 66 34 132")
    return 326, [path], []


GLYPHS = {"r": g_r, "u": g_u, "t": g_t, "i": g_i, "s": g_s}
TRACK = 46.0              # letter-spacing in glyph units


def wordmark(text="rutis", stroke=S):
    """Return (svg_fragment, total_width) for the wordmark, y-up glyph space."""
    x = 0.0
    parts = []
    for ch in text:
        adv, paths, dots = GLYPHS[ch]()
        inner = []
        for d in paths:
            inner.append(f'    <path d="{d}"/>')
        for cx, cy, r in dots:
            inner.append(f'    <circle cx="{cx:.2f}" cy="{cy:.2f}" r="{r:.2f}" '
                         f'fill="currentColor" stroke="none"/>')
        parts.append(
            f'  <g transform="translate({x:.2f} 0)" fill="none" stroke="currentColor" '
            f'stroke-width="{stroke:.2f}" stroke-linecap="round" stroke-linejoin="round">\n'
            + "\n".join(inner) + "\n  </g>")
        x += adv + TRACK
    total = x - TRACK
    return "\n".join(parts) + "\n", total


if __name__ == "__main__":
    import os
    frag, w = wordmark()
    pad = 120
    SZ = 1500.0
    scale = (SZ - 2 * pad) / w
    body = (f'  <rect width="{SZ}" height="{SZ}" fill="#ffffff"/>\n'
            f'  <g color="#000000" transform="translate({pad} {SZ - pad:.1f}) '
            f'scale({scale:.4f} {-scale:.4f})">\n'
            f'{frag}  </g>\n')
    out = os.path.join(os.path.dirname(__file__), "..", "..", "assets", "logo", "brand", "preview")
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "preview-wordmark.svg"), "w") as f:
        f.write(f'<svg xmlns="http://www.w3.org/2000/svg" width="{SZ:.0f}" '
                f'height="{SZ:.0f}" viewBox="0 0 {SZ:.0f} {SZ:.0f}">\n{body}</svg>\n')
    print("wordmark width (glyph units):", round(w, 1))
