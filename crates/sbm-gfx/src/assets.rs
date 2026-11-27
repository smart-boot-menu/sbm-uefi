//! Crisp `no_std` geometric OS emblems, encryption badges, and hardware icons.
//!
//! Pure integer signed-distance / anti-aliased rasterization directly onto
//! [`Framebuffer`]: zero external binary image assets, zero unsafe, minimal code
//! footprint.

use crate::draw::{Framebuffer, Rgba};

/// Draw a smooth anti-aliased filled circle (`cx`, `cy`, `radius` in pixels).
pub fn aa_circle(fb: &mut Framebuffer, cx: i32, cy: i32, radius: i32, color: Rgba) {
    if radius <= 0 {
        return;
    }
    let r2 = radius * radius;
    let outer = (radius + 1) * (radius + 1);
    let inner = (radius - 1).max(0) * (radius - 1).max(0);
    let x0 = (cx - radius - 1).max(0);
    let y0 = (cy - radius - 1).max(0);
    let x1 = (cx + radius + 1).min(fb.width.saturating_sub(1) as i32);
    let y1 = (cy + radius + 1).min(fb.height.saturating_sub(1) as i32);
    for py in y0..=y1 {
        let dy = py - cy;
        for px in x0..=x1 {
            let dx = px - cx;
            let d2 = dx * dx + dy * dy;
            if d2 <= inner {
                fb.plot(px as u32, py as u32, color);
            } else if d2 <= outer {
                let span = (outer - inner).max(1);
                let num = (outer - d2).clamp(0, span);
                let alpha = ((num * 255) / span) as u8;
                let _ = r2;
                fb.blend_pixel(px as u32, py as u32, color, alpha);
            }
        }
    }
}

/// Draw a smooth anti-aliased ring (`cx`, `cy`, outer `radius`, `thickness`).
pub fn aa_ring(fb: &mut Framebuffer, cx: i32, cy: i32, radius: i32, thickness: i32, color: Rgba) {
    if radius <= 1 || thickness <= 0 {
        return;
    }
    let r_out = radius;
    let r_in = (radius - thickness).max(0);
    let x0 = (cx - r_out - 1).max(0);
    let y0 = (cy - r_out - 1).max(0);
    let x1 = (cx + r_out + 1).min(fb.width.saturating_sub(1) as i32);
    let y1 = (cy + r_out + 1).min(fb.height.saturating_sub(1) as i32);
    for py in y0..=y1 {
        let dy = py - cy;
        for px in x0..=x1 {
            let dx = px - cx;
            let dist = isqrt_4x((dx * dx + dy * dy) as u32);
            // dist is in quarter-pixels (4 * r).
            let lo = (r_in * 4) as u32;
            let hi = (r_out * 4) as u32;
            let alpha = if dist >= lo + 2 && dist <= hi.saturating_sub(2) {
                255
            } else if dist + 2 >= lo && dist < lo + 2 {
                (((dist + 2 - lo) * 255) / 4) as u8
            } else if dist > hi.saturating_sub(2) && dist <= hi + 2 {
                (((hi + 2 - dist) * 255) / 4) as u8
            } else {
                0
            };
            if alpha > 0 {
                fb.blend_pixel(px as u32, py as u32, color, alpha);
            }
        }
    }
}

