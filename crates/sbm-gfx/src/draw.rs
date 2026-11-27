use alloc::string::String;
use alloc::vec::Vec;

use crate::font8x8::FONT8X8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba(pub u32);

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self(((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
    }

    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }

    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }

    pub const fn b(self) -> u8 {
        self.0 as u8
    }

    pub fn mix(self, other: Self, num: u32, den: u32) -> Self {
        if den == 0 {
            return self;
        }
        let num = num.min(den);
        let inv = den - num;
        let ch = |a: u8, b: u8| ((a as u32 * inv + b as u32 * num) / den) as u8;
        Self::rgb(
            ch(self.r(), other.r()),
            ch(self.g(), other.g()),
            ch(self.b(), other.b()),
        )
    }

    /// Relative luminance approximation (`0..=255`) for contrast-aware blending.
    pub const fn luma(self) -> u8 {
        (((self.r() as u32) * 54 + (self.g() as u32) * 183 + (self.b() as u32) * 19) >> 8) as u8
    }
}

pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub px: Vec<u32>,
    clip: (u32, u32, u32, u32),
    y_bias: i32,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize).saturating_mul(height as usize);
        Self {
            width,
            height,
            px: alloc::vec![0; len],
            clip: (0, 0, width, height),
            y_bias: 0,
        }
    }

    /// Constrain subsequent draw calls to `(x, y, w, h)` while translating all
    /// content `y` coordinates by `y_bias` (pixel-exact scrollable viewports).
    pub fn set_viewport(&mut self, x: u32, y: u32, w: u32, h: u32, y_bias: i32) {
        let x0 = x.min(self.width);
        let y0 = y.min(self.height);
        let x1 = x.saturating_add(w).min(self.width);
        let y1 = y.saturating_add(h).min(self.height);
        self.clip = (x0, y0, x1, y1);
        self.y_bias = y_bias;
    }

    pub fn clear_viewport(&mut self) {
        self.clip = (0, 0, self.width, self.height);
        self.y_bias = 0;
    }

    /// Active clip rectangle `(x0, y0, x1, y1)` in screen pixels.
    #[inline]
    pub const fn clip_bounds(&self) -> (u32, u32, u32, u32) {
        self.clip
    }

    /// Active vertical translation in screen pixels (negative when scrolled down).
    #[inline]
    pub const fn y_bias(&self) -> i32 {
        self.y_bias
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgba> {
        if x < self.width && y < self.height {
            Some(Rgba(self.px[(y * self.width + x) as usize]))
        } else {
            None
        }
    }

    pub fn fill(&mut self, color: Rgba) {
        self.px.fill(color.0);
    }

    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: Rgba) {
        if self.width == 0 || self.height == 0 || w == 0 || h == 0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.clip;
        let x0 = x.max(cx0).min(cx1);
        let x1 = x.saturating_add(w).min(cx1);
        if x0 >= x1 {
            return;
        }
        let sy0 = (y as i64) + (self.y_bias as i64);
        let sy1 = sy0 + (h as i64);
        let y0 = sy0.max(cy0 as i64).min(cy1 as i64) as u32;
        let y1 = sy1.max(cy0 as i64).min(cy1 as i64) as u32;
        for row in y0..y1 {
            let start = (row * self.width + x0) as usize;
            let end = (row * self.width + x1) as usize;
            self.px[start..end].fill(color.0);
        }
    }

    #[inline]
    pub fn plot(&mut self, x: u32, y: u32, color: Rgba) {
        let (cx0, cy0, cx1, cy1) = self.clip;
        let sy = (y as i64) + (self.y_bias as i64);
        if x >= cx0 && x < cx1 && sy >= cy0 as i64 && sy < cy1 as i64 {
            self.px[((sy as u32) * self.width + x) as usize] = color.0;
        }
    }

    /// Alpha-composite `color` with coverage `alpha` (`0..=255`) at pre-clipped
    /// screen pixel `(sx, sy)` (used by vector/glyph inner loops after clipping).
    #[inline]
    pub(crate) fn blend_pixel_screen(&mut self, sx: u32, sy: u32, color: Rgba, alpha: u8) {
        let idx = (sy * self.width + sx) as usize;
        if alpha == 255 {
            self.px[idx] = color.0;
            return;
        }
        let bg = Rgba(self.px[idx]);
        let a = u32::from(alpha);
        let inv = 255 - a;
        let r = (u32::from(bg.r()) * inv + u32::from(color.r()) * a + 127) / 255;
        let g = (u32::from(bg.g()) * inv + u32::from(color.g()) * a + 127) / 255;
        let b = (u32::from(bg.b()) * inv + u32::from(color.b()) * a + 127) / 255;
        self.px[idx] = Rgba::rgb(r as u8, g as u8, b as u8).0;
    }

    /// Linear-light (gamma ~2.0) alpha-composite at pre-clipped screen pixel `(sx, sy)`.
    #[inline]
    pub(crate) fn blend_pixel_linear_screen(&mut self, sx: u32, sy: u32, color: Rgba, alpha: u8) {
        let idx = (sy * self.width + sx) as usize;
        if alpha == 255 {
            self.px[idx] = color.0;
            return;
        }
        let bg = Rgba(self.px[idx]);
        let a = u32::from(alpha);
        let inv = 255 - a;
        let blend_ch = |b: u8, f: u8| -> u8 {
            let bl = u32::from(b) * u32::from(b);
            let fl = u32::from(f) * u32::from(f);
            let mixed = (bl * inv + fl * a + 127) / 255;
            isqrt_u32(mixed).min(255) as u8
        };
        self.px[idx] = Rgba::rgb(
            blend_ch(bg.r(), color.r()),
            blend_ch(bg.g(), color.g()),
            blend_ch(bg.b(), color.b()),
        )
        .0;
    }

    /// Alpha-composite `color` with coverage `alpha` (`0..=255`) onto content `(x, y)`.
    #[inline]
    pub fn blend_pixel(&mut self, x: u32, y: u32, color: Rgba, alpha: u8) {
        if alpha == 0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.clip;
        let sy = (y as i64) + (self.y_bias as i64);
        if x < cx0 || x >= cx1 || sy < cy0 as i64 || sy >= cy1 as i64 {
            return;
        }
        self.blend_pixel_screen(x, sy as u32, color, alpha);
    }

    /// Linear-light (gamma ~2.0) alpha-composite for stem-weight-consistent
    /// antialiased text and vector edges across both dark and light themes.
    #[inline]
    pub fn blend_pixel_linear(&mut self, x: u32, y: u32, color: Rgba, alpha: u8) {
        if alpha == 0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.clip;
        let sy = (y as i64) + (self.y_bias as i64);
        if x < cx0 || x >= cx1 || sy < cy0 as i64 || sy >= cy1 as i64 {
            return;
        }
        self.blend_pixel_linear_screen(x, sy as u32, color, alpha);
    }

    #[cfg(feature = "experimental-layouts")]
    pub fn fill_hatch(&mut self, x: u32, y: u32, w: u32, h: u32, step: u32, color: Rgba) {
        let step = step.max(4);
        let (cx0, cy0, cx1, cy1) = self.clip;
        let x0 = x.max(cx0).min(cx1);
        let x1 = x.saturating_add(w).min(cx1);
        if x0 >= x1 {
            return;
        }
        let sy0 = (y as i64) + (self.y_bias as i64);
        let sy1 = sy0 + (h as i64);
        let y0 = sy0.max(cy0 as i64).min(cy1 as i64) as u32;
        let y1 = sy1.max(cy0 as i64).min(cy1 as i64) as u32;
        let step_i = step as i64;
        for py in y0..y1 {
            let content_y = (py as i64 - self.y_bias as i64).rem_euclid(step_i) as u32;
            for px in x0..x1 {
                if (px + content_y).is_multiple_of(step) {
                    self.px[(py * self.width + px) as usize] = color.0;
                }
            }
        }
    }

    pub fn text(&mut self, text: &str, x: u32, y: u32, size: u32, color: Rgba) {
        if size == 0 {
            return;
        }
        let mut cursor = x;
        for ch in text.chars() {
            self.glyph(ch, cursor, y, size, color);
            cursor = cursor.saturating_add(size);
        }
    }

    pub fn glyph(&mut self, ch: char, x: u32, y: u32, size: u32, color: Rgba) {
        let bits = glyph_bits(ch);
        for row in 0..size {
            let src_row = ((row * 8) / size) as usize;
            let bits = bits[src_row.min(7)];
            for col in 0..size {
                let src_col = (col * 8) / size;
                if (bits >> src_col) & 1 == 1 {
                    self.plot(x.saturating_add(col), y.saturating_add(row), color);
                }
            }
        }
    }

    #[cfg(feature = "experimental-layouts")]
    pub fn hline(&mut self, x: u32, y: u32, w: u32, color: Rgba) {
        self.fill_rect(x, y, w, 1, color);
    }

    #[cfg(feature = "experimental-layouts")]
    pub fn vline(&mut self, x: u32, y: u32, h: u32, color: Rgba) {
        self.fill_rect(x, y, 1, h, color);
    }

    #[cfg(feature = "experimental-layouts")]
    pub fn dashed_hline(&mut self, x: u32, y: u32, w: u32, dash: u32, gap: u32, color: Rgba) {
        let step = dash.saturating_add(gap).max(1);
        let mut offset = 0;
        while offset < w {
            let seg = dash.min(w.saturating_sub(offset));
            self.fill_rect(x.saturating_add(offset), y, seg, 1, color);
            offset = offset.saturating_add(step);
        }
    }

    #[cfg(feature = "experimental-layouts")]
    pub fn rect_outline(&mut self, x: u32, y: u32, w: u32, h: u32, thickness: u32, color: Rgba) {
        if w == 0 || h == 0 || thickness == 0 {
            return;
        }
        let t = thickness.min(w / 2).min(h / 2).max(1);
        self.fill_rect(x, y, w, t, color);
        self.fill_rect(x, y.saturating_add(h).saturating_sub(t), w, t, color);
        self.fill_rect(x, y, t, h, color);
        self.fill_rect(x.saturating_add(w).saturating_sub(t), y, t, h, color);
    }

    #[cfg(feature = "experimental-layouts")]
    pub fn dot_grid(&mut self, x: u32, y: u32, w: u32, h: u32, step: u32, color: Rgba) {
        let step = step.max(4);
        let x1 = x.saturating_add(w).min(self.width);
        let y1 = y.saturating_add(h).min(self.height);
        let mut py = y;
        while py < y1 {
            let mut px = x;
            while px < x1 {
                self.plot(px, py, color);
                px = px.saturating_add(step);
            }
            py = py.saturating_add(step);
        }
    }

    #[cfg(feature = "experimental-layouts")]
    pub fn corner_ticks(&mut self, x: u32, y: u32, w: u32, h: u32, len: u32, color: Rgba) {
        if w < 4 || h < 4 {
            return;
        }
        let len = len.min(w / 3).min(h / 3).max(2);
        let rx = x.saturating_add(w);
        let by = y.saturating_add(h);
        // Top-left
        self.hline(x, y, len, color);
        self.vline(x, y, len, color);
        // Top-right
        self.hline(rx.saturating_sub(len), y, len, color);
        self.vline(rx.saturating_sub(1), y, len, color);
        // Bottom-left
        self.hline(x, by.saturating_sub(1), len, color);
        self.vline(x, by.saturating_sub(len), len, color);
        // Bottom-right
        self.hline(rx.saturating_sub(len), by.saturating_sub(1), len, color);
        self.vline(rx.saturating_sub(1), by.saturating_sub(len), len, color);
    }

    pub fn to_ppm(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let header = alloc::format!("P6\n{} {}\n255\n", self.width, self.height);
        out.extend_from_slice(header.as_bytes());
        for pixel in &self.px {
            let color = Rgba(*pixel);
            out.push(color.r());
            out.push(color.g());
            out.push(color.b());
        }
        out
    }
}

