//! Anti-aliased `no_std` typography backed by `noto-sans-mono-bitmap` 0.4.
//!
//! Zero heap allocation during glyph rendering, zero floating-point math,
//! pixel-aligned baselines, and optional 12 px compact mode via 2x downsampling
//! of the 24 px glyphs for crisp high-density technical eyebrows and labels.

use alloc::string::String;
use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};

use crate::draw::{Framebuffer, Rgba};

/// Selectable anti-aliased text style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextStyle {
    /// 12 px compact anti-aliased mono (2x box-filtered from 24 px Regular).
    Micro,
    /// 12 px bold compact anti-aliased mono (2x box-filtered from 24 px Bold).
    MicroBold,
    /// 16 px Regular Noto Sans Mono.
    Body,
    /// 16 px Bold Noto Sans Mono.
    BodyBold,
    /// 20 px Bold Noto Sans Mono.
    Title,
    /// 24 px Bold Noto Sans Mono.
    Hero,
}

impl TextStyle {
    pub const fn line_height(self) -> u32 {
        match self {
            Self::Micro | Self::MicroBold => 12,
            Self::Body | Self::BodyBold => 16,
            Self::Title => 20,
            Self::Hero => 24,
        }
    }

    pub const fn char_width(self) -> u32 {
        match self {
            Self::Micro => (get_raster_width(FontWeight::Regular, RasterHeight::Size24) as u32) / 2,
            Self::MicroBold => {
                (get_raster_width(FontWeight::Bold, RasterHeight::Size24) as u32) / 2
            }
            Self::Body => get_raster_width(FontWeight::Regular, RasterHeight::Size16) as u32,
            Self::BodyBold => get_raster_width(FontWeight::Bold, RasterHeight::Size16) as u32,
            Self::Title => get_raster_width(FontWeight::Bold, RasterHeight::Size20) as u32,
            Self::Hero => get_raster_width(FontWeight::Bold, RasterHeight::Size24) as u32,
        }
    }

    const fn backing(self) -> (FontWeight, RasterHeight, bool) {
        match self {
            Self::Micro => (FontWeight::Regular, RasterHeight::Size24, true),
            Self::MicroBold => (FontWeight::Bold, RasterHeight::Size24, true),
            Self::Body => (FontWeight::Regular, RasterHeight::Size16, false),
            Self::BodyBold => (FontWeight::Bold, RasterHeight::Size16, false),
            Self::Title => (FontWeight::Bold, RasterHeight::Size20, false),
            Self::Hero => (FontWeight::Bold, RasterHeight::Size24, false),
        }
    }
}

/// Width in pixels of `text` rendered in `style` with optional extra `tracking`.
pub fn measure_text(text: &str, style: TextStyle, tracking: u32) -> u32 {
    let count = text.chars().count() as u32;
    if count == 0 {
        return 0;
    }
    count
        .saturating_mul(style.char_width())
        .saturating_add(count.saturating_sub(1).saturating_mul(tracking))
}

/// Truncate `text` with `..` so it fits within `max_width` pixels.
pub fn fit_mono(text: &str, style: TextStyle, tracking: u32, max_width: u32) -> String {
    if measure_text(text, style, tracking) <= max_width {
        return text.into();
    }
    let step = style.char_width().saturating_add(tracking).max(1);
    let ellipsis = measure_text("..", style, tracking);
    if max_width <= ellipsis {
        return String::new();
    }
    let room = ((max_width - ellipsis) / step) as usize;
    let mut out: String = text.chars().take(room).collect();
    out.push_str("..");
    out
}

/// Draw anti-aliased Noto Sans Mono text clipped to `max_width`.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_fit(
    fb: &mut Framebuffer,
    text: &str,
    x: u32,
    y: u32,
    style: TextStyle,
    tracking: u32,
    max_width: u32,
    color: Rgba,
) -> u32 {
    if max_width < style.char_width() {
        return 0;
    }
    let fitted = fit_mono(text, style, tracking, max_width);
    draw_text(fb, &fitted, x, y, style, tracking, color)
}

/// Draw anti-aliased Noto Sans Mono text and return rendered pixel width.
pub fn draw_text(
    fb: &mut Framebuffer,
    text: &str,
    x: u32,
    y: u32,
    style: TextStyle,
    tracking: u32,
    color: Rgba,
) -> u32 {
    let (weight, height, half) = style.backing();
    let step = style.char_width().saturating_add(tracking);
    let mut cx = x;
    for ch in text.chars() {
        let glyph = get_raster(ch, weight, height).or_else(|| get_raster('?', weight, height));
        if let Some(raster) = glyph {
            if half {
                let rw = raster.width() / 2;
                let rh = raster.height() / 2;
                let pixels = raster.pixels();
                let stride = raster.width();
                for dy in 0..rh {
                    let sy = dy * 2;
                    for dx in 0..rw {
                        let sx = dx * 2;
                        let sum = u32::from(pixels[sy * stride + sx])
                            + u32::from(pixels[sy * stride + sx + 1])
                            + u32::from(pixels[(sy + 1) * stride + sx])
                            + u32::from(pixels[(sy + 1) * stride + sx + 1]);
                        // Slight contrast boost keeps 12 px stems crisp on 1x displays.
                        let alpha = ((sum * 5) / 16).min(255) as u8;
                        if alpha > 8 {
                            fb.blend_pixel(cx + dx as u32, y + dy as u32, color, alpha);
                        }
                    }
                }
            } else {
                let rw = raster.width();
                let rh = raster.height();
                let pixels = raster.pixels();
                for dy in 0..rh {
                    let row_off = dy * rw;
                    for dx in 0..rw {
                        let alpha = pixels[row_off + dx];
                        if alpha > 0 {
                            fb.blend_pixel(cx + dx as u32, y + dy as u32, color, alpha);
                        }
                    }
                }
            }
        }
        cx = cx.saturating_add(step);
    }
    cx.saturating_sub(x)
}
