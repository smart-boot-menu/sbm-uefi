use alloc::string::String;

use crate::draw::{Framebuffer, Rgba};
use crate::layout::{Rect, layout_menu};
use crate::state::{Focus, MenuState, Screen};
use crate::theme::Theme;
use crate::typography::{FontMode, TextRenderer, TextRole, UiScale};
use crate::vector::{draw_vector_emblem, fill_rounded_panel, fill_rounded_rect};

#[derive(Clone, Debug)]
pub struct CardView {
    pub title: String,
    pub subtitle: String,
    pub hint: String,
}

const ACTIONS: [&str; 3] = ["Firmware", "Restart", "Shut down"];

#[inline]
fn on_accent_fg(accent: Rgba) -> Rgba {
    if accent.luma() > 140 {
        Rgba::rgb(0, 0, 0)
    } else {
        Rgba::rgb(255, 255, 255)
    }
}

pub fn paint_splash(fb: &mut Framebuffer, dark: bool) {
    let mut renderer = TextRenderer::new(FontMode::AtkinsonNextMono, fb.height, UiScale::default());
    paint_splash_with(fb, dark, false, &mut renderer);
}

pub fn paint_splash_with(
    fb: &mut Framebuffer,
    dark: bool,
    high_contrast: bool,
    renderer: &mut TextRenderer<'_>,
) {
    renderer.set_display_height(fb.height);
    let theme = Theme::resolve(dark, high_contrast);
    fb.fill(theme.bg);
    let title = "Smart Boot Menu";
    let sub = "Pre-OS boot manager";
    let title_m = renderer.measure(title, TextRole::Display);
    let sub_m = renderer.measure(sub, TextRole::Caption);
    let gap = (renderer.line_metrics(TextRole::Caption).line_height / 2).max(8);
    let total_h = title_m
        .height
        .saturating_add(gap)
        .saturating_add(sub_m.height);
    let top_y = fb.height.saturating_sub(total_h) / 2;

    let title_x = fb.width.saturating_sub(title_m.width) / 2;
    renderer.draw_top(fb, title, title_x, top_y, TextRole::Display, theme.text);

    let sub_x = fb.width.saturating_sub(sub_m.width) / 2;
    let sub_y = top_y.saturating_add(title_m.height).saturating_add(gap);
    renderer.draw_top(fb, sub, sub_x, sub_y, TextRole::Caption, theme.muted);
}

pub fn paint_menu(fb: &mut Framebuffer, cards: &[CardView], state: &MenuState) {
    let mut renderer = TextRenderer::new(FontMode::AtkinsonNextMono, fb.height, UiScale::default());
    paint_menu_with(fb, cards, state, false, &mut renderer);
}

