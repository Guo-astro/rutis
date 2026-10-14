#!/usr/bin/env python3
"""Build a single brand sheet SVG from the final assets (for review / README).

Kept square on purpose: the rasteriser used in this repo fits non-square SVGs
by their shorter side and clips the rest.
"""

import os
import re
import sys

sys.path.insert(0, os.path.dirname(__file__))
import gen_rutis_brand as B  # noqa: E402

BASE = os.path.join(os.path.dirname(__file__), "..", "..", "assets", "logo", "brand")
S = 1800.0
CL, CR = 120.0, 1020.0
WL, WR = 840.0, 660.0
INK = "#9AA0A6"


def strip(svg_text):
    body = svg_text.split(">", 1)[1].rsplit("</svg>", 1)[0]
    vb = [float(v) for v in re.search(r'viewBox="([^"]+)"', svg_text).group(1).split()]
    return body, vb


def place(svg_text, x, y, w=None, h=None):
    body, vb = strip(svg_text)
    k = (w / vb[2]) if w else (h / vb[3])
    return (f'  <g transform="translate({x - vb[0] * k:.2f} {y - vb[1] * k:.2f}) '
            f'scale({k:.5f})">\n{body}  </g>\n')


def label(x, y, text):
    return (f'  <text x="{x:.0f}" y="{y:.0f}" font-family="Helvetica Neue,Helvetica,Arial" '
            f'font-size="20" fill="{INK}" letter-spacing="2">{text}</text>\n')


def main():
    src = {n: open(os.path.join(BASE, n)).read() for n in (
        "rutis-lockup.svg", "rutis-lockup-stacked.svg", "rutis-lockup-inverse.svg",
        "rutis-icon-dark.svg", "rutis-icon-light.svg", "rutis-mark-inverse.svg",
        "favicon.svg")}

    s = [f'  <rect width="{S}" height="{S}" fill="#ffffff"/>\n']

    # row 1
    s.append(label(CL, 110, "PRIMARY"))
    s.append(place(src["rutis-lockup.svg"], CL, 140, w=WL))
    s.append(label(CR, 110, "STACKED"))
    s.append(place(src["rutis-lockup-stacked.svg"], CR, 140, h=236))

    # row 2
    s.append(label(CL, 560, "INVERSE"))
    s.append(f'  <rect x="{CL:.0f}" y="{590:.0f}" width="{WL:.0f}" height="180" rx="26" '
             f'fill="#0B0F14"/>\n')
    s.append(place(src["rutis-lockup-inverse.svg"], CL + 186, 615, h=130))
    s.append(label(CR, 560, "APP ICON"))
    s.append(place(src["rutis-icon-dark.svg"], CR, 590, h=180))
    s.append(place(src["rutis-icon-light.svg"], CR + 222, 590, h=180))

    # row 3
    s.append(label(CL, 1010, "FAVICON"))
    x = CL
    for size in (16, 32, 48, 64, 128):
        s.append(place(src["favicon.svg"], x, 1290 - size, h=float(size)))
        s.append(label(x, 1332, str(size)))
        x += size + 52
    s.append(label(CR, 1010, "MARK / INVERSE"))
    s.append(f'  <rect x="{CR:.0f}" y="{1040:.0f}" width="{WR:.0f}" height="240" rx="26" '
             f'fill="#0B0F14"/>\n')
    s.append(place(src["rutis-mark-inverse.svg"], CR + (WR - 278) / 2, 1085, h=150))

    s.append(f'  <text x="{CL:.0f}" y="1690" font-family="Helvetica Neue,Helvetica,Arial" '
             f'font-size="28" fill="#202124">rutis</text>\n')
    s.append(f'  <text x="{CL:.0f}" y="1738" font-family="Helvetica Neue,Helvetica,Arial" '
             f'font-size="22" fill="#5F6368">woven lemniscate · custom monoline wordmark · '
             f'monochrome</text>\n')

    out = os.path.join(BASE, "brand-sheet.svg")
    with open(out, "w") as f:
        f.write(f'<svg xmlns="http://www.w3.org/2000/svg" width="{S:.0f}" height="{S:.0f}" '
                f'viewBox="0 0 {S:.0f} {S:.0f}">\n' + "".join(s) + "</svg>\n")
    print("sheet:", out)


if __name__ == "__main__":
    main()
