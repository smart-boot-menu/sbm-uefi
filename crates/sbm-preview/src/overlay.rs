//! Screens and overlays that exist only in the simulator. They use the same
//! theme tokens and bitmap font as the firmware renderer and always say
//! SIMULATED so they cannot be mistaken for production UI.

use sbm_gfx::{Framebuffer, Rgba, Theme, font_px};

use crate::session::SimScreen;

pub fn paint_sim_screen(fb: &mut Framebuffer, dark: bool, screen: &SimScreen) {
    let theme = Theme::from_dark(dark);
    fb.fill(theme.bg);
    let size = font_px(fb.height);
    banner(fb, &theme, size);
    let mut out = Writer { y: size * 4, size };
    match screen {
        SimScreen::Booted {
            name,
            path,
            family,
            notes,
        } => {
            out.line(fb, &format!("{name} would start now"), theme.ok, 3);
            out.line(
                fb,
                &format!(
                    "The menu handed {path} to a simulated LoadImage/StartImage ({family:?})."
                ),
                theme.text,
                2,
            );
            out.line(
                fb,
                "No operating system runs in the native preview. The boot attempt was recorded on the simulated ESP.",
                theme.muted,
                2,
            );
            for note in notes {
                out.line(fb, note, theme.text, 2);
            }
            out.gap();
            out.line(
                fb,
                "Enter  restart after a healthy boot (the OS agent confirms)",
                theme.accent,
                2,
            );
            out.line(
                fb,
                "X      restart without confirmation (counts as a failed boot)",
                theme.accent,
                2,
            );
            out.line(fb, "P      power off", theme.accent, 2);
        }
        SimScreen::FirmwareSetup => {
            out.line(fb, "Firmware setup", theme.ok, 3);
            out.line(
                fb,
                "Real firmware would set OsIndications and reset into its setup UI. Nothing was written to host NVRAM.",
                theme.text,
                2,
            );
            out.gap();
            out.line(fb, "Enter  power on again", theme.accent, 2);
        }
        SimScreen::PoweredOff => {
            out.line(fb, "Powered off", theme.muted, 3);
            out.line(fb, "The simulated computer is off.", theme.text, 2);
            out.gap();
            out.line(fb, "Enter  power on", theme.accent, 2);
        }
    }
}

struct Writer {
    y: u32,
    size: u32,
}

impl Writer {
    fn line(&mut self, fb: &mut Framebuffer, text: &str, color: Rgba, scale: u32) {
        let size = self.size;
        let glyph = size * scale / 2;
        for chunk in wrap(text, glyph, fb.width.saturating_sub(size * 2)) {
            fb.text(&chunk, size, self.y, glyph, color);
            self.y = self.y.saturating_add(glyph + size / 3);
        }
    }

    fn gap(&mut self) {
        self.y = self.y.saturating_add(self.size);
    }
}

fn banner(fb: &mut Framebuffer, theme: &Theme, size: u32) {
    let h = size * 2;
    fb.fill_rect(0, 0, fb.width, h, theme.header);
    fb.fill_rect(0, h.saturating_sub(2), fb.width, 2, theme.danger);
    fb.text(
        "SIMULATED  native preview, not firmware",
        size,
        size / 2,
        size,
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
    "Simulator keys",
    "  F1 help      F2 next scenario    Shift+F2 previous",
    "  F3 display size (window / fixed presets)",
    "  F5 reset the computer     F12 screenshot (PNG)",
    "  Cmd+Q / close window  quit",
];

pub fn paint_help(fb: &mut Framebuffer, dark: bool) {
    let theme = Theme::from_dark(dark);
    let size = (font_px(fb.height) * 3 / 4).max(8);
    let width = HELP
        .iter()
        .map(|line| line.len() as u32 * size)
        .max()
        .unwrap_or(0)
        .saturating_add(size * 2)
        .min(fb.width);
    let height = (HELP.len() as u32 + 2) * (size + size / 3);
    let x = fb.width.saturating_sub(width) / 2;
    let y = fb.height.saturating_sub(height) / 2;
    fb.round_panel(x, y, width, height, size / 2, 2, theme.header, theme.accent);
    let mut row = y + size;
    for line in HELP {
        let color = if line.starts_with(' ') {
            theme.text
        } else {
            theme.accent
        };
        fb.text(line, x + size, row, size, color);
        row += size + size / 3;
    }
}

fn wrap(text: &str, size: u32, max: u32) -> Vec<String> {
    let per_line = (max / size.max(1)).max(8) as usize;
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.len() + 1 + word.len() > per_line {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}
