use alloc::string::{String, ToString};

use crate::draw::{Framebuffer, Rgba, fit_text, text_width};
use crate::layout::{Rect, font_px, layout_menu};
use crate::state::{Focus, MenuState, Screen};
use crate::theme::Theme;

#[derive(Clone, Debug)]
pub struct CardView {
    pub title: String,
    pub subtitle: String,
    pub hint: String,
}

const ACTIONS: [&str; 3] = ["Firmware", "Restart", "Shut down"];

pub fn paint_splash(fb: &mut Framebuffer, dark: bool) {
    let theme = Theme::from_dark(dark);
    fb.fill(theme.bg);
    let size = font_px(fb.height).saturating_mul(2).max(16);
    let title = "Smart Boot Menu";
    let width = text_width(title, size);
    let x = fb.width.saturating_sub(width) / 2;
    let y = fb.height.saturating_sub(size) / 2;
    fb.text(title, x, y, size, theme.text);
}

pub fn paint_menu(fb: &mut Framebuffer, cards: &[CardView], state: &MenuState) {
    let theme = Theme::from_dark(state.dark);
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
    let title_size = layout.font.saturating_add(layout.font / 2).max(layout.font);
    fb.text(
        "SBM",
        layout.font,
        layout.font / 2,
        title_size,
        theme.accent,
    );
    let brand_w = text_width("SBM", title_size);
    fb.text(
        "Smart Boot Menu",
        layout
            .font
            .saturating_add(brand_w)
            .saturating_add(layout.font / 2),
        layout.font / 2 + title_size.saturating_sub(layout.font) / 2,
        layout.font,
        theme.text,
    );
    let status = status_line(state, cards.len());
    let status_w = text_width(&status, layout.font);
    fb.text(
        &status,
        fb.width
            .saturating_sub(status_w)
            .saturating_sub(layout.font),
        layout.font,
        layout.font,
        theme.muted,
    );

    if cards.is_empty() {
        let message = "No operating systems found";
        let size = layout.font;
        let width = text_width(message, size);
        fb.text(
            message,
            fb.width.saturating_sub(width) / 2,
            layout.header.h.saturating_add(layout.font * 3),
            size,
            theme.muted,
        );
    }

    for (index, rect) in &layout.cards {
        let Some(card) = cards.get(*index) else {
            continue;
        };
        draw_card(
            fb,
            &theme,
            *rect,
            card,
            *index == state.selected && state.focus == Focus::Cards,
            state.anim_ms,
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
        fb.round_panel(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            layout.font / 2,
            if selected { 2 } else { 1 },
            fill,
            border,
        );
        let label = ACTIONS[index];
        let label = fit_text(label, layout.font, rect.w.saturating_sub(layout.font));
        let width = text_width(&label, layout.font);
        fb.text(
            &label,
            rect.x.saturating_add(rect.w.saturating_sub(width) / 2),
            rect.y
                .saturating_add(rect.h.saturating_sub(layout.font) / 2),
            layout.font,
            if index == 2 { theme.danger } else { theme.text },
        );
    }
    let hint = fit_text(
        "Arrows move    Enter boot    Tab actions    T theme    C recovery    D details",
        layout.font,
        fb.width.saturating_sub(layout.font * 2),
    );
    fb.text(
        &hint,
        layout.font,
        layout.footer.y.saturating_add(layout.font / 3),
        layout.font,
        theme.muted,
    );
    if let Some((x, y)) = state.cursor {
        draw_cursor(fb, x, y, theme.accent);
    }
}

pub fn paint_error(fb: &mut Framebuffer, dark: bool, message: &str) {
    let theme = Theme::from_dark(dark);
    fb.fill(theme.bg);
    let size = font_px(fb.height);
    fb.text(
        "Cannot start this operating system",
        size,
        size * 2,
        size + size / 2,
        theme.danger,
    );
    let mut y = size * 5;
    for line in wrap_lines(message, size, fb.width.saturating_sub(size * 2)) {
        fb.text(&line, size, y, size, theme.text);
        y = y.saturating_add(size + size / 4);
    }
    fb.text(
        "Enter returns to the menu",
        size,
        y.saturating_add(size),
        size,
        theme.muted,
    );
}

pub fn paint_diagnostics(fb: &mut Framebuffer, dark: bool, lines: &[&str]) {
    let theme = Theme::from_dark(dark);
    fb.fill(theme.bg);
    let size = font_px(fb.height).clamp(12, 18);
    fb.text("Diagnostics", size, size, size + 4, theme.accent);
    let mut y = size * 3;
    for line in lines {
        let line = fit_text(line, size, fb.width.saturating_sub(size * 2));
        fb.text(&line, size, y, size, theme.text);
        y = y.saturating_add(size + 4);
        if y >= fb.height.saturating_sub(size) {
            break;
        }
    }
    fb.text(
        "Esc closes",
        size,
        fb.height.saturating_sub(size * 2),
        size,
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

fn draw_card(
    fb: &mut Framebuffer,
    theme: &Theme,
    rect: Rect,
    card: &CardView,
    selected: bool,
    anim_ms: u32,
) {
    let font = (rect.h / 5).max(8);
    let radius = font.min(16);
    fb.round_rect(
        rect.x.saturating_add(2),
        rect.y.saturating_add(3),
        rect.w,
        rect.h,
        radius,
        theme.shadow,
    );
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
    fb.round_panel(
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        radius,
        if selected { 3 } else { 1 },
        fill,
        border,
    );
    let icon = font.saturating_mul(2).min(rect.h.saturating_sub(font));
    let icon_x = rect.x.saturating_add(font / 2);
    let icon_y = rect.y.saturating_add(rect.h.saturating_sub(icon) / 2);
    draw_icon(fb, &card.hint, icon_x, icon_y, icon, theme);
    let text_x = icon_x.saturating_add(icon).saturating_add(font / 2);
    let text_w = rect
        .x
        .saturating_add(rect.w)
        .saturating_sub(text_x)
        .saturating_sub(font / 2);
    let title = fit_text(&card.title, font, text_w);
    let subtitle = fit_text(&card.subtitle, font.min(font), text_w);
    let sub_size = (font * 3 / 4).max(8);
    fb.text(
        &title,
        text_x,
        rect.y.saturating_add(rect.h / 2).saturating_sub(font),
        font,
        theme.text,
    );
    fb.text(
        &subtitle,
        text_x,
        rect.y.saturating_add(rect.h / 2).saturating_add(2),
        sub_size,
        theme.muted,
    );
}

fn draw_icon(fb: &mut Framebuffer, hint: &str, x: u32, y: u32, size: u32, theme: &Theme) {
    fb.round_rect(x, y, size, size, size / 4, theme.accent);
    let inset = size / 5;
    let inner = size.saturating_sub(inset * 2);
    match hint {
        "windows" => {
            let gap = (inner / 8).max(1);
            let box_w = inner.saturating_sub(gap) / 2;
            let color = Rgba::rgb(255, 255, 255);
            fb.fill_rect(x + inset, y + inset, box_w, box_w, color);
            fb.fill_rect(x + inset + box_w + gap, y + inset, box_w, box_w, color);
            fb.fill_rect(x + inset, y + inset + box_w + gap, box_w, box_w, color);
            fb.fill_rect(
                x + inset + box_w + gap,
                y + inset + box_w + gap,
                box_w,
                box_w,
                color,
            );
        }
        "recovery" => {
            fb.fill_rect(
                x + size / 2 - inset / 2,
                y + inset,
                inset.max(2),
                inner,
                Rgba::rgb(255, 255, 255),
            );
            fb.fill_rect(
                x + inset,
                y + size / 2 - inset / 2,
                inner,
                inset.max(2),
                Rgba::rgb(255, 255, 255),
            );
        }
        _ => {
            let letter = match hint {
                "grub" => "G",
                "systemd" => "s",
                "bsd" => "B",
                "usb" => "U",
                "linux" => "L",
                _ => "*",
            };
            let letter_size = inner.min(size * 2 / 3).max(8);
            fb.text(
                letter,
                x + (size.saturating_sub(letter_size)) / 2,
                y + (size.saturating_sub(letter_size)) / 2,
                letter_size,
                Rgba::rgb(255, 255, 255),
            );
        }
    }
}

fn draw_cursor(fb: &mut Framebuffer, x: u32, y: u32, color: Rgba) {
    fb.fill_rect(x.saturating_sub(1), y, 2, 14, color);
    fb.fill_rect(x, y.saturating_sub(1), 10, 2, color);
}

fn wrap_lines(text: &str, size: u32, max_width: u32) -> alloc::vec::Vec<String> {
    let mut lines = alloc::vec::Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            alloc::format!("{current} {word}")
        };
        if text_width(&candidate, size) > max_width && !current.is_empty() {
            lines.push(current);
            current = word.to_string();
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_pixels_follow_the_theme_and_selection() {
        let cards = alloc::vec![
            CardView {
                title: "Windows".into(),
                subtitle: "Windows Boot Manager".into(),
                hint: "windows".into()
            },
            CardView {
                title: "Fedora Linux".into(),
                subtitle: "Unified kernel".into(),
                hint: "linux".into()
            },
            CardView {
                title: "SBM Core Slot A".into(),
                subtitle: "Recovery".into(),
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
}