pub fn paint_menu_with(
    fb: &mut Framebuffer,
    cards: &[CardView],
    state: &MenuState,
    high_contrast: bool,
    renderer: &mut TextRenderer<'_>,
) {
    renderer.set_display_height(fb.height);
    let theme = Theme::resolve(state.dark, high_contrast);

    let layout = layout_menu(
        fb.width,
        fb.height,
        cards.len(),
        ACTIONS.len(),
        state.selected,
    );
    fb.fill(theme.bg);
    fb.fill_rect(0, 0, fb.width, layout.header.h, theme.header);
    fb.fill_rect(
        0,
        layout.header.h.saturating_sub(2),
        fb.width,
        2,
        theme.accent,
    );

    // Header: baseline-aligned brand pill, application title, and status line.
    let pad = layout.font.max(12);
    let head_lm = renderer.line_metrics(TextRole::Heading);
    let header_baseline = ((layout.header.h as f32 + head_lm.cap_height) * 0.5) as u32;

    let brand_w = renderer.measure("SBM", TextRole::Heading).width;
    let badge_pad_x = (layout.font / 2).max(6);
    let badge_w = brand_w.saturating_add(badge_pad_x * 2);
    let badge_h = (head_lm.line_height + 2).min(layout.header.h.saturating_sub(8));
    let badge_y = layout.header.h.saturating_sub(badge_h) / 2;
    fill_rounded_rect(
        fb,
        pad,
        badge_y,
        badge_w,
        badge_h,
        (badge_h / 4).max(4),
        theme.accent,
    );
    renderer.draw_centered(
        fb,
        "SBM",
        Rect {
            x: pad,
            y: badge_y,
            w: badge_w,
            h: badge_h,
        },
        TextRole::Heading,
        on_accent_fg(theme.accent),
    );

    let title_x = pad
        .saturating_add(badge_w)
        .saturating_add((layout.font * 3 / 4).max(10));
    let status = status_line(state, cards.len());
    let status_w = renderer.measure(&status, TextRole::Body).width;
    let status_x = fb
        .width
        .saturating_sub(status_w)
        .saturating_sub(pad)
        .max(title_x);
    let title_budget = status_x.saturating_sub(title_x).saturating_sub(pad);
    renderer.draw_truncated_baseline(
        fb,
        "Smart Boot Menu",
        title_x,
        header_baseline,
        title_budget,
        TextRole::Heading,
        theme.text,
    );
    renderer.draw_baseline(
        fb,
        &status,
        status_x,
        header_baseline,
        TextRole::Body,
        if state.countdown_ms.is_some() && !cards.is_empty() {
            theme.accent
        } else {
            theme.muted
        },
    );

    if cards.is_empty() {
        let message = "No operating systems found";
        let m = renderer.measure(message, TextRole::Body);
        renderer.draw_top(
            fb,
            message,
            fb.width.saturating_sub(m.width) / 2,
            layout.header.h.saturating_add(layout.font * 3),
            TextRole::Body,
            theme.muted,
        );
    }

    for (index, rect) in &layout.cards {
        let Some(card) = cards.get(*index) else {
            continue;
        };
        draw_card_vector(
            fb,
            &theme,
            *rect,
            card,
            *index == state.selected && state.focus == Focus::Cards,
            state.anim_ms,
            renderer,
        );
    }

    for (index, rect) in layout.actions.iter().enumerate() {
        let selected = state.focus == Focus::Actions && state.action == index;
        let fill = if selected {
            theme.card_selected
        } else {
            theme.card
        };
        let border = if selected { theme.accent } else { theme.border };
        let thickness = if selected || theme.high_contrast {
            2
        } else {
            1
        };
        fill_rounded_panel(
            fb,
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            (rect.h / 4).clamp(6, 16),
            thickness,
            fill,
            border,
        );
        let inset = (layout.font / 2).max(8);
        let inner = Rect {
            x: rect.x.saturating_add(inset),
            y: rect.y,
            w: rect.w.saturating_sub(inset * 2),
            h: rect.h,
        };
        renderer.draw_centered(
            fb,
            ACTIONS[index],
            inner,
            TextRole::Body,
            if index == 2 { theme.danger } else { theme.text },
        );
    }

    let hint = "Arrows move · Enter boot · Tab actions · T theme · C recovery · D diagnostics";
    let cap_lm = renderer.line_metrics(TextRole::Caption);
    let footer_baseline = layout
        .footer
        .y
        .saturating_add(((layout.footer.h as f32 + cap_lm.cap_height) * 0.5) as u32);
    renderer.draw_truncated_baseline(
        fb,
        hint,
        pad,
        footer_baseline,
        fb.width.saturating_sub(pad * 2),
        TextRole::Caption,
        theme.muted,
    );

    if let Some((x, y)) = state.cursor {
        draw_cursor(fb, x, y, theme.accent);
    }
}

