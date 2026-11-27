#!/usr/bin/env python3
"""Reproducibly optimize upstream Atkinson Hyperlegible Next & Mono TTF assets.

Requires `fonttools` (`pip install fonttools`).

Preserves:
- All upstream Latin, Latin-Extended, punctuation, and math codepoints (`--unicodes=*`)
  in the default `latin-full` profile, or Basic Latin + Latin-1 + punctuation in `latin1-min`.
- OpenType GPOS `kern` kerning tables (`--layout-features=kern`).
- SIL OFL 1.1 copyright, license, and typographic family metadata in the font `name`
  table (IDs 0, 1, 2, 4, 13, 14, 16, 17).
- Honest `.notdef` missing-glyph outline (`--notdef-glyph --notdef-outline`).

Strips:
- TrueType hinting bytecode (`fpgm`, `prep`, `cvt `) unused by analytical vector rasterizers.
- Unused GSUB stylistic tables and legacy metric tables (`DSIG`, `hdmx`, `LTSH`, `VDMX`).
"""

from __future__ import annotations

import argparse
import hashlib
import pathlib
import subprocess
import sys

PROFILES = {
    "latin-full": "*",
    "latin1-min": (
        "U+0020-007E,U+00A0-00FF,U+03BC,U+2013-2026,U+20AC,U+2212,U+2260,U+2264,U+2265"
    ),
}

FONTS = (
    "AtkinsonHyperlegibleNext-Regular.ttf",
    "AtkinsonHyperlegibleNext-SemiBold.ttf",
    "AtkinsonHyperlegibleMono-Regular.ttf",
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--src",
        type=pathlib.Path,
        required=True,
        help="Directory containing unmodified upstream TTF files",
    )
    parser.add_argument(
        "--dst",
        type=pathlib.Path,
        default=pathlib.Path("crates/sbm-gfx/assets/fonts"),
        help="Output directory for optimized TTF files",
    )
    parser.add_argument(
        "--profile",
        choices=sorted(PROFILES),
        default="latin-full",
        help="Unicode coverage profile (default: latin-full)",
    )
    args = parser.parse_args()
    args.dst.mkdir(parents=True, exist_ok=True)

    unicodes = PROFILES[args.profile]
    total_in = 0
    total_out = 0

    for fname in FONTS:
        src = args.src / fname
        if not src.is_file():
            print(f"error: missing {src}", file=sys.stderr)
            return 1
        dst = args.dst / fname
        subprocess.run(
            [
                sys.executable,
                "-m",
                "fontTools.subset",
                str(src),
                f"--unicodes={unicodes}",
                "--no-hinting",
                "--layout-features=kern",
                "--drop-tables+=DSIG,hdmx,LTSH,VDMX",
                "--name-IDs=0,1,2,4,13,14,16,17",
                "--name-languages=0x0409",
                "--notdef-glyph",
                "--notdef-outline",
                "--no-recalc-timestamp",
                f"--output-file={dst}",
            ],
            check=True,
        )
        in_sz = src.stat().st_size
        out_sz = dst.stat().st_size
        total_in += in_sz
        total_out += out_sz
        sha = hashlib.sha256(dst.read_bytes()).hexdigest()
        print(f"{fname}: {in_sz} -> {out_sz} bytes ({100 * (1 - out_sz / in_sz):.1f}% saved) sha256={sha}")

    print(f"TOTAL: {total_in} -> {total_out} bytes ({100 * (1 - total_out / total_in):.1f}% saved)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