/// Fast integer square root for `0..=65025` (8-bit squared channels).
#[inline]
fn isqrt_u32(v: u32) -> u32 {
    if v <= 1 {
        return v;
    }
    let mut x = 1u32 << (32 - v.leading_zeros()).div_ceil(2);
    loop {
        let next = (x + v / x) >> 1;
        if next >= x {
            return x;
        }
        x = next;
    }
}

pub fn text_width(text: &str, size: u32) -> u32 {
    (text.chars().count() as u32).saturating_mul(size)
}

pub fn fit_text(text: &str, size: u32, max_width: u32) -> String {
    if size == 0 || text_width(text, size) <= max_width {
        return text.into();
    }
    let ellipsis = size.saturating_mul(3);
    if max_width <= ellipsis {
        return String::from("...");
    }
    let room = ((max_width - ellipsis) / size) as usize;
    let mut out: String = text.chars().take(room).collect();
    out.push_str("...");
    out
}

/// Draw a visible missing-glyph tofu box (`.notdef`) for legacy 8x8 mode
/// rather than misleadingly substituting `'?'`.
const NOTDEF_8X8: [u8; 8] = [0x7E, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x00];

fn glyph_bits(ch: char) -> [u8; 8] {
    match ch {
        '\u{00B7}' => [0x00, 0x00, 0x00, 0x18, 0x18, 0x00, 0x00, 0x00], // · middle dot
        '\u{00D7}' => [0x00, 0x00, 0x42, 0x24, 0x18, 0x24, 0x42, 0x00], // × multiplication
        '\u{2013}' | '\u{2014}' => [0x00, 0x00, 0x00, 0x7E, 0x00, 0x00, 0x00, 0x00], // – / — dash
        _ => {
            let index = u32::from(ch);
            if index < 128 {
                FONT8X8[index as usize]
            } else {
                NOTDEF_8X8
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_a_has_ink_and_text_clips() {
        let mut fb = Framebuffer::new(64, 16);
        fb.glyph('A', 0, 0, 8, Rgba::rgb(255, 255, 255));
        assert!(fb.px.iter().any(|pixel| *pixel != 0));
        let fitted = fit_text("Fedora Linux", 8, 40);
        assert!(fitted.ends_with("..."));
        assert!(text_width(&fitted, 8) <= 40);
    }

    #[test]
    fn unknown_glyph_uses_notdef_box_not_question_mark() {
        assert_ne!(glyph_bits('漢'), FONT8X8[b'?' as usize]);
        assert_eq!(glyph_bits('漢'), NOTDEF_8X8);
    }

    #[test]
    fn blend_pixel_and_linear_blend_produce_antialiased_intermediate() {
        let mut fb = Framebuffer::new(4, 1);
        fb.fill(Rgba::rgb(0, 0, 0));
        fb.blend_pixel(0, 0, Rgba::rgb(255, 255, 255), 128);
        fb.blend_pixel_linear(1, 0, Rgba::rgb(255, 255, 255), 128);
        let p0 = fb.pixel(0, 0).unwrap();
        let p1 = fb.pixel(1, 0).unwrap();
        assert!(p0.r() > 120 && p0.r() < 135);
        // Linear-light 50% coverage maps to ~180 in sRGB (gamma 2.0)
        assert!(p1.r() > 170 && p1.r() < 190);
    }

    #[test]
    fn viewport_clips_and_translates_y_for_all_pixel_blends() {
        let mut fb = Framebuffer::new(20, 20);
        let bg = Rgba::rgb(0, 0, 0);
        let fg = Rgba::rgb(200, 100, 50);
        fb.fill(bg);
        fb.set_viewport(4, 4, 10, 10, -20);
        // Content y=26 -> screen y=6 (inside viewport [4, 14))
        fb.blend_pixel(8, 26, fg, 255);
        // Content y=22 -> screen y=2 (above viewport clip!)
        fb.blend_pixel(8, 22, fg, 255);
        fb.clear_viewport();
        assert_eq!(fb.pixel(8, 6), Some(fg));
        assert_eq!(fb.pixel(8, 2), Some(bg));
    }
}
