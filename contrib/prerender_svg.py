#!/usr/bin/env python3
"""Rasterize the liana-ui SVG assets to PNG.

The app ships the generated PNGs so it does not need an SVG renderer at
runtime. Run this after editing any SVG under liana-ui/static and commit the
result next to its source.

Requires rsvg-convert (librsvg).
"""

import subprocess
import sys
from pathlib import Path

STATIC = Path(__file__).resolve().parent.parent / "liana-ui" / "static"

# Render width in pixels, chosen well above the largest size each asset is
# displayed at so it stays sharp on high density screens.
LOGO_WIDTH = 512
DIAGRAM_WIDTH = 1600

ASSETS = [
    ("logos/LIANA_SYMBOL_Gray.svg", LOGO_WIDTH),
    ("logos/LIANA_SYMBOL_Green.svg", LOGO_WIDTH),
    ("logos/LIANA_SYMBOL_Blue.svg", LOGO_WIDTH),
    ("logos/LIANA_BRAND_Gray.svg", LOGO_WIDTH),
    ("logos/liana_wallet.svg", LOGO_WIDTH),
    ("logos/liana_business.svg", LOGO_WIDTH),
    ("logos/logo-wizardsardine.svg", LOGO_WIDTH),
    ("icons/blueprint.svg", LOGO_WIDTH),
    ("icons/discussion.svg", LOGO_WIDTH),
    ("icons/syncdata.svg", LOGO_WIDTH),
    ("icons/success-mark.svg", LOGO_WIDTH),
    ("icons/key-mark.svg", LOGO_WIDTH),
    ("images/inheritance_template_description.svg", DIAGRAM_WIDTH),
    ("images/custom_template_description.svg", DIAGRAM_WIDTH),
    ("images/multisig_security_template.svg", DIAGRAM_WIDTH),
]


def render(source: Path, target: Path, width: int) -> None:
    subprocess.run(
        ["rsvg-convert", "--width", str(width), "--keep-aspect-ratio",
         "--output", str(target), str(source)],
        check=True,
    )


def main() -> int:
    if subprocess.run(["which", "rsvg-convert"], capture_output=True).returncode != 0:
        print("rsvg-convert not found, install librsvg", file=sys.stderr)
        return 1

    for relative, width in ASSETS:
        source = STATIC / relative
        if not source.exists():
            print(f"missing source {source}", file=sys.stderr)
            return 1
        target = source.with_suffix(".png")
        render(source, target, width)
        print(f"{relative} -> {target.relative_to(STATIC)} ({width}px wide)")

    return 0


if __name__ == "__main__":
    sys.exit(main())
