//! Accessible, resolution-independent `no_std` vector typography engine.
//!
//! Powered by `ttf-parser` (zero-copy TrueType/OpenType parsing + GPOS kerning)
//! and `ab_glyph_rasterizer` (the single shared CPU analytical vector rasterizer
//! also used by [`crate::vector`]).
//!
//! Design properties:
//! - Bundles compact `fontTools`-optimized TrueType vector outlines for
//!   Atkinson Hyperlegible Next (Regular + SemiBold) and Atkinson Hyperlegible
//!   Mono (Regular) under SIL OFL 1.1.
//! - Zero eager rasterization of the font; zero persistent disk cache.
//! - Optional small, bounded, temporary in-memory glyph cache (default 192
//!   glyphs / 64 KiB cap) that can be cleared or disabled for benchmarking.
//! - Supports optional external language-pack `.ttf`/`.otf` fallback fonts.
//! - Renders unknown codepoints honestly as the font's `.notdef` outline —
//!   never as `'?'` or misleading approximations.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use ab_glyph_rasterizer::{Point, Rasterizer};
use ttf_parser::gpos::{PairAdjustment, PositioningSubtable};
use ttf_parser::{Face, GlyphId, OutlineBuilder, Tag};

use crate::draw::{
    Framebuffer, Rgba, fit_text as bitmap_fit_text, text_width as bitmap_text_width,
};
use crate::layout::Rect;

/// Optimized Atkinson Hyperlegible Next Regular (SIL OFL 1.1).
pub static ATKINSON_NEXT_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-Regular.ttf");

/// Optimized Atkinson Hyperlegible Next SemiBold (SIL OFL 1.1).
pub static ATKINSON_NEXT_SEMIBOLD: &[u8] =
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleNext-SemiBold.ttf");

/// Optimized Atkinson Hyperlegible Mono Regular (SIL OFL 1.1).
pub static ATKINSON_MONO_REGULAR: &[u8] =
    include_bytes!("../assets/fonts/AtkinsonHyperlegibleMono-Regular.ttf");

/// Total bundled vector font asset size in bytes.
pub const BUNDLED_FONT_BYTES: usize =
    ATKINSON_NEXT_REGULAR.len() + ATKINSON_NEXT_SEMIBOLD.len() + ATKINSON_MONO_REGULAR.len();

/// Shared semantic typography roles across SBM UEFI and SBM Core.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextRole {
    /// Large hardware specifications, splash titles, and primary figures.
    Display,
    /// Device names, operating system titles, and section headings.
    Heading,
    /// Ordinary interface content, controls, and descriptions.
    Body,
    /// Secondary information, keyboard hints, and metadata captions.
    Caption,
    /// Technical identifiers, hexadecimal values, paths, and diagnostics.
    Technical,
}

impl TextRole {
    pub const ALL: [Self; 5] = [
        Self::Display,
        Self::Heading,
        Self::Body,
        Self::Caption,
        Self::Technical,
    ];
}

/// Selectable typography experiment / rendering mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontMode {
    /// Existing 8x8 pixel bitmap renderer (preserved for baseline comparison).
    Bitmap8x8,
    /// Atkinson Hyperlegible Next across all roles.
    AtkinsonNext,
    /// Atkinson Hyperlegible Next for UI prose + Atkinson Hyperlegible Mono for Technical.
    #[default]
    AtkinsonNextMono,
}

impl FontMode {
    pub const ALL: [Self; 3] = [Self::Bitmap8x8, Self::AtkinsonNext, Self::AtkinsonNextMono];

    pub fn id(self) -> &'static str {
        match self {
            Self::Bitmap8x8 => "bitmap",
            Self::AtkinsonNext => "next",
            Self::AtkinsonNextMono => "next-mono",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bitmap8x8 => "Bitmap 8x8",
            Self::AtkinsonNext => "Atkinson Next",
            Self::AtkinsonNextMono => "Atkinson Next + Mono",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Bitmap8x8 => Self::AtkinsonNext,
            Self::AtkinsonNext => Self::AtkinsonNextMono,
            Self::AtkinsonNextMono => Self::Bitmap8x8,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Bitmap8x8 => Self::AtkinsonNextMono,
            Self::AtkinsonNext => Self::Bitmap8x8,
            Self::AtkinsonNextMono => Self::AtkinsonNext,
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "bitmap" | "bitmap8x8" | "8x8" | "legacy" => Some(Self::Bitmap8x8),
            "next" | "atkinson-next" | "sans" => Some(Self::AtkinsonNext),
            "next-mono" | "atkinson" | "atkinson-next-mono" | "mono" | "both" => {
                Some(Self::AtkinsonNextMono)
            }
            _ => None,
        }
    }
}

/// Adjustable interface scaling percentage (clamped to `75..=200`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UiScale(u16);

impl UiScale {
    pub const PRESETS: [u16; 7] = [85, 100, 115, 125, 150, 175, 200];

    pub const fn new(percent: u16) -> Self {
        let clamped = if percent < 75 {
            75
        } else if percent > 200 {
            200
        } else {
            percent
        };
        Self(clamped)
    }

    pub const fn percent(self) -> u16 {
        self.0
    }

