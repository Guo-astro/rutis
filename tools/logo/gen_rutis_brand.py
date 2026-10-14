#!/usr/bin/env python3
"""Compose the rutis brand set from the woven mark + the custom monoline wordmark.

Outputs (assets/logo/brand/):
  rutis-mark.svg / -inverse.svg          standalone mark, transparent
  rutis-lockup.svg / -inverse.svg        horizontal lockup
  rutis-lockup-stacked.svg               vertical lockup
  rutis-icon-dark.svg / -light.svg       app-icon tiles
  favicon.svg                            optically corrected small-size tile
  favicon/                               raster sizes + favicon.ico

The weave gap is cut with an SVG mask, so every asset is genuinely
transparent-background (an opaque cut-out would show as white).
"""

import math
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from gen_rutis_mark import A, BAND, CX, CY, GAP, bezier_path  # noqa: E402
from rutis_letters import XH, wordmark                       # noqa: E402

BASE = os.path.join(os.path.dirname(__file__), "..", "..", "assets", "logo", "brand")
OUT = BASE

BBOX = (CX - A - BAND / 2, CY - A / 2 - BAND / 2, 2 * A + BAND, A + BAND)
MW, MH = BBOX[2], BBOX[3]          # mark size in mark units

GS = 0.50                          # glyph units -> mark units
XH_U = XH * GS                     # wordmark x-height in mark units

WM_FRAG, WM_W = wordmark()
WM_W_U = WM_W * GS

OVER_ANG = math.degrees(math.atan2(A * math.cos(math.pi), -A * math.sin(math.pi / 2)))


def mark_group(fill, mask_id, band=BAND, eu=6.0, over_extra=24.0):
    """The woven mark, centred on the origin, at an arbitrary stroke weight."""
    ew = band + 2 * eu
    eh = band + 2 * GAP
    ow = ew + over_extra
    oh = band
    bbox = (CX - A - band / 2, CY - A / 2 - band / 2, 2 * A + band, A + band)
    return (
        f'  <defs>\n'
        f'    <mask id="{mask_id}" maskUnits="userSpaceOnUse" x="0" y="0" '
        f'width="1000" height="1000">\n'
        f'      <rect x="0" y="0" width="1000" height="1000" fill="#fff"/>\n'
        f'      <rect x="{CX - ew / 2:.2f}" y="{CY - eh / 2:.2f}" width="{ew:.2f}" '
        f'height="{eh:.2f}" fill="#000" transform="rotate({OVER_ANG:.2f} {CX} {CY})"/>\n'
        f'    </mask>\n'
        f'  </defs>\n'
        f'  <g transform="translate({-bbox[0]:.2f} {-bbox[1]:.2f})">\n'
        f'    <path d="{bezier_path()}" fill="none" stroke="{fill}" '
        f'stroke-width="{band}" stroke-linecap="round" stroke-linejoin="round" '
        f'mask="url(#{mask_id})"/>\n'
        f'    <rect x="{CX - ow / 2:.2f}" y="{CY - oh / 2:.2f}" width="{ow:.2f}" '
        f'height="{oh:.2f}" fill="{fill}" transform="rotate({OVER_ANG:.2f} {CX} {CY})"/>\n'
        f'  </g>\n'
    ), bbox


def wordmark_group(x, baseline_y, color):
    return (f'  <g color="{color}" transform="translate({x:.2f} {baseline_y:.2f}) '
            f'scale({GS} {-GS})">\n{WM_FRAG}  </g>\n')


def svg(w, h, vb, body):
    v = " ".join(f"{n:.2f}" for n in vb)
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0f}" height="{h:.0f}" '
            f'viewBox="{v}" role="img" aria-label="rutis">\n{body}</svg>\n')


def frame(vb, pad=48.0):
    return (vb[0] - pad, vb[1] - pad, vb[2] + 2 * pad, vb[3] + 2 * pad)


def write(name, content):
    path = os.path.join(OUT, name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(content)


def lockup_horizontal(fill, mask_id, gap=96.0):
    body, _ = mark_group(fill, mask_id)
    x0 = MW + gap
    baseline = MH / 2 + XH_U / 2
    body += wordmark_group(x0, baseline, fill)
    top = min(0.0, baseline - XH * 1.45 * GS)
    vb = (0, top, x0 + WM_W_U, 0 - top + baseline - (baseline - MH))
    vb = (0, top, x0 + WM_W_U, MH - top)
    return svg(vb[2] + 96, vb[3] + 96, frame(vb), body)


def lockup_stacked(fill, mask_id, gap=110.0):
    body, _ = mark_group(fill, mask_id)
    baseline = MH + gap + XH_U
    body += wordmark_group((MW - WM_W_U) / 2, baseline, fill)
    vb = (0, 0, MW, baseline)
    return svg(vb[2] + 96, vb[3] + 96, frame(vb), body)


def tile(bg, fg, name, edge=None, radius=232.0, band=BAND, fill_ratio=0.72, id_prefix=None):
    """A rounded-square app tile with the mark centred."""
    S = 1024.0
    mid = id_prefix or name
    grp, bbox = mark_group(fg, mid + "-mask", band=band)
    sc = (S * fill_ratio) / bbox[2]
    tx = (S - bbox[2] * sc) / 2
    ty = (S - bbox[3] * sc) / 2
    out = f'  <rect width="{S}" height="{S}" rx="{radius}" fill="{bg}"/>\n'
    if edge:
        out += (f'  <rect x="2" y="2" width="{S - 4}" height="{S - 4}" rx="{radius - 2}" '
                f'fill="none" stroke="{edge}" stroke-width="4"/>\n')
    out += f'  <g transform="translate({tx:.2f} {ty:.2f}) scale({sc:.5f})">\n{grp}  </g>\n'
    return svg(S, S, (0, 0, S, S), out)


def main():
    body, _ = mark_group("#000000", "mw")
    write("rutis-mark.svg", svg(MW + 96, MH + 96, frame((0, 0, MW, MH)), body))
    body, _ = mark_group("#ffffff", "mwi")
    write("rutis-mark-inverse.svg", svg(MW + 96, MH + 96, frame((0, 0, MW, MH)), body))

    write("rutis-lockup.svg", lockup_horizontal("#000000", "lh"))
    write("rutis-lockup-inverse.svg", lockup_horizontal("#ffffff", "lhi"))
    write("rutis-lockup-stacked.svg", lockup_stacked("#000000", "ls"))

    # canonical single-file master, same geometry as the primary lockup
    write(os.path.join("..", "rutis-logo.svg"),
          lockup_horizontal("#000000", "master"))

    write("rutis-icon-dark.svg", tile("#0A0A0A", "#FFFFFF", "rutis-icon-dark", id_prefix="tile-d"))
    write("rutis-icon-light.svg",
          tile("#FFFFFF", "#0A0A0A", "rutis-icon-light", edge="#E4E4E4", id_prefix="tile-l"))

    # small-size tile: heavier stroke + larger fill so the mark survives 16 px
    write("favicon.svg",
          tile("#0A0A0A", "#FFFFFF", "favicon", radius=200.0,
               band=BAND * 1.70, fill_ratio=0.82, id_prefix="fav"))

    print("brand set written to", os.path.abspath(OUT))


if __name__ == "__main__":
    main()
