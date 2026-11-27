//! Screens and overlays that exist only in the simulator. They use the same
//! theme tokens and active typography renderer as the firmware UI and always
//! say SIMULATED so they cannot be mistaken for production UI.

use sbm_gfx::{
    FontMode, Framebuffer, Rgba, TextRenderer, TextRole, Theme, UiScale, fill_rounded_panel,
};

use crate::session::SimScreen;

pub fn paint_sim_screen(fb: &mut Framebuffer, dark: bool, screen: &SimScreen) {
    let mut renderer = TextRenderer::new(FontMode::AtkinsonNextMono, fb.height, UiScale::default());
    paint_sim_screen_with(fb, dark, false, screen, &mut renderer);
}

pub fn paint_sim_screen_with(
    fb: &mut Framebuffer,
    dark: bool,
    high_contrast: bool,
    screen: &SimScreen,
    renderer: &mut TextRenderer<'_>,
) {
    renderer.set_display_height(fb.height);
    let theme = Theme::resolve(dark, high_contrast);
    fb.fill(theme.bg);
    let pad = (fb.width / 24).max(20);
    banner(fb, &theme, pad, renderer);
    let banner_h = renderer.line_metrics(TextRole::Body).line_height * 2;
    let mut out = Writer {
        y: banner_h.saturating_add(pad),
        pad,
        max_w: fb.width.saturating_sub(pad * 2),
    };

    match screen {
        SimScreen::Booted {
            name,
            path,
            family,
            notes,
        } => {
            out.line(
                fb,
                renderer,
                &format!("{name} would start now"),
                TextRole::Display,
                theme.ok,
            );
            out.line(
                fb,
                renderer,
                &format!(
                    "The menu handed {path} to a simulated LoadImage/StartImage ({family:?})."
                ),
                TextRole::Technical,
                theme.text,
            );
            out.line(
                fb,
                renderer,
                "No operating system runs in the native preview. The boot attempt was recorded on the simulated ESP.",
                TextRole::Body,
                theme.muted,
            );
            for note in notes {
                let role = if note.starts_with("Kernel command line:") {
                    TextRole::Technical
                } else {
                    TextRole::Body
                };
                out.line(fb, renderer, note, role, theme.text);
            }
            out.gap();
            out.line(
                fb,
                renderer,
                "Enter  restart after a healthy boot (the OS agent confirms)",
                TextRole::Body,
                theme.accent,
            );
            out.line(
                fb,
                renderer,
                "X      restart without confirmation (counts as a failed boot)",
                TextRole::Body,
                theme.accent,
            );
            out.line(
                fb,
                renderer,
                "P      power off",
                TextRole::Body,
                theme.accent,
            );
        }
        SimScreen::FirmwareSetup => {
            out.line(fb, renderer, "Firmware setup", TextRole::Display, theme.ok);
            out.line(
                fb,
                renderer,
                "Real firmware would set OsIndications and reset into its setup UI. Nothing was written to host NVRAM.",
                TextRole::Body,
                theme.text,
            );
            out.gap();
            out.line(
                fb,
                renderer,
                "Enter  power on again",
                TextRole::Body,
                theme.accent,
            );
        }
        SimScreen::PoweredOff => {
            out.line(fb, renderer, "Powered off", TextRole::Display, theme.muted);
            out.line(
                fb,
                renderer,
                "The simulated computer is off.",
                TextRole::Body,
                theme.text,
            );
            out.gap();
            out.line(
                fb,
                renderer,
                "Enter  power on",
                TextRole::Body,
                theme.accent,
            );
        }
    }
}

struct Writer {
    y: u32,
    pad: u32,
    max_w: u32,
}

impl Writer {
    fn line(
        &mut self,
        fb: &mut Framebuffer,
        renderer: &mut TextRenderer<'_>,
        text: &str,
        role: TextRole,
        color: Rgba,
    ) {
        let used = renderer.draw_wrapped(fb, text, self.pad, self.y, self.max_w, 6, role, color);
        self.y = self.y.saturating_add(used).saturating_add(self.pad / 4);
    }