    pub fn step_up(self) -> Self {
        for &p in &Self::PRESETS {
            if p > self.0 {
                return Self(p);
            }
        }
        Self(*Self::PRESETS.last().unwrap_or(&200))
    }

    pub fn step_down(self) -> Self {
        for &p in Self::PRESETS.iter().rev() {
            if p < self.0 {
                return Self(p);
            }
        }
        Self(*Self::PRESETS.first().unwrap_or(&85))
    }
}

impl Default for UiScale {
    fn default() -> Self {
        Self(100)
    }
}

/// Built-in font face slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FaceSlot {
    NextRegular = 0,
    NextSemiBold = 1,
    MonoRegular = 2,
}

/// Vertical metrics for one typography role at the active resolution and scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineMetrics {
    pub px_size: f32,
    pub ascent: f32,
    pub descent: f32,
    pub cap_height: f32,
    pub line_gap: f32,
    pub line_height: u32,
}

/// Measured bounding box and baseline offset for a rendered string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextMetrics {
    pub width: u32,
    pub height: u32,
    pub baseline: u32,
}

/// Runtime telemetry for the optional bounded temporary glyph cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub entries: usize,
    pub bytes: usize,
}

/// Error returned when loading an optional external language-pack font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontLoadError {
    InvalidFontData,
    TooManyFallbacks,
}

/// Compute the responsive pixel size for `role` given display height and `scale`.
///
/// Designed for legibility from 800x600 through 3840x2160 (4K) without arbitrary
/// integer scaling of a single bitmap font.
pub fn role_pixel_size(role: TextRole, display_height: u32, scale: UiScale) -> f32 {
    let base_body: f32 = if display_height >= 1900 {
        30.0
    } else if display_height >= 1300 {
        23.0
    } else if display_height >= 900 {
        19.0
    } else if display_height >= 700 {
        16.0
    } else {
        14.0
    };
    let role_mult: f32 = match role {
        TextRole::Display => 2.00,
        TextRole::Heading => 1.32,
        TextRole::Body => 1.00,
        TextRole::Caption => 0.84,
        TextRole::Technical => 0.90,
    };
    let scaled = base_body * role_mult * (f32::from(scale.percent()) / 100.0);
    // Round to nearest 0.5 px for crisp raster alignment while remaining smooth across scales.
    (libm::roundf(scaled * 2.0) * 0.5).clamp(10.0, 128.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GlyphKey {
    face_id: u8,
    glyph_id: u16,
    /// Pixel size quantized in 0.25 px increments.
    px_q4: u16,
}

#[derive(Clone, Debug)]
struct CachedGlyph {
    key: GlyphKey,
    /// Horizontal offset from pen origin to left edge of coverage bitmap.
    left: i16,
    /// Vertical offset from baseline to top edge of coverage bitmap (negative = above baseline).
    top: i16,
    width: u16,
    height: u16,
    /// Coverage alpha values (`0..=255`).
    coverage: Vec<u8>,
}

/// Small, bounded, temporary in-memory glyph cache (never persisted).
struct GlyphCache {
    enabled: bool,
    max_entries: usize,
    max_bytes: usize,
    bytes: usize,
    entries: Vec<CachedGlyph>,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl GlyphCache {
    fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            enabled: true,
            max_entries,
            max_bytes,
            bytes: 0,
            entries: Vec::new(),
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    fn get(&mut self, key: GlyphKey) -> Option<&CachedGlyph> {
        if !self.enabled {
            return None;
        }
        if let Some(pos) = self.entries.iter().position(|e| e.key == key) {
            self.hits += 1;
            // Move-to-back LRU promotion when not already recent.
            let last = self.entries.len() - 1;
            if pos != last {
                let item = self.entries.remove(pos);
                self.entries.push(item);
            }
            return self.entries.last();
        }
        None
    }

    fn insert(&mut self, entry: CachedGlyph) {
        if !self.enabled {
            return;
        }
        let cost = entry.coverage.len();
        if cost > self.max_bytes {
            return;
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= self.max_entries || self.bytes + cost > self.max_bytes)
        {
            let evicted = self.entries.remove(0);
            self.bytes = self.bytes.saturating_sub(evicted.coverage.len());
            self.evictions += 1;
        }
        self.bytes += cost;
        self.entries.push(entry);
    }

    fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
            entries: self.entries.len(),
            bytes: self.bytes,
        }
    }
}

/// Maximum optional external language-pack fallback fonts mounted simultaneously.
const MAX_FALLBACK_FONTS: usize = 4;

/// Vector typography engine with shared roles, GPOS kerning, wrapping,
/// truncation, optional fallback language packs, and bounded temporary caching.
pub struct TextRenderer<'a> {
    mode: FontMode,
    display_height: u32,
    scale: UiScale,
    next_regular: Face<'static>,
    next_semibold: Face<'static>,
    mono_regular: Face<'static>,
    fallbacks: Vec<Face<'a>>,
    cache: GlyphCache,
}

impl<'a> TextRenderer<'a> {
    /// Default bounded temporary cache capacity (192 glyphs / 64 KiB).
    pub const DEFAULT_CACHE_ENTRIES: usize = 192;
    pub const DEFAULT_CACHE_BYTES: usize = 65_536;

