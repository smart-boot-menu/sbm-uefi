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
}

pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub px: Vec<u32>,
    #[cfg(feature = "experimental-layouts")]
    clip: Option<(u32, u32, u32, u32)>,
    #[cfg(feature = "experimental-layouts")]
    y_bias: i32,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize).saturating_mul(height as usize);
        Self {
            width,
            height,
            px: alloc::vec![0; len],
            #[cfg(feature = "experimental-layouts")]
            clip: None,
            #[cfg(feature = "experimental-layouts")]
            y_bias: 0,
        }
    }

    /// Constrain subsequent draw calls to `(x, y, w, h)` while translating all
    /// `y` coordinates by `y_bias` (used for pixel-exact scrollable viewports).
    #[cfg(feature = "experimental-layouts")]
    pub fn set_viewport(&mut self, x: u32, y: u32, w: u32, h: u32, y_bias: i32) {
        let x0 = x.min(self.width);
        let y0 = y.min(self.height);
        let x1 = x.saturating_add(w).min(self.width);
        let y1 = y.saturating_add(h).min(self.height);
        self.clip = Some((x0, y0, x1, y1));
        self.y_bias = y_bias;
    }

    #[cfg(feature = "experimental-layouts")]
    pub fn clear_viewport(&mut self) {
        self.clip = None;
        self.y_bias = 0;
    }

    #[cfg(feature = "experimental-layouts")]
    #[inline]
    fn clip_bounds(&self) -> (u32, u32, u32, u32) {
        self.clip.unwrap_or((0, 0, self.width, self.height))
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
        #[cfg(feature = "experimental-layouts")]
        {
            let (cx0, cy0, cx1, cy1) = self.clip_bounds();
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
        #[cfg(not(feature = "experimental-layouts"))]
        {
            let x0 = x.min(self.width);
            let y0 = y.min(self.height);
            let x1 = x.saturating_add(w).min(self.width);
            let y1 = y.saturating_add(h).min(self.height);
            for row in y0..y1 {
                let start = (row * self.width + x0) as usize;
                let end = (row * self.width + x1) as usize;
                self.px[start..end].fill(color.0);
            }
        }
    }

    pub fn plot(&mut self, x: u32, y: u32, color: Rgba) {
        #[cfg(feature = "experimental-layouts")]
        {
            let (cx0, cy0, cx1, cy1) = self.clip_bounds();
            let sy = (y as i64) + (self.y_bias as i64);
            if x >= cx0 && x < cx1 && sy >= cy0 as i64 && sy < cy1 as i64 {
                self.px[((sy as u32) * self.width + x) as usize] = color.0;
            }
        }
        #[cfg(not(feature = "experimental-layouts"))]
        {
            if x < self.width && y < self.height {
                self.px[(y * self.width + x) as usize] = color.0;
            }
        }
    }

    #[cfg(feature = "experimental-layouts")]
    #[inline]
    pub fn blend_pixel(&mut self, x: u32, y: u32, color: Rgba, alpha: u8) {
        if alpha == 0 {
            return;
        }
        let (cx0, cy0, cx1, cy1) = self.clip_bounds();
        let sy = (y as i64) + (self.y_bias as i64);
        if x < cx0 || x >= cx1 || sy < cy0 as i64 || sy >= cy1 as i64 {
            return;
        }
        let idx = ((sy as u32) * self.width + x) as usize;
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

    #[cfg(feature = "experimental-layouts")]
    pub fn fill_hatch(&mut self, x: u32, y: u32, w: u32, h: u32, step: u32, color: Rgba) {
        let step = step.max(4);
        let (cx0, cy0, cx1, cy1) = self.clip_bounds();
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

    pub fn round_rect(&mut self, x: u32, y: u32, w: u32, h: u32, radius: u32, color: Rgba) {
        if w == 0 || h == 0 {
            return;
        }
        let radius = radius.min(w / 2).min(h / 2);
        for row in 0..h {
            for col in 0..w {
                if inside_round(col, row, w, h, radius) {
                    self.plot(x.saturating_add(col), y.saturating_add(row), color);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn round_panel(
        &mut self,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        radius: u32,
        thickness: u32,
        fill: Rgba,
        border: Rgba,
    ) {
        self.round_rect(x, y, w, h, radius, border);
        if w > thickness * 2 && h > thickness * 2 {
            self.round_rect(
                x.saturating_add(thickness),
                y.saturating_add(thickness),
                w - thickness * 2,
                h - thickness * 2,
                radius.saturating_sub(thickness),
                fill,
            );
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

    #[cfg(feature = "experimental-layouts")]
    pub fn circle_dot(&mut self, cx: u32, cy: u32, r: u32, color: Rgba) {
        let ri = r as i32;
        let r2 = ri * ri;
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                if dx * dx + dy * dy <= r2 {
                    let px = cx as i32 + dx;
                    let py = cy as i32 + dy;
                    if px >= 0 && py >= 0 {
                        self.plot(px as u32, py as u32, color);
                    }
                }
            }
        }
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

fn glyph_bits(ch: char) -> [u8; 8] {
    let index = u32::from(ch);
    if index < 128 {
        FONT8X8[index as usize]
    } else {
        FONT8X8[b'?' as usize]
    }
}

fn inside_round(col: u32, row: u32, w: u32, h: u32, radius: u32) -> bool {
    if radius == 0 {
        return true;
    }
    let cx = if col < radius {
        radius
    } else if col >= w.saturating_sub(radius) {
        w.saturating_sub(radius)
    } else {
        return true;
    };
    let cy = if row < radius {
        radius
    } else if row >= h.saturating_sub(radius) {
        h.saturating_sub(radius)
    } else {
        return true;
    };
    let dx = col as i32 - cx as i32;
    let dy = row as i32 - cy as i32;
    dx * dx + dy * dy <= (radius as i32) * (radius as i32)
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
}