/// Draw an anti-aliased OS / boot-target emblem inside `(x, y, size, size)`.
#[allow(clippy::too_many_arguments)]
pub fn draw_os_emblem(
    fb: &mut Framebuffer,
    hint: &str,
    title: &str,
    x: u32,
    y: u32,
    size: u32,
    badge_bg: Rgba,
    fg: Rgba,
) {
    if size < 10 {
        return;
    }
    fb.round_rect(x, y, size, size, (size / 4).max(3), badge_bg);
    let lower = title.to_ascii_lowercase();
    let cx = (x + size / 2) as i32;
    let cy = (y + size / 2) as i32;
    let inset = (size / 5).max(2);
    let inner = size.saturating_sub(inset * 2);

    if hint == "windows" || lower.contains("windows") {
        // Classic 4-pane window grid.
        let gap = (inner / 7).max(1);
        let pane = inner.saturating_sub(gap) / 2;
        fb.fill_rect(x + inset, y + inset, pane, pane, fg);
        fb.fill_rect(x + inset + pane + gap, y + inset, pane, pane, fg);
        fb.fill_rect(x + inset, y + inset + pane + gap, pane, pane, fg);
        fb.fill_rect(
            x + inset + pane + gap,
            y + inset + pane + gap,
            pane,
            pane,
            fg,
        );
    } else if hint == "recovery" || lower.contains("sbm") || lower.contains("recovery") {
        // Precision cross inside subtle shield frame.
        let arm = (inner / 3).max(2);
        fb.fill_rect(x + (size - arm) / 2, y + inset, arm, inner, fg);
        fb.fill_rect(x + inset, y + (size - arm) / 2, inner, arm, fg);
    } else if lower.contains("ubuntu") {
        // Ubuntu-inspired ring with 3 satellite nodes.
        let r = ((inner as i32) * 38 / 100).max(3);
        aa_ring(fb, cx, cy, r, (size as i32 / 9).max(2), fg);
        let dot_r = (size as i32 / 10).max(1);
        aa_circle(fb, cx - r, cy, dot_r, fg);
        aa_circle(fb, cx + r / 2, cy - (r * 86 / 100), dot_r, fg);
        aa_circle(fb, cx + r / 2, cy + (r * 86 / 100), dot_r, fg);
    } else if lower.contains("arch") {
        // Arch-inspired crisp geometric delta (A-peak with bottom notch).
        let top_y = (y + inset) as i32;
        let bot_y = (y + size - inset) as i32;
        let h = (bot_y - top_y).max(1);
        for py in top_y..=bot_y {
            let t = py - top_y;
            let half_w = (t * (inner as i32 / 2)) / h;
            let notch = if t > h * 62 / 100 {
                ((t - h * 62 / 100) * (inner as i32 / 4)) / (h * 38 / 100).max(1)
            } else {
                0
            };
            for px in (cx - half_w)..=(cx + half_w) {
                if notch > 0 && (px - cx).abs() < notch {
                    continue;
                }
                if px >= 0 && py >= 0 {
                    fb.plot(px as u32, py as u32, fg);
                }
            }
        }
    } else if lower.contains("debian") {
        // Debian-inspired concentric open swirl.
        let r = ((inner as i32) * 44 / 100).max(3);
        aa_ring(fb, cx, cy, r, (size as i32 / 10).max(2), fg);
        aa_ring(
            fb,
            cx + 1,
            cy - 1,
            (r * 58 / 100).max(2),
            (size as i32 / 12).max(1),
            fg,
        );
    } else if lower.contains("fedora") {
        // Fedora-inspired bold 'f' ring mark.
        let r = ((inner as i32) * 42 / 100).max(3);
        aa_ring(fb, cx, cy, r, (size as i32 / 10).max(2), fg);
        let bar = (size / 10).max(2);
        fb.fill_rect(
            x + size / 2 - bar / 2,
            y + inset + 2,
            bar,
            inner.saturating_sub(4),
            fg,
        );
        fb.fill_rect(x + size / 2 - bar, y + size / 2 - bar / 2, bar * 3, bar, fg);
    } else if hint == "usb" {
        // USB connector silhouette: plug body + 2 pin holes.
        let bw = (inner * 62 / 100).max(6);
        let bh = (inner * 58 / 100).max(5);
        let bx = x + (size - bw) / 2;
        let by = y + size - inset - bh;
        fb.fill_rect(bx, by, bw, bh, fg);
        let pw = (bw * 66 / 100).max(4);
        let ph = (inner - bh).max(3);
        let px = x + (size - pw) / 2;
        fb.fill_rect(px, y + inset, pw, ph, fg);
        if pw >= 6 && ph >= 3 {
            fb.fill_rect(
                px + 1,
                y + inset + 1,
                2,
                ph.saturating_sub(2).max(1),
                badge_bg,
            );
            fb.fill_rect(
                px + pw - 3,
                y + inset + 1,
                2,
                ph.saturating_sub(2).max(1),
                badge_bg,
            );
        }
    } else {
        // Linux / systemd / GRUB terminal prompt mark `>_`.
        let step = (inner / 5).max(2);
        let mid = y + size / 2;
        for i in 0..step {
            fb.fill_rect(x + inset + i, mid.saturating_sub(step) + i, 2, 2, fg);
            fb.fill_rect(x + inset + i, mid + step - i - 2, 2, 2, fg);
        }
        fb.fill_rect(
            x + inset + step + 2,
            y + size - inset - 3,
            inner.saturating_sub(step + 2).max(3),
            2,
            fg,
        );
    }
}

