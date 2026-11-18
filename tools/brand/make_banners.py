#!/usr/bin/env python3
"""Puts the current Nocturne icon on the banners and social cards, in place of the one drawn on them.

The banners carry translated words that are not redrawn here. Only the icon's patch of each
picture is repainted: every row of the patch takes the ruled-paper colour that row has just right
of it, and the icon from assets/brand/nocturne-source.png goes on top with a soft shadow.
Run tools/brand/make_icons.py first. Requires Pillow.

Usage: python tools/brand/make_banners.py [repo root]
"""
import shutil
import sys
from pathlib import Path

from PIL import Image, ImageFilter

# Per picture size: the patch to repaint (left, top, right, bottom), the column its rows are
# copied from, and the icon's box (left, top, side).
LAYOUTS = {
    (720, 300): ((58, 50, 268, 276), 270, (82, 76, 160)),
    (1280, 640): ((100, 150, 440, 510), 444, (150, 200, 256)),
}


def repaint(path: Path, icon: Image.Image) -> None:
    with Image.open(path) as opened:
        picture = opened.convert("RGB")
    (left, top, right, bottom), column, (x, y, side) = LAYOUTS[picture.size]
    for row in range(top, bottom):
        picture.paste(picture.getpixel((column, row)), (left, row, right, row + 1))
    small = icon.resize((side, side), Image.LANCZOS)
    pad = side // 8
    shadow = Image.new("L", (side + 2 * pad, side + 2 * pad), 0)
    shadow.paste(small.getchannel("A").point(lambda a: a * 70 // 255), (pad, pad))
    shadow = shadow.filter(ImageFilter.GaussianBlur(side / 24))
    picture.paste((4, 6, 28), (x - pad, y - pad + side // 40), shadow)
    picture.paste(small, (x, y), small)
    picture.save(path, optimize=True)


def main() -> None:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parents[2]
    icon = Image.open(root / "assets" / "brand" / "nocturne-source.png").convert("RGBA")
    brand, demo = root / "assets" / "brand", root / "assets" / "demo"
    done = []
    for path in [brand / "nocturne-social-light.png", brand / "nocturne-social-dark.png", *sorted(demo.glob("banner-??.png"))]:
        repaint(path, icon)
        done.append(path.name)
    # The start page's banner is the light social card.
    shutil.copyfile(brand / "nocturne-social-light.png", demo / "banner-start.png")
    print(f"repainted {len(done)} pictures and copied banner-start.png: {', '.join(done)}")


if __name__ == "__main__":
    main()