fn draw_card_vector(
    fb: &mut Framebuffer,
    theme: &Theme,
    rect: Rect,
    card: &CardView,
    selected: bool,
    anim_ms: u32,
    renderer: &mut TextRenderer<'_>,
) {
    let radius = (rect.h / 7).clamp(8, 22);
    if !theme.high_contrast {
        fill_rounded_rect(
            fb,
            rect.x.saturating_add(2),
            rect.y.saturating_add(3),
            rect.w,
            rect.h,
            radius,
            theme.shadow,
        );
    }
    let border = if selected {
        theme
            .border
            .mix(theme.accent, 160u32.saturating_sub(anim_ms.min(160)), 160)
    } else {
        theme.border
    };
    let fill = if selected {
        theme.card_selected
    } else {
        theme.card
    };
    let thickness = if selected {
        3
    } else if theme.high_contrast {
        2
    } else {
        1
    };
    fill_rounded_panel(
        fb, rect.x, rect.y, rect.w, rect.h, radius, thickness, fill, border,
    );

    let pad = (rect.h / 6).clamp(10, 32);
    let icon = (rect.h.saturating_sub(pad * 2)).clamp(24, 96);
    let icon_x = rect.x.saturating_add(pad);
    let icon_y = rect.y.saturating_add(rect.h.saturating_sub(icon) / 2);
    draw_vector_emblem(
        fb,
        &card.hint,
        &card.title,
        icon_x,
        icon_y,
        icon,
        theme.accent,
        on_accent_fg(theme.accent),
    );

    let text_x = icon_x
        .saturating_add(icon)
        .saturating_add((pad * 3 / 4).max(10));
    let text_w = rect
        .x
        .saturating_add(rect.w)
        .saturating_sub(text_x)
        .saturating_sub(pad);

    let head_lm = renderer.line_metrics(TextRole::Heading);
    let cap_lm = renderer.line_metrics(TextRole::Caption);
    let tech_lm = renderer.line_metrics(TextRole::Technical);
    let sub_h = cap_lm.line_height.max(tech_lm.line_height);
    let row_gap = (rect.h / 16).clamp(3, 10);
    let block_h = head_lm
        .line_height
        .saturating_add(row_gap)
        .saturating_add(sub_h);
    let block_top = rect.y.saturating_add(rect.h.saturating_sub(block_h) / 2);

    let title_baseline = block_top.saturating_add(libm::ceilf(head_lm.ascent) as u32);
    renderer.draw_truncated_baseline(
        fb,
        &card.title,
        text_x,
        title_baseline,
        text_w,
        TextRole::Heading,
        theme.text,
    );

    let sub_baseline = block_top
        .saturating_add(head_lm.line_height)
        .saturating_add(row_gap)
        .saturating_add(libm::ceilf(cap_lm.ascent.max(tech_lm.ascent)) as u32);

    // Separate human family label (Caption) from technical EFI filename (Technical / Mono)
    // aligned on the exact same typographic baseline.
    let trimmed_sub = card.subtitle.trim();
    if let Some((family, file)) = split_card_subtitle(trimmed_sub) {
        let fam_w = renderer.draw_truncated_baseline(
            fb,
            family,
            text_x,
            sub_baseline,
            text_w,
            TextRole::Caption,
            theme.muted,
        );
        let sep = " · ";
        let sep_w = renderer.measure(sep, TextRole::Caption).width;
        let used = fam_w.saturating_add(sep_w);
        if text_w > used.saturating_add(24) {
            renderer.draw_baseline(
                fb,
                sep,
                text_x.saturating_add(fam_w),
                sub_baseline,
                TextRole::Caption,
                theme.muted,
            );
            renderer.draw_truncated_baseline(
                fb,
                file,
                text_x.saturating_add(used),
                sub_baseline,
                text_w.saturating_sub(used),
                TextRole::Technical,
                theme.muted,
            );
        }
    } else {
        renderer.draw_truncated_baseline(
            fb,
            trimmed_sub,
            text_x,
            sub_baseline,
            text_w,
            TextRole::Caption,
            theme.muted,
        );
    }
}

/// Split `"Linux  vmlinuz.efi"` into `("Linux", "vmlinuz.efi")` when a technical
/// filename/identifier suffix is present.
fn split_card_subtitle(subtitle: &str) -> Option<(&str, &str)> {
    let (left, right) = subtitle.split_once("  ")?;
    let left = left.trim();
    let right = right.trim();
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some((left, right))
}

pub fn paint_error(fb: &mut Framebuffer, dark: bool, message: &str) {
    let mut renderer = TextRenderer::new(FontMode::AtkinsonNextMono, fb.height, UiScale::default());
    paint_error_with(fb, dark, false, message, &mut renderer);
}

