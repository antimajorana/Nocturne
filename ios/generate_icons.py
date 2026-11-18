#!/usr/bin/env python3
"""Generate opaque RGB iOS and iPadOS AppIcon PNGs from an RGBA source image."""

import argparse
from pathlib import Path
from PIL import Image

BG_COLOR = (5, 28, 72)
ICONS = (
    ("AppIcon60x60@2x.png", 120),
    ("AppIcon60x60@3x.png", 180),
    ("AppIcon76x76@1x~ipad.png", 76),
    ("AppIcon76x76@2x~ipad.png", 152),
    ("AppIcon83.5x83.5@2x~ipad.png", 167),
    ("AppIcon1024x1024.png", 1024),
)
DEFAULT_SOURCE = Path(__file__).resolve().parent.parent / "assets" / "brand" / "nocturne-source.png"


def composite_opaque_square(source: Path) -> Image.Image:
    """Composite any source image onto an opaque #051C48 square RGB canvas."""
    with Image.open(source) as raw:
        rgba = raw.convert("RGBA")
    side = max(rgba.width, rgba.height)
    canvas = Image.new("RGBA", (side, side), (*BG_COLOR, 255))
    offset = ((side - rgba.width) // 2, (side - rgba.height) // 2)
    canvas.alpha_composite(rgba, dest=offset)
    return canvas.convert("RGB")


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate opaque RGB iOS AppIcon PNGs.")
    parser.add_argument("--source", type=Path, default=DEFAULT_SOURCE, help="Source PNG path")
    parser.add_argument("--out", type=Path, required=True, help="Target .app bundle directory")
    args = parser.parse_args()

    if not args.source.is_file():
        raise SystemExit(f"icon source does not exist: {args.source}")

    args.out.mkdir(parents=True, exist_ok=True)
    resampling = getattr(Image, "Resampling", Image).LANCZOS
    base = composite_opaque_square(args.source)
    for filename, size in ICONS:
        resized = base.resize((size, size), resample=resampling) if base.size != (size, size) else base
        if resized.mode != "RGB":
            resized = resized.convert("RGB")
        resized.save(args.out / filename, format="PNG")


if __name__ == "__main__":
    main()
