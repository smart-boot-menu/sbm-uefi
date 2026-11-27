//! Native host preview for the `sbm-gfx` UEFI software renderer (Level 0).
//!
//! Renders deterministic synthetic boot cards directly on the development host
//! without launching QEMU or compiling a UEFI target. Clearly labelled as a
//! mock host preview and never counts as firmware qualification.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use sbm_gfx::{
    CardView, Framebuffer, MenuState, Screen, paint_diagnostics, paint_error, paint_menu,
    paint_splash,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewScreen {
    Menu,
    Splash,
    Diagnostics,
    Error,
    All,
}

#[derive(Debug, Clone)]
pub struct PreviewOptions {
    pub width: u32,
    pub height: u32,
    pub dark: bool,
    pub screen: PreviewScreen,
    pub selected: usize,
    pub out: Option<PathBuf>,
    pub open: bool,
}

impl Default for PreviewOptions {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
            dark: true,
            screen: PreviewScreen::Menu,
            selected: 0,
            out: None,
            open: false,
        }
    }
}

pub fn parse_preview_args<I: IntoIterator<Item = String>>(
    args: I,
) -> Result<PreviewOptions, String> {
    let mut opts = PreviewOptions::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--width" => {
                let v = iter.next().ok_or("--width requires a value")?;
                opts.width = v
                    .parse::<u32>()
                    .map_err(|_| format!("invalid --width: {v}"))?;
            }
            "--height" => {
                let v = iter.next().ok_or("--height requires a value")?;
                opts.height = v
                    .parse::<u32>()
                    .map_err(|_| format!("invalid --height: {v}"))?;
            }
            "--theme" => {
                let v = iter.next().ok_or("--theme requires dark or light")?;
                opts.dark = match v.as_str() {
                    "dark" => true,
                    "light" => false,
                    _ => return Err(format!("unknown --theme {v} (expected dark or light)")),
                };
            }
            "--screen" => {
                let v = iter
                    .next()
                    .ok_or("--screen requires menu|splash|diagnostics|error|all")?;
                opts.screen = match v.as_str() {
                    "menu" => PreviewScreen::Menu,
                    "splash" => PreviewScreen::Splash,
                    "diagnostics" => PreviewScreen::Diagnostics,
                    "error" => PreviewScreen::Error,
                    "all" => PreviewScreen::All,
                    _ => {
                        return Err(format!(
                            "unknown --screen {v} (expected menu|splash|diagnostics|error|all)"
                        ));
                    }
                };
            }
            "--selected" => {
                let v = iter.next().ok_or("--selected requires an index")?;
                opts.selected = v
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --selected: {v}"))?;
            }
            "--out" => {
                let v = iter.next().ok_or("--out requires a path")?;
                opts.out = Some(PathBuf::from(v));
            }
            "--open" => opts.open = true,
            "--help" | "-h" => {
                return Err(
                    "usage: xtask preview [--width 1280] [--height 800] [--theme dark|light] \
                     [--screen menu|splash|diagnostics|error|all] [--selected N] \
                     [--out PATH.{png,ppm}] [--open]"
                        .into(),
                );
            }
            other => return Err(format!("unknown preview option: {other}")),
        }
    }
    if opts.width < 320 || opts.height < 200 || opts.width > 4096 || opts.height > 2160 {
        return Err(format!(
            "preview dimensions {}x{} out of supported range (320x200..=4096x2160)",
            opts.width, opts.height
        ));
    }
    Ok(opts)
}

pub fn sample_cards() -> Vec<CardView> {
    vec![
        CardView {
            title: "Fedora Linux 45".into(),
            subtitle: "MOCK · \\EFI\\fedora\\grubaa64.efi".into(),
            hint: "Default".into(),
        },
        CardView {
            title: "Windows Boot Manager".into(),
            subtitle: "MOCK · \\EFI\\Microsoft\\Boot\\bootmgfw.efi".into(),
            hint: "BitLocker protected".into(),
        },
        CardView {
            title: "Ubuntu 26.04 LTS".into(),
            subtitle: "MOCK · \\EFI\\ubuntu\\shimaa64.efi".into(),
            hint: "Internal NVMe".into(),
        },
        CardView {
            title: "Manage Computer".into(),
            subtitle: "MOCK · SBM Core Slot A (Linux recovery)".into(),
            hint: "Press C".into(),
        },
        CardView {
            title: "Removable USB".into(),
            subtitle: "MOCK · Removable · \\EFI\\BOOT\\BOOTAA64.EFI".into(),
            hint: "External".into(),
        },
    ]
}