pub fn paint_error_with(
    fb: &mut Framebuffer,
    dark: bool,
    high_contrast: bool,
    message: &str,
    renderer: &mut TextRenderer<'_>,
) {
    renderer.set_display_height(fb.height);
    let theme = Theme::resolve(dark, high_contrast);
    fb.fill(theme.bg);
    let pad = (fb.width / 20).clamp(20, 72);
    let mut y = (fb.height / 10).clamp(28, 96);
    let head_lm = renderer.line_metrics(TextRole::Heading);
    renderer.draw_truncated(
        fb,
        "Cannot start this operating system",
        pad,
        y,
        fb.width.saturating_sub(pad * 2),
        TextRole::Heading,
        theme.danger,
    );
    y = y.saturating_add(head_lm.line_height).saturating_add(16);
    let body_h = renderer.draw_wrapped(
        fb,
        message,
        pad,
        y,
        fb.width.saturating_sub(pad * 2),
        12,
        TextRole::Body,
        theme.text,
    );
    y = y.saturating_add(body_h).saturating_add(24);
    renderer.draw_top(
        fb,
        "Enter or Esc returns to the menu",
        pad,
        y,
        TextRole::Caption,
        theme.muted,
    );
}

pub fn paint_diagnostics(fb: &mut Framebuffer, dark: bool, lines: &[&str]) {
    let mut renderer = TextRenderer::new(FontMode::AtkinsonNextMono, fb.height, UiScale::default());
    paint_diagnostics_with(fb, dark, false, lines, &mut renderer);
}

pub fn paint_diagnostics_with(
    fb: &mut Framebuffer,
    dark: bool,
    high_contrast: bool,
    lines: &[&str],
    renderer: &mut TextRenderer<'_>,
) {
    renderer.set_display_height(fb.height);
    let theme = Theme::resolve(dark, high_contrast);
    fb.fill(theme.bg);
    let pad = (fb.width / 24).clamp(18, 64);
    let head_lm = renderer.line_metrics(TextRole::Heading);
    let tech_lm = renderer.line_metrics(TextRole::Technical);
    let cap_lm = renderer.line_metrics(TextRole::Caption);

    let mut y = pad;
    renderer.draw_top(fb, "Diagnostics", pad, y, TextRole::Heading, theme.accent);
    y = y.saturating_add(head_lm.line_height).saturating_add(14);
    fb.fill_rect(
        pad,
        y.saturating_sub(6),
        fb.width.saturating_sub(pad * 2),
        1,
        theme.border,
    );

    let bottom_limit = fb
        .height
        .saturating_sub(cap_lm.line_height)
        .saturating_sub(pad)
        .saturating_sub(8);
    let max_w = fb.width.saturating_sub(pad * 2);
    for line in lines {
        if y.saturating_add(tech_lm.line_height) > bottom_limit {
            break;
        }
        renderer.draw_truncated(fb, line, pad, y, max_w, TextRole::Technical, theme.text);
        y = y.saturating_add(tech_lm.line_height).saturating_add(3);
    }

    renderer.draw_top(
        fb,
        "Esc closes · T theme",
        pad,
        fb.height
            .saturating_sub(cap_lm.line_height)
            .saturating_sub(pad / 2),
        TextRole::Caption,
        theme.muted,
    );
}

fn status_line(state: &MenuState, n_cards: usize) -> String {
    if state.screen == Screen::Error {
        return String::from("Error");
    }
    match state.countdown_ms {
        Some(ms) if n_cards > 0 => {
            let seconds = ms.div_ceil(1000);
            alloc::format!("Booting in {seconds}s")
        }
        _ => String::from("Choose an operating system"),
    }
}

