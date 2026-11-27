# Typography & Unified Vector Rendering Interface (`sbm-gfx`)

Coordination contract for **Agent 18** (sequential paragraph-style disk/partition layout) and **Agent 15** (UEFI correctness, performance, and `sbm-app` lifecycle).

## 1. Shared Typography API (`sbm_gfx::typography`)

`sbm-gfx` exposes a `#![forbid(unsafe_code)]`, `no_std` + `alloc` vector typography engine backed by `ttf-parser` 0.25.1 + `ab_glyph_rasterizer` 0.1.10 and bundled `fontTools`-optimized Atkinson Hyperlegible Next (Regular + SemiBold) and Atkinson Hyperlegible Mono (Regular) TTFs (`101,836 B` total).

### Semantic Roles (`TextRole`)

| Role | Face (`AtkinsonNextMono`) | Weight | Relative Scale | Use For |
| --- | --- | --- | ---: | --- |
| `TextRole::Display` | Atkinson Hyperlegible Next | SemiBold | `2.00×` | Large hardware specs, disk capacities, splash titles |
| `TextRole::Heading` | Atkinson Hyperlegible Next | SemiBold | `1.32×` | Disk titles, OS names, card headings, section headers |
| `TextRole::Body` | Atkinson Hyperlegible Next | Regular | `1.00×` | Ordinary mixed-case prose, controls, descriptions |
| `TextRole::Caption` | Atkinson Hyperlegible Next | Regular | `0.84×` | Secondary labels, hints, partition tags |
| `TextRole::Technical` | Atkinson Hyperlegible Mono | Regular | `0.90×` | EFI paths, `/dev/...`, PARTUUIDs, LBAs, hex values, diagnostics |

### Core `TextRenderer` Methods (For Layout Code / Agent 18)

```rust
use sbm_gfx::typography::{FontMode, TextRenderer, TextRole, UiScale};

let mut renderer = TextRenderer::new(FontMode::AtkinsonNextMono, fb.height, UiScale::default());

// Accurate measurement & vertical metrics (for baseline alignment and wrapping)
let lm = renderer.line_metrics(TextRole::Body); // px_size, ascent, descent, cap_height, line_gap, line_height
let m  = renderer.measure("Samsung 990 PRO 2 TB", TextRole::Heading); // width, height, baseline

// Ellipsis truncation & word wrapping (respects GPOS kerning & role metrics)
let fitted: String     = renderer.truncate(label, max_width_px, TextRole::Technical);
let lines: Vec<String> = renderer.wrap(paragraph, max_width_px, TextRole::Body);

// Drawing primitives (all gamma-aware linear-light antialiased into Framebuffer)
renderer.draw_baseline(fb, text, x, baseline_y, TextRole::Body, theme.text);
renderer.draw_top(fb, text, x, top_y, TextRole::Heading, theme.text);
renderer.draw_centered(fb, text, rect, TextRole::Body, theme.text);
renderer.draw_truncated(fb, text, x, top_y, max_w, TextRole::Technical, theme.muted);
renderer.draw_wrapped(fb, text, rect, TextRole::Body, theme.text);

// Optional external language-pack fallback fonts (up to 4 .ttf/.otf slices)
renderer.add_fallback_font(extra_font_bytes)?;
```

- **Unknown characters** always render as the font's honest `.notdef` box — never `'?'` or misleading substitutes.
- **Comparison baseline:** Passing `FontMode::Bitmap8x8` renders through the preserved legacy 8×8 bitmap font.
- **High-contrast themes:** `Theme::resolve(dark, high_contrast)` returns `Theme::dark()`, `Theme::light()`, `Theme::high_contrast_dark()`, or `Theme::high_contrast_light()`.

## 2. Unified CPU Vector Backend (`sbm_gfx::vector`)

Instead of introducing a second rasterizer for shapes or irregular wrapped partition polygons, `sbm_gfx::vector` reuses the exact same `ab_glyph_rasterizer` analytical coverage engine:

```rust
use sbm_gfx::vector::{
    VectorPath, fill_ortho_polygon, stroke_ortho_polygon,
    fill_rounded_panel, fill_rounded_rect, draw_vector_emblem, draw_vector_lock,
};

// Direct integration with Agent 18's wrapped partition row slices (`&[Rect]`):
// Builds the unified closed perimeter of contiguous wrapped line segments with
// smooth rounded corners at every convex/concave vertex and rasterizes it cleanly.
fill_ortho_polygon(fb, &segments, radius_px, fill_color);
stroke_ortho_polygon(fb, &segments, radius_px, border_width_px, border_color);

// Or build arbitrary custom vector paths (lines, quadratic & cubic Beziers):
let mut path = VectorPath::new();
path.rounded_rect(x, y, w, h, r);
path.fill(fb, color);
```

`Framebuffer::blend_pixel(x, y, color, alpha)` (sRGB) and `Framebuffer::blend_pixel_linear(x, y, color, alpha)` (gamma ~2.0 linear-light AA for crisp dark/light text & thin strokes) are both public on `Framebuffer`.

## 3. UEFI Lifecycle & Performance Notes (For Agent 15)

- `sbm_menu::paint_current_with(fb, cards, state, high_contrast, &mut renderer)` accepts a persistent `TextRenderer` owned by `sbm-app` across splash and menu frames. The legacy `paint_current(...)` wrapper remains backwards-compatible.
- `sbm-app` tracks a `FrameKey` so idle frames are never continuously redrawn.
- The temporary in-memory LRU glyph cache is strictly bounded (default **192 entries / 64 KiB cap**, typical menu uses **~12.6 KiB**, zero disk persistence) and emits one serial telemetry line on boot:
  `SBM_MARK glyph_cache entries=110 bytes=12861 hits=237 misses=110`.
- Measured release EFI sizes (`opt-level = "z"`, LTO): **392,704 B (`aarch64`)** and **439,808 B (`x86_64`)** — strictly under 42% of the 1 MiB target.

## 4. Non-Conflicting `sbm-preview` Controls

To avoid conflicting with Agent 18's existing `F1`–`F6`, `1`–`4`, and `F12` bindings, typography controls in `sbm-preview` use:
- `F7` / `Shift+F7`: cycle font mode (`Atkinson Next + Mono` / `Atkinson Next` / `Bitmap 8x8`)
- `F8`: toggle high-contrast theme
- `+` / `-` / `0`: adjust UI scale (`85%`–`200%`) / reset to `100%`
- CLI flags: `--font bitmap|next|next-mono|all`, `--scale <percent>`, `--contrast normal|high|both`