    /// Construct a renderer using the bundled zero-copy Atkinson Hyperlegible
    /// vector fonts. Performs zero glyph rasterization on construction.
    pub fn new(mode: FontMode, display_height: u32, scale: UiScale) -> Self {
        let next_regular = Face::parse(ATKINSON_NEXT_REGULAR, 0)
            .expect("bundled AtkinsonHyperlegibleNext-Regular.ttf must parse");
        let next_semibold = Face::parse(ATKINSON_NEXT_SEMIBOLD, 0)
            .expect("bundled AtkinsonHyperlegibleNext-SemiBold.ttf must parse");
        let mono_regular = Face::parse(ATKINSON_MONO_REGULAR, 0)
            .expect("bundled AtkinsonHyperlegibleMono-Regular.ttf must parse");
        Self {
            mode,
            display_height: display_height.max(200),
            scale,
            next_regular,
            next_semibold,
            mono_regular,
            fallbacks: Vec::new(),
            cache: GlyphCache::new(Self::DEFAULT_CACHE_ENTRIES, Self::DEFAULT_CACHE_BYTES),
        }
    }

    /// Mount an optional external language-pack TrueType/OpenType font (e.g.
    /// loaded from `\EFI\sbm\fonts\...`) as a fallback for codepoints outside
    /// the bundled Latin/symbol set.
    pub fn add_fallback_font(&mut self, data: &'a [u8]) -> Result<(), FontLoadError> {
        if self.fallbacks.len() >= MAX_FALLBACK_FONTS {
            return Err(FontLoadError::TooManyFallbacks);
        }
        let face = Face::parse(data, 0).map_err(|_| FontLoadError::InvalidFontData)?;
        self.fallbacks.push(face);
        Ok(())
    }