fn draw_cursor(fb: &mut Framebuffer, x: u32, y: u32, color: Rgba) {
    fb.fill_rect(x.saturating_sub(1), y, 2, 14, color);
    fb.fill_rect(x, y.saturating_sub(1), 10, 2, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::text_width;

    #[test]
    fn menu_pixels_follow_the_theme_and_selection() {
        let cards = alloc::vec![
            CardView {
                title: "Windows".into(),
                subtitle: "Windows  bootmgfw.efi".into(),
                hint: "windows".into()
            },
            CardView {
                title: "Fedora Linux".into(),
                subtitle: "Linux  vmlinuz.efi".into(),
                hint: "linux".into()
            },
            CardView {
                title: "SBM Core Slot A".into(),
                subtitle: "Recovery  sbm-core.efi".into(),
                hint: "recovery".into()
            },
        ];
        let mut dark = Framebuffer::new(1024, 768);
        let state = MenuState::new(0, true, Some(8_000));
        paint_menu(&mut dark, &cards, &state);
        assert_eq!(dark.pixel(1, 1).unwrap(), Theme::dark().header);
        let layout = layout_menu(1024, 768, 3, 3, 0);
        let card = layout.cards[0].1;
        let sample = dark
            .pixel(card.x + card.w / 2, card.y + card.h / 2)
            .unwrap();
        assert_ne!(sample, Theme::dark().bg);
        let mut light = Framebuffer::new(1024, 768);
        let mut light_state = MenuState::new(1, false, None);
        paint_menu(&mut light, &cards, &light_state);
        assert_ne!(light.pixel(1, 1).unwrap(), dark.pixel(1, 1).unwrap());
        light_state.screen = Screen::Error;
        light_state.message = "load error".into();
        paint_error(&mut light, false, &light_state.message);
        assert_eq!(light.pixel(0, 0).unwrap(), Theme::light().bg);
    }

    #[test]
    fn six_titles_fit_at_1024x768() {
        let layout = layout_menu(1024, 768, 6, 3, 0);
        assert!(layout.cols <= 3, "cols {}", layout.cols);
        assert_eq!(layout.cards.len(), 6);
        let tr = TextRenderer::new(FontMode::AtkinsonNextMono, 768, UiScale::default());
        for title in ["Windows", "Fedora Linux", "systemd-boot", "SBM Core Slot A"] {
            for (_, rect) in &layout.cards {
                // Legacy bitmap constraint still holds
                let font = (rect.h / 5).max(8);
                let icon = font.saturating_mul(2).min(rect.h.saturating_sub(font));
                let text_w = rect
                    .w
                    .saturating_sub(font / 2)
                    .saturating_sub(icon)
                    .saturating_sub(font / 2)
                    .saturating_sub(font / 2);
                assert!(
                    text_width(title, font) <= text_w,
                    "{title} needs more than {text_w} at {font}px in {rect:?}"
                );
                // Vector Atkinson Heading also fits without truncation!
                let pad = (rect.h / 6).clamp(10, 32);
                let vicon = (rect.h.saturating_sub(pad * 2)).clamp(24, 96);
                let vtext_w = rect
                    .w
                    .saturating_sub(pad)
                    .saturating_sub(vicon)
                    .saturating_sub((pad * 3 / 4).max(10))
                    .saturating_sub(pad);
                assert!(
                    tr.measure(title, TextRole::Heading).width <= vtext_w,
                    "vector {title} width {} > {vtext_w} in {rect:?}",
                    tr.measure(title, TextRole::Heading).width
                );
            }
        }
    }

    #[test]
    fn all_three_font_modes_render_distinct_frames() {
        let cards = alloc::vec![
            CardView {
                title: "Fedora Linux".into(),
                subtitle: "Linux  vmlinuz-6.12.efi".into(),
                hint: "linux".into(),
            },
            CardView {
                title: "Windows".into(),
                subtitle: "Windows  bootmgfw.efi".into(),
                hint: "windows".into(),
            },
        ];
        let state = MenuState::new(0, true, Some(5_000));
        let mut frames = alloc::vec::Vec::new();
        for mode in FontMode::ALL {
            let mut fb = Framebuffer::new(1024, 768);
            let mut tr = TextRenderer::new(mode, 768, UiScale::default());
            paint_menu_with(&mut fb, &cards, &state, false, &mut tr);
            frames.push(fb.px);
        }
        assert_ne!(
            frames[0], frames[1],
            "Bitmap8x8 and AtkinsonNext must differ"
        );
        assert_ne!(
            frames[1], frames[2],
            "AtkinsonNext and AtkinsonNextMono must differ on Technical filenames"
        );
    }
}
