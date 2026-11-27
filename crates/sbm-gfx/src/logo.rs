//! `no_std` hardware logo decoding (QOI) and aspect-preserving framebuffer
//! blitting for CPU/GPU badges cached on the ESP.
//!
//! Badges are distributed in `smart-boot-menu/hardware-logos` as maximally
//! compressed 128 px AVIF 4:4:4 files and transcoded once per machine into
//! compact `.qoi` files under `\EFI\SBM\cache\logos\`. Decoding QOI at boot
//! via `rapid-qoi` takes microseconds with zero `unsafe` and zero transitive
//! dependencies.

use alloc::vec::Vec;

use libm::{ceilf, floorf, sqrtf};
use rapid_qoi::Qoi;

use crate::draw::{Framebuffer, Rgba};

/// Maximum accepted badge edge in pixels (guards against malformed cache files).
pub const MAX_LOGO_EDGE: u32 = 512;

/// Decoded RGBA8888 hardware badge ready for framebuffer blitting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogoImage {
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA bytes (`width * height * 4`).
    pub rgba: Vec<u8>,
    /// True when the decoded image contains meaningful transparency (`a < 250`).
    /// Opaque stickers receive a subtle rounded-corner clip when drawn in tiles;
    /// transparent badges preserve their native alpha silhouette.
    pub has_alpha: bool,
}

impl LogoImage {
    /// Construct from validated straight-alpha RGBA8888 bytes.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        if width == 0 || height == 0 || width > MAX_LOGO_EDGE || height > MAX_LOGO_EDGE {
            return None;
        }
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        if rgba.len() != expected {
            return None;
        }
        let has_alpha = rgba.as_chunks::<4>().0.iter().any(|px| px[3] < 250);
        Some(Self {
            width,
            height,
            rgba,
            has_alpha,
        })
    }

    /// Decode a cached `.qoi` hardware badge into straight-alpha RGBA8888.
    pub fn from_qoi(bytes: &[u8]) -> Option<Self> {
        let (header, pixels) = Qoi::decode_alloc(bytes).ok()?;
        let (w, h) = (header.width, header.height);
        if w == 0 || h == 0 || w > MAX_LOGO_EDGE || h > MAX_LOGO_EDGE {
            return None;
        }
        if header.colors.has_alpha() {
            Self::from_rgba(w, h, pixels)
        } else {
            let count = (w as usize).checked_mul(h as usize)?;
            if pixels.len() != count.checked_mul(3)? {
                return None;
            }
            let mut rgba = Vec::with_capacity(count.checked_mul(4)?);
            for rgb in pixels.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            Some(Self {
                width: w,
                height: h,
                rgba,
                has_alpha: false,
            })
        }
    }
}

/// Smoothly compute rounded-rectangle alpha coverage (`0.0..=1.0`) at pixel center
/// `(dx + 0.5, dy + 0.5)` inside a `dst_w × dst_h` box with radius `r`.
#[inline]
fn rounded_box_coverage(dx: u32, dy: u32, dst_w: u32, dst_h: u32, r: f32) -> f32 {
    if r <= 0.5 {
        return 1.0;
    }
    let px = dx as f32 + 0.5;
    let py = dy as f32 + 0.5;
    let w = dst_w as f32;
    let h = dst_h as f32;
    let rx = if px < r {
        r - px
    } else if px > w - r {
        px - (w - r)
    } else {
        0.0
    };
    let ry = if py < r {
        r - py
    } else if py > h - r {
        py - (h - r)
    } else {
        0.0
    };
    if rx == 0.0 || ry == 0.0 {
        return 1.0;
    }
    let dist = sqrtf(rx * rx + ry * ry) - r;
    (0.5 - dist).clamp(0.0, 1.0)
}

