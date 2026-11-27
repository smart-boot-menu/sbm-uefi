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
    fill_rounded_panel, fill_rounded_rect, fill_circle,
    draw_vector_emblem, draw_vector_lock, draw_vector_close_x, draw_vector_chevron,
};

// Direct integration with wrapped partition row slices (`&[Rect]`):
// Builds the unified closed perimeter of contiguous wrapped line segments with
// smooth rounded corners at every convex/concave vertex and rasterizes it cleanly.
// Both TextRenderer and vector::* honor Framebuffer::with_viewport clip & scroll bias.
fill_ortho_polygon(fb, &segments, radius_px, fill_color);
stroke_ortho_polygon(fb, &segments, radius_px, border_width_px, border_color);

// Or build arbitrary custom vector paths (lines, quadratic & cubic Beziers):
let mut path = VectorPath::new();
path.rounded_rect(x, y, w, h, r);
path.fill(fb, color);
```

`Framebuffer::blend_pixel(x, y, color, alpha)` (sRGB) and `Framebuffer::blend_pixel_linear(x, y, color, alpha)` (gamma ~2.0 linear-light AA for crisp dark/light text & thin strokes) are both public on `Framebuffer` and unconditionally honor the active viewport clip rectangle and vertical scroll bias.

## 3. UEFI Lifecycle & Performance Notes (Consolidated with Agent 15)

- `sbm_menu::paint_current_with(fb, entries, visible, state, lines, high_contrast, &mut renderer)` accepts a persistent `TextRenderer` owned by `sbm-app` across splash and menu frames. The legacy `paint_current(...)` wrapper remains backwards-compatible.
- `paint.rs` uses a single unified code path for `AtkinsonNextMono`, `AtkinsonNext`, and `Bitmap8x8` (the duplicate ~330-line legacy paint engine and `noto-sans-mono-bitmap` crate were removed).
- `sbm-app` tracks a `FrameKey` (including focus and selected action) so idle frames are never continuously redrawn while every keyboard/pointer transition repaints immediately.
- The temporary in-memory LRU glyph cache is strictly bounded (default **192 entries / 64 KiB cap**, Classic `1280×800` uses **79 entries / 7.5 KiB**, experimental LBA dashboards use **164–192 entries / 13.8–22.1 KiB**, zero disk persistence) and emits one serial telemetry line on boot:
  `SBM_MARK glyph_cache glyphs=... bytes=... hits=... misses=...`.
- Consolidated release EFI sizes (`opt-level = "z"`, LTO, `sha2` `opt-level = 3`, SBAT + `NX_COMPAT`): **410,112 B (`aarch64`)** and **465,920 B (`x86_64`)** — strictly under 45% of the 1 MiB target.

## 4. Non-Conflicting `sbm-preview` Controls

Consolidated simulator controls in `sbm-preview`:
- `1`–`4` / `F4`: switch layout (`Classic` / `Bento` / `Schematic` / `Storage`)
- `F6`: toggle progressive discovery
- `F7` / `Shift+F7`: cycle font mode (`Atkinson Next + Mono` / `Atkinson Next` / `Bitmap 8x8`)
- `F8`: toggle high-contrast theme (Light/Dark × Normal/High-Contrast)
- `+` / `-` / `0`: adjust UI scale (`75%`–`200%`) / reset to `100%`
- CLI flags: `--layout classic|bento|schematic|storage|experiments|all`, `--font bitmap|next|next-mono|all`, `--scale 75..=200`, `--contrast normal|high|both`, `--discovery instant|progressive|slow|sparse`, `--scroll N`