    fn gap(&mut self) {
        self.y = self.y.saturating_add(self.pad / 2);
    }
}

fn banner(fb: &mut Framebuffer, theme: &Theme, pad: u32, renderer: &mut TextRenderer<'_>) {
    let lm = renderer.line_metrics(TextRole::Body);
    let h = lm.line_height.saturating_mul(2);
    fb.fill_rect(0, 0, fb.width, h, theme.header);
    fb.fill_rect(0, h.saturating_sub(2), fb.width, 2, theme.danger);
    let top_y = h.saturating_sub(lm.line_height) / 2;
    renderer.draw_truncated(
        fb,
        "SIMULATED  native preview, not firmware",
        pad,
        top_y,
        fb.width.saturating_sub(pad * 2),
        TextRole::Body,
        theme.danger,
    );
}

pub const HELP: &[&str] = &[
    "Production keys",
    "  Arrows  move     Enter/Space  boot     Tab  actions",
    "  T theme   C SBM Core (Manage Computer)   D diagnostics",
    "  F firmware   R restart   S shut down   Esc back",
    "  Mouse: hover selects, click boots",
    "",
    "Simulator & typography keys",
    "  F1 help      F2 next scenario    Shift+F2 previous",
    "  F3 display size (window / fixed presets including 4K)",
    "  F5 reset computer     F12 screenshot (PNG)",
    "  F7 font mode (Bitmap / Atkinson Next / Next + Mono)  Shift+F7 prev",
    "  F8 high contrast      + / - UI scale      0 reset scale",
    "  Cmd+Q / close window  quit",
];

pub fn paint_help(fb: &mut Framebuffer, dark: bool) {
    let mut renderer = TextRenderer::new(FontMode::AtkinsonNextMono, fb.height, UiScale::default());
    paint_help_with(fb, dark, false, &mut renderer);
}

pub fn paint_help_with(
    fb: &mut Framebuffer,
    dark: bool,
    high_contrast: bool,
    renderer: &mut TextRenderer<'_>,
) {
    renderer.set_display_height(fb.height);
    let theme = Theme::resolve(dark, high_contrast);
    let lm = renderer.line_metrics(TextRole::Caption);
    let pad = (lm.line_height * 5 / 4).max(16);
    let row_step = lm.line_height.saturating_add(4);
    let max_line_w = HELP
        .iter()
        .map(|line| {
            let role = if line.starts_with(' ') {
                TextRole::Technical
            } else {
                TextRole::Heading
            };
            renderer.measure(line, role).width
        })
        .max()
        .unwrap_or(0);
    let width = max_line_w
        .saturating_add(pad * 2)
        .min(fb.width.saturating_sub(16));
    let height = (HELP.len() as u32)
        .saturating_mul(row_step)
        .saturating_add(pad * 2)
        .min(fb.height.saturating_sub(16));
    let x = fb.width.saturating_sub(width) / 2;
    let y = fb.height.saturating_sub(height) / 2;
    let radius = (pad / 2).clamp(6, 14);
    if renderer.mode() == FontMode::Bitmap8x8 {
        fb.round_panel(x, y, width, height, radius, 2, theme.header, theme.accent);
    } else {
        fill_rounded_panel(
            fb,
            x,
            y,
            width,
            height,
            radius,
            2,
            theme.header,
            theme.accent,
        );
    }
    let mut row = y.saturating_add(pad);
    let inner_w = width.saturating_sub(pad * 2);
    for line in HELP {
        if !line.is_empty() {
            let (role, color) = if line.starts_with(' ') {
                (TextRole::Technical, theme.text)
            } else {
                (TextRole::Heading, theme.accent)
            };
            renderer.draw_truncated(fb, line, x + pad, row, inner_w, role, color);
        }
        row = row.saturating_add(row_step);
    }
}
