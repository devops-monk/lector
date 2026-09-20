#!/usr/bin/env python3
"""Generate Lector's icons from one vector description.

Two marks, because they have different jobs:

  * the menu bar wants a *template* image -- pure black plus alpha, which macOS
    tints itself for light and dark menu bars. It has to read at 18px, so it
    carries no detail that would turn to mush at that size.
  * the app icon is the one people see in Finder and the Dock, at 512px and up,
    so it can afford a background and a little texture.

Both are drawn at 8x and downsampled, which is cheaper than fighting Pillow for
antialiased vector output.

    python3 scripts/make-icons.py
"""
import math
import os
import subprocess
import sys

from PIL import Image, ImageDraw

OUT = os.path.join(os.path.dirname(__file__), "..", "src-tauri", "icons")
SS = 8  # supersample factor


def mark(size, fg, *, lines=False):
    """The Lector mark: a page with a folded corner, speaking.

    A page alone reads as any document app; big concentric arcs alone read as
    wifi. The page has to be unmistakably a *page* -- wide, dog-eared, ruled --
    and the arcs have to sit tight against its edge so they read as sound coming
    off it rather than a signal radiating into space.
    """
    s = size * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    u = s / 100.0  # one unit = 1% of the canvas

    # The page: a portrait sheet at roughly 3:4, which is what makes it read as
    # paper rather than as a bar or a remote control.
    x0, y0, x1, y1 = 12 * u, 14 * u, 60 * u, 86 * u
    fold = 16 * u  # size of the dog-ear
    d.rounded_rectangle((x0, y0, x1, y1), radius=4 * u, fill=fg)
    # Knock the corner out, then lay the folded triangle back over it.
    d.polygon([(x1 - fold, y0), (x1 + u, y0), (x1 + u, y0 + fold)], fill=(0, 0, 0, 0))
    d.polygon(
        [(x1 - fold, y0), (x1, y0 + fold), (x1 - fold, y0 + fold)],
        fill=fg,
    )

    if lines:
        hole = (0, 0, 0, 0)
        for y, w in ((36, 30), (48, 30), (60, 22)):
            d.rounded_rectangle(
                (20 * u, y * u, (20 + w) * u, (y + 5.5) * u), radius=2.75 * u, fill=hole
            )

    # Sound coming off the page's right edge. Small and tight: two arcs, not
    # three, and a short sweep, so the group never becomes the wifi glyph.
    cx, cy = 58 * u, 58 * u
    for radius, width in ((15, 5.5), (26, 5.5)):
        r = radius * u
        d.arc(
            (cx - r, cy - r, cx + r, cy + r),
            start=-42,
            end=42,
            fill=fg,
            width=int(width * u),
        )

    return img.resize((size, size), Image.LANCZOS)


def tray_mark(size, fg):
    """The menu bar mark, which is a different drawing problem entirely.

    At 22px a filled page is just a black slab, ruled lines close up into a grey
    smear, and three arcs merge into one. So this is line art with exactly two
    elements: an outlined sheet and a pair of well-separated arcs. Everything
    that cannot survive being twenty pixels tall was tried and cut -- the
    dog-ear, the text lines, the third arc.

    Compared against filled, single-arc, spine-only and open-book variants at
    true size; the outline won because it reads as a *document* rather than as a
    generic block, and stays light enough to sit in a menu bar.
    """
    s = size * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    u = s / 100.0
    # Rounded to whole pixels at the target size, so strokes land on the grid
    # instead of smearing across two rows.
    stroke = max(1, round(size * 0.09)) * SS

    d.rounded_rectangle(
        (12 * u, 16 * u, 46 * u, 84 * u), radius=6 * u, outline=fg, width=int(stroke)
    )

    # Two arcs, pushed apart far enough that the gap survives antialiasing.
    cx, cy = 52 * u, 50 * u
    for radius in (18, 38):
        r = radius * u
        d.arc(
            (cx - r, cy - r, cx + r, cy + r), start=-44, end=44, fill=fg, width=int(stroke)
        )

    return img.resize((size, size), Image.LANCZOS)


def squircle(size, radius_ratio=0.2237):
    """macOS-style rounded square mask, at the platform's corner ratio."""
    s = size * SS
    m = Image.new("L", (s, s), 0)
    ImageDraw.Draw(m).rounded_rectangle((0, 0, s - 1, s - 1), radius=int(s * radius_ratio), fill=255)
    return m.resize((size, size), Image.LANCZOS)


def app_icon(size):
    """The Finder/Dock icon: ink on warm paper."""
    s = size * SS
    bg = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(bg)
    # A soft vertical wash rather than a flat fill, so it does not look dead
    # next to other icons.
    top, bottom = (250, 246, 238), (226, 214, 196)
    for y in range(s):
        t = y / max(s - 1, 1)
        d.line(
            [(0, y), (s, y)],
            fill=tuple(int(a + (b - a) * t) for a, b in zip(top, bottom)) + (255,),
        )
    bg = bg.resize((size, size), Image.LANCZOS)
    bg.putalpha(squircle(size))

    # The mark inset, in a dark ink that stays legible on the light wash.
    #
    # Detail is dropped below 64px: at 32 the ruled lines and the arcs blur into
    # each other, and a clean silhouette beats a smudged drawing. The large sizes
    # are what people actually look at; the small ones only have to be
    # recognisable.
    inner = int(size * 0.62)
    m = mark(inner, (38, 34, 30, 255), lines=size >= 64)
    bg.paste(m, ((size - inner) // 2, (size - inner) // 2), m)
    return bg


def main():
    os.makedirs(OUT, exist_ok=True)

    # Menu bar. @2x for retina; macOS picks by the "Template" filename suffix.
    for px, name in ((22, "trayTemplate.png"), (44, "trayTemplate@2x.png")):
        tray_mark(px, (0, 0, 0, 255)).save(os.path.join(OUT, name))

    # App icon, in the sizes Tauri's bundler expects.
    for px, name in (
        (32, "32x32.png"),
        (128, "128x128.png"),
        (256, "128x128@2x.png"),
        (512, "icon.png"),
    ):
        app_icon(px).save(os.path.join(OUT, name))

    # .icns, via the platform's own tool so the result is exactly what macOS wants.
    iconset = os.path.join(OUT, "icon.iconset")
    os.makedirs(iconset, exist_ok=True)
    for px, name in (
        (16, "icon_16x16.png"), (32, "icon_16x16@2x.png"),
        (32, "icon_32x32.png"), (64, "icon_32x32@2x.png"),
        (128, "icon_128x128.png"), (256, "icon_128x128@2x.png"),
        (256, "icon_256x256.png"), (512, "icon_256x256@2x.png"),
        (512, "icon_512x512.png"), (1024, "icon_512x512@2x.png"),
    ):
        app_icon(px).save(os.path.join(iconset, name))
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(OUT, "icon.icns")], check=True)
    for f in os.listdir(iconset):
        os.remove(os.path.join(iconset, f))
    os.rmdir(iconset)

    print("wrote:", ", ".join(sorted(os.listdir(OUT))))


if __name__ == "__main__":
    sys.exit(main())