pub fn sample_diagnostics_lines() -> Vec<&'static str> {
    vec![
        "MODE: MOCK HOST PREVIEW (NOT UEFI FIRMWARE QUALIFICATION)",
        "Renderer: sbm-gfx pure-Rust software framebuffer (host native)",
        "Platform: simulated GOP 1280x800 BGRX (no physical hardware access)",
        "Secure Boot: simulated (enrolled-key validation requires sbm-lab QEMU)",
        "Policy: offline-first, remote disabled, local policy revision 0",
        "Recovery Slot A: mock-sha256 (verified only inside real UEFI boot)",
    ]
}

pub fn render_screen(
    width: u32,
    height: u32,
    dark: bool,
    screen: PreviewScreen,
    selected: usize,
) -> Framebuffer {
    let mut fb = Framebuffer::new(width, height);
    let cards = sample_cards();
    let mut state = MenuState::new(
        selected.min(cards.len().saturating_sub(1)),
        dark,
        Some(5000),
    );
    match screen {
        PreviewScreen::Menu | PreviewScreen::All => {
            state.screen = Screen::Menu;
            paint_menu(&mut fb, &cards, &state);
        }
        PreviewScreen::Splash => {
            paint_splash(&mut fb, dark);
        }
        PreviewScreen::Diagnostics => {
            let lines = sample_diagnostics_lines();
            paint_diagnostics(&mut fb, dark, &lines);
        }
        PreviewScreen::Error => {
            paint_error(
                &mut fb,
                dark,
                "MOCK PREVIEW: Secure Boot rejected untrusted payload (EFI_SECURITY_VIOLATION). \
                 No firmware or disk state was changed.",
            );
        }
    }
    fb
}

pub fn encode_ppm(fb: &Framebuffer) -> Vec<u8> {
    let header = format!("P6\n{} {}\n255\n", fb.width, fb.height);
    let mut out = Vec::with_capacity(header.len() + fb.px.len() * 3);
    out.extend_from_slice(header.as_bytes());
    for &px in &fb.px {
        out.push(((px >> 16) & 0xff) as u8);
        out.push(((px >> 8) & 0xff) as u8);
        out.push((px & 0xff) as u8);
    }
    out
}

/// Pure-Rust PNG encoder using uncompressed (stored) DEFLATE blocks.
/// Keeps `xtask` completely offline with zero additional external crates.
pub fn encode_png(fb: &Framebuffer) -> Vec<u8> {
    let w = fb.width as usize;
    let h = fb.height as usize;
    let row_len = 1 + w * 3;
    let mut raw = Vec::with_capacity(row_len * h);
    for y in 0..h {
        raw.push(0); // filter type 0 (None)
        let row_start = y * w;
        for x in 0..w {
            let px = fb.px[row_start + x];
            raw.push(((px >> 16) & 0xff) as u8);
            raw.push(((px >> 8) & 0xff) as u8);
            raw.push((px & 0xff) as u8);
        }
    }

    let zlib = zlib_stored(&raw);
    let mut png = Vec::with_capacity(64 + zlib.len());
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");

    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&fb.width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&fb.height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 2; // truecolor RGB
    ihdr[10] = 0;
    ihdr[11] = 0;
    ihdr[12] = 0;
    write_png_chunk(&mut png, *b"IHDR", &ihdr);
    write_png_chunk(&mut png, *b"IDAT", &zlib);
    write_png_chunk(&mut png, *b"IEND", &[]);
    png
}

fn zlib_stored(data: &[u8]) -> Vec<u8> {
    const MAX_BLOCK: usize = 65_535;
    let blocks = data.len().div_ceil(MAX_BLOCK).max(1);
    let mut out = Vec::with_capacity(2 + data.len() + blocks * 5 + 4);
    out.push(0x78); // CMF: deflate, 32K window
    out.push(0x01); // FLG: no dict, check bits valid (0x7801 % 31 == 0)

    if data.is_empty() {
        out.push(0x01);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0xffffu16.to_le_bytes());
    } else {
        let mut offset = 0;
        while offset < data.len() {
            let end = (offset + MAX_BLOCK).min(data.len());
            let chunk = &data[offset..end];
            let is_final = end == data.len();
            out.push(if is_final { 0x01 } else { 0x00 });
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
            offset = end;
        }
    }

    let adler = adler32(data);
    out.extend_from_slice(&adler.to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65_521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + u32::from(byte)) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
    }
    (b << 16) | a
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn write_png_chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn write_framebuffer(path: &Path, fb: &Framebuffer) -> Result<usize, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("mkdir {}: {err}", parent.display()))?;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let bytes = match ext.as_str() {
        "ppm" => encode_ppm(fb),
        _ => encode_png(fb),
    };
    fs::write(path, &bytes).map_err(|err| format!("write {}: {err}", path.display()))?;
    Ok(bytes.len())
}