/// Blit a decoded [`LogoImage`] aspect-preserving and centered inside
/// `(x, y, box_w, box_h)` using premultiplied-alpha area averaging.
///
/// Opaque badges (`!logo.has_alpha`) receive an antialiased rounded-corner clip
/// of `corner_radius` pixels so rectangular stickers blend seamlessly into
/// rounded hardware tiles. Transparent badges preserve their native silhouette.
/// Respects the framebuffer's active viewport clip and `(x_bias, y_bias)`.
pub fn draw_logo_image(
    fb: &mut Framebuffer,
    logo: &LogoImage,
    x: u32,
    y: u32,
    box_w: u32,
    box_h: u32,
    corner_radius: u32,
) -> bool {
    if logo.width == 0 || logo.height == 0 || box_w == 0 || box_h == 0 {
        return false;
    }
    let sw = logo.width;
    let sh = logo.height;
    let expected = match (sw as usize)
        .checked_mul(sh as usize)
        .and_then(|n| n.checked_mul(4))
    {
        Some(n) if logo.rgba.len() == n => n,
        _ => return false,
    };
    let _ = expected;

    // Aspect-preserving fit inside `(box_w, box_h)`.
    let (dst_w, dst_h) = if (sw as u64) * (box_h as u64) >= (sh as u64) * (box_w as u64) {
        let dw = box_w;
        let dh =
            ((sh as u64 * box_w as u64 + sw as u64 / 2) / sw as u64).clamp(1, box_h as u64) as u32;
        (dw, dh)
    } else {
        let dh = box_h;
        let dw =
            ((sw as u64 * box_h as u64 + sh as u64 / 2) / sh as u64).clamp(1, box_w as u64) as u32;
        (dw, dh)
    };

    let ox = x.saturating_add(box_w.saturating_sub(dst_w) / 2);
    let oy = y.saturating_add(box_h.saturating_sub(dst_h) / 2);

    // Quick visibility reject against active framebuffer viewport.
    let (cx0, cy0, cx1, cy1) = fb.clip_bounds();
    let sx0 = (ox as i64) + (fb.x_bias() as i64);
    let sy0 = (oy as i64) + (fb.y_bias() as i64);
    if sx0 + (dst_w as i64) <= cx0 as i64
        || sx0 >= cx1 as i64
        || sy0 + (dst_h as i64) <= cy0 as i64
        || sy0 >= cy1 as i64
    {
        return true;
    }

    let eff_radius = if logo.has_alpha {
        0.0
    } else {
        (corner_radius.min(dst_w / 2).min(dst_h / 2)) as f32
    };

    let scale_x = sw as f32 / dst_w as f32;
    let scale_y = sh as f32 / dst_h as f32;

    for dy in 0..dst_h {
        let fy0 = dy as f32 * scale_y;
        let fy1 = (dy + 1) as f32 * scale_y;
        let iy0 = (floorf(fy0) as i32).clamp(0, sh as i32 - 1) as u32;
        let iy1 = (ceilf(fy1) as i32).clamp(iy0 as i32 + 1, sh as i32) as u32;

        for dx in 0..dst_w {
            let corner_cov = if eff_radius > 0.5 {
                rounded_box_coverage(dx, dy, dst_w, dst_h, eff_radius)
            } else {
                1.0
            };
            if corner_cov <= 0.0 {
                continue;
            }

            let fx0 = dx as f32 * scale_x;
            let fx1 = (dx + 1) as f32 * scale_x;
            let ix0 = (floorf(fx0) as i32).clamp(0, sw as i32 - 1) as u32;
            let ix1 = (ceilf(fx1) as i32).clamp(ix0 as i32 + 1, sw as i32) as u32;

            let mut acc_r = 0.0f32;
            let mut acc_g = 0.0f32;
            let mut acc_b = 0.0f32;
            let mut acc_a = 0.0f32;
            let mut acc_w = 0.0f32;

            for sy in iy0..iy1 {
                let wy = (fy1.min((sy + 1) as f32) - fy0.max(sy as f32)).max(0.0);
                if wy <= 0.0 {
                    continue;
                }
                let row_base = (sy * sw) as usize * 4;
                for sx in ix0..ix1 {
                    let wx = (fx1.min((sx + 1) as f32) - fx0.max(sx as f32)).max(0.0);
                    if wx <= 0.0 {
                        continue;
                    }
                    let w = wx * wy;
                    let idx = row_base + (sx as usize) * 4;
                    let a = f32::from(logo.rgba[idx + 3]) / 255.0;
                    let wa = w * a;
                    acc_r += f32::from(logo.rgba[idx]) * wa;
                    acc_g += f32::from(logo.rgba[idx + 1]) * wa;
                    acc_b += f32::from(logo.rgba[idx + 2]) * wa;
                    acc_a += wa;
                    acc_w += w;
                }
            }

            if acc_w <= 0.0 || acc_a <= 0.0 {
                continue;
            }
            let out_a = ((acc_a / acc_w) * corner_cov * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            if out_a == 0 {
                continue;
            }
            let r = (acc_r / acc_a + 0.5).clamp(0.0, 255.0) as u8;
            let g = (acc_g / acc_a + 0.5).clamp(0.0, 255.0) as u8;
            let b = (acc_b / acc_a + 0.5).clamp(0.0, 255.0) as u8;
            fb.blend_pixel(ox + dx, oy + dy, Rgba::rgb(r, g, b), out_a);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use rapid_qoi::{Colors, Qoi};

    use super::*;

    #[test]
    fn qoi_roundtrip_rgb_and_rgba() {
        // Opaque RGB badge
        let rgb = [255u8, 80, 20, 255, 80, 20, 20, 120, 240, 20, 120, 240];
        let qoi_rgb = Qoi {
            width: 2,
            height: 2,
            colors: Colors::Srgb,
        }
        .encode_alloc(&rgb)
        .expect("encode RGB QOI");
        let decoded_rgb = LogoImage::from_qoi(&qoi_rgb).expect("decode RGB QOI");
        assert_eq!((decoded_rgb.width, decoded_rgb.height), (2, 2));
        assert!(!decoded_rgb.has_alpha);
        assert_eq!(&decoded_rgb.rgba[0..4], &[255, 80, 20, 255]);
        assert_eq!(&decoded_rgb.rgba[8..12], &[20, 120, 240, 255]);

        // Transparent RGBA badge
        let rgba = [
            220u8, 20, 40, 255, 0, 0, 0, 0, 220, 20, 40, 128, 255, 255, 255, 255,
        ];
        let qoi_rgba = Qoi {
            width: 2,
            height: 2,
            colors: Colors::SrgbLinA,
        }
        .encode_alloc(&rgba)
        .expect("encode RGBA QOI");
        let decoded_rgba = LogoImage::from_qoi(&qoi_rgba).expect("decode RGBA QOI");
        assert_eq!((decoded_rgba.width, decoded_rgba.height), (2, 2));
        assert!(decoded_rgba.has_alpha);
        assert_eq!(decoded_rgba.rgba, rgba);
    }

    #[test]
    fn rejects_corrupt_and_oversized_qoi() {
        assert!(LogoImage::from_qoi(b"not a qoi file").is_none());
        assert!(LogoImage::from_rgba(0, 10, Vec::new()).is_none());
        assert!(LogoImage::from_rgba(MAX_LOGO_EDGE + 1, 1, Vec::new()).is_none());
    }

    #[test]
    fn draw_logo_image_blends_rounds_opaque_corners_and_respects_scroll() {
        // 32x32 opaque orange badge
        let mut rgba = Vec::with_capacity(32 * 32 * 4);
        for _ in 0..(32 * 32) {
            rgba.extend_from_slice(&[240, 110, 20, 255]);
        }
        let logo = LogoImage::from_rgba(32, 32, rgba).unwrap();
        assert!(!logo.has_alpha);

        let bg = Rgba::rgb(0x10, 0x16, 0x1E);
        let mut fb = Framebuffer::new(80, 40);
        fb.fill(bg);
        assert!(draw_logo_image(&mut fb, &logo, 10, 4, 24, 24, 6));

        // Center pixel is painted orange
        let center = fb.pixel(22, 16).unwrap();
        assert!(center.r() > 220 && center.g() > 95 && center.b() < 40);
        // Corner pixel (10, 4) is clipped by rounded-corner mask back to bg
        assert_eq!(fb.pixel(10, 4).unwrap(), bg);

        // Transparent badge preserves exact background where alpha == 0
        let mut t_rgba = alloc::vec![0u8; 16 * 16 * 4];
        for y in 4..12 {
            for x in 4..12 {
                let i = (y * 16 + x) * 4;
                t_rgba[i..i + 4].copy_from_slice(&[230, 30, 50, 255]);
            }
        }
        let t_logo = LogoImage::from_rgba(16, 16, t_rgba).unwrap();
        assert!(t_logo.has_alpha);

        let mut fb2 = Framebuffer::new(80, 40);
        fb2.fill(bg);
        fb2.set_viewport_xy(10, 0, 30, 40, -10, 0);
        assert!(draw_logo_image(&mut fb2, &t_logo, 12, 8, 16, 16, 6));
        fb2.clear_viewport();

        // Drawn at content x=12..28 shifted by -10 -> screen x=2..18, clipped to x>=10
        assert_eq!(fb2.pixel(5, 16).unwrap(), bg, "must respect left clip");
        let shifted_center = fb2.pixel(10, 16).unwrap();
        assert!(
            shifted_center.r() > 200 && shifted_center.g() < 50,
            "red center should land at scrolled screen x=10, got {shifted_center:?}"
        );
    }
}
