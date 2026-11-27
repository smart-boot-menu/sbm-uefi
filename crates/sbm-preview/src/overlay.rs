//! Screens and overlays that exist only in the simulator. They share the
//! unified `TextRenderer`, `Theme`, and vector panel backend with the firmware
//! renderer and always say SIMULATED so they cannot be mistaken for production UI.

use sbm_gfx::{Framebuffer, TextRenderer, TextRole, Theme, fill_rounded_panel};

use crate::session::SimScreen;

pub fn paint_sim_screen(
    fb: &mut Framebuffer,
    dark: bool,
    high_contrast: bool,
    screen: &SimScreen,
    renderer: &mut TextRenderer<'_>,
) {
    renderer.set_display_height(fb.height);
    let theme = Theme::resolve(dark, high_contrast);
    fb.fill(theme.bg);
    let pad = (fb.width / 24).clamp(18, 56);
    let head_h = renderer.line_metrics(TextRole::Heading).line_height;
    let body_h = renderer.line_metrics(TextRole::Body).line_height;
    let cap_h = renderer.line_metrics(TextRole::Caption).line_height;

    let banner_h = cap_h.saturating_add(16).max(32);
    fb.fill_rect(0, 0, fb.width, banner_h, theme.header);
    fb.fill_rect(0, banner_h.saturating_sub(2), fb.width, 2, theme.danger);
    renderer.draw_truncated(
        fb,
        "SIMULATED  native preview, not firmware",
        pad,
        banner_h.saturating_sub(cap_h) / 2,
        fb.width.saturating_sub(pad * 2),
        TextRole::Caption,
        theme.danger,
    );

    let max_w = fb.width.saturating_sub(pad * 2);
    let mut y = banner_h.saturating_add(pad);
    match screen {
        SimScreen::Booted {
            name,
            path,
            family,
            notes,
        } => {
            renderer.draw_truncated(
                fb,
                &format!("{name} would start now"),
                pad,
                y,
                max_w,
                TextRole::Heading,
                theme.ok,
            );
            y = y.saturating_add(head_h).saturating_add(14);
            let h1 = renderer.draw_wrapped(
                fb,
                &format!(
                    "The menu handed {path} to a simulated LoadImage/StartImage ({family:?})."
                ),
                pad,
                y,
                max_w,
                4,
                TextRole::Technical,
                theme.text,
            );
            y = y.saturating_add(h1).saturating_add(10);
            let h2 = renderer.draw_wrapped(
                fb,
                "No operating system runs in the native preview. The boot attempt was recorded on the simulated ESP.",
                pad,
                y,
                max_w,
                4,
                TextRole::Body,
                theme.muted,
            );
            y = y.saturating_add(h2).saturating_add(10);
            for note in notes {
                let nh =
                    renderer.draw_wrapped(fb, note, pad, y, max_w, 4, TextRole::Body, theme.text);
                y = y.saturating_add(nh).saturating_add(8);
            }
            y = y.saturating_add(body_h / 2);
            for line in [
                "Enter  restart after a healthy boot (the OS agent confirms)",
                "X      restart without confirmation (counts as a failed boot)",
                "P      power off",
            ] {
                let lh =
                    renderer.draw_wrapped(fb, line, pad, y, max_w, 2, TextRole::Body, theme.accent);
                y = y.saturating_add(lh).saturating_add(6);
            }
        }
        SimScreen::FirmwareSetup => {
            renderer.draw_truncated(
                fb,
                "Firmware setup",
                pad,
                y,
                max_w,
                TextRole::Heading,
                theme.ok,
            );
            y = y.saturating_add(head_h).saturating_add(14);
            let h = renderer.draw_wrapped(
                fb,
                "Real firmware would set OsIndications and reset into its setup UI. Nothing was written to host NVRAM.",
                pad,
                y,
                max_w,
                4,
                TextRole::Body,
                theme.text,
            );
            y = y.saturating_add(h).saturating_add(body_h);
            renderer.draw_truncated(
                fb,
                "Enter  power on again",
                pad,
                y,
                max_w,
                TextRole::Body,
                theme.accent,
            );
        }
        SimScreen::PoweredOff => {
            renderer.draw_truncated(
                fb,
                "Powered off",
                pad,
                y,
                max_w,
                TextRole::Heading,
                theme.muted,
            );
            y = y.saturating_add(head_h).saturating_add(14);
            renderer.draw_truncated(
                fb,
                "The simulated computer is off.",
                pad,
                y,
                max_w,
                TextRole::Body,
                theme.text,
            );
            y = y.saturating_add(body_h).saturating_add(body_h);
            renderer.draw_truncated(
                fb,
                "Enter  power on",
                pad,
                y,
                max_w,
                TextRole::Body,
                theme.accent,
            );
        }
    }
}

pub const HELP: &[&str] = &[
    "Production keys",
    "  Arrows  move     Enter/Space  boot selected     Tab  actions",
    "  T dark/light     C SBM Core (Manage Computer)   D diagnostics",
    "  F firmware       R restart   S shut down        Esc cancel / back",
    "  Mouse (dashboards): 1st click selects & cancels autoboot, 2nd click boots",
    "  Esc or [X] buttons: abort policy autoboot countdown",
    "  Scroll wheel / trackpad / PageUp / PageDown: scroll drives",
    "",
    "Simulator, layout & accessibility keys",
    "  F1 help          F2 next scenario      Shift+F2 previous",
    "  F3 display size (window / fixed presets)",
    "  F4 next layout   Shift+F4 previous     1-4 jump (Classic/A/B/C)",
    "  V / F9 or header pill: toggle Compact (OS cards) / Detailed (LBA map)",
    "  F5 reset computer                      F6 discovery mode",
    "  F7 font mode     Shift+F7 previous     F8 high-contrast theme",
    "  + / - adjust UI scale (75%-200%)       0 reset scale to 100%",
    "  F12 screenshot (PNG)                   Cmd+Q / close  quit",
];

pub fn paint_help(
    fb: &mut Framebuffer,
    dark: bool,
    high_contrast: bool,
    renderer: &mut TextRenderer<'_>,
) {
    renderer.set_display_height(fb.height);
    let theme = Theme::resolve(dark, high_contrast);
    let line_h = renderer.line_metrics(TextRole::Technical).line_height;
    let step = line_h.saturating_add(4);
    let pad = (line_h).clamp(14, 28);
    let max_line_w = HELP
        .iter()
        .map(|line| renderer.measure(line, TextRole::Technical).width)
        .max()
        .unwrap_or(0);
    let width = max_line_w
        .saturating_add(pad * 2)
        .min(fb.width.saturating_sub(24).max(120));
    let height = (HELP.len() as u32)
        .saturating_mul(step)
        .saturating_add(pad * 2)
        .min(fb.height.saturating_sub(24).max(120));
    let x = fb.width.saturating_sub(width) / 2;
    let y = fb.height.saturating_sub(height) / 2;
    fill_rounded_panel(fb, x, y, width, height, 10, 2, theme.header, theme.accent);
    let mut row = y.saturating_add(pad);
    let max_y = y.saturating_add(height).saturating_sub(pad / 2);
    let inner_w = width.saturating_sub(pad * 2);
    for line in HELP {
        if row.saturating_add(line_h) > max_y {
            break;
        }
        let color = if line.starts_with(' ') {
            theme.text
        } else {
            theme.accent
        };
        let role = if line.starts_with(' ') {
            TextRole::Technical
        } else {
            TextRole::Caption
        };
        renderer.draw_truncated(fb, line, x.saturating_add(pad), row, inner_w, role, color);
        row = row.saturating_add(step);
    }
}