pub fn run_preview(root: &Path, opts: &PreviewOptions) -> Result<Vec<PathBuf>, String> {
    let started = Instant::now();
    let theme_name = if opts.dark { "dark" } else { "light" };
    let default_dir = root.join("build/preview");
    let mut outputs = Vec::new();

    if opts.screen == PreviewScreen::All {
        let dir = opts.out.clone().unwrap_or(default_dir);
        for (name, screen) in [
            ("menu", PreviewScreen::Menu),
            ("splash", PreviewScreen::Splash),
            ("diagnostics", PreviewScreen::Diagnostics),
            ("error", PreviewScreen::Error),
        ] {
            for (tname, dark) in [("dark", true), ("light", false)] {
                let fb = render_screen(opts.width, opts.height, dark, screen, opts.selected);
                let out_path = dir.join(format!("uefi-{name}-{tname}.png"));
                let size = write_framebuffer(&out_path, &fb)?;
                println!(
                    "MOCK HOST PREVIEW (Level 0, not firmware qualification): {} ({}x{}, {} bytes)",
                    out_path.display(),
                    opts.width,
                    opts.height,
                    size
                );
                outputs.push(out_path);
            }
        }
    } else {
        let screen_name = match opts.screen {
            PreviewScreen::Menu => "menu",
            PreviewScreen::Splash => "splash",
            PreviewScreen::Diagnostics => "diagnostics",
            PreviewScreen::Error => "error",
            PreviewScreen::All => "all",
        };
        let out_path = opts
            .out
            .clone()
            .unwrap_or_else(|| default_dir.join(format!("uefi-{screen_name}-{theme_name}.png")));
        let fb = render_screen(
            opts.width,
            opts.height,
            opts.dark,
            opts.screen,
            opts.selected,
        );
        let size = write_framebuffer(&out_path, &fb)?;
        println!(
            "MOCK HOST PREVIEW (Level 0, not firmware qualification): {} ({}x{}, {} bytes, {:.1} ms)",
            out_path.display(),
            opts.width,
            opts.height,
            size,
            started.elapsed().as_secs_f64() * 1000.0
        );
        outputs.push(out_path);
    }

    if opts.open
        && let Some(first) = outputs.first()
    {
        #[cfg(target_os = "macos")]
        let _ = Command::new("open").arg(first).status();
        #[cfg(target_os = "linux")]
        let _ = Command::new("xdg-open").arg(first).status();
    }

    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_screens_with_valid_png_and_ppm() {
        for screen in [
            PreviewScreen::Menu,
            PreviewScreen::Splash,
            PreviewScreen::Diagnostics,
            PreviewScreen::Error,
        ] {
            let fb = render_screen(640, 480, true, screen, 0);
            assert_eq!(fb.px.len(), 640 * 480);
            let unique_first = fb.px[0];
            assert!(
                fb.px.iter().any(|&p| p != unique_first),
                "screen {screen:?} rendered a flat single-color buffer"
            );
            let png = encode_png(&fb);
            assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(&png[12..16], b"IHDR");
            assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");

            let ppm = encode_ppm(&fb);
            assert!(ppm.starts_with(b"P6\n640 480\n255\n"));
        }
    }

    #[test]
    fn dark_and_light_themes_produce_distinct_frames() {
        let dark = render_screen(640, 480, true, PreviewScreen::Menu, 0);
        let light = render_screen(640, 480, false, PreviewScreen::Menu, 0);
        assert_ne!(dark.px, light.px);
    }

    #[test]
    fn parses_preview_cli_options() {
        let opts = parse_preview_args(
            [
                "--width",
                "1024",
                "--height",
                "768",
                "--theme",
                "light",
                "--screen",
                "diagnostics",
                "--selected",
                "2",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse");
        assert_eq!(opts.width, 1024);
        assert_eq!(opts.height, 768);
        assert!(!opts.dark);
        assert_eq!(opts.screen, PreviewScreen::Diagnostics);
        assert_eq!(opts.selected, 2);
    }
}