    pub fn mode(&self) -> FontMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: FontMode) {
        self.mode = mode;
    }

    pub fn scale(&self) -> UiScale {
        self.scale
    }

    pub fn set_scale(&mut self, scale: UiScale) {
        if self.scale != scale {
            self.scale = scale;
            self.cache.clear();
        }
    }

    pub fn display_height(&self) -> u32 {
        self.display_height
    }

    /// Update the active display height (clears temporary glyph cache on change).
    pub fn set_display_height(&mut self, display_height: u32) {
        let h = display_height.max(200);
        if self.display_height != h {
            self.display_height = h;
            self.cache.clear();
        }
    }

    /// Enable or disable the bounded temporary in-memory glyph cache (used by benchmarks).
    pub fn set_cache_enabled(&mut self, enabled: bool) {
        self.cache.enabled = enabled;
        if !enabled {
            self.cache.clear();
        }
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// Pixel size resolved for `role` at the current display height and UI scale.
    pub fn role_px(&self, role: TextRole) -> f32 {
        role_pixel_size(role, self.display_height, self.scale)
    }

    /// Primary built-in face slot selected for `role` under the active [`FontMode`].
    pub fn face_slot(&self, role: TextRole) -> FaceSlot {
        match (self.mode, role) {
            (FontMode::AtkinsonNextMono, TextRole::Technical) => FaceSlot::MonoRegular,
            (_, TextRole::Display | TextRole::Heading) => FaceSlot::NextSemiBold,
            (_, TextRole::Body | TextRole::Caption | TextRole::Technical) => FaceSlot::NextRegular,
        }
    }

    fn face_by_id(&self, face_id: u8) -> &Face<'_> {
        match face_id {
            0 => &self.next_regular,
            1 => &self.next_semibold,
            2 => &self.mono_regular,
            n => self
                .fallbacks
                .get((n - 3) as usize)
                .unwrap_or(&self.next_regular),
        }
    }

    /// Resolve a character to `(face_id, glyph_id)`.
    ///
    /// Checks the role's primary face first, then any mounted optional
    /// language-pack fallbacks, and finally returns `.notdef` (`GlyphId(0)`) on
    /// the primary face — never substituting `'?'` or approximations.
    fn resolve_glyph(&self, primary_slot: FaceSlot, ch: char) -> (u8, GlyphId) {
        let primary_id = primary_slot as u8;
        if let Some(gid) = self.face_by_id(primary_id).glyph_index(ch) {
            return (primary_id, gid);
        }
        for (i, fb_face) in self.fallbacks.iter().enumerate() {
            if let Some(gid) = fb_face.glyph_index(ch) {
                return (3 + i as u8, gid);
            }
        }
        (primary_id, GlyphId(0))
    }

    /// Vertical line metrics for `role`.
    pub fn line_metrics(&self, role: TextRole) -> LineMetrics {
        let px = self.role_px(role);
        if self.mode == FontMode::Bitmap8x8 {
            let sz = libm::roundf(px).max(8.0);
            let lh = (libm::ceilf(sz * 1.30) as u32).max(10);
            return LineMetrics {
                px_size: sz,
                ascent: sz * 0.875,
                descent: sz * 0.125,
                cap_height: sz * 0.875,
                line_gap: sz * 0.30,
                line_height: lh,
            };
        }
        let face = self.face_by_id(self.face_slot(role) as u8);
        let scale = px / f32::from(face.units_per_em().max(1));
        let ascent = f32::from(face.ascender()) * scale;
        let descent = f32::from(-face.descender()) * scale;
        let cap_height = face
            .capital_height()
            .map(|c| f32::from(c) * scale)
            .unwrap_or(ascent * 0.72);
        let raw_gap = f32::from(face.line_gap()) * scale;
        let line_gap = raw_gap.max(px * 0.12);
        let line_height = (libm::ceilf(ascent + descent + line_gap) as u32).max(1);
        LineMetrics {
            px_size: px,
            ascent,
            descent,
            cap_height,
            line_gap,
            line_height,
        }
    }

    /// Exact horizontal advance width of `text` in `role`, including GPOS kerning.
    pub fn measure_advance(&self, text: &str, role: TextRole) -> f32 {
        let px = self.role_px(role);
        if self.mode == FontMode::Bitmap8x8 {
            return bitmap_text_width(text, libm::roundf(px).max(8.0) as u32) as f32;
        }
        let slot = self.face_slot(role);
        let mut width = 0.0f32;
        let mut prev: Option<(u8, GlyphId)> = None;
        for ch in text.chars() {
            let (fid, gid) = self.resolve_glyph(slot, ch);
            let face = self.face_by_id(fid);
            let scale = px / f32::from(face.units_per_em().max(1));
            if let Some((pfid, pgid)) = prev
                && pfid == fid
            {
                width += f32::from(kern_unscaled(face, pgid, gid)) * scale;
            }
            let adv = face.glyph_hor_advance(gid).unwrap_or(0);
            width += f32::from(adv) * scale;
            prev = Some((fid, gid));
        }
        width.max(0.0)
    }

    /// Measure `text` rendered in `role`.
    pub fn measure(&self, text: &str, role: TextRole) -> TextMetrics {
        let lm = self.line_metrics(role);
        let width = libm::ceilf(self.measure_advance(text, role)) as u32;
        TextMetrics {
            width,
            height: libm::ceilf(lm.ascent + lm.descent) as u32,
            baseline: libm::ceilf(lm.ascent) as u32,
        }
    }

    /// Truncate `text` with a typographic ellipsis (`…` in vector mode, `...` in
    /// Bitmap8x8 mode) so its measured width fits within `max_width` pixels.
    pub fn truncate(&self, text: &str, role: TextRole, max_width: u32) -> String {
        if max_width == 0 {
            return String::new();
        }
        if self.mode == FontMode::Bitmap8x8 {
            let sz = libm::roundf(self.role_px(role)).max(8.0) as u32;
            return bitmap_fit_text(text, sz, max_width);
        }
        if self.measure(text, role).width <= max_width {
            return String::from(text);
        }
        let ellipsis = "…";
        let ell_w = self.measure_advance(ellipsis, role);
        let limit = max_width as f32;
        if ell_w > limit {
            return String::new();
        }
        let budget = limit - ell_w;
        let slot = self.face_slot(role);
        let px = self.role_px(role);
        let mut out = String::new();
        let mut width = 0.0f32;
        let mut prev: Option<(u8, GlyphId)> = None;
        for ch in text.chars() {
            let (fid, gid) = self.resolve_glyph(slot, ch);
            let face = self.face_by_id(fid);
            let scale = px / f32::from(face.units_per_em().max(1));
            let kern = if let Some((pfid, pgid)) = prev
                && pfid == fid
            {
                f32::from(kern_unscaled(face, pgid, gid)) * scale
            } else {
                0.0
            };
            let adv = f32::from(face.glyph_hor_advance(gid).unwrap_or(0)) * scale;
            if width + kern + adv > budget {
                break;
            }
            width += kern + adv;
            prev = Some((fid, gid));
            out.push(ch);
        }
        while out.ends_with(' ') {
            out.pop();
        }
        out.push_str(ellipsis);
        out
    }

    /// Wrap `text` into lines fitting within `max_width` pixels, respecting
    /// explicit newlines and breaking overlong single tokens (such as paths).
    pub fn wrap(&self, text: &str, role: TextRole, max_width: u32) -> Vec<String> {
        let mut lines = Vec::new();
        if max_width == 0 {
            lines.push(String::new());
            return lines;
        }
        for paragraph in text.split('\n') {
            if paragraph.trim().is_empty() {
                lines.push(String::new());
                continue;
            }
            let mut current = String::new();
            for word in paragraph.split_whitespace() {
                let candidate = if current.is_empty() {
                    String::from(word)
                } else {
                    alloc::format!("{current} {word}")
                };
                if self.measure(&candidate, role).width <= max_width {
                    current = candidate;
                } else if !current.is_empty() {
                    lines.push(current);
                    current = String::new();
                    self.push_word_or_split(&mut lines, &mut current, word, role, max_width);
                } else {
                    self.push_word_or_split(&mut lines, &mut current, word, role, max_width);
                }
            }
            if !current.is_empty() {
                lines.push(current);
            }
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        lines
    }

    fn push_word_or_split(
        &self,
        lines: &mut Vec<String>,
        current: &mut String,
        word: &str,
        role: TextRole,
        max_width: u32,
    ) {
        if self.measure(word, role).width <= max_width {
            *current = String::from(word);
            return;
        }
        let mut chunk = String::new();
        for ch in word.chars() {
            let mut probe = chunk.clone();
            probe.push(ch);
            if !chunk.is_empty() && self.measure(&probe, role).width > max_width {
                lines.push(chunk);
                chunk = String::new();
            }
            chunk.push(ch);
        }
        *current = chunk;
    }

    /// Draw `text` aligned to the exact typographic `baseline_y` coordinate.
    /// Returns the horizontal advance in pixels.
    pub fn draw_baseline(
        &mut self,
        fb: &mut Framebuffer,
        text: &str,
        x: u32,
        baseline_y: u32,
        role: TextRole,
        color: Rgba,
    ) -> u32 {
        let px = self.role_px(role);
        if self.mode == FontMode::Bitmap8x8 {
            let sz = libm::roundf(px).max(8.0) as u32;
            let top_y = baseline_y.saturating_sub((sz * 7) / 8);
            fb.text(text, x, top_y, sz, color);
            return bitmap_text_width(text, sz);
        }

        let slot = self.face_slot(role);
        let px_q4 = libm::roundf(px * 4.0) as u16;
        let mut pen_x = x as f32;
        let base_y = baseline_y as i32;
        let mut prev: Option<(u8, GlyphId)> = None;

        for ch in text.chars() {
            let (fid, gid) = self.resolve_glyph(slot, ch);
            let (kern_px, adv_px) = {
                let face = self.face_by_id(fid);
                let scale = px / f32::from(face.units_per_em().max(1));
                let k = if let Some((pfid, pgid)) = prev
                    && pfid == fid
                {
                    f32::from(kern_unscaled(face, pgid, gid)) * scale
                } else {
                    0.0
                };
                let adv = f32::from(face.glyph_hor_advance(gid).unwrap_or(0)) * scale;
                (k, adv)
            };
            pen_x += kern_px;
            if ch != ' ' {
                let draw_x = libm::roundf(pen_x) as i32;
                self.blit_glyph(fb, fid, gid, px, px_q4, draw_x, base_y, color);
            }
            pen_x += adv_px;
            prev = Some((fid, gid));
        }
        libm::ceilf((pen_x - x as f32).max(0.0)) as u32
    }

    /// Draw `text` with top of line box at `top_y`. Returns pixel width drawn.
    pub fn draw_top(
        &mut self,
        fb: &mut Framebuffer,
        text: &str,
        x: u32,
        top_y: u32,
        role: TextRole,
        color: Rgba,
    ) -> u32 {
        let lm = self.line_metrics(role);
        let baseline_y = top_y.saturating_add(libm::ceilf(lm.ascent) as u32);
        self.draw_baseline(fb, text, x, baseline_y, role, color)
    }

    /// Draw `text` truncated to `max_width` with top of line box at `top_y`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_truncated(
        &mut self,
        fb: &mut Framebuffer,
        text: &str,
        x: u32,
        top_y: u32,
        max_width: u32,
        role: TextRole,
        color: Rgba,
    ) -> u32 {
        let fitted = self.truncate(text, role, max_width);
        self.draw_top(fb, &fitted, x, top_y, role, color)
    }

    /// Draw `text` truncated to `max_width` aligned to `baseline_y`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_truncated_baseline(
        &mut self,
        fb: &mut Framebuffer,
        text: &str,
        x: u32,
        baseline_y: u32,
        max_width: u32,
        role: TextRole,
        color: Rgba,
    ) -> u32 {
        let fitted = self.truncate(text, role, max_width);
        self.draw_baseline(fb, &fitted, x, baseline_y, role, color)
    }

    /// Draw `text` optically centered inside `rect` (truncated if wider than `rect.w`).
    pub fn draw_centered(
        &mut self,
        fb: &mut Framebuffer,
        text: &str,
        rect: Rect,
        role: TextRole,
        color: Rgba,
    ) -> u32 {
        let fitted = self.truncate(text, role, rect.w);
        let tm = self.measure(&fitted, role);
        let lm = self.line_metrics(role);
        let x = rect.x.saturating_add(rect.w.saturating_sub(tm.width) / 2);
        // Optical vertical centering aligns the cap-height midline with rect center.
        let baseline_y = rect
            .y
            .saturating_add(((rect.h as f32 + lm.cap_height) * 0.5) as u32);
        self.draw_baseline(fb, &fitted, x, baseline_y, role, color)
    }

    /// Draw wrapped `text` inside `(x, top_y, max_width)` up to `max_lines`.
    /// Truncates the final line with an ellipsis if additional text remains.
    /// Returns the total vertical height consumed in pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_wrapped(
        &mut self,
        fb: &mut Framebuffer,
        text: &str,
        x: u32,
        top_y: u32,
        max_width: u32,
        max_lines: usize,
        role: TextRole,
        color: Rgba,
    ) -> u32 {
        if max_lines == 0 {
            return 0;
        }
        let lm = self.line_metrics(role);
        let lines = self.wrap(text, role, max_width);
        let count = lines.len().min(max_lines);
        let mut y = top_y;
        for (i, line) in lines.iter().take(count).enumerate() {
            if i + 1 == count && lines.len() > count {
                let mut combined = line.clone();
                combined.push('…');
                let fitted = self.truncate(&combined, role, max_width);
                self.draw_top(fb, &fitted, x, y, role, color);
            } else {
                self.draw_top(fb, line, x, y, role, color);
            }
            y = y.saturating_add(lm.line_height);
        }
        y.saturating_sub(top_y)
    }

    #[allow(clippy::too_many_arguments)]
    fn blit_glyph(
        &mut self,
        fb: &mut Framebuffer,
        face_id: u8,
        glyph_id: GlyphId,
        px: f32,
        px_q4: u16,
        pen_x: i32,
        baseline_y: i32,
        color: Rgba,
    ) {
        let key = GlyphKey {
            face_id,
            glyph_id: glyph_id.0,
            px_q4,
        };
        if let Some(cached) = self.cache.get(key) {
            composite_bitmap(
                fb,
                pen_x + i32::from(cached.left),
                baseline_y + i32::from(cached.top),
                usize::from(cached.width),
                usize::from(cached.height),
                &cached.coverage,
                color,
            );
            return;
        }
        if self
            .face_by_id(face_id)
            .glyph_bounding_box(glyph_id)
            .is_none()
        {
            return;
        }
        self.cache.misses += 1;
        let Some(rasterized) = rasterize_glyph(self.face_by_id(face_id), glyph_id, px, key) else {
            return;
        };
        composite_bitmap(
            fb,
            pen_x + i32::from(rasterized.left),
            baseline_y + i32::from(rasterized.top),
            usize::from(rasterized.width),
            usize::from(rasterized.height),
            &rasterized.coverage,
            color,
        );
        self.cache.insert(rasterized);
    }
}