/// Draw a tiny padlock icon (for BitLocker / LUKS encrypted partitions).
pub fn draw_lock_icon(fb: &mut Framebuffer, x: u32, y: u32, size: u32, color: Rgba) {
    let size = size.max(10);
    let body_h = size * 55 / 100;
    let body_w = size * 76 / 100;
    let bx = x + (size - body_w) / 2;
    let by = y + size - body_h;
    fb.round_rect(bx, by, body_w, body_h, 2, color);
    // Shackle arch above lock body.
    let sh_w = (body_w * 64 / 100).max(4);
    let sh_x = x + (size - sh_w) / 2;
    let sh_h = size - body_h;
    fb.fill_rect(sh_x, y + 1, sh_w, 2, color);
    fb.fill_rect(sh_x, y + 1, 2, sh_h, color);
    fb.fill_rect(sh_x + sh_w.saturating_sub(2), y + 1, 2, sh_h, color);
}

/// Draw a crisp anti-aliased close/abort `×` glyph centered in `(x, y, size, size)`.
pub fn draw_close_x(fb: &mut Framebuffer, x: u32, y: u32, size: u32, color: Rgba) {
    if size < 5 {
        return;
    }
    let inset = (size / 4).max(2) as i32;
    let span = (size as i32 - inset * 2).max(3);
    // Half-stroke thickness in quarter-pixels (gives ~1.5..2.0 px bold strokes).
    let half_t = if size >= 18 { 5i32 } else { 4i32 };
    for py in 0..size as i32 {
        for px in 0..size as i32 {
            let lx = px - inset;
            let ly = py - inset;
            if lx < -1 || ly < -1 || lx > span + 1 || ly > span + 1 {
                continue;
            }
            // Perpendicular distance (scaled by 4*sqrt(2) ≈ 5.66 -> use * 3 approx) to both diagonals.
            let d1 = (lx - ly).abs() * 3;
            let d2 = (lx + ly - (span - 1)).abs() * 3;
            let d = d1.min(d2);
            let alpha = if d <= half_t {
                255u8
            } else if d < half_t + 4 {
                ((half_t + 4 - d) * 255 / 4) as u8
            } else {
                0
            };
            if alpha > 0 {
                fb.blend_pixel(x + px as u32, y + py as u32, color, alpha);
            }
        }
    }
}

/// Draw a small triangle chevron (`▲` when `up == true`, `▼` otherwise) for scrollbar buttons.
pub fn draw_chevron(fb: &mut Framebuffer, x: u32, y: u32, size: u32, up: bool, color: Rgba) {
    if size < 5 {
        return;
    }
    let h = (size / 2).max(3);
    let top = y + (size.saturating_sub(h)) / 2;
    let cx = x + size / 2;
    for row in 0..h {
        let half = if up { row } else { h - 1 - row };
        let rx = cx.saturating_sub(half);
        let rw = half * 2 + 1;
        fb.fill_rect(rx, top + row, rw, 1, color);
    }
}

/// Integer square root of `16 * v` (gives `4 * sqrt(v)` for quarter-pixel AA).
fn isqrt_4x(v: u32) -> u32 {
    let target = (v as u64) * 16;
    if target == 0 {
        return 0;
    }
    let mut x = target;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + target / x) / 2;
    }
    x as u32
}
