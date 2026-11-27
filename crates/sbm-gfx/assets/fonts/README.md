# Bundled Vector Typography Assets

These TrueType (`.ttf`) vector font files are bundled into `sbm-gfx` for
resolution-independent, dynamically rasterized, accessible typography across
UEFI (`x86_64-unknown-uefi`, `aarch64-unknown-uefi`) and the native simulator.

## Upstream Sources & Provenance

| Font Family | Upstream Repository | Pinned Commit | License | Reserved Font Names |
| --- | --- | --- | --- | --- |
| Atkinson Hyperlegible Next | <https://github.com/googlefonts/atkinson-hyperlegible-next> | `7925f50f649b3813257faf2f4c0b381011f434f1` (2025-02-21) | SIL OFL 1.1 ([OFL-AtkinsonHyperlegibleNext.txt](OFL-AtkinsonHyperlegibleNext.txt)) | None declared |
| Atkinson Hyperlegible Mono | <https://github.com/googlefonts/atkinson-hyperlegible-next-mono> | `154d50362016cc3e873eb21d242cd0772384c8f9` (2024-11-20) | SIL OFL 1.1 ([OFL-AtkinsonHyperlegibleMono.txt](OFL-AtkinsonHyperlegibleMono.txt)) | None declared |

## Bundled Files & Optimization

Optimized from the unmodified upstream static `.ttf` files using
`scripts/optimize-fonts.py --profile latin-full` (`fontTools` `pyftsubset`):

- Strips TrueType hinting bytecode (`fpgm`, `prep`, `cvt `) unused by our
  analytical coverage rasterizer (`ab_glyph_rasterizer`), reducing size by 44.6%.
- Preserves **all 362 / 359 upstream Unicode codepoints** (Basic Latin, Latin-1
  Supplement, Latin Extended-A, typographic punctuation, math/technical symbols).
- Preserves OpenType **GPOS `kern`** pair-positioning kerning tables.
- Preserves the honest **`.notdef`** missing-glyph outline (never substituted
  with `?` or misleading approximations).
- Preserves SIL OFL 1.1 copyright, license, and typographic family metadata in
  `name` table entries `0, 1, 2, 4, 13, 14, 16, 17` alongside the full license
  texts in this directory.

| File | Upstream Size | Bundled Size | Saved | Codepoints | Role Usage |
| --- | ---: | ---: | ---: | ---: | --- |
| `AtkinsonHyperlegibleNext-Regular.ttf` | 65,068 B | 37,560 B | 42.3% | 362 | `Body`, `Caption` |
| `AtkinsonHyperlegibleNext-SemiBold.ttf` | 66,680 B | 38,256 B | 42.6% | 362 | `Display`, `Heading` |
| `AtkinsonHyperlegibleMono-Regular.ttf` | 52,020 B | 26,020 B | 50.0% | 359 | `Technical` |
| **Total** | **183,768 B** | **101,836 B** | **44.6%** | — | All 5 roles |

## License Compliance

Both font families are distributed under the SIL Open Font License, Version 1.1.
Neither upstream `OFL.txt` declares a Reserved Font Name. All modified/optimized
copies remain strictly under SIL OFL 1.1 with copyright and license notices
preserved in both the binary `name` tables and the companion `OFL-*.txt` files.
