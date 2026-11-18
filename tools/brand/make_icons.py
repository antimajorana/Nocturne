#!/usr/bin/env python3
"""Draws the Nocturne app icon (a sunset over water) and writes every icon file made from it.

The icon is described by the numbers below, in units of the icon's width, so each size is drawn
fresh rather than shrunk from a large picture. The numbers were measured from the owner's design
(2026-11-18). Requires Pillow and numpy.

Usage: python tools/brand/make_icons.py [repo root]   (default: two folders up from this file)

Writes:
  assets/brand/nocturne-source.png        1024 px, rounded corners, transparent outside
  assets/brand/nocturne-{512,256,128,64,32}.png
  assets/brand/nocturne-square.png        1024 px, square, no transparency (iOS rounds it itself)
  assets/app-icon/nocturne.ico            16, 24, 32, 48, 64, 128 and 256 px
"""
import sys
from pathlib import Path

import numpy as np
from PIL import Image

CORNER = 0.2237  # corner radius
HORIZON = 0.5776  # where the sky meets the water
SKY_TOP, SKY_HORIZON = (58, 45, 141), (40, 36, 106)
WATER_TOP, WATER_BOTTOM = (18, 22, 74), (8, 10, 36)
SUN_X, SUN_Y, SUN_R = 0.5, 0.4592, 0.2039
SUN_TOP, SUN_BOTTOM = (255, 225, 153), (255, 123, 68)
GLOW, GLOW_ALPHA, GLOW_R = (255, 152, 90), 0.55, 0.4105  # warm light around the sun, sky only
BAR_H = 0.02566  # the four reflections on the water: centre y, width, colour
BARS = [
    (0.6388, 0.3526, (255, 176, 103)),
    (0.7013, 0.2539, (195, 121, 84)),
    (0.7642, 0.1566, (127, 117, 176)),
    (0.8266, 0.0697, (77, 72, 144)),
]
STARS = [(0.2303, 0.2303, 0.0184, (201, 184, 255)), (0.2928, 0.1855, 0.0092, (255, 210, 122))]


def lerp(a, b, t):
    t = np.clip(t, 0.0, 1.0)[..., None]
    return np.asarray(a, float) * (1 - t) + np.asarray(b, float) * t


def render(size: int, rounded: bool = True) -> Image.Image:
    """The icon at `size` pixels, drawn 8x larger and averaged down for smooth edges.

    Drawn a few rows at a time, so a 1024 px icon does not need gigabytes of memory.
    """
    rows = max(1, min(size, 32768 // size))
    strips = [_strip(size, top, min(rows, size - top), rounded) for top in range(0, size, rows)]
    out = np.vstack(strips)
    return Image.fromarray(out, "RGBA").convert("RGBA" if rounded else "RGB")


def _strip(size: int, top: int, rows: int, rounded: bool) -> np.ndarray:
    """Rows `top..top+rows` of the icon as RGBA bytes."""
    n = size * 8
    x, y = np.meshgrid((np.arange(n) + 0.5) / n, (np.arange(top * 8, (top + rows) * 8) + 0.5) / n)
    above = y < HORIZON
    img = np.where(above[..., None], lerp(SKY_TOP, SKY_HORIZON, y / HORIZON), lerp(WATER_TOP, WATER_BOTTOM, (y - HORIZON) / (1 - HORIZON)))
    from_sun = np.hypot(x - SUN_X, y - SUN_Y)
    glow = np.clip(GLOW_ALPHA * (1 - from_sun / GLOW_R), 0, None) * above
    img = img + glow[..., None] * (np.asarray(GLOW, float) - img)
    sun = (from_sun <= SUN_R) & above
    img = np.where(sun[..., None], lerp(SUN_TOP, SUN_BOTTOM, (y - (SUN_Y - SUN_R)) / (2 * SUN_R)), img)
    for cy, width, colour in BARS:
        dx = np.clip(np.abs(x - 0.5) - (width - BAR_H) / 2, 0, None)
        img[np.hypot(dx, y - cy) <= BAR_H / 2] = colour
    for sx, sy, r, colour in STARS:
        img[np.hypot(x - sx, y - sy) <= r] = colour
    if rounded:
        qx = np.clip(np.abs(x - 0.5) - (0.5 - CORNER), 0, None)
        qy = np.clip(np.abs(y - 0.5) - (0.5 - CORNER), 0, None)
        alpha = (np.hypot(qx, qy) <= CORNER).astype(float)
    else:
        alpha = np.ones_like(x)
    # Average each 8x8 block, weighting colour by coverage so the edges do not darken.
    a = alpha.reshape(rows, 8, size, 8).mean(axis=(1, 3))
    rgb = (img * alpha[..., None]).reshape(rows, 8, size, 8, 3).mean(axis=(1, 3))
    rgb = np.where(a[..., None] > 0, rgb / np.maximum(a, 1e-9)[..., None], 0)
    return np.dstack([rgb, a * 255]).round().clip(0, 255).astype(np.uint8)


def main() -> None:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parents[2]
    brand = root / "assets" / "brand"
    render(1024).save(brand / "nocturne-source.png", optimize=True)
    for size in (512, 256, 128, 64, 32):
        render(size).save(brand / f"nocturne-{size}.png", optimize=True)
    render(1024, rounded=False).save(brand / "nocturne-square.png", optimize=True)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    frames = [render(s) for s in sizes]
    frames[-1].save(root / "assets" / "app-icon" / "nocturne.ico", sizes=[(s, s) for s in sizes], append_images=frames[:-1])
    print(f"wrote 7 PNGs and nocturne.ico ({', '.join(map(str, sizes))} px) under {root / 'assets'}")


if __name__ == "__main__":
    main()
