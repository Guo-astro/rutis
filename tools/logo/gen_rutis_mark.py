#!/usr/bin/env python3
"""Generate vector variants of the rutis mark.

Concept: a Gerono lemniscate ribbon = the loop software runs through when it
iterates on itself. The crossing is hand-woven: the under-band is cut with a
slot parallel to the over-band, so the mark reads as one physical ribbon
passing over and under itself rather than two merged strokes.

Curves are emitted as cubic Beziers (Hermite control points from the analytic
derivative), so there is no polyline faceting. Only SVG is emitted here;
rasterisation is done separately.
"""

import math
import os

A = 360.0          # horizontal half-width of the lemniscate
CX, CY = 500.0, 500.0
BAND = 62.0        # ribbon thickness
GAP = 20.0         # white margin between the two bands at the crossing
SPANS = 32         # cubic segments around the whole loop

OUT_DIR = os.path.join(os.path.dirname(__file__), "..", "..", "assets", "logo", "vector")

TAU = 2 * math.pi
OVER_T = math.pi / 2       # this pass rides on top
UNDER_T = 3 * math.pi / 2  # this pass passes underneath


def point(t):
    return (CX + A * math.cos(t), CY + (A / 2.0) * math.sin(2 * t))


def deriv(t):
    return (-A * math.sin(t), A * math.cos(2 * t))


def bezier_path():
    """Closed lemniscate as SPANS cubic Beziers."""
    parts = []
    p0 = point(0.0)
    parts.append(f"M {p0[0]:.2f} {p0[1]:.2f}")
    for i in range(SPANS):
        t0 = TAU * i / SPANS
        t1 = TAU * (i + 1) / SPANS
        dt = t1 - t0
        a, b = point(t0), point(t1)
        da, db = deriv(t0), deriv(t1)
        c1 = (a[0] + da[0] * dt / 3.0, a[1] + da[1] * dt / 3.0)
        c2 = (b[0] - db[0] * dt / 3.0, b[1] - db[1] * dt / 3.0)
        parts.append(
            f"C {c1[0]:.2f} {c1[1]:.2f} {c2[0]:.2f} {c2[1]:.2f} {b[0]:.2f} {b[1]:.2f}"
        )
    parts.append("Z")
    return " ".join(parts)


def svg(body, w=1000, h=1000):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" '
        f'viewBox="0 0 {w} {h}" role="img" aria-label="rutis">\n'
        f'  <rect width="{w}" height="{h}" fill="#ffffff"/>\n'
        f'{body}'
        f'</svg>\n'
    )


def band(d, color):
    return (f'  <path d="{d}" fill="none" stroke="{color}" stroke-width="{BAND}" '
            f'stroke-linecap="round" stroke-linejoin="round"/>\n')


def weave():
    """Cut a slot in the under-band parallel to the over-band, then redraw the
    over-band on top. Both operations are rotated rectangles centred on the
    crossing."""
    ang = OVER_T / TAU * 360.0 - 90.0  # not used; angle derived below
    # over-direction unit vector at the crossing
    d = deriv(OVER_T)
    ang = math.degrees(math.atan2(d[1], d[0]))
    eu = 6.0   # how far the cut overshoots the under-band edges
    erase_w, erase_h = BAND + 2 * eu, BAND + 2 * GAP   # along over-dir, across it
    over_w, over_h = erase_w + 24.0, BAND
    out = (
        f'  <rect x="{CX - erase_w / 2:.2f}" y="{CY - erase_h / 2:.2f}" '
        f'width="{erase_w:.2f}" height="{erase_h:.2f}" fill="#ffffff" '
        f'transform="rotate({ang:.2f} {CX} {CY})"/>\n'
        f'  <rect x="{CX - over_w / 2:.2f}" y="{CY - over_h / 2:.2f}" '
        f'width="{over_w:.2f}" height="{over_h:.2f}" fill="#000000" '
        f'transform="rotate({ang:.2f} {CX} {CY})"/>\n'
    )
    return out


def write(name, body):
    with open(os.path.join(OUT_DIR, name), "w") as f:
        f.write(svg(body))


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    d = bezier_path()

    # 1. woven ribbon
    write("rutis-v1-weave.svg", band(d, "#000000") + weave())

    # 2. plain ribbon + bead threaded at the top of the right loop
    apex = (CX + A * math.cos(-math.pi / 4), CY + (A / 2.0) * math.sin(-math.pi / 2))
    write("rutis-v2-bead.svg",
          band(d, "#000000")
          + f'  <circle cx="{apex[0]:.2f}" cy="{apex[1]:.2f}" r="{BAND * 0.78:.2f}" fill="#000000"/>\n')

    # 3. plain ribbon
    write("rutis-v3-plain.svg", band(d, "#000000"))

    # 4. plain ribbon + solid coupling bead at the crossing
    write("rutis-v4-seam.svg",
          band(d, "#000000")
          + f'  <circle cx="{CX}" cy="{CY}" r="{BAND * 0.9:.2f}" fill="#000000"/>\n')

    print("written to", os.path.abspath(OUT_DIR))


if __name__ == "__main__":
    main()