/// Composite an 8-bit coverage mask directly onto `Framebuffer`.
fn composite_bitmap(
    fb: &mut Framebuffer,
    x0: i32,
    y0: i32,
    width: usize,
    height: usize,
    coverage: &[u8],
    color: Rgba,
) {
    if width == 0 || height == 0 {
        return;
    }
    let fb_w = fb.width as i32;
    let fb_h = fb.height as i32;
    for dy in 0..height {
        let py = y0 + dy as i32;
        if py < 0 || py >= fb_h {
            continue;
        }
        let row = dy * width;
        for dx in 0..width {
            let alpha = coverage[row + dx];
            if alpha == 0 {
                continue;
            }
            let px = x0 + dx as i32;
            if px >= 0 && px < fb_w {
                fb.blend_pixel_linear(px as u32, py as u32, color, alpha);
            }
        }
    }
}

struct GlyphOutlineSink<'a> {
    raster: &'a mut Rasterizer,
    scale: f32,
    ox: f32,
    oy: f32,
    cur: Point,
    start: Point,
    has_point: bool,
}

impl<'a> GlyphOutlineSink<'a> {
    #[inline]
    fn map(&self, x: f32, y: f32) -> Point {
        Point {
            x: x * self.scale - self.ox,
            y: self.oy - y * self.scale,
        }
    }
}

impl OutlineBuilder for GlyphOutlineSink<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.cur = p;
        self.start = p;
        self.has_point = true;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        if self.has_point {
            self.raster.draw_line(self.cur, p);
        }
        self.cur = p;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let c = self.map(x1, y1);
        let p = self.map(x, y);
        if self.has_point {
            self.raster.draw_quad(self.cur, c, p);
        }
        self.cur = p;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let c1 = self.map(x1, y1);
        let c2 = self.map(x2, y2);
        let p = self.map(x, y);
        if self.has_point {
            self.raster.draw_cubic(self.cur, c1, c2, p);
        }
        self.cur = p;
    }

    fn close(&mut self) {
        if self.has_point && (self.cur.x != self.start.x || self.cur.y != self.start.y) {
            self.raster.draw_line(self.cur, self.start);
        }
        self.cur = self.start;
    }
}

/// Rasterize a single glyph outline dynamically via `ttf-parser` + `ab_glyph_rasterizer`.
fn rasterize_glyph(
    face: &Face<'_>,
    glyph_id: GlyphId,
    px: f32,
    key: GlyphKey,
) -> Option<CachedGlyph> {
    let bb = face.glyph_bounding_box(glyph_id)?;
    let scale = px / f32::from(face.units_per_em().max(1));
    let min_x = libm::floorf(f32::from(bb.x_min) * scale) as i32;
    let max_x = libm::ceilf(f32::from(bb.x_max) * scale) as i32;
    // In font coordinates +y is up; in screen coordinates +y is down.
    let top_y = libm::floorf(-f32::from(bb.y_max) * scale) as i32;
    let bot_y = libm::ceilf(-f32::from(bb.y_min) * scale) as i32;

    let width = (max_x - min_x).max(0) as usize;
    let height = (bot_y - top_y).max(0) as usize;
    if width == 0 || height == 0 {
        return None;
    }

    let mut raster = Rasterizer::new(width, height);
    let mut sink = GlyphOutlineSink {
        raster: &mut raster,
        scale,
        ox: min_x as f32,
        oy: (-top_y) as f32,
        cur: Point { x: 0.0, y: 0.0 },
        start: Point { x: 0.0, y: 0.0 },
        has_point: false,
    };
    face.outline_glyph(glyph_id, &mut sink)?;

    let mut coverage = vec![0u8; width * height];
    raster.for_each_pixel(|idx, cov| {
        if idx < coverage.len() {
            coverage[idx] = (cov.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    });

    Some(CachedGlyph {
        key,
        left: min_x.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
        top: top_y.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
        width: width as u16,
        height: height as u16,
        coverage,
    })
}

/// Look up horizontal kerning in font design units across both legacy `kern`
/// and OpenType `GPOS` (`kern` feature `PairAdjustment` Format1 & Format2).
pub fn kern_unscaled(face: &Face<'_>, left: GlyphId, right: GlyphId) -> i16 {
    if let Some(kern) = face.tables().kern {
        for subtable in kern.subtables {
            if subtable.horizontal
                && !subtable.variable
                && let Some(val) = subtable.glyphs_kerning(left, right)
            {
                return val;
            }
        }
    }
    let Some(gpos) = face.tables().gpos else {
        return 0;
    };
    let kern_tag = Tag::from_bytes(b"kern");
    let mut seen_lookups = 0u64;
    let mut total = 0i32;

    for feature in gpos.features {
        if feature.tag != kern_tag {
            continue;
        }
        for lookup_idx in feature.lookup_indices {
            if lookup_idx < 64 {
                let bit = 1u64 << lookup_idx;
                if seen_lookups & bit != 0 {
                    continue;
                }
                seen_lookups |= bit;
            }
            let Some(lookup) = gpos.lookups.get(lookup_idx) else {
                continue;
            };
            for subtable in lookup.subtables.into_iter::<PositioningSubtable<'_>>() {
                if let PositioningSubtable::Pair(pair) = subtable {
                    let pair_vals = match pair {
                        PairAdjustment::Format1 { coverage, sets } => coverage
                            .get(left)
                            .and_then(|idx| sets.get(idx))
                            .and_then(|set| set.get(right)),
                        PairAdjustment::Format2 {
                            coverage,
                            classes,
                            matrix,
                        } => coverage.get(left).and_then(|_| {
                            let c1 = classes.0.get(left);
                            let c2 = classes.1.get(right);
                            matrix.get((c1, c2))
                        }),
                    };
                    if let Some((v1, v2)) = pair_vals {
                        total += i32::from(v1.x_advance) + i32::from(v2.x_advance);
                    }
                }
            }
        }
    }
    total.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_name_record(name: ttf_parser::name::Name<'_>) -> Option<String> {
        if name.is_unicode() {
            let (chunks, _) = name.name.as_chunks::<2>();
            let units: Vec<u16> = chunks
                .iter()
                .map(|&pair| u16::from_be_bytes(pair))
                .collect();
            Some(
                char::decode_utf16(units)
                    .map(|r| r.unwrap_or('\u{FFFD}'))
                    .collect(),
            )
        } else if name.platform_id == ttf_parser::PlatformId::Macintosh && name.encoding_id == 0 {
            Some(name.name.iter().map(|&b| char::from(b)).collect())
        } else {
            None
        }
    }

    #[test]
    fn bundled_fonts_parse_and_preserve_ofl_metadata() {
        for bytes in [
            ATKINSON_NEXT_REGULAR,
            ATKINSON_NEXT_SEMIBOLD,
            ATKINSON_MONO_REGULAR,
        ] {
            let face = Face::parse(bytes, 0).expect("bundled font must parse");
            assert!(face.units_per_em() > 0);
            let mut has_copyright = false;
            let mut has_ofl = false;
            for name in face.names() {
                if let Some(s) = decode_name_record(name) {
                    if s.contains("Atkinson Hyperlegible") {
                        has_copyright = true;
                    }
                    if s.contains("SIL Open Font License") {
                        has_ofl = true;
                    }
                }
            }
            assert!(
                has_copyright,
                "missing OFL copyright metadata in name table"
            );
            assert!(has_ofl, "missing OFL license metadata in name table");
        }
        const {
            assert!(BUNDLED_FONT_BYTES < 110_000);
        }
    }

    #[test]
    fn gpos_kerning_is_active_for_atkinson_next() {
        let face = Face::parse(ATKINSON_NEXT_REGULAR, 0).unwrap();
        let a = face.glyph_index('A').unwrap();
        let v = face.glyph_index('V').unwrap();
        let t = face.glyph_index('T').unwrap();
        let o = face.glyph_index('o').unwrap();
        let kern_av = kern_unscaled(&face, a, v);
        let kern_to = kern_unscaled(&face, t, o);
        assert!(
            kern_av < 0,
            "expected negative GPOS kerning for AV, got {kern_av}"
        );
        assert!(
            kern_to < 0,
            "expected negative GPOS kerning for To, got {kern_to}"
        );

        let tr = TextRenderer::new(FontMode::AtkinsonNextMono, 1080, UiScale::default());
        let w_a = tr.measure_advance("A", TextRole::Display);
        let w_v = tr.measure_advance("V", TextRole::Display);
        let w_av = tr.measure_advance("AV", TextRole::Display);
        assert!(
            w_av < w_a + w_v,
            "kerning must tighten AV: {w_av} vs {}",
            w_a + w_v
        );
    }

    #[test]
    fn mono_technical_role_has_uniform_advance_widths() {
        let tr = TextRenderer::new(FontMode::AtkinsonNextMono, 1080, UiScale::default());
        let w_i = tr.measure_advance("i", TextRole::Technical);
        let w_m = tr.measure_advance("M", TextRole::Technical);
        let w_0 = tr.measure_advance("0", TextRole::Technical);
        let w_x = tr.measure_advance("x", TextRole::Technical);
        assert!((w_i - w_m).abs() < 0.01, "mono i={w_i} M={w_m}");
        assert!((w_0 - w_x).abs() < 0.01, "mono 0={w_0} x={w_x}");
    }

    #[test]
    fn unknown_characters_render_honest_notdef_not_question_mark() {
        let mut tr = TextRenderer::new(FontMode::AtkinsonNextMono, 1080, UiScale::default());
        let mut fb_unknown = Framebuffer::new(64, 64);
        let mut fb_qmark = Framebuffer::new(64, 64);
        let bg = Rgba::rgb(0x10, 0x14, 0x1A);
        let fg = Rgba::rgb(0xFF, 0xFF, 0xFF);
        fb_unknown.fill(bg);
        fb_qmark.fill(bg);
        tr.draw_top(&mut fb_unknown, "漢", 8, 8, TextRole::Heading, fg);
        tr.draw_top(&mut fb_qmark, "?", 8, 8, TextRole::Heading, fg);
        // Unknown character must draw visible `.notdef` ink AND differ from `?`.
        assert!(fb_unknown.px.iter().any(|&p| p != bg.0));
        assert_ne!(fb_unknown.px, fb_qmark.px);
    }

    #[test]
    fn antialiased_text_produces_intermediate_coverage_in_light_and_dark() {
        for (bg, fg) in [
            (Rgba::rgb(0x10, 0x14, 0x1A), Rgba::rgb(0xF2, 0xF5, 0xF8)),
            (Rgba::rgb(0xF3, 0xF5, 0xF8), Rgba::rgb(0x1A, 0x1D, 0x21)),
        ] {
            let mut tr = TextRenderer::new(FontMode::AtkinsonNextMono, 1080, UiScale::default());
            let mut fb = Framebuffer::new(320, 64);
            fb.fill(bg);
            tr.draw_top(&mut fb, "Smart Boot Menu", 12, 12, TextRole::Heading, fg);
            let aa = fb.px.iter().filter(|&&p| p != bg.0 && p != fg.0).count();
            assert!(aa > 30, "expected antialiased edge pixels, got {aa}");
        }
    }

    #[test]
    fn truncation_wrapping_and_scaling_respect_bounds() {
        for height in [600, 768, 800, 1080, 1600, 2160] {
            for scale in [85, 100, 125, 150] {
                let tr = TextRenderer::new(FontMode::AtkinsonNextMono, height, UiScale::new(scale));
                let disp = tr.role_px(TextRole::Display);
                let head = tr.role_px(TextRole::Heading);
                let body = tr.role_px(TextRole::Body);
                let cap = tr.role_px(TextRole::Caption);
                assert!(disp > head && head > body && body > cap);

                let truncated = tr.truncate(
                    "Fedora Linux Workstation 45 Unified Kernel Image",
                    TextRole::Heading,
                    180,
                );
                assert!(truncated.ends_with('…'));
                assert!(tr.measure(&truncated, TextRole::Heading).width <= 180);

                let wrapped = tr.wrap(
                    "Cannot start \\EFI\\Microsoft\\Boot\\bootmgfw.efi because Secure Boot rejected the signature.",
                    TextRole::Body,
                    220,
                );
                assert!(wrapped.len() >= 2);
                for line in &wrapped {
                    assert!(
                        tr.measure(line, TextRole::Body).width <= 220,
                        "wrapped line too wide: {line:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn bounded_cache_tracks_hits_and_never_exceeds_memory_cap() {
        let mut tr = TextRenderer::new(FontMode::AtkinsonNextMono, 1080, UiScale::default());
        let mut fb = Framebuffer::new(640, 200);
        let fg = Rgba::rgb(0xFF, 0xFF, 0xFF);
        // Zero eager rasterization before any draw.
        assert_eq!(tr.cache_stats().entries, 0);
        assert_eq!(tr.cache_stats().bytes, 0);

        tr.draw_top(&mut fb, "Fedora Linux 45", 10, 10, TextRole::Heading, fg);
        let after_first = tr.cache_stats();
        assert!(after_first.entries > 0);
        assert!(after_first.bytes <= TextRenderer::DEFAULT_CACHE_BYTES);

        tr.draw_top(&mut fb, "Fedora Linux 45", 10, 50, TextRole::Heading, fg);
        let after_second = tr.cache_stats();
        assert!(after_second.hits > after_first.hits);
        assert_eq!(after_second.misses, after_first.misses);

        // Stress-test cache cap across many glyphs at large 4K Display size.
        tr.set_display_height(2160);
        assert_eq!(tr.cache_stats().entries, 0);
        for role in TextRole::ALL {
            tr.draw_top(
                &mut fb,
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*()",
                0,
                0,
                role,
                fg,
            );
        }
        let stressed = tr.cache_stats();
        assert!(stressed.entries <= TextRenderer::DEFAULT_CACHE_ENTRIES);
        assert!(stressed.bytes <= TextRenderer::DEFAULT_CACHE_BYTES);
    }

    #[test]
    fn fallback_font_chain_mounts_without_allocating_glyphs() {
        let mut tr = TextRenderer::new(FontMode::AtkinsonNextMono, 1080, UiScale::default());
        tr.add_fallback_font(ATKINSON_MONO_REGULAR)
            .expect("valid TTF fallback must mount");
        assert!(tr.add_fallback_font(b"not a font").is_err());
    }
}
