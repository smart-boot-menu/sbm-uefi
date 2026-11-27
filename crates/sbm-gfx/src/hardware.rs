//! Experimental hardware-bento, schematic, and storage-first layouts with
//! paragraph-wrapped 1D-LBA-to-2D orthogonal-polygon disk widgets.
//!
//! Built on the same `no_std` + `alloc` [`Framebuffer`] and [`MenuState`] as
//! the production menu, isolated behind the `experimental-layouts` feature so
//! `sbm-app` stays unchanged until a direction is approved.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::assets::{draw_chevron, draw_close_x, draw_lock_icon, draw_os_emblem};
use crate::disk_wrap::{
    WrapInput, WrappedRegion, fill_polygon, hatch_polygon, layout_disk_wrap, stroke_merged_polygon,
};
use crate::draw::{Framebuffer, Rgba};
use crate::layout::{Hit, Rect};
use crate::state::{Focus, MenuState};
use crate::text::{TextStyle, draw_text, draw_text_fit, fit_mono, measure_text};

/// Selectable menu layout for interactive design comparison.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayoutMode {
    #[default]
    Classic,
    /// Experiment A: Hardware bento dashboard with wrapped LBA polygon storage widgets.
    Bento,
    /// Experiment B: Hardware schematic / silkscreen with bus traces & LBA polygon blocks.
    Schematic,
    /// Experiment C: Storage-first full-width wrapped LBA disk canvases + compact hardware strip.
    Storage,
}

impl LayoutMode {
    pub const ALL: [Self; 4] = [Self::Classic, Self::Bento, Self::Schematic, Self::Storage];
    pub const EXPERIMENTS: [Self; 3] = [Self::Bento, Self::Schematic, Self::Storage];

    pub fn id(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Bento => "bento",
            Self::Schematic => "schematic",
            Self::Storage => "storage",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::Bento => "A: Bento",
            Self::Schematic => "B: Schematic",
            Self::Storage => "C: Storage",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Classic => Self::Bento,
            Self::Bento => Self::Schematic,
            Self::Schematic => Self::Storage,
            Self::Storage => Self::Classic,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Classic => Self::Storage,
            Self::Bento => Self::Classic,
            Self::Schematic => Self::Bento,
            Self::Storage => Self::Schematic,
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "classic" | "0" => Some(Self::Classic),
            "bento" | "a" | "1" => Some(Self::Bento),
            "schematic" | "b" | "2" => Some(Self::Schematic),
            "storage" | "c" | "3" => Some(Self::Storage),
            _ => None,
        }
    }
}

/// Progressive-discovery state for any hardware subsystem.
///
/// Never fabricate values: `Pending` means discovery has not reported yet;
/// `Unavailable` means firmware did not expose the information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeState<T> {
    Pending,
    Unavailable,
    Ready(T),
}

#[derive(Clone, Debug)]
pub struct CpuSpec {
    pub model: Option<String>,
    pub arch: String,
    pub cores: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct MemorySpec {
    pub total_mib: u64,
}

#[derive(Clone, Debug)]
pub struct BoardSpec {
    pub vendor: Option<String>,
    pub product: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FirmwareSpec {
    pub vendor: Option<String>,
    pub version: String,
    pub arch: String,
}

#[derive(Clone, Debug)]
pub struct DisplaySpec {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecureBootSummary {
    Enabled,
    Disabled,
    SetupMode,
    Unsupported,
}

impl SecureBootSummary {
    pub fn badge(self) -> &'static str {
        match self {
            Self::Enabled => "SB:ON",
            Self::Disabled => "SB:OFF",
            Self::SetupMode => "SB:SETUP",
            Self::Unsupported => "SB:N/A",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SecuritySpec {
    pub secure_boot: SecureBootSummary,
    pub tpm_present: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct NicSpec {
    pub name: String,
    pub wireless: bool,
    pub up: bool,
    pub address: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartitionKind {
    Esp,
    Windows,
    Linux,
    Luks,
    Recovery,
    Msr,
    Swap,
    Data,
    Apfs,
    Other,
}

/// A selectable boot target linked to its OS partition (or its ESP when standalone).
///
/// `esp_part` (where the EFI bootloader resides) and `os_part` (inferred OS
/// partition, if any) are tracked separately and never assumed identical.
#[derive(Clone, Debug)]
pub struct BootTargetView {
    pub card_index: usize,
    pub title: String,
    pub family: String,
    pub hint: String,
    pub loader_file: String,
    pub esp_part: Option<usize>,
    pub os_part: Option<usize>,
    pub healthy: bool,
    pub is_recovery: bool,
    pub note: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PartitionView {
    pub index: usize,
    pub label: String,
    pub kind: PartitionKind,
    pub fs: Option<String>,
    pub free_bytes: Option<u64>,
    pub encrypted: Option<String>,
    pub is_recovery: bool,
    pub detected_os: Option<String>,
    pub loader_count: usize,
    /// Bootable targets hosted by this partition (OS root targets when inferred,
    /// or standalone ESP targets such as SBM Core / removable installers).
    pub boot_targets: Vec<BootTargetView>,
}

/// One contiguous LBA span on a physical disk: either a real partition or an
/// unallocated gap. Always kept in strict ascending `start_lba` order.
#[derive(Clone, Debug)]
pub enum RegionKind {
    Partition(PartitionView),
    Unallocated,
}

#[derive(Clone, Debug)]
pub struct DiskRegion {
    /// Exact starting Logical Block Address (independent of visual geometry).
    pub start_lba: u64,
    /// Exclusive ending Logical Block Address.
    pub end_lba: u64,
    /// Exact byte capacity `(end_lba - start_lba) * sector_size`.
    pub size_bytes: u64,
    pub kind: RegionKind,
}

impl DiskRegion {
    pub fn boot_targets(&self) -> &[BootTargetView] {
        match &self.kind {
            RegionKind::Partition(p) => &p.boot_targets,
            RegionKind::Unallocated => &[],
        }
    }
}

#[derive(Clone, Debug)]
pub struct StorageDevice {
    pub designator: String,
    pub device_name: String,
    pub bus: String,
    pub removable: bool,
    pub model: Option<String>,
    pub size_bytes: u64,
    pub sector_size: u32,
    pub total_sectors: u64,
    /// All partitions and unallocated gaps sorted strictly by ascending `start_lba`.
    pub regions: Vec<DiskRegion>,
}

impl StorageDevice {
    pub fn boot_target_count(&self) -> usize {
        self.regions.iter().map(|r| r.boot_targets().len()).sum()
    }

    pub fn has_selected(&self, selected: usize) -> bool {
        self.regions
            .iter()
            .any(|r| r.boot_targets().iter().any(|t| t.card_index == selected))
    }

    pub fn find_selected(&self, selected: usize) -> Option<(&DiskRegion, &BootTargetView)> {
        for r in &self.regions {
            if let Some(t) = r.boot_targets().iter().find(|t| t.card_index == selected) {
                return Some((r, t));
            }
        }
        None
    }
}

/// Complete hardware + storage view model fed to experimental layouts.
#[derive(Clone, Debug)]
pub struct SystemSpec {
    pub host_label: String,
    pub cpu: ProbeState<CpuSpec>,
    pub memory: ProbeState<MemorySpec>,
    pub board: ProbeState<BoardSpec>,
    pub gpu: ProbeState<Vec<String>>,
    pub firmware: ProbeState<FirmwareSpec>,
    pub display: DisplaySpec,
    pub security: ProbeState<SecuritySpec>,
    pub network: ProbeState<Vec<NicSpec>>,
    pub storage: ProbeState<Vec<StorageDevice>>,
    pub policy_note: Option<String>,
    pub discovery_status: Option<String>,
}

/// A hit-testable interactive region (either a sub-target pill or a connected
/// partition polygon made of one or more row segments).
#[derive(Clone, Debug)]
pub struct CardHitZone {
    pub card_index: usize,
    pub segments: Vec<Rect>,
    /// True for explicit target pills (checked before enclosing polygon body).
    pub priority: bool,
}

/// Interactive hit result for experimental layouts (extends [`Hit`] with
/// autoboot abort `[X]` buttons and drive-list scrollbar controls).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExperimentalHit {
    None,
    Card(usize),
    Action(usize),
    /// Clicking either the top-right `[X]` or the flashing partition's `[X]`.
    AbortAutoboot,
    /// Clicking the scrollbar up/down arrow buttons (signed pixel delta).
    ScrollDelta(i32),
    /// Clicking on the scrollbar track to jump to a specific scroll offset.
    ScrollTo(u32),
}

/// Interactive hit regions produced by any layout so mouse + keyboard stay
/// wired to `MenuState` and `step()`.
#[derive(Clone, Debug, Default)]
pub struct LayoutHitMap {
    pub zones: Vec<CardHitZone>,
    pub actions: Vec<Rect>,
    /// Clickable `[X]` abort-autoboot buttons (top-right countdown pill + flashing partition).
    pub abort_buttons: Vec<Rect>,
    pub scroll_up: Option<Rect>,
    pub scroll_down: Option<Rect>,
    pub scroll_track: Option<Rect>,
    pub scroll_thumb: Option<Rect>,
    pub scroll_y: u32,
    pub max_scroll_y: u32,
    pub viewport_h: u32,
    /// Content-relative `(top_y, bottom_y)` of the row holding `state.selected`.
    pub selected_drive_band: Option<(u32, u32)>,
    pub cols: usize,
}

impl LayoutHitMap {
    pub fn hit_test(&self, x: u32, y: u32) -> Hit {
        // Explicit multi-target pills win over their parent polygon body.
        for zone in self.zones.iter().filter(|z| z.priority) {
            if zone.segments.iter().any(|s| s.contains(x, y)) {
                return Hit::Card(zone.card_index);
            }
        }
        for zone in self.zones.iter().filter(|z| !z.priority) {
            if zone.segments.iter().any(|s| s.contains(x, y)) {
                return Hit::Card(zone.card_index);
            }
        }
        for (index, rect) in self.actions.iter().enumerate() {
            if rect.contains(x, y) {
                return Hit::Action(index);
            }
        }
        Hit::None
    }

    /// Full hit test including `[X]` autoboot abort buttons and scrollbar controls.
    pub fn hit_test_experimental(&self, x: u32, y: u32) -> ExperimentalHit {
        // Abort `[X]` buttons always take highest priority so clicking `[X]` on
        // the flashing partition aborts autoboot instead of launching it.
        if self.abort_buttons.iter().any(|r| r.contains(x, y)) {
            return ExperimentalHit::AbortAutoboot;
        }
        if self.scroll_up.is_some_and(|r| r.contains(x, y)) {
            return ExperimentalHit::ScrollDelta(-110);
        }
        if self.scroll_down.is_some_and(|r| r.contains(x, y)) {
            return ExperimentalHit::ScrollDelta(110);
        }
        if let Some(track) = self.scroll_track
            && track.contains(x, y)
            && self.max_scroll_y > 0
        {
            let thumb_h = self.scroll_thumb.map(|t| t.h).unwrap_or(24);
            let travel = track.h.saturating_sub(thumb_h).max(1);
            let rel = y.saturating_sub(track.y + thumb_h / 2).min(travel);
            let target = ((rel as u64 * self.max_scroll_y as u64) / travel as u64) as u32;
            return ExperimentalHit::ScrollTo(target.min(self.max_scroll_y));
        }
        match self.hit_test(x, y) {
            Hit::None => ExperimentalHit::None,
            Hit::Card(i) => ExperimentalHit::Card(i),
            Hit::Action(i) => ExperimentalHit::Action(i),
        }
    }
}

const ACTIONS: [&str; 3] = ["Firmware", "Restart", "Shut down"];

/// Palette inspired by 0SFS's deep-slate bento cards and M5Paper's high-contrast
/// technical datasheets.
#[derive(Clone, Copy)]
struct Palette {
    bg: Rgba,
    header: Rgba,
    surface: Rgba,
    surface_alt: Rgba,
    surface_hover: Rgba,
    surface_selected: Rgba,
    unalloc_bg: Rgba,
    unalloc_hatch: Rgba,
    tint_cpu: Rgba,
    tint_mem: Rgba,
    tint_gpu: Rgba,
    tint_net: Rgba,
    tint_sec: Rgba,
    border: Rgba,
    border_strong: Rgba,
    grid_dot: Rgba,
    text: Rgba,
    muted: Rgba,
    eyebrow: Rgba,
    accent: Rgba,
    focus: Rgba,
    ok: Rgba,
    warn: Rgba,
    danger: Rgba,
    part_esp: Rgba,
    part_win: Rgba,
    part_linux: Rgba,
    part_enc: Rgba,
    part_rec: Rgba,
    part_data: Rgba,
    part_dim: Rgba,
    on_accent: Rgba,
}

impl Palette {
    fn from_dark(dark: bool) -> Self {
        if dark {
            Self {
                bg: Rgba::rgb(0x08, 0x0E, 0x14),
                header: Rgba::rgb(0x0E, 0x16, 0x1F),
                surface: Rgba::rgb(0x11, 0x1A, 0x23),
                surface_alt: Rgba::rgb(0x16, 0x21, 0x2D),
                surface_hover: Rgba::rgb(0x1D, 0x2B, 0x3A),
                surface_selected: Rgba::rgb(0x16, 0x2E, 0x44),
                unalloc_bg: Rgba::rgb(0x0C, 0x13, 0x1B),
                unalloc_hatch: Rgba::rgb(0x1A, 0x26, 0x33),
                tint_cpu: Rgba::rgb(0x18, 0x1E, 0x1B),
                tint_mem: Rgba::rgb(0x15, 0x18, 0x27),
                tint_gpu: Rgba::rgb(0x10, 0x20, 0x2D),
                tint_net: Rgba::rgb(0x12, 0x20, 0x1E),
                tint_sec: Rgba::rgb(0x1C, 0x17, 0x24),
                border: Rgba::rgb(0x2A, 0x37, 0x44),
                border_strong: Rgba::rgb(0x49, 0x5D, 0x70),
                grid_dot: Rgba::rgb(0x18, 0x25, 0x32),
                text: Rgba::rgb(0xF1, 0xF5, 0xF7),
                muted: Rgba::rgb(0x95, 0xA6, 0xB6),
                eyebrow: Rgba::rgb(0xAE, 0xBE, 0xCC),
                accent: Rgba::rgb(0xE8, 0xB6, 0x47),
                focus: Rgba::rgb(0x7C, 0xDC, 0xFF),
                ok: Rgba::rgb(0x3D, 0xDC, 0x97),
                warn: Rgba::rgb(0xF0, 0xB4, 0x4C),
                danger: Rgba::rgb(0xFF, 0x5D, 0x6C),
                part_esp: Rgba::rgb(0xE8, 0xB6, 0x47),
                part_win: Rgba::rgb(0x4C, 0x8D, 0xFF),
                part_linux: Rgba::rgb(0x3D, 0xDC, 0x97),
                part_enc: Rgba::rgb(0xC0, 0x84, 0xFC),
                part_rec: Rgba::rgb(0x38, 0xBD, 0xF8),
                part_data: Rgba::rgb(0x64, 0x74, 0x8B),
                part_dim: Rgba::rgb(0x47, 0x55, 0x69),
                on_accent: Rgba::rgb(0x0B, 0x10, 0x14),
            }
        } else {
            Self {
                bg: Rgba::rgb(0xF1, 0xF3, 0xEF),
                header: Rgba::rgb(0xFF, 0xFF, 0xFF),
                surface: Rgba::rgb(0xFF, 0xFF, 0xFF),
                surface_alt: Rgba::rgb(0xF6, 0xF8, 0xFA),
                surface_hover: Rgba::rgb(0xEB, 0xF2, 0xFA),
                surface_selected: Rgba::rgb(0xE0, 0xEE, 0xFF),
                unalloc_bg: Rgba::rgb(0xED, 0xF0, 0xF3),
                unalloc_hatch: Rgba::rgb(0xD3, 0xDC, 0xE5),
                tint_cpu: Rgba::rgb(0xFB, 0xF8, 0xEF),
                tint_mem: Rgba::rgb(0xF5, 0xF3, 0xFB),
                tint_gpu: Rgba::rgb(0xF0, 0xF7, 0xFB),
                tint_net: Rgba::rgb(0xF0, 0xF8, 0xF4),
                tint_sec: Rgba::rgb(0xFA, 0xF2, 0xF7),
                border: Rgba::rgb(0xC4, 0xCE, 0xD8),
                border_strong: Rgba::rgb(0x73, 0x84, 0x94),
                grid_dot: Rgba::rgb(0xD5, 0xDD, 0xE3),
                text: Rgba::rgb(0x10, 0x16, 0x1D),
                muted: Rgba::rgb(0x4F, 0x5E, 0x6C),
                eyebrow: Rgba::rgb(0x43, 0x52, 0x60),
                accent: Rgba::rgb(0xB3, 0x72, 0x00),
                focus: Rgba::rgb(0x0B, 0x57, 0xD0),
                ok: Rgba::rgb(0x0D, 0x7A, 0x4F),
                warn: Rgba::rgb(0xB4, 0x53, 0x09),
                danger: Rgba::rgb(0xC5, 0x22, 0x1F),
                part_esp: Rgba::rgb(0xD9, 0x91, 0x00),
                part_win: Rgba::rgb(0x1D, 0x6F, 0xE2),
                part_linux: Rgba::rgb(0x13, 0x8D, 0x5B),
                part_enc: Rgba::rgb(0x7C, 0x3A, 0xED),
                part_rec: Rgba::rgb(0x02, 0x84, 0xC7),
                part_data: Rgba::rgb(0x64, 0x74, 0x8B),
                part_dim: Rgba::rgb(0x8A, 0x99, 0xAC),
                on_accent: Rgba::rgb(0xFF, 0xFF, 0xFF),
            }
        }
    }

    fn part_accent(&self, part: &PartitionView) -> Rgba {
        if part.encrypted.is_some() {
            return self.part_enc;
        }
        match part.kind {
            PartitionKind::Esp => self.part_esp,
            PartitionKind::Windows => self.part_win,
            PartitionKind::Linux => self.part_linux,
            PartitionKind::Luks => self.part_enc,
            PartitionKind::Recovery => self.part_rec,
            PartitionKind::Data | PartitionKind::Apfs => self.part_data,
            PartitionKind::Msr | PartitionKind::Swap | PartitionKind::Other => self.part_dim,
        }
    }

    fn part_tint(&self, part: &PartitionView, selected: bool, hovered: bool) -> Rgba {
        let accent = self.part_accent(part);
        if selected {
            self.surface_selected.mix(accent, 14, 100)
        } else if hovered {
            self.surface_hover.mix(accent, 12, 100)
        } else if !part.boot_targets.is_empty() {
            self.surface_alt.mix(accent, 11, 100)
        } else {
            self.surface_alt.mix(accent, 6, 100)
        }
    }
}

/// Responsive anti-aliased typography + spacing metrics.
#[derive(Clone, Copy)]
struct Metrics {
    micro: TextStyle,
    micro_bold: TextStyle,
    body: TextStyle,
    body_bold: TextStyle,
    title: TextStyle,
    hero: TextStyle,
    pad: u32,
    gap: u32,
    radius: u32,
    compact: bool,
    wide: bool,
}

impl Metrics {
    fn for_size(w: u32, h: u32) -> Self {
        if w >= 1800 && h >= 980 {
            Self {
                micro: TextStyle::Micro,
                micro_bold: TextStyle::MicroBold,
                body: TextStyle::Body,
                body_bold: TextStyle::BodyBold,
                title: TextStyle::Title,
                hero: TextStyle::Hero,
                pad: 14,
                gap: 12,
                radius: 12,
                compact: false,
                wide: true,
            }
        } else if w >= 1000 && h >= 680 {
            Self {
                micro: TextStyle::Micro,
                micro_bold: TextStyle::MicroBold,
                body: TextStyle::Micro,
                body_bold: TextStyle::MicroBold,
                title: TextStyle::BodyBold,
                hero: TextStyle::Title,
                pad: 10,
                gap: 8,
                radius: 10,
                compact: false,
                wide: false,
            }
        } else {
            Self {
                micro: TextStyle::Micro,
                micro_bold: TextStyle::MicroBold,
                body: TextStyle::Micro,
                body_bold: TextStyle::MicroBold,
                title: TextStyle::MicroBold,
                hero: TextStyle::BodyBold,
                pad: 7,
                gap: 6,
                radius: 8,
                compact: true,
                wide: false,
            }
        }
    }
}

/// Compute hit-map without rendering pixels (used for pointer/keyboard routing).
pub fn layout_experimental(
    mode: LayoutMode,
    width: u32,
    height: u32,
    spec: &SystemSpec,
    state: &MenuState,
) -> LayoutHitMap {
    layout_experimental_scrolled(mode, width, height, spec, state, 0)
}

/// Compute hit-map at a specific vertical drive-list scroll offset.
pub fn layout_experimental_scrolled(
    mode: LayoutMode,
    width: u32,
    height: u32,
    spec: &SystemSpec,
    state: &MenuState,
    scroll_y: u32,
) -> LayoutHitMap {
    let mut dummy = Framebuffer::new(0, 0);
    render_mode(
        &mut dummy, width, height, mode, spec, state, scroll_y, false,
    )
}

/// Render an experimental layout and return its interactive hit-map.
pub fn paint_experimental(
    fb: &mut Framebuffer,
    mode: LayoutMode,
    spec: &SystemSpec,
    state: &MenuState,
) -> LayoutHitMap {
    paint_experimental_scrolled(fb, mode, spec, state, 0)
}

/// Render an experimental layout at a specific vertical drive-list scroll offset.
pub fn paint_experimental_scrolled(
    fb: &mut Framebuffer,
    mode: LayoutMode,
    spec: &SystemSpec,
    state: &MenuState,
    scroll_y: u32,
) -> LayoutHitMap {
    let (w, h) = (fb.width, fb.height);
    let map = render_mode(fb, w, h, mode, spec, state, scroll_y, true);
    if let Some((x, y)) = state.cursor {
        let pal = Palette::from_dark(state.dark);
        draw_cursor(fb, x, y, pal.focus);
    }
    map
}

#[allow(clippy::too_many_arguments)]
fn render_mode(
    fb: &mut Framebuffer,
    w: u32,
    h: u32,
    mode: LayoutMode,
    spec: &SystemSpec,
    state: &MenuState,
    scroll_y: u32,
    paint: bool,
) -> LayoutHitMap {
    let pal = Palette::from_dark(state.dark);
    let m = Metrics::for_size(w, h);
    match mode {
        LayoutMode::Bento | LayoutMode::Classic => {
            paint_bento(fb, w, h, spec, state, &pal, m, scroll_y, paint)
        }
        LayoutMode::Schematic => paint_schematic(fb, w, h, spec, state, &pal, m, scroll_y, paint),
        LayoutMode::Storage => paint_storage_first(fb, w, h, spec, state, &pal, m, scroll_y, paint),
    }
}

/// Smooth triangle-wave pulse `0..=255` driven by remaining autoboot countdown ms.
/// Speeds up during the final 3 seconds for unmistakable visual urgency.
fn autoboot_pulse(countdown_ms: u32) -> u32 {
    let period = if countdown_ms <= 3000 { 420 } else { 840 };
    let phase = countdown_ms % period;
    let half = period / 2;
    if phase < half {
        phase * 255 / half
    } else {
        (period - phase) * 255 / half
    }
}

// ============================================================================
// Experiment A: Hardware Bento Dashboard + Wrapped LBA Storage Widgets
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn paint_bento(
    fb: &mut Framebuffer,
    w: u32,
    h: u32,
    spec: &SystemSpec,
    state: &MenuState,
    pal: &Palette,
    m: Metrics,
    scroll_y: u32,
    paint: bool,
) -> LayoutHitMap {
    if paint {
        fb.fill(pal.bg);
    }
    let header_h = if m.compact {
        34
    } else if m.wide {
        50
    } else {
        42
    };
    let footer_h = if m.compact {
        34
    } else if m.wide {
        48
    } else {
        40
    };
    let header_abort = draw_header(
        fb,
        w,
        header_h,
        "A // HARDWARE BENTO",
        spec,
        state,
        pal,
        m,
        paint,
    );

    let x0 = m.pad;
    let inner_w = w.saturating_sub(m.pad * 2);
    let mut y = header_h.saturating_add(m.gap);

    let bento_h = if m.compact {
        106
    } else if m.wide {
        148
    } else {
        118
    };
    if paint {
        draw_bento_hw_grid(
            fb,
            Rect {
                x: x0,
                y,
                w: inner_w,
                h: bento_h,
            },
            spec,
            pal,
            m,
        );
    }
    y = y.saturating_add(bento_h).saturating_add(m.gap);

    let footer_y = h.saturating_sub(footer_h);
    let storage_h = footer_y.saturating_sub(y).saturating_sub(m.gap);
    let storage_rect = Rect {
        x: x0,
        y,
        w: inner_w,
        h: storage_h,
    };

    let mut hit = draw_storage_workspace(
        fb,
        storage_rect,
        spec,
        state,
        pal,
        m,
        StorageStyle::Bento,
        scroll_y,
        paint,
    );
    if let Some(r) = header_abort {
        hit.abort_buttons.push(r);
    }
    hit.actions = draw_footer(
        fb,
        Rect {
            x: 0,
            y: footer_y,
            w,
            h: footer_h,
        },
        state,
        pal,
        m,
        paint,
    );
    hit
}

fn draw_bento_hw_grid(
    fb: &mut Framebuffer,
    area: Rect,
    spec: &SystemSpec,
    pal: &Palette,
    m: Metrics,
) {
    let tiles = if m.compact {
        let row_h = area.h.saturating_sub(m.gap) / 2;
        let col_w = area.w.saturating_sub(m.gap * 2) / 3;
        vec![
            (
                0,
                Rect {
                    x: area.x,
                    y: area.y,
                    w: col_w,
                    h: row_h,
                },
            ),
            (
                1,
                Rect {
                    x: area.x + col_w + m.gap,
                    y: area.y,
                    w: col_w,
                    h: row_h,
                },
            ),
            (
                2,
                Rect {
                    x: area.x + (col_w + m.gap) * 2,
                    y: area.y,
                    w: area.w.saturating_sub((col_w + m.gap) * 2),
                    h: row_h,
                },
            ),
            (
                3,
                Rect {
                    x: area.x,
                    y: area.y + row_h + m.gap,
                    w: col_w,
                    h: area.h.saturating_sub(row_h + m.gap),
                },
            ),
            (
                4,
                Rect {
                    x: area.x + col_w + m.gap,
                    y: area.y + row_h + m.gap,
                    w: area.w.saturating_sub(col_w + m.gap),
                    h: area.h.saturating_sub(row_h + m.gap),
                },
            ),
        ]
    } else {
        let gaps = m.gap * 4;
        let avail = area.w.saturating_sub(gaps);
        let w0 = avail * 26 / 100;
        let w1 = avail * 22 / 100;
        let w2 = avail * 20 / 100;
        let w3 = avail * 16 / 100;
        let w4 = avail.saturating_sub(w0 + w1 + w2 + w3);
        let widths = [w0, w1, w2, w3, w4];
        let mut out = Vec::with_capacity(5);
        let mut x = area.x;
        for (idx, tw) in widths.into_iter().enumerate() {
            out.push((
                idx,
                Rect {
                    x,
                    y: area.y,
                    w: tw,
                    h: area.h,
                },
            ));
            x = x.saturating_add(tw).saturating_add(m.gap);
        }
        out
    };

    for (idx, r) in tiles {
        match idx {
            0 => draw_cpu_tile(fb, r, spec, pal, m),
            1 => draw_mem_gpu_tile(fb, r, spec, pal, m),
            2 => draw_board_fw_tile(fb, r, spec, pal, m),
            3 => draw_security_tile(fb, r, spec, pal, m),
            _ => draw_network_tile(fb, r, spec, pal, m),
        }
    }
}

fn draw_bento_panel(fb: &mut Framebuffer, r: Rect, fill: Rgba, border: Rgba, radius: u32) {
    fb.round_panel(r.x, r.y, r.w, r.h, radius, 1, fill, border);
}

fn draw_cpu_tile(fb: &mut Framebuffer, r: Rect, spec: &SystemSpec, pal: &Palette, m: Metrics) {
    draw_bento_panel(fb, r, pal.tint_cpu, pal.border, m.radius);
    let px = r.x + m.pad;
    let tight = r.h < 68;
    let py = r.y + if tight { 5 } else { m.pad.min(8) };
    let tw = r.w.saturating_sub(m.pad * 2);
    let hero_style = if tight { m.body_bold } else { m.hero };
    draw_eyebrow(fb, px, py, tw, "01 / PROCESSOR", pal.eyebrow, m);
    let mut y = py
        + m.micro.line_height()
        + (if tight {
            2
        } else if m.compact {
            3
        } else {
            6
        });
    match &spec.cpu {
        ProbeState::Pending => {
            draw_pill(fb, px, y, "PROBING CPU..", pal.warn, pal, m);
        }
        ProbeState::Unavailable => {
            draw_text_fit(fb, "CPU unavailable", px, y, m.body, 0, tw, pal.muted);
        }
        ProbeState::Ready(cpu) => {
            let hero = match cpu.cores {
                Some(c) => format!("{c}C · {}", cpu.arch.to_ascii_uppercase()),
                None => cpu.arch.to_ascii_uppercase(),
            };
            draw_text_fit(fb, &hero, px, y, hero_style, 0, tw, pal.accent);
            y += hero_style.line_height() + (if tight || m.compact { 2 } else { 5 });
            let model = cpu.model.as_deref().unwrap_or("Model not exposed");
            let col = if cpu.model.is_some() {
                pal.text
            } else {
                pal.muted
            };
            draw_text_fit(
                fb,
                model,
                px,
                y,
                if tight { m.micro } else { m.body },
                0,
                tw,
                col,
            );
            if !m.compact && y + m.body.line_height() + m.micro.line_height() + 8 < r.y + r.h {
                let board = match &spec.board {
                    ProbeState::Ready(b) => b
                        .product
                        .clone()
                        .or_else(|| b.vendor.clone())
                        .unwrap_or_else(|| "Board n/a".into()),
                    ProbeState::Pending => "Board probing..".into(),
                    ProbeState::Unavailable => "Board not exposed".into(),
                };
                draw_text_fit(
                    fb,
                    &board,
                    px,
                    r.y + r.h - m.pad - m.micro.line_height(),
                    m.micro,
                    0,
                    tw,
                    pal.muted,
                );
            }
        }
    }
}

fn draw_mem_gpu_tile(fb: &mut Framebuffer, r: Rect, spec: &SystemSpec, pal: &Palette, m: Metrics) {
    draw_bento_panel(fb, r, pal.tint_mem, pal.border, m.radius);
    let px = r.x + m.pad;
    let tight = r.h < 68;
    let py = r.y + if tight { 5 } else { m.pad.min(8) };
    let tw = r.w.saturating_sub(m.pad * 2);
    let hero_style = if tight { m.body_bold } else { m.hero };
    draw_eyebrow(fb, px, py, tw, "02 / MEMORY + GPU", pal.eyebrow, m);
    let mut y = py
        + m.micro.line_height()
        + (if tight {
            2
        } else if m.compact {
            3
        } else {
            6
        });
    match &spec.memory {
        ProbeState::Pending => {
            draw_pill(fb, px, y, "PROBING RAM..", pal.warn, pal, m);
        }
        ProbeState::Unavailable => {
            draw_text_fit(fb, "RAM n/a", px, y, hero_style, 0, tw, pal.muted);
        }
        ProbeState::Ready(mem) => {
            let label = format_mib(mem.total_mib);
            draw_text_fit(fb, &label, px, y, hero_style, 0, tw, pal.text);
        }
    }
    y += hero_style.line_height() + (if tight || m.compact { 2 } else { 5 });
    match &spec.gpu {
        ProbeState::Pending => {
            draw_text_fit(fb, "GPU probing..", px, y, m.micro, 0, tw, pal.warn);
        }
        ProbeState::Unavailable => {
            draw_text_fit(fb, "GPU not exposed", px, y, m.micro, 0, tw, pal.muted);
        }
        ProbeState::Ready(gpus) if gpus.is_empty() => {
            draw_text_fit(
                fb,
                "GOP only (no PCI GPU)",
                px,
                y,
                m.micro,
                0,
                tw,
                pal.muted,
            );
        }
        ProbeState::Ready(gpus) => {
            draw_text_fit(
                fb,
                &gpus[0],
                px,
                y,
                if tight { m.micro } else { m.body },
                0,
                tw,
                pal.focus,
            );
            if gpus.len() > 1
                && !m.compact
                && y + m.body.line_height() + m.micro.line_height() + 6 < r.y + r.h
            {
                draw_text_fit(
                    fb,
                    &format!("+{} more: {}", gpus.len() - 1, gpus[1]),
                    px,
                    r.y + r.h - m.pad - m.micro.line_height(),
                    m.micro,
                    0,
                    tw,
                    pal.muted,
                );
            }
        }
    }
}

fn draw_board_fw_tile(fb: &mut Framebuffer, r: Rect, spec: &SystemSpec, pal: &Palette, m: Metrics) {
    draw_bento_panel(fb, r, pal.tint_gpu, pal.border, m.radius);
    let px = r.x + m.pad;
    let tight = r.h < 68;
    let py = r.y + if tight { 5 } else { m.pad.min(8) };
    let tw = r.w.saturating_sub(m.pad * 2);
    let hero_style = if tight { m.body_bold } else { m.hero };
    draw_eyebrow(fb, px, py, tw, "03 / FIRMWARE + GOP", pal.eyebrow, m);
    let mut y = py
        + m.micro.line_height()
        + (if tight {
            2
        } else if m.compact {
            3
        } else {
            6
        });
    let disp = format!("{}×{}", spec.display.width, spec.display.height);
    draw_text_fit(fb, &disp, px, y, hero_style, 0, tw, pal.focus);
    y += hero_style.line_height() + (if tight || m.compact { 2 } else { 5 });
    match &spec.firmware {
        ProbeState::Pending => {
            draw_text_fit(fb, "UEFI probing..", px, y, m.micro, 0, tw, pal.warn);
        }
        ProbeState::Unavailable => {
            draw_text_fit(fb, "Firmware n/a", px, y, m.micro, 0, tw, pal.muted);
        }
        ProbeState::Ready(fw) => {
            let v = fw.vendor.as_deref().unwrap_or("UEFI vendor n/a");
            draw_text_fit(
                fb,
                v,
                px,
                y,
                if tight { m.micro } else { m.body },
                0,
                tw,
                pal.text,
            );
            if !m.compact && y + m.body.line_height() + m.micro.line_height() + 6 < r.y + r.h {
                draw_text_fit(
                    fb,
                    &format!("SBM {} · {}", fw.version, fw.arch),
                    px,
                    r.y + r.h - m.pad - m.micro.line_height(),
                    m.micro,
                    0,
                    tw,
                    pal.muted,
                );
            }
        }
    }
}

fn draw_security_tile(fb: &mut Framebuffer, r: Rect, spec: &SystemSpec, pal: &Palette, m: Metrics) {
    draw_bento_panel(fb, r, pal.tint_sec, pal.border, m.radius);
    let px = r.x + m.pad;
    let tight = r.h < 68;
    let py = r.y + if tight { 5 } else { m.pad.min(8) };
    let tw = r.w.saturating_sub(m.pad * 2);
    draw_eyebrow(fb, px, py, tw, "04 / SECURITY", pal.eyebrow, m);
    let mut y = py
        + m.micro.line_height()
        + (if tight {
            2
        } else if m.compact {
            3
        } else {
            6
        });
    match &spec.security {
        ProbeState::Pending => {
            draw_pill(fb, px, y, "PROBING..", pal.warn, pal, m);
        }
        ProbeState::Unavailable => {
            draw_text_fit(fb, "Not exposed", px, y, m.body, 0, tw, pal.muted);
        }
        ProbeState::Ready(sec) => {
            let sb_col = match sec.secure_boot {
                SecureBootSummary::Enabled => pal.ok,
                SecureBootSummary::Disabled => pal.warn,
                SecureBootSummary::SetupMode => pal.accent,
                SecureBootSummary::Unsupported => pal.muted,
            };
            draw_pill(fb, px, y, sec.secure_boot.badge(), sb_col, pal, m);
            y += m.micro.line_height()
                + (if tight {
                    6
                } else if m.compact {
                    8
                } else {
                    14
                });
            let tpm = match sec.tpm_present {
                Some(true) => ("TPM 2.0 OK", pal.ok),
                Some(false) => ("TPM: none", pal.muted),
                None => ("TPM: n/a", pal.muted),
            };
            draw_text_fit(
                fb,
                tpm.0,
                px,
                y,
                if tight { m.micro_bold } else { m.body_bold },
                0,
                tw,
                tpm.1,
            );
            if !m.compact
                && let Some(note) = &spec.policy_note
                && r.y + r.h > y + m.body.line_height() + m.micro.line_height() + 8
            {
                draw_text_fit(
                    fb,
                    note,
                    px,
                    r.y + r.h - m.pad - m.micro.line_height(),
                    m.micro,
                    0,
                    tw,
                    pal.warn,
                );
            }
        }
    }
}

fn draw_network_tile(fb: &mut Framebuffer, r: Rect, spec: &SystemSpec, pal: &Palette, m: Metrics) {
    draw_bento_panel(fb, r, pal.tint_net, pal.border, m.radius);
    let px = r.x + m.pad;
    let py = r.y + m.pad.min(8);
    let tw = r.w.saturating_sub(m.pad * 2);
    draw_eyebrow(fb, px, py, tw, "05 / NETWORK", pal.eyebrow, m);
    let mut y = py + m.micro.line_height() + (if m.compact { 3 } else { 6 });
    match &spec.network {
        ProbeState::Pending => {
            draw_pill(fb, px, y, "PROBING..", pal.warn, pal, m);
        }
        ProbeState::Unavailable => {
            draw_text_fit(fb, "SNP not exposed", px, y, m.body, 0, tw, pal.muted);
        }
        ProbeState::Ready(nics) if nics.is_empty() => {
            draw_text_fit(fb, "Offline / no NIC", px, y, m.body, 0, tw, pal.muted);
        }
        ProbeState::Ready(nics) => {
            for nic in nics.iter().take(if m.compact { 1 } else { 2 }) {
                let kind = if nic.wireless { "WIFI" } else { "ETH" };
                let st = if nic.up { "UP" } else { "DOWN" };
                let col = if nic.up { pal.ok } else { pal.muted };
                draw_text_fit(
                    fb,
                    &format!("{} {kind} {st}", nic.name),
                    px,
                    y,
                    m.body_bold,
                    0,
                    tw,
                    col,
                );
                y += m.body.line_height() + 2;
                if let Some(addr) = &nic.address
                    && y + m.micro.line_height() <= r.y + r.h - 4
                {
                    draw_text_fit(fb, addr, px, y, m.micro, 0, tw, pal.muted);
                    y += m.micro.line_height() + 4;
                }
            }
        }
    }
}

// ============================================================================
// Experiment B: Hardware Schematic / PCB Silkscreen + LBA Polygon Blocks
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn paint_schematic(
    fb: &mut Framebuffer,
    w: u32,
    h: u32,
    spec: &SystemSpec,
    state: &MenuState,
    pal: &Palette,
    m: Metrics,
    scroll_y: u32,
    paint: bool,
) -> LayoutHitMap {
    if paint {
        fb.fill(pal.bg);
        let grid_step = if m.compact { 16 } else { 20 };
        fb.dot_grid(0, 0, w, h, grid_step, pal.grid_dot);
    }

    let header_h = if m.compact {
        34
    } else if m.wide {
        48
    } else {
        40
    };
    let footer_h = if m.compact {
        34
    } else if m.wide {
        48
    } else {
        40
    };
    let header_abort = draw_header(
        fb,
        w,
        header_h,
        "B // HARDWARE SCHEMATIC",
        spec,
        state,
        pal,
        m,
        paint,
    );

    let x0 = m.pad;
    let inner_w = w.saturating_sub(m.pad * 2);
    let top_y = header_h.saturating_add(m.gap);
    let footer_y = h.saturating_sub(footer_h);
    let work_h = footer_y.saturating_sub(top_y).saturating_sub(m.gap);

    let ic_h = if m.compact {
        66
    } else if m.wide {
        106
    } else {
        84
    };
    let bus_band_h = if m.compact { 22 } else { 28 };

    if paint {
        let ic_area = Rect {
            x: x0,
            y: top_y,
            w: inner_w,
            h: ic_h,
        };
        draw_schematic_ics(fb, ic_area, spec, pal, m);

        let spine_y = top_y + ic_h + bus_band_h / 2;
        let left_drop = x0 + inner_w / 10;
        let right_drop = x0 + inner_w - inner_w / 10;
        fb.hline(
            left_drop,
            spine_y,
            right_drop.saturating_sub(left_drop),
            pal.border_strong,
        );
        fb.hline(
            left_drop,
            spine_y + 1,
            right_drop.saturating_sub(left_drop),
            pal.border_strong,
        );
        let hub_x = x0 + inner_w / 2;
        fb.vline(hub_x, top_y + ic_h, bus_band_h / 2, pal.accent);
        fb.vline(hub_x + 1, top_y + ic_h, bus_band_h / 2, pal.accent);
        fb.circle_dot(hub_x, spine_y, 3, pal.accent);

        let bus_tag = "PCIE / SATA / USB STORAGE BUS";
        let tag_w = measure_text(bus_tag, m.micro_bold, 1) + 12;
        let tag_x = hub_x.saturating_sub(tag_w / 2);
        let tag_y = spine_y.saturating_sub(m.micro.line_height() / 2 + 2);
        fb.fill_rect(tag_x, tag_y, tag_w, m.micro.line_height() + 4, pal.bg);
        fb.rect_outline(
            tag_x,
            tag_y,
            tag_w,
            m.micro.line_height() + 4,
            1,
            pal.border_strong,
        );
        draw_text(
            fb,
            bus_tag,
            tag_x + 6,
            tag_y + 2,
            m.micro_bold,
            1,
            pal.eyebrow,
        );
    }

    let storage_y = top_y + ic_h + bus_band_h;
    let storage_h = work_h.saturating_sub(ic_h + bus_band_h);
    let storage_rect = Rect {
        x: x0,
        y: storage_y,
        w: inner_w,
        h: storage_h,
    };

    let mut hit = draw_storage_workspace(
        fb,
        storage_rect,
        spec,
        state,
        pal,
        m,
        StorageStyle::Schematic {
            spine_y: top_y + ic_h + bus_band_h / 2,
        },
        scroll_y,
        paint,
    );
    if let Some(r) = header_abort {
        hit.abort_buttons.push(r);
    }
    hit.actions = draw_footer(
        fb,
        Rect {
            x: 0,
            y: footer_y,
            w,
            h: footer_h,
        },
        state,
        pal,
        m,
        paint,
    );
    hit
}

fn draw_schematic_ics(
    fb: &mut Framebuffer,
    area: Rect,
    spec: &SystemSpec,
    pal: &Palette,
    m: Metrics,
) {
    let count = if m.compact { 4 } else { 5 };
    let link_w = if m.compact { 10 } else { 16 };
    let block_w = area.w.saturating_sub(link_w * (count as u32 - 1)) / count as u32;
    let mid_y = area.y + area.h / 2;

    let mut x = area.x;
    for idx in 0..count {
        let bw = if idx + 1 == count {
            area.x + area.w - x
        } else {
            block_w
        };
        let r = Rect {
            x,
            y: area.y,
            w: bw,
            h: area.h,
        };
        fb.fill_rect(r.x, r.y, r.w, r.h, pal.surface);
        fb.rect_outline(r.x, r.y, r.w, r.h, 1, pal.border_strong);
        fb.corner_ticks(r.x, r.y, r.w, r.h, 6, pal.accent);

        let px = r.x + 8;
        let py = r.y + 6;
        let tw = r.w.saturating_sub(16);
        let l1_y = py + m.micro.line_height() + 7;
        let l2_y = r.y + r.h - m.micro.line_height() - 6;

        match idx {
            0 => {
                draw_ic_tag(fb, px, py, "U1", "CPU", pal.accent, pal, m);
                let line1 = match &spec.cpu {
                    ProbeState::Pending => "PROBING..".into(),
                    ProbeState::Unavailable => "NOT EXPOSED".into(),
                    ProbeState::Ready(c) => match c.cores {
                        Some(n) => format!("{n}C · {}", c.arch.to_ascii_uppercase()),
                        None => c.arch.to_ascii_uppercase(),
                    },
                };
                let line2 = match &spec.cpu {
                    ProbeState::Ready(c) => c.model.as_deref().unwrap_or("Model n/a"),
                    _ => "",
                };
                let c1 = if matches!(spec.cpu, ProbeState::Pending) {
                    pal.warn
                } else {
                    pal.text
                };
                draw_text_fit(fb, &line1, px, l1_y, m.title, 0, tw, c1);
                if !line2.is_empty() && l2_y > l1_y + m.title.line_height() {
                    draw_text_fit(fb, line2, px, l2_y, m.micro, 0, tw, pal.muted);
                }
            }
            1 => {
                draw_ic_tag(fb, px, py, "M1", "MEM+GPU", pal.focus, pal, m);
                let line1 = match &spec.memory {
                    ProbeState::Pending => "PROBING..".into(),
                    ProbeState::Unavailable => "RAM N/A".into(),
                    ProbeState::Ready(mem) => format_mib(mem.total_mib),
                };
                let line2 = match &spec.gpu {
                    ProbeState::Pending => "GPU probing..",
                    ProbeState::Unavailable => "GPU n/a",
                    ProbeState::Ready(g) => g.first().map(String::as_str).unwrap_or("GOP only"),
                };
                draw_text_fit(fb, &line1, px, l1_y, m.title, 0, tw, pal.text);
                if l2_y > l1_y + m.title.line_height() {
                    draw_text_fit(fb, line2, px, l2_y, m.micro, 0, tw, pal.focus);
                }
            }
            2 => {
                draw_ic_tag(fb, px, py, "F1", "UEFI/GOP", pal.part_rec, pal, m);
                let line1 = format!("{}×{}", spec.display.width, spec.display.height);
                let line2 = match &spec.firmware {
                    ProbeState::Pending => "FW probing..",
                    ProbeState::Unavailable => "FW n/a",
                    ProbeState::Ready(fw) => fw.vendor.as_deref().unwrap_or("UEFI"),
                };
                draw_text_fit(fb, &line1, px, l1_y, m.title, 0, tw, pal.text);
                if l2_y > l1_y + m.title.line_height() {
                    draw_text_fit(fb, line2, px, l2_y, m.micro, 0, tw, pal.muted);
                }
            }
            3 => {
                draw_ic_tag(fb, px, py, "S1", "SEC/TPM", pal.ok, pal, m);
                let (l1, c1, l2): (&str, Rgba, &str) = match &spec.security {
                    ProbeState::Pending => ("PROBING..", pal.warn, "TPM.."),
                    ProbeState::Unavailable => ("N/A", pal.muted, "Not exposed"),
                    ProbeState::Ready(s) => {
                        let c = match s.secure_boot {
                            SecureBootSummary::Enabled => pal.ok,
                            SecureBootSummary::Disabled => pal.warn,
                            _ => pal.muted,
                        };
                        let tpm = match s.tpm_present {
                            Some(true) => "TPM 2.0 OK",
                            Some(false) => "NO TPM",
                            None => "TPM N/A",
                        };
                        (s.secure_boot.badge(), c, tpm)
                    }
                };
                draw_text_fit(fb, l1, px, l1_y, m.title, 0, tw, c1);
                if l2_y > l1_y + m.title.line_height() {
                    draw_text_fit(fb, l2, px, l2_y, m.micro, 0, tw, pal.muted);
                }
            }
            _ => {
                draw_ic_tag(fb, px, py, "N1", "NET", pal.ok, pal, m);
                let (l1, c1, l2) = match &spec.network {
                    ProbeState::Pending => ("PROBING..".into(), pal.warn, String::new()),
                    ProbeState::Unavailable => ("SNP N/A".into(), pal.muted, String::new()),
                    ProbeState::Ready(nics) => match nics.first() {
                        Some(nic) => (
                            format!("{} {}", nic.name, if nic.up { "UP" } else { "DOWN" }),
                            if nic.up { pal.ok } else { pal.muted },
                            nic.address.clone().unwrap_or_else(|| "no IP".into()),
                        ),
                        None => ("OFFLINE".into(), pal.muted, "no NIC".into()),
                    },
                };
                draw_text_fit(fb, &l1, px, l1_y, m.title, 0, tw, c1);
                if !l2.is_empty() && l2_y > l1_y + m.title.line_height() {
                    draw_text_fit(fb, &l2, px, l2_y, m.micro, 0, tw, pal.muted);
                }
            }
        }

        if idx + 1 < count {
            let lx = r.x + r.w;
            fb.hline(lx, mid_y, link_w, pal.border_strong);
            fb.circle_dot(lx, mid_y, 2, pal.accent);
            fb.circle_dot(lx + link_w, mid_y, 2, pal.accent);
        }
        x = x.saturating_add(bw).saturating_add(link_w);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_ic_tag(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    code: &str,
    name: &str,
    accent: Rgba,
    pal: &Palette,
    m: Metrics,
) {
    let cw = measure_text(code, m.micro_bold, 0) + 8;
    let h = m.micro.line_height() + 4;
    fb.fill_rect(x, y, cw, h, accent);
    draw_text(fb, code, x + 4, y + 2, m.micro_bold, 0, pal.on_accent);
    draw_text(fb, name, x + cw + 6, y + 2, m.micro_bold, 1, pal.eyebrow);
}

// ============================================================================
// Experiment C: Storage-First Full-Width LBA Paragraph Canvases + Spec Strip
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn paint_storage_first(
    fb: &mut Framebuffer,
    w: u32,
    h: u32,
    spec: &SystemSpec,
    state: &MenuState,
    pal: &Palette,
    m: Metrics,
    scroll_y: u32,
    paint: bool,
) -> LayoutHitMap {
    if paint {
        fb.fill(pal.bg);
    }
    let header_h = if m.compact {
        34
    } else if m.wide {
        48
    } else {
        40
    };
    let strip_h = if m.compact || m.wide { 36 } else { 28 };
    let footer_h = if m.compact {
        34
    } else if m.wide {
        48
    } else {
        40
    };

    let header_abort = draw_header(
        fb,
        w,
        header_h,
        "C // STORAGE-FIRST",
        spec,
        state,
        pal,
        m,
        paint,
    );

    let x0 = m.pad;
    let inner_w = w.saturating_sub(m.pad * 2);
    let strip_y = header_h.saturating_add(m.gap);
    if paint {
        draw_compact_hw_strip(
            fb,
            Rect {
                x: x0,
                y: strip_y,
                w: inner_w,
                h: strip_h,
            },
            spec,
            pal,
            m,
        );
    }

    let storage_y = strip_y.saturating_add(strip_h).saturating_add(m.gap);
    let footer_y = h.saturating_sub(footer_h);
    let storage_h = footer_y.saturating_sub(storage_y).saturating_sub(m.gap);

    let mut hit = draw_storage_workspace(
        fb,
        Rect {
            x: x0,
            y: storage_y,
            w: inner_w,
            h: storage_h,
        },
        spec,
        state,
        pal,
        m,
        StorageStyle::FullWidthRack,
        scroll_y,
        paint,
    );
    if let Some(r) = header_abort {
        hit.abort_buttons.push(r);
    }
    hit.actions = draw_footer(
        fb,
        Rect {
            x: 0,
            y: footer_y,
            w,
            h: footer_h,
        },
        state,
        pal,
        m,
        paint,
    );
    hit
}

fn draw_compact_hw_strip(
    fb: &mut Framebuffer,
    r: Rect,
    spec: &SystemSpec,
    pal: &Palette,
    m: Metrics,
) {
    fb.round_panel(
        r.x,
        r.y,
        r.w,
        r.h,
        m.radius.min(8),
        1,
        pal.surface,
        pal.border,
    );
    let cpu = match &spec.cpu {
        ProbeState::Pending => "CPU: probing..".into(),
        ProbeState::Unavailable => "CPU: n/a".into(),
        ProbeState::Ready(c) => {
            let short = c
                .model
                .as_deref()
                .map(short_cpu_name)
                .unwrap_or_else(|| c.arch.clone());
            match c.cores {
                Some(n) => format!("CPU {short} ({n}C)"),
                None => format!("CPU {short}"),
            }
        }
    };
    let ram = match &spec.memory {
        ProbeState::Pending => "RAM: ..".into(),
        ProbeState::Unavailable => "RAM: n/a".into(),
        ProbeState::Ready(mem) => format!("RAM {}", format_mib(mem.total_mib)),
    };
    let gpu = match &spec.gpu {
        ProbeState::Pending => "GPU: ..".into(),
        ProbeState::Unavailable => "GPU: n/a".into(),
        ProbeState::Ready(g) => format!(
            "GPU {}",
            g.first()
                .map(|s| short_gpu_name(s))
                .unwrap_or_else(|| "GOP".into())
        ),
    };
    let sec = match &spec.security {
        ProbeState::Pending => "SEC: ..".into(),
        ProbeState::Unavailable => "SEC: n/a".into(),
        ProbeState::Ready(s) => {
            let tpm = match s.tpm_present {
                Some(true) => " TPM2",
                Some(false) => " NO-TPM",
                None => "",
            };
            format!("{}{tpm}", s.secure_boot.badge())
        }
    };
    let net = match &spec.network {
        ProbeState::Pending => "NET: ..".into(),
        ProbeState::Unavailable => "NET: n/a".into(),
        ProbeState::Ready(nics) => match nics.first() {
            Some(nic) => format!(
                "NET {} {}",
                nic.name,
                nic.address
                    .as_deref()
                    .unwrap_or(if nic.up { "UP" } else { "DOWN" })
            ),
            None => "NET offline".into(),
        },
    };
    let disp = format!("GOP {}×{}", spec.display.width, spec.display.height);

    let segments = [
        (cpu, pal.accent),
        (ram, pal.text),
        (gpu, pal.focus),
        (sec, pal.ok),
        (net, pal.muted),
        (disp, pal.eyebrow),
    ];

    if m.compact {
        let cell_w = r.w / 3;
        let row_h = r.h / 2;
        for (i, (label, color)) in segments.iter().enumerate() {
            let col = (i % 3) as u32;
            let row = (i / 3) as u32;
            let cx = r.x + col * cell_w + 8;
            let cy = r.y + row * row_h + row_h.saturating_sub(m.micro.line_height()) / 2;
            if col > 0 && row == 0 {
                fb.vline(
                    r.x + col * cell_w,
                    r.y + 4,
                    r.h.saturating_sub(8),
                    pal.border,
                );
            }
            draw_text_fit(
                fb,
                label,
                cx,
                cy,
                m.micro_bold,
                0,
                cell_w.saturating_sub(14),
                *color,
            );
        }
    } else {
        let cell_w = r.w / segments.len() as u32;
        let ty = r.y + r.h.saturating_sub(m.micro.line_height()) / 2;
        for (i, (label, color)) in segments.iter().enumerate() {
            let cx = r.x + i as u32 * cell_w;
            if i > 0 {
                fb.vline(cx, r.y + 5, r.h.saturating_sub(10), pal.border);
            }
            draw_text_fit(
                fb,
                label,
                cx + 8,
                ty,
                m.micro_bold,
                0,
                cell_w.saturating_sub(14),
                *color,
            );
        }
    }
}

// ============================================================================
// Storage Workspace & Wrapped LBA Orthogonal-Polygon Disk Widgets
// ============================================================================

#[derive(Clone, Copy)]
enum StorageStyle {
    Bento,
    Schematic { spine_y: u32 },
    FullWidthRack,
}

#[allow(clippy::too_many_arguments)]
fn draw_storage_workspace(
    fb: &mut Framebuffer,
    area: Rect,
    spec: &SystemSpec,
    state: &MenuState,
    pal: &Palette,
    m: Metrics,
    style: StorageStyle,
    scroll_y: u32,
    paint: bool,
) -> LayoutHitMap {
    let mut hit = LayoutHitMap {
        cols: 1,
        ..LayoutHitMap::default()
    };

    let devices = match &spec.storage {
        ProbeState::Pending => {
            if paint {
                draw_bento_panel(fb, area, pal.surface, pal.border, m.radius);
                draw_text_fit(
                    fb,
                    "PROBING STORAGE DEVICES...",
                    area.x + m.pad,
                    area.y + m.pad,
                    m.title,
                    0,
                    area.w.saturating_sub(m.pad * 2),
                    pal.warn,
                );
            }
            return hit;
        }
        ProbeState::Unavailable => {
            if paint {
                draw_bento_panel(fb, area, pal.surface, pal.border, m.radius);
                draw_text_fit(
                    fb,
                    "No storage devices exposed by firmware",
                    area.x + m.pad,
                    area.y + m.pad,
                    m.title,
                    0,
                    area.w.saturating_sub(m.pad * 2),
                    pal.muted,
                );
            }
            return hit;
        }
        ProbeState::Ready(list) if list.is_empty() => {
            if paint {
                draw_bento_panel(fb, area, pal.surface, pal.border, m.radius);
                draw_text_fit(
                    fb,
                    "No physical storage devices discovered",
                    area.x + m.pad,
                    area.y + m.pad,
                    m.title,
                    0,
                    area.w.saturating_sub(m.pad * 2),
                    pal.muted,
                );
            }
            return hit;
        }
        ProbeState::Ready(list) => list,
    };

    let bus_gutter = match style {
        StorageStyle::Schematic { .. } => {
            if m.compact {
                12
            } else {
                18
            }
        }
        _ => 0,
    };
    let label_h = match style {
        StorageStyle::Schematic { .. } => 0,
        StorageStyle::Bento | StorageStyle::FullWidthRack => m.micro.line_height() + 6,
    };
    let work = Rect {
        x: area.x.saturating_add(bus_gutter),
        y: area.y.saturating_add(label_h),
        w: area.w.saturating_sub(bus_gutter),
        h: area.h.saturating_sub(label_h),
    };
    hit.viewport_h = work.h;

    let n = devices.len();
    let cols: usize = match style {
        // Storage-first always uses 1 full-width column so every disk has maximum
        // horizontal canvas for paragraph-style LBA polygon wrapping!
        StorageStyle::FullWidthRack => 1,
        StorageStyle::Bento | StorageStyle::Schematic { .. } => {
            if n == 1 {
                1
            } else if n >= 4 && work.w >= 1650 {
                3
            } else if n >= 3 && work.w >= 760 {
                2
            } else if n == 2 && work.w >= 1100 {
                // Two rich disks side-by-side in Bento/Schematic (e.g. bitlocker, lba-stress).
                2
            } else {
                1
            }
        }
    };
    let used_cols = cols.min(n).max(1);
    hit.cols = used_cols;
    let rows = n.div_ceil(used_cols);

    // Compute natural comfortable height for every row based on its region count
    // and boot target richness so multi-drive setups stay spacious and scrollable.
    let mut row_weights = Vec::with_capacity(rows);
    let mut row_heights = Vec::with_capacity(rows);
    for row in 0..rows {
        let row_start = row * used_cols;
        let row_end = (row_start + used_cols).min(n);
        let row_devs = &devices[row_start..row_end];
        let max_score = row_devs
            .iter()
            .map(|d| {
                let r_pts = d.regions.len().clamp(1, 10) as u32 * 3;
                let t_pts = d.boot_target_count().min(4) as u32 * 6;
                (8 + r_pts + t_pts).min(36)
            })
            .max()
            .unwrap_or(10);
        let max_regions = row_devs.iter().map(|d| d.regions.len()).max().unwrap_or(1);
        let max_targets = row_devs
            .iter()
            .map(StorageDevice::boot_target_count)
            .max()
            .unwrap_or(0);
        let base_h: u32 = match style {
            StorageStyle::FullWidthRack => {
                if m.compact {
                    155
                } else if m.wide {
                    255
                } else {
                    205
                }
            }
            StorageStyle::Bento | StorageStyle::Schematic { .. } => {
                if m.compact {
                    165
                } else if m.wide {
                    265
                } else {
                    215
                }
            }
        };
        let rich_bonus = if max_regions >= 6 || max_targets >= 3 {
            if m.compact { 34 } else { 52 }
        } else if max_regions >= 3 || max_targets >= 1 {
            if m.compact { 16 } else { 26 }
        } else {
            0
        };
        row_weights.push(max_score);
        row_heights.push(base_h.saturating_add(rich_bonus));
    }

    let total_gaps = m.gap.saturating_mul(rows.saturating_sub(1) as u32);
    let natural_total_h: u32 = row_heights
        .iter()
        .copied()
        .sum::<u32>()
        .saturating_add(total_gaps);
    let scrollable = natural_total_h > work.h && rows > 1;

    // If everything fits comfortably inside `work.h`, stretch rows proportionally
    // to fill 100% of the available canvas (identical to non-scrolling layouts).
    if !scrollable {
        let total_weight: u32 = row_weights.iter().sum::<u32>().max(1);
        let avail_h = work.h.saturating_sub(total_gaps);
        let mut used = 0u32;
        for row in 0..rows {
            let rh = if row + 1 == rows {
                avail_h.saturating_sub(used)
            } else {
                (avail_h.saturating_mul(row_weights[row]) / total_weight).max(58)
            };
            row_heights[row] = rh;
            used = used.saturating_add(rh);
        }
    }

    let content_h: u32 = row_heights
        .iter()
        .copied()
        .sum::<u32>()
        .saturating_add(total_gaps);
    let max_scroll_y = content_h.saturating_sub(work.h);
    let eff_scroll_y = scroll_y.min(max_scroll_y);
    hit.scroll_y = eff_scroll_y;
    hit.max_scroll_y = max_scroll_y;

    let sb_w = if scrollable {
        if m.compact { 14 } else { 16 }
    } else {
        0
    };
    let sb_gap = if scrollable { 6 } else { 0 };
    let content_w = work.w.saturating_sub(sb_w + sb_gap);
    let content_viewport = Rect {
        x: work.x,
        y: work.y,
        w: content_w,
        h: work.h,
    };

    // Draw eyebrow header for Bento & Storage-First (including scroll hint when scrollable).
    if paint && matches!(style, StorageStyle::Bento | StorageStyle::FullWidthRack) {
        let total_targets: usize = devices.iter().map(StorageDevice::boot_target_count).sum();
        let prefix = match style {
            StorageStyle::FullWidthRack => "LBA STORAGE RACK  //  ",
            _ => "STORAGE TOPOLOGY (LBA ORDER)  //  ",
        };
        let scroll_hint = if scrollable { " · SCROLL DRIVES" } else { "" };
        let title = format!(
            "{prefix}{} DEVICE{} · {} BOOT TARGET{}{scroll_hint}",
            devices.len(),
            if devices.len() == 1 { "" } else { "S" },
            total_targets,
            if total_targets == 1 { "" } else { "S" },
        );
        draw_eyebrow(fb, area.x, area.y, area.w, &title, pal.eyebrow, m);
    }

    let col_w =
        content_w.saturating_sub(m.gap.saturating_mul(used_cols as u32 - 1)) / used_cols as u32;

    // Translate screen cursor into content-space ONLY when cursor is inside the storage viewport.
    let content_cursor = state.cursor.and_then(|(cx, cy)| {
        content_viewport
            .contains(cx, cy)
            .then_some((cx, cy.saturating_add(eff_scroll_y)))
    });

    // Enable pixel-exact viewport clipping & scroll translation for drive rows + schematic bus taps.
    let clip_rect = Rect {
        x: area.x,
        y: work.y,
        w: content_w.saturating_add(bus_gutter),
        h: work.h,
    };
    if paint {
        fb.set_viewport(
            clip_rect.x,
            clip_rect.y,
            clip_rect.w,
            clip_rect.h,
            -(eff_scroll_y as i32),
        );
    }

    let trunk_x = area.x + bus_gutter / 2;
    let mut cur_rel_y = 0u32;
    let mut last_tap_y = work.y;

    for (row, &rh) in row_heights.iter().enumerate() {
        let row_top = cur_rel_y;
        let row_bottom = cur_rel_y.saturating_add(rh);
        let row_y = work.y.saturating_add(row_top);
        let row_mid_y = row_y.saturating_add(rh / 2);

        // Track selected drive's content-space vertical band so keyboard navigation auto-scrolls.
        let row_start = row * used_cols;
        let row_end = (row_start + used_cols).min(n);
        if devices[row_start..row_end]
            .iter()
            .any(|dev| dev.has_selected(state.selected))
        {
            hit.selected_drive_band = Some((row_top, row_bottom));
        }

        // Skip rows completely outside the visible viewport window.
        let visible_row =
            row_bottom > eff_scroll_y && row_top < eff_scroll_y.saturating_add(work.h);
        if visible_row {
            last_tap_y = row_mid_y;

            if paint && let StorageStyle::Schematic { .. } = style {
                let row_dev = &devices[row_start];
                let tap_col = bus_color(&row_dev.bus, row_dev.removable, pal);
                fb.hline(trunk_x, row_mid_y, work.x.saturating_sub(trunk_x), tap_col);
                fb.circle_dot(trunk_x, row_mid_y, 3, tap_col);
                fb.circle_dot(work.x, row_mid_y, 2, tap_col);
            }

            for col in 0..used_cols {
                let idx = row_start + col;
                if idx >= n {
                    break;
                }
                let dev = &devices[idx];
                let dx = work.x + (col as u32) * (col_w + m.gap);
                let dw = if col + 1 == used_cols {
                    work.x + content_w - dx
                } else {
                    col_w
                };
                let rect = Rect {
                    x: dx,
                    y: row_y,
                    w: dw,
                    h: rh,
                };

                if paint && let StorageStyle::Schematic { spine_y } = style {
                    let bus_col = bus_color(&dev.bus, dev.removable, pal);
                    if row == 0 {
                        let drop_x = rect.x + rect.w / 2;
                        fb.vline(drop_x, spine_y, rect.y.saturating_sub(spine_y), bus_col);
                        fb.circle_dot(drop_x, rect.y, 2, bus_col);
                    }
                    if col > 0 {
                        let prev_right = rect.x.saturating_sub(m.gap);
                        fb.hline(prev_right, row_mid_y, m.gap, pal.border_strong);
                        fb.circle_dot(prev_right, row_mid_y, 2, bus_col);
                        fb.circle_dot(rect.x, row_mid_y, 2, bus_col);
                    }
                }

                let (dev_zones, dev_aborts) =
                    draw_storage_device(fb, rect, dev, state, content_cursor, pal, m, style, paint);
                // Convert hit zones & partition [X] buttons from content-space back to
                // clipped screen-space so off-screen portions never receive stray clicks!
                for zone in dev_zones {
                    let clipped: Vec<Rect> = zone
                        .segments
                        .into_iter()
                        .filter_map(|seg| shift_and_clip_rect(seg, eff_scroll_y, content_viewport))
                        .collect();
                    if !clipped.is_empty() {
                        hit.zones.push(CardHitZone {
                            card_index: zone.card_index,
                            segments: clipped,
                            priority: zone.priority,
                        });
                    }
                }
                for abort_r in dev_aborts {
                    if let Some(sr) = shift_and_clip_rect(abort_r, eff_scroll_y, content_viewport) {
                        hit.abort_buttons.push(sr);
                    }
                }
            }
        }

        cur_rel_y = row_bottom.saturating_add(m.gap);
    }

    if paint {
        if let StorageStyle::Schematic { spine_y } = style {
            let trunk_top = work.y.saturating_add(eff_scroll_y);
            let trunk_h = last_tap_y.saturating_sub(trunk_top);
            if trunk_h > 0 {
                fb.vline(trunk_x, trunk_top, trunk_h, pal.border_strong);
            }
            fb.clear_viewport();
            // Connect upper bus spine down to the top of the schematic storage viewport.
            fb.vline(
                trunk_x,
                spine_y,
                work.y.saturating_sub(spine_y),
                pal.border_strong,
            );
            fb.hline(
                trunk_x,
                spine_y,
                (area.x + area.w / 10).saturating_sub(trunk_x),
                pal.border_strong,
            );
            fb.circle_dot(trunk_x, spine_y, 3, pal.accent);
        } else {
            fb.clear_viewport();
        }
    }

    // Subtle top/bottom scroll-available edge indicators + sleek interactive scrollbar.
    if scrollable {
        let sb_x = work.x + content_w + sb_gap;
        let btn_h = sb_w.clamp(14, 18);
        let up_rect = Rect {
            x: sb_x,
            y: work.y,
            w: sb_w,
            h: btn_h,
        };
        let down_rect = Rect {
            x: sb_x,
            y: work.y + work.h.saturating_sub(btn_h),
            w: sb_w,
            h: btn_h,
        };
        let track_y = up_rect.y + up_rect.h + 3;
        let track_h = down_rect.y.saturating_sub(track_y + 3).max(20);
        let track_rect = Rect {
            x: sb_x,
            y: track_y,
            w: sb_w,
            h: track_h,
        };
        let thumb_h = ((track_h as u64 * work.h as u64) / content_h.max(1) as u64)
            .clamp(22, track_h as u64) as u32;
        let travel = track_h.saturating_sub(thumb_h);
        let thumb_y = track_y
            + if max_scroll_y > 0 {
                ((travel as u64 * eff_scroll_y as u64) / max_scroll_y as u64) as u32
            } else {
                0
            };
        let thumb_rect = Rect {
            x: sb_x + 2,
            y: thumb_y,
            w: sb_w.saturating_sub(4).max(6),
            h: thumb_h,
        };

        hit.scroll_up = Some(up_rect);
        hit.scroll_down = Some(down_rect);
        hit.scroll_track = Some(track_rect);
        hit.scroll_thumb = Some(thumb_rect);

        if paint {
            let up_hov = state.cursor.is_some_and(|(x, y)| up_rect.contains(x, y));
            let dn_hov = state.cursor.is_some_and(|(x, y)| down_rect.contains(x, y));
            let th_hov = state.cursor.is_some_and(|(x, y)| thumb_rect.contains(x, y));

            // Scrollbar track background.
            fb.round_panel(
                track_rect.x,
                track_rect.y,
                track_rect.w,
                track_rect.h,
                5,
                1,
                pal.surface_alt,
                pal.border,
            );
            // Up & down buttons with crisp chevrons.
            fb.round_panel(
                up_rect.x,
                up_rect.y,
                up_rect.w,
                up_rect.h,
                4,
                1,
                if up_hov {
                    pal.surface_hover
                } else {
                    pal.surface
                },
                if eff_scroll_y > 0 {
                    pal.border_strong
                } else {
                    pal.border
                },
            );
            draw_chevron(
                fb,
                up_rect.x + 2,
                up_rect.y + 2,
                up_rect.w.saturating_sub(4),
                true,
                if eff_scroll_y > 0 {
                    if up_hov { pal.focus } else { pal.text }
                } else {
                    pal.border
                },
            );
            fb.round_panel(
                down_rect.x,
                down_rect.y,
                down_rect.w,
                down_rect.h,
                4,
                1,
                if dn_hov {
                    pal.surface_hover
                } else {
                    pal.surface
                },
                if eff_scroll_y < max_scroll_y {
                    pal.border_strong
                } else {
                    pal.border
                },
            );
            draw_chevron(
                fb,
                down_rect.x + 2,
                down_rect.y + 2,
                down_rect.w.saturating_sub(4),
                false,
                if eff_scroll_y < max_scroll_y {
                    if dn_hov { pal.focus } else { pal.text }
                } else {
                    pal.border
                },
            );
            // Proportional scrollbar thumb.
            let thumb_col = if th_hov { pal.focus } else { pal.border_strong };
            fb.round_rect(
                thumb_rect.x,
                thumb_rect.y,
                thumb_rect.w,
                thumb_rect.h,
                4,
                thumb_col,
            );

            // Subtle scroll-overflow accent hairline at top/bottom edge of content viewport.
            if eff_scroll_y > 0 {
                fb.hline(
                    content_viewport.x + 8,
                    content_viewport.y,
                    content_viewport.w.saturating_sub(16),
                    pal.focus,
                );
            }
            if eff_scroll_y < max_scroll_y {
                fb.hline(
                    content_viewport.x + 8,
                    content_viewport.y + content_viewport.h.saturating_sub(1),
                    content_viewport.w.saturating_sub(16),
                    pal.accent,
                );
            }
        }
    }

    hit
}

fn shift_and_clip_rect(r: Rect, scroll_y: u32, viewport: Rect) -> Option<Rect> {
    let sy0 = (r.y as i64) - (scroll_y as i64);
    let sy1 = sy0 + (r.h as i64);
    let vx0 = viewport.x as i64;
    let vy0 = viewport.y as i64;
    let vx1 = (viewport.x + viewport.w) as i64;
    let vy1 = (viewport.y + viewport.h) as i64;
    let x0 = (r.x as i64).max(vx0);
    let y0 = sy0.max(vy0);
    let x1 = ((r.x + r.w) as i64).min(vx1);
    let y1 = sy1.min(vy1);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(Rect {
        x: x0 as u32,
        y: y0 as u32,
        w: (x1 - x0) as u32,
        h: (y1 - y0) as u32,
    })
}

#[allow(clippy::too_many_arguments)]
fn draw_storage_device(
    fb: &mut Framebuffer,
    r: Rect,
    dev: &StorageDevice,
    state: &MenuState,
    cursor: Option<(u32, u32)>,
    pal: &Palette,
    m: Metrics,
    style: StorageStyle,
    paint: bool,
) -> (Vec<CardHitZone>, Vec<Rect>) {
    let mut zones = Vec::new();
    let mut abort_buttons = Vec::new();
    let has_selected = state.focus == Focus::Cards && dev.has_selected(state.selected);
    let bus_col = bus_color(&dev.bus, dev.removable, pal);

    if paint {
        let border = if has_selected {
            pal.border_strong
        } else {
            pal.border
        };
        match style {
            StorageStyle::Schematic { .. } => {
                fb.fill_rect(r.x, r.y, r.w, r.h, pal.surface);
                fb.rect_outline(r.x, r.y, r.w, r.h, 1, border);
                fb.corner_ticks(r.x, r.y, r.w, r.h, 7, bus_col);
            }
            StorageStyle::Bento | StorageStyle::FullWidthRack => {
                fb.round_panel(r.x, r.y, r.w, r.h, m.radius, 1, pal.surface, border);
                fb.fill_rect(
                    r.x + 1,
                    r.y + m.radius.min(r.h / 2),
                    3,
                    r.h.saturating_sub(m.radius.min(r.h / 2) * 2),
                    bus_col,
                );
            }
        }
    }

    let pad = if m.compact { 6 } else { 8 };
    let px = r.x + pad + 3;
    let inner_w = r.w.saturating_sub(pad * 2 + 4);
    let header_h = m.body.line_height().max(m.micro.line_height() + 4) + 5;
    let y0 = r.y + pad.min(7);

    // 1. Device Header Row: [NVME0] [NVMe] Model   |   Capacity + LBA count
    if paint {
        let cap = if inner_w >= 460
            && matches!(
                style,
                StorageStyle::Schematic { .. } | StorageStyle::FullWidthRack
            ) {
            format!(
                "{} · {} LBA",
                format_bytes(dev.size_bytes),
                format_sectors(dev.total_sectors)
            )
        } else {
            format_bytes(dev.size_bytes)
        };
        let cap_w = measure_text(&cap, m.body_bold, 0);
        let desig_w = draw_badge_solid(fb, px, y0, &dev.designator, bus_col, pal.on_accent, m);
        let bus_tag = if dev.removable {
            format!("{} REM", dev.bus.to_ascii_uppercase())
        } else {
            dev.bus.to_ascii_uppercase()
        };
        let bus_w = draw_pill(fb, px + desig_w + 5, y0, &bus_tag, bus_col, pal, m);
        let model_x = px + desig_w + bus_w + 12;
        let model_max = (px + inner_w)
            .saturating_sub(cap_w + 10)
            .saturating_sub(model_x);
        let model = dev.model.as_deref().unwrap_or("Model not exposed");
        let model_col = if dev.model.is_some() {
            pal.text
        } else {
            pal.muted
        };
        draw_text_fit(
            fb,
            model,
            model_x,
            y0 + 1,
            m.body_bold,
            0,
            model_max,
            model_col,
        );
        draw_text(
            fb,
            &cap,
            px + inner_w.saturating_sub(cap_w),
            y0 + 1,
            m.body_bold,
            0,
            pal.accent,
        );
    }

    // Reserve optional Selected Target / LBA Provenance footer inside tall single-disk cards.
    let map_y = y0.saturating_add(header_h);
    let mut map_bottom = r.y + r.h.saturating_sub(pad);
    let show_provenance = !m.compact
        && map_bottom.saturating_sub(map_y) >= 200
        && dev.find_selected(state.selected).is_some();
    let prov_h = if show_provenance {
        m.micro.line_height() * 2 + 12
    } else {
        0
    };
    if show_provenance {
        map_bottom = map_bottom.saturating_sub(prov_h + 6);
    }

    let map_rect = Rect {
        x: px,
        y: map_y,
        w: inner_w,
        h: map_bottom.saturating_sub(map_y),
    };

    if dev.regions.is_empty() || map_rect.w < 24 || map_rect.h < 20 {
        return (zones, abort_buttons);
    }

    // 2. Build WrapInput in EXACT ascending `start_lba` order (never reordered!).
    let inputs: Vec<WrapInput> = dev
        .regions
        .iter()
        .map(|reg| {
            let targets = reg.boot_targets().len();
            let (min_w, min_h, interactive) = match &reg.kind {
                RegionKind::Unallocated => (
                    if m.compact {
                        54
                    } else if m.wide {
                        86
                    } else {
                        66
                    },
                    if m.compact { 26 } else { 32 },
                    false,
                ),
                RegionKind::Partition(p) if targets >= 2 => (
                    if m.compact {
                        150
                    } else if m.wide {
                        240
                    } else {
                        185
                    },
                    if m.compact {
                        42
                    } else if m.wide {
                        62
                    } else {
                        48
                    },
                    true,
                ),
                RegionKind::Partition(p) if targets == 1 => {
                    let _ = p;
                    (
                        if m.compact {
                            116
                        } else if m.wide {
                            176
                        } else {
                            138
                        },
                        if m.compact {
                            38
                        } else if m.wide {
                            54
                        } else {
                            44
                        },
                        true,
                    )
                }
                RegionKind::Partition(p) => {
                    let w = match p.kind {
                        PartitionKind::Msr | PartitionKind::Swap => {
                            if m.compact {
                                54
                            } else if m.wide {
                                84
                            } else {
                                64
                            }
                        }
                        _ => {
                            if m.compact {
                                70
                            } else if m.wide {
                                110
                            } else {
                                84
                            }
                        }
                    };
                    (w, if m.compact { 28 } else { 34 }, false)
                }
            };
            WrapInput {
                size_bytes: reg.size_bytes.max(1),
                min_width: min_w.min(map_rect.w.max(1)),
                min_height: min_h.min(map_rect.h.max(1)),
                interactive,
            }
        })
        .collect();

    // 3. Compute connected paragraph-wrapped orthogonal polygon layout.
    let wrapped = layout_disk_wrap(map_rect, &inputs);

    // 4. Paint polygon fills, hatches, internal labels/logos/targets, then crisp merged outer borders!
    // Hover check applies to the ENTIRE connected polygon (all segments share one hover state).
    let mut selected_outline: Option<(Vec<Rect>, Rgba, u32)> = None;
    let mut hovered_outline: Option<(Vec<Rect>, Rgba)> = None;
    let mut hovered_region: Option<&DiskRegion> = None;
    let countdown_pulse = state.countdown_ms.map(autoboot_pulse);

    for (reg, poly) in dev.regions.iter().zip(&wrapped.regions) {
        let hovered = cursor.is_some_and(|(cx, cy)| poly.contains(cx, cy));
        if hovered {
            hovered_region = Some(reg);
        }
        let selected_target = if state.focus == Focus::Cards {
            reg.boot_targets()
                .iter()
                .find(|t| t.card_index == state.selected)
        } else {
            None
        };
        let is_selected = selected_target.is_some();
        let is_autoboot_target = is_selected && state.countdown_ms.is_some();

        // Register whole-polygon hit zone (one logical interaction target for the entire shape!).
        if !reg.boot_targets().is_empty() {
            let primary_card = selected_target
                .map(|t| t.card_index)
                .unwrap_or(reg.boot_targets()[0].card_index);
            zones.push(CardHitZone {
                card_index: primary_card,
                segments: poly.segments.clone(),
                priority: false,
            });
        }

        if paint {
            match &reg.kind {
                RegionKind::Unallocated => {
                    let fill = if hovered {
                        pal.surface_hover
                    } else {
                        pal.unalloc_bg
                    };
                    fill_polygon(fb, &poly.segments, fill);
                    hatch_polygon(fb, &poly.segments, 8, pal.unalloc_hatch);
                    stroke_merged_polygon(fb, &poly.segments, 1, pal.border);
                    draw_unallocated_content(fb, reg, poly, pal, m);
                    if hovered {
                        hovered_outline = Some((poly.segments.clone(), pal.border_strong));
                    }
                }
                RegionKind::Partition(part) => {
                    let base_fill = pal.part_tint(part, is_selected, hovered);
                    // Flash the ENTIRE connected polygon smoothly while autoboot counts down!
                    // Keep the partition's OS hue vivid (avoid desaturating blue+yellow to gray).
                    let fill = if let Some(pulse) = countdown_pulse
                        && is_autoboot_target
                    {
                        let part_col = pal.part_accent(part);
                        if state.dark {
                            base_fill.mix(part_col, pulse * 95 / 255, 255).mix(
                                pal.accent,
                                pulse * 36 / 255,
                                255,
                            )
                        } else {
                            base_fill.mix(pal.accent, pulse * 55 / 255, 255).mix(
                                part_col,
                                pulse * 35 / 255,
                                255,
                            )
                        }
                    } else {
                        base_fill
                    };
                    fill_polygon(fb, &poly.segments, fill);

                    // Crisp bus/OS accent tab at the top-left of the partition's first segment
                    // so reading-order start of every L/Z polygon is visually anchored!
                    if let Some(first) = poly.segments.first() {
                        let accent = if is_autoboot_target {
                            pal.accent.mix(pal.focus, countdown_pulse.unwrap_or(0), 255)
                        } else {
                            pal.part_accent(part)
                        };
                        let tab_h = first.h.saturating_sub(2).max(1);
                        fb.fill_rect(first.x + 1, first.y + 1, 3, tab_h, accent);
                    }

                    // Normal merged border (shared internal row edges are automatically canceled!).
                    stroke_merged_polygon(fb, &poly.segments, 1, pal.border);

                    let (mut sub_zones, mut part_aborts) = draw_partition_content(
                        fb,
                        reg,
                        part,
                        poly,
                        state,
                        cursor,
                        is_selected,
                        hovered,
                        pal,
                        m,
                        true,
                    );
                    zones.append(&mut sub_zones);
                    abort_buttons.append(&mut part_aborts);

                    if is_autoboot_target {
                        let pulse = countdown_pulse.unwrap_or(200);
                        let glow = pal.focus.mix(pal.accent, pulse, 255);
                        let thickness = if pulse > 140 { 3 } else { 2 };
                        selected_outline = Some((poly.segments.clone(), glow, thickness));
                    } else if is_selected {
                        let glow = pal.border_strong.mix(
                            pal.focus,
                            160u32.saturating_sub(state.anim_ms.min(160)),
                            160,
                        );
                        selected_outline = Some((poly.segments.clone(), glow, 2));
                    } else if hovered {
                        hovered_outline = Some((poly.segments.clone(), pal.part_accent(part)));
                    }
                }
            }
        } else if let RegionKind::Partition(part) = &reg.kind {
            let (mut sub_zones, mut part_aborts) = draw_partition_content(
                fb,
                reg,
                part,
                poly,
                state,
                cursor,
                is_selected,
                hovered,
                pal,
                m,
                false,
            );
            zones.append(&mut sub_zones);
            abort_buttons.append(&mut part_aborts);
        }
    }

    // Draw hovered and selected outer polygon perimeters LAST so they sit cleanly
    // on top of adjacent neighbors with zero clipping.
    if paint {
        if let Some((segs, col)) = hovered_outline {
            stroke_merged_polygon(fb, &segs, 2, col);
        }
        if let Some((segs, col, thickness)) = selected_outline {
            stroke_merged_polygon(fb, &segs, thickness, col);
        }
        // Subtle outer frame around the entire LBA disk map.
        fb.rect_outline(
            map_rect.x,
            map_rect.y,
            map_rect.w,
            map_rect.h,
            1,
            pal.border_strong,
        );

        if show_provenance {
            let pr = Rect {
                x: px,
                y: map_bottom + 6,
                w: inner_w,
                h: prov_h,
            };
            if let Some(hreg) = hovered_region {
                draw_hover_lba_bar(fb, pr, dev, hreg, pal, m);
            } else if let Some((sel_reg, sel_target)) = dev.find_selected(state.selected) {
                draw_provenance_bar(
                    fb,
                    pr,
                    dev,
                    sel_reg,
                    sel_target,
                    state.countdown_ms.is_some(),
                    pal,
                    m,
                );
            }
        }
    }

    (zones, abort_buttons)
}

fn draw_unallocated_content(
    fb: &mut Framebuffer,
    reg: &DiskRegion,
    poly: &WrappedRegion,
    pal: &Palette,
    m: Metrics,
) {
    let lr = poly.label_rect;
    if lr.w < 30 || lr.h < 16 {
        return;
    }
    let pad = 6u32;
    let tx = lr.x + pad;
    let tw = lr.w.saturating_sub(pad * 2);
    let cap = format_bytes(reg.size_bytes);
    let scale_tag = if poly.clamped { " ~" } else { "" };

    if lr.h >= m.micro.line_height() * 2 + 12 {
        let ty = lr.y + lr.h.saturating_sub(m.micro.line_height() * 2 + 3) / 2;
        draw_text_fit(fb, "UNALLOC", tx, ty, m.micro_bold, 0, tw, pal.muted);
        draw_text_fit(
            fb,
            &format!("{cap}{scale_tag}"),
            tx,
            ty + m.micro.line_height() + 3,
            m.micro,
            0,
            tw,
            pal.eyebrow,
        );
    } else {
        let ty = lr.y + lr.h.saturating_sub(m.micro.line_height()) / 2;
        draw_text_fit(
            fb,
            &format!("FREE {cap}{scale_tag}"),
            tx,
            ty,
            m.micro,
            0,
            tw,
            pal.muted,
        );
    }
}

/// Render partition label, exact capacity, filesystem, OS emblem, encryption lock,
/// interactive boot action(s), and (when this partition is counting down to autoboot)
/// an inline `[X]` abort-autoboot button inside `poly.label_rect`.
///
/// Returns `(sub_zones, abort_buttons)`.
#[allow(clippy::too_many_arguments)]
fn draw_partition_content(
    fb: &mut Framebuffer,
    reg: &DiskRegion,
    part: &PartitionView,
    poly: &WrappedRegion,
    state: &MenuState,
    cursor: Option<(u32, u32)>,
    is_selected: bool,
    hovered: bool,
    pal: &Palette,
    m: Metrics,
    paint: bool,
) -> (Vec<CardHitZone>, Vec<Rect>) {
    let mut sub_zones = Vec::new();
    let mut abort_buttons = Vec::new();
    let lr = poly.label_rect;
    if lr.w < 24 || lr.h < 16 {
        return (sub_zones, abort_buttons);
    }

    let selected_card = state.selected;
    let countdown_ms = state.countdown_ms;
    let pad = if m.compact { 5 } else { 7 };
    let x0 = lr.x + pad + 2;
    let w = lr.w.saturating_sub(pad * 2 + 2);
    let y0 = lr.y + pad.min(6);
    let bottom = lr.y + lr.h.saturating_sub(pad.min(5));
    let accent = pal.part_accent(part);

    let cap = format_bytes(reg.size_bytes);
    let fs = part.fs.as_deref().unwrap_or(part_short_tag(part));
    let scale_mark = if poly.clamped { "~" } else { "" };
    let wrap_mark = if poly.is_polygonal() && w >= 150 {
        " · WRAP"
    } else {
        ""
    };

    // Case 1: Partition with 2+ boot targets (e.g. Arch + systemd-boot, or ESP with SBM Core A/B).
    if part.boot_targets.len() >= 2 {
        let hdr_h = m.micro.line_height() + 4;
        if paint {
            let hdr = format!("p{} · {cap}{scale_mark} · {fs}{wrap_mark}", part.index);
            draw_text_fit(fb, &hdr, x0, y0, m.micro_bold, 0, w, accent);
        }
        let btn_top = y0 + hdr_h;
        let btn_avail = bottom.saturating_sub(btn_top);
        let n = part.boot_targets.len();
        let b_cols = if n >= 2 && w >= 190 { n.min(2) } else { 1 };
        let b_rows = n.div_ceil(b_cols);
        let gap = 4u32;
        let col_w = w.saturating_sub(gap * (b_cols as u32 - 1)) / b_cols as u32;
        let max_bh = if m.wide { 42 } else { 32 };
        let row_h =
            (btn_avail.saturating_sub(gap * (b_rows as u32 - 1)) / b_rows as u32).clamp(16, max_bh);

        for (idx, target) in part.boot_targets.iter().enumerate() {
            let col = idx % b_cols;
            let row = idx / b_cols;
            let bx = x0 + (col as u32) * (col_w + gap);
            let by = btn_top + (row as u32) * (row_h + gap);
            if by + 12 > lr.y + lr.h {
                break;
            }
            let bw = if col + 1 == b_cols {
                x0 + w - bx
            } else {
                col_w
            };
            let bh = row_h.min((lr.y + lr.h).saturating_sub(by + 2));
            let br = Rect {
                x: bx,
                y: by,
                w: bw,
                h: bh,
            };
            sub_zones.push(CardHitZone {
                card_index: target.card_index,
                segments: vec![br],
                priority: true,
            });
            let t_sel = is_selected && selected_card == target.card_index;
            let t_countdown = if t_sel { countdown_ms } else { None };
            if let Some(abort_r) =
                draw_inline_target_pill(fb, br, target, t_sel, t_countdown, cursor, pal, m, paint)
            {
                abort_buttons.push(abort_r);
            }
        }
        return (sub_zones, abort_buttons);
    }

    // Case 2: Partition with exactly 1 boot target (entire polygon is one unified boot widget!).
    if let Some(target) = part.boot_targets.first() {
        let icon_sz = if lr.h >= 54 && w >= 130 {
            if m.wide { 28 } else { 24 }
        } else if lr.h >= 34 && w >= 90 {
            18
        } else {
            0
        };
        let mut tx = x0;
        let mut tw = w;
        if icon_sz > 0 {
            if paint {
                let iy = lr.y + lr.h.saturating_sub(icon_sz) / 2;
                let badge_bg = if !target.healthy { pal.danger } else { accent };
                draw_os_emblem(
                    fb,
                    &target.hint,
                    &target.title,
                    x0,
                    iy,
                    icon_sz,
                    badge_bg,
                    pal.on_accent,
                );
            }
            tx = x0 + icon_sz + 8;
            tw = (x0 + w).saturating_sub(tx);
        }

        let title_style = if lr.h >= 50 && tw >= 110 {
            m.title
        } else {
            m.body_bold
        };
        let line1_h = m.micro.line_height();
        let title_h = title_style.line_height();
        let is_autoboot = is_selected && countdown_ms.is_some();

        if lr.h >= line1_h + title_h + m.micro.line_height() + 14 {
            // 3 rich lines: (1) pN · exact capacity · FS [+ lock], (2) OS Name + [BOOT/AUTO] + [X], (3) EFI provenance
            let total_h = line1_h + title_h + line1_h + 6;
            let mut cy = lr.y + lr.h.saturating_sub(total_h) / 2;

            if paint {
                // Top meta line with optional padlock icon.
                let mut mx = tx;
                if part.encrypted.is_some() {
                    draw_lock_icon(fb, mx, cy, line1_h, pal.part_enc);
                    mx += line1_h + 4;
                }
                let enc_tag = part
                    .encrypted
                    .as_deref()
                    .map(|e| format!(" · {e}"))
                    .unwrap_or_default();
                let meta = format!("p{} · {cap}{scale_mark} · {fs}{enc_tag}", part.index);
                draw_text_fit(
                    fb,
                    &meta,
                    mx,
                    cy,
                    m.micro_bold,
                    0,
                    (tx + tw).saturating_sub(mx),
                    accent,
                );
            }
            cy += line1_h + 3;

            // Hero OS name line + BOOT / AUTO countdown indicator + clickable [X] abort button!
            let pill_h = m.micro.line_height() + 6;
            let pill_y = cy + title_h.saturating_sub(pill_h) / 2;
            let mut right_edge = tx + tw;

            if let Some(left_ms) = countdown_ms
                && is_selected
            {
                let x_sz = pill_h.clamp(18, 24);
                let x_rect = Rect {
                    x: right_edge.saturating_sub(x_sz),
                    y: pill_y,
                    w: x_sz,
                    h: x_sz,
                };
                abort_buttons.push(x_rect);
                if paint {
                    let x_hov = cursor.is_some_and(|(cx, cy)| x_rect.contains(cx, cy));
                    draw_abort_x_button(fb, x_rect, x_hov, pal);
                }
                right_edge = x_rect.x.saturating_sub(5);

                if paint && right_edge > tx + 135 {
                    let pulse = autoboot_pulse(left_ms);
                    let secs = left_ms.div_ceil(1000);
                    let badge = if hovered {
                        format!("BOOT NOW ({secs}s)")
                    } else {
                        format!("AUTO {secs}s")
                    };
                    let bw = measure_text(&badge, m.micro_bold, 0) + 12;
                    let bx = right_edge.saturating_sub(bw);
                    let bcol = if hovered {
                        pal.focus
                    } else {
                        pal.accent.mix(pal.focus, pulse / 2, 255)
                    };
                    draw_pill(fb, bx, pill_y + 1, &badge, bcol, pal, m);
                    right_edge = bx.saturating_sub(8);
                }
            } else if paint {
                let boot_badge = if hovered && tw >= 165 {
                    Some("CLICK: COUNTDOWN")
                } else if is_selected && tw >= 170 {
                    Some("ENTER: COUNTDOWN")
                } else {
                    None
                };
                if let Some(b) = boot_badge {
                    let bw = measure_text(b, m.micro_bold, 0) + 12;
                    let bx = right_edge.saturating_sub(bw);
                    draw_pill(
                        fb,
                        bx,
                        pill_y + 1,
                        b,
                        if is_selected { pal.focus } else { accent },
                        pal,
                        m,
                    );
                    right_edge = bx.saturating_sub(8);
                }
            }

            if paint {
                let prefix = if is_selected { "> " } else { "" };
                let title_col = if is_autoboot {
                    if state.dark { pal.text } else { pal.focus }
                } else if is_selected {
                    pal.focus
                } else if !target.healthy {
                    pal.danger
                } else {
                    pal.text
                };
                draw_text_fit(
                    fb,
                    &format!("{prefix}{}", target.title),
                    tx,
                    cy,
                    title_style,
                    0,
                    right_edge.saturating_sub(tx),
                    title_col,
                );
                cy += title_h + 3;

                // Explicit EFI provenance line (never assumes OS partition == ESP).
                let mut prov = match (target.esp_part, target.os_part) {
                    (Some(esp), Some(os)) if esp != os => {
                        format!("EFI:p{esp} -> OS:p{os} · {}", target.loader_file)
                    }
                    (Some(esp), _) => format!("EFI:p{esp} · {}", target.loader_file),
                    (None, _) => target.loader_file.clone(),
                };
                if let Some(note) = &target.note {
                    prov = format!("[{note}] {prov}");
                }
                let pcol = if !target.healthy {
                    pal.danger
                } else if target.note.is_some() {
                    pal.warn
                } else {
                    pal.muted
                };
                draw_text_fit(fb, &prov, tx, cy, m.micro, 0, tw, pcol);
            }
        } else if lr.h >= title_h + line1_h + 8 {
            let total_h = title_h + line1_h + 3;
            let cy = lr.y + lr.h.saturating_sub(total_h) / 2;
            let mut right_edge = tx + tw;
            if is_autoboot {
                let x_sz = 18u32.min(lr.h.saturating_sub(4));
                let x_rect = Rect {
                    x: right_edge.saturating_sub(x_sz),
                    y: cy,
                    w: x_sz,
                    h: x_sz,
                };
                abort_buttons.push(x_rect);
                if paint {
                    let x_hov = cursor.is_some_and(|(cx, cy)| x_rect.contains(cx, cy));
                    draw_abort_x_button(fb, x_rect, x_hov, pal);
                }
                right_edge = x_rect.x.saturating_sub(5);
            }
            if paint {
                let prefix = if is_selected { "> " } else { "" };
                let title_col = if is_autoboot {
                    pal.accent
                } else if is_selected {
                    pal.focus
                } else {
                    pal.text
                };
                draw_text_fit(
                    fb,
                    &format!("{prefix}{}", target.title),
                    tx,
                    cy,
                    title_style,
                    0,
                    right_edge.saturating_sub(tx),
                    title_col,
                );
                let sub = format!("p{} · {cap}{scale_mark} · {fs}", part.index);
                draw_text_fit(fb, &sub, tx, cy + title_h + 3, m.micro, 0, tw, pal.muted);
            }
        } else {
            let cy = lr.y + lr.h.saturating_sub(m.micro.line_height()) / 2;
            let mut right_edge = tx + tw;
            if is_autoboot {
                let x_sz = lr.h.saturating_sub(4).clamp(14, 18);
                let x_rect = Rect {
                    x: right_edge.saturating_sub(x_sz),
                    y: lr.y + lr.h.saturating_sub(x_sz) / 2,
                    w: x_sz,
                    h: x_sz,
                };
                abort_buttons.push(x_rect);
                if paint {
                    let x_hov = cursor.is_some_and(|(cx, cy)| x_rect.contains(cx, cy));
                    draw_abort_x_button(fb, x_rect, x_hov, pal);
                }
                right_edge = x_rect.x.saturating_sub(4);
            }
            if paint {
                draw_text_fit(
                    fb,
                    &format!("{} {cap}", target.title),
                    tx,
                    cy,
                    m.micro_bold,
                    0,
                    right_edge.saturating_sub(tx),
                    if is_autoboot {
                        pal.accent
                    } else if is_selected {
                        pal.focus
                    } else {
                        pal.text
                    },
                );
            }
        }

        // Subtle live countdown progress stripe along the bottom of the flashing partition's label area.
        if paint
            && let Some(left_ms) = countdown_ms
            && is_selected
            && state.countdown_total_ms > 0
            && lr.w >= 48
        {
            let bar_w = lr.w.saturating_sub(16);
            let fill_w = (bar_w as u64 * left_ms as u64 / state.countdown_total_ms as u64) as u32;
            let bar_y = lr.y + lr.h.saturating_sub(4);
            fb.fill_rect(lr.x + 8, bar_y, bar_w, 2, pal.border);
            fb.fill_rect(lr.x + 8, bar_y, fill_w, 2, pal.accent);
        }

        return (sub_zones, abort_buttons);
    }

    if !paint {
        return (sub_zones, abort_buttons);
    }

    // Case 3: Non-bootable partition (ESP without standalone targets, MSR, Data, WinRE, Swap, /home).
    let lh = m.micro.line_height();
    let title = match part.kind {
        PartitionKind::Msr if w < 120 => "MSR",
        PartitionKind::Recovery if w < 110 => "WinRE",
        PartitionKind::Esp if w < 100 => "EFI",
        _ => part.detected_os.as_deref().unwrap_or(part.label.as_str()),
    };
    let base_cap = format!("{cap}{scale_mark}");
    let with_fs = format!("{base_cap} · {fs}");
    let sub = if part.loader_count > 0 {
        let full = format!("{base_cap} · {fs} · {} EFI", part.loader_count);
        if measure_text(&full, m.micro, 0) <= w {
            full
        } else if measure_text(&with_fs, m.micro, 0) <= w {
            with_fs
        } else {
            base_cap
        }
    } else if let Some(free) = part.free_bytes {
        let full = format!("{base_cap} · {fs} · {} free", format_bytes(free));
        if measure_text(&full, m.micro, 0) <= w {
            full
        } else if measure_text(&with_fs, m.micro, 0) <= w {
            with_fs
        } else {
            base_cap
        }
    } else if measure_text(&with_fs, m.micro, 0) <= w {
        with_fs
    } else {
        base_cap
    };

    if lr.h >= lh * 2 + 12 {
        let ty = lr.y + lr.h.saturating_sub(lh * 2 + 3) / 2;
        let head = format!("p{} {title}", part.index);
        draw_text_fit(fb, &head, x0, ty, m.micro_bold, 0, w, pal.text);
        draw_text_fit(
            fb,
            &sub,
            x0,
            ty + lh + 3,
            m.micro,
            0,
            w,
            if part.loader_count > 0 {
                accent
            } else {
                pal.muted
            },
        );
    } else {
        let ty = lr.y + lr.h.saturating_sub(lh) / 2;
        draw_text_fit(
            fb,
            &format!("p{} {cap}{scale_mark}", part.index),
            x0,
            ty,
            m.micro,
            0,
            w,
            pal.text,
        );
    }

    (sub_zones, abort_buttons)
}

#[allow(clippy::too_many_arguments)]
fn draw_inline_target_pill(
    fb: &mut Framebuffer,
    r: Rect,
    target: &BootTargetView,
    selected: bool,
    countdown_ms: Option<u32>,
    cursor: Option<(u32, u32)>,
    pal: &Palette,
    m: Metrics,
    paint: bool,
) -> Option<Rect> {
    let abort_rect = countdown_ms.map(|_| {
        let sz = r.h.saturating_sub(6).clamp(14, 20);
        Rect {
            x: r.x + r.w.saturating_sub(sz + 4),
            y: r.y + r.h.saturating_sub(sz) / 2,
            w: sz,
            h: sz,
        }
    });
    if !paint {
        return abort_rect;
    }

    let pulse = countdown_ms.map(autoboot_pulse);
    let fill = if let Some(p) = pulse {
        pal.surface_selected.mix(pal.accent, p * 75 / 255, 255)
    } else if selected {
        pal.surface_selected
    } else {
        pal.surface
    };
    let border = if let Some(p) = pulse {
        pal.focus.mix(pal.accent, p, 255)
    } else if selected {
        pal.focus
    } else if !target.healthy {
        pal.danger
    } else {
        pal.border_strong
    };
    fb.round_panel(
        r.x,
        r.y,
        r.w,
        r.h,
        5,
        if selected { 2 } else { 1 },
        fill,
        border,
    );

    let icon_sz = r.h.saturating_sub(8).clamp(12, 20);
    let ix = r.x + 5;
    let iy = r.y + r.h.saturating_sub(icon_sz) / 2;
    let badge_bg = if !target.healthy {
        pal.danger
    } else if target.is_recovery {
        pal.part_rec
    } else if target.hint == "windows" {
        pal.part_win
    } else {
        pal.part_linux
    };
    draw_os_emblem(
        fb,
        &target.hint,
        &target.title,
        ix,
        iy,
        icon_sz,
        badge_bg,
        pal.on_accent,
    );

    let right_limit = abort_rect
        .map(|ar| ar.x.saturating_sub(4))
        .unwrap_or(r.x + r.w.saturating_sub(5));
    let tx = ix + icon_sz + 6;
    let tw = right_limit.saturating_sub(tx);
    let prefix = if selected { "> " } else { "" };
    let tcol = if countdown_ms.is_some() {
        pal.accent
    } else if selected {
        pal.focus
    } else if !target.healthy {
        pal.danger
    } else {
        pal.text
    };
    let lh = m.micro.line_height();
    if r.h >= lh * 2 + 8 {
        let ty = r.y + r.h.saturating_sub(lh * 2 + 2) / 2;
        draw_text_fit(
            fb,
            &format!("{prefix}{}", target.title),
            tx,
            ty,
            m.micro_bold,
            0,
            tw,
            tcol,
        );
        let sub = match target.esp_part {
            Some(esp) => format!("EFI:p{esp} · {}", target.loader_file),
            None => target.loader_file.clone(),
        };
        draw_text_fit(fb, &sub, tx, ty + lh + 2, m.micro, 0, tw, pal.muted);
    } else {
        let ty = r.y + r.h.saturating_sub(lh) / 2;
        draw_text_fit(
            fb,
            &format!("{prefix}{}", target.title),
            tx,
            ty,
            m.micro_bold,
            0,
            tw,
            tcol,
        );
    }

    if let Some(ar) = abort_rect {
        let hov = cursor.is_some_and(|(cx, cy)| ar.contains(cx, cy));
        draw_abort_x_button(fb, ar, hov, pal);
    }
    abort_rect
}

/// Draw a crisp clickable `[×]` button for aborting autoboot.
fn draw_abort_x_button(fb: &mut Framebuffer, r: Rect, hovered: bool, pal: &Palette) {
    let fill = if hovered {
        pal.danger
    } else {
        pal.surface.mix(pal.danger, 48, 255)
    };
    let border = if hovered { pal.text } else { pal.danger };
    let fg = if hovered { pal.on_accent } else { pal.danger };
    fb.round_panel(r.x, r.y, r.w, r.h, 5, 1, fill, border);
    let sz = r.w.min(r.h);
    draw_close_x(
        fb,
        r.x + r.w.saturating_sub(sz) / 2,
        r.y + r.h.saturating_sub(sz) / 2,
        sz,
        fg,
    );
}

fn draw_hover_lba_bar(
    fb: &mut Framebuffer,
    r: Rect,
    dev: &StorageDevice,
    reg: &DiskRegion,
    pal: &Palette,
    m: Metrics,
) {
    fb.round_panel(r.x, r.y, r.w, r.h, 6, 1, pal.surface_alt, pal.border_strong);
    let ix = r.x + 10;
    let iw = r.w.saturating_sub(20);
    let lh = m.micro.line_height();
    let (line1, line2, col2) = match &reg.kind {
        RegionKind::Unallocated => (
            format!(
                "INSPECT: UNALLOCATED GAP  ·  LBA {}..{}  ·  {} ({} sectors × {}B)",
                reg.start_lba,
                reg.end_lba,
                format_bytes(reg.size_bytes),
                reg.end_lba.saturating_sub(reg.start_lba),
                dev.sector_size,
            ),
            "Non-bootable free space preserved in exact LBA order".into(),
            pal.muted,
        ),
        RegionKind::Partition(p) => {
            let fs = p.fs.as_deref().unwrap_or(part_short_tag(p));
            let line1 = format!(
                "INSPECT: {}p{} {}  ·  {} ({fs})  ·  LBA {}..{}",
                dev.device_name,
                p.index,
                p.label,
                format_bytes(reg.size_bytes),
                reg.start_lba,
                reg.end_lba,
            );
            let line2 = if !p.boot_targets.is_empty() {
                let names: Vec<&str> = p.boot_targets.iter().map(|t| t.title.as_str()).collect();
                format!(
                    "BOOT TARGETS: {}  ·  Click to start countdown · Click again to boot now",
                    names.join(", ")
                )
            } else if p.loader_count > 0 {
                format!(
                    "EFI SYSTEM PARTITION  ·  Hosts {} EFI bootloaders",
                    p.loader_count
                )
            } else if let Some(free) = p.free_bytes {
                format!("Non-bootable partition  ·  {} free", format_bytes(free))
            } else {
                "Non-bootable partition preserved in exact LBA order".into()
            };
            let col2 = if !p.boot_targets.is_empty() {
                pal.focus
            } else {
                pal.muted
            };
            (line1, line2, col2)
        }
    };
    draw_text_fit(fb, &line1, ix, r.y + 4, m.micro_bold, 0, iw, pal.text);
    draw_text_fit(fb, &line2, ix, r.y + 4 + lh + 3, m.micro, 0, iw, col2);
}

#[allow(clippy::too_many_arguments)]
fn draw_provenance_bar(
    fb: &mut Framebuffer,
    r: Rect,
    dev: &StorageDevice,
    reg: &DiskRegion,
    target: &BootTargetView,
    counting_down: bool,
    pal: &Palette,
    m: Metrics,
) {
    fb.round_panel(r.x, r.y, r.w, r.h, 6, 1, pal.surface_alt, pal.border);
    let ix = r.x + 10;
    let iw = r.w.saturating_sub(20);
    let lh = m.micro.line_height();
    let line1 = match (target.esp_part, target.os_part) {
        (Some(esp), Some(os)) if esp != os => format!(
            "SELECTED: {}  ·  EFI: {}p{} ({}) -> OS: {}p{}  ·  LBA {}..{} ({})",
            target.title,
            dev.device_name,
            esp,
            target.loader_file,
            dev.device_name,
            os,
            reg.start_lba,
            reg.end_lba,
            format_bytes(reg.size_bytes),
        ),
        (Some(esp), _) => format!(
            "SELECTED: {}  ·  EFI: {}p{} ({})  ·  LBA {}..{} ({})",
            target.title,
            dev.device_name,
            esp,
            target.loader_file,
            reg.start_lba,
            reg.end_lba,
            format_bytes(reg.size_bytes),
        ),
        (None, _) => format!(
            "SELECTED: {}  ·  {}  ·  LBA {}..{}",
            target.title, target.loader_file, reg.start_lba, reg.end_lba
        ),
    };
    draw_text_fit(fb, &line1, ix, r.y + 5, m.micro_bold, 0, iw, pal.text);
    let action_hint = if counting_down {
        "Enter / click again to boot now  ·  Esc / [X] to abort"
    } else {
        "Enter / click to start countdown  ·  Tab for power actions"
    };
    let line2 = match &target.note {
        Some(note) => format!("STATUS: {note}  ·  {action_hint}  ·  C for SBM Core"),
        None => format!("STATUS: READY  ·  {action_hint}"),
    };
    draw_text_fit(
        fb,
        &line2,
        ix,
        r.y + 5 + lh + 2,
        m.micro,
        0,
        iw,
        if target.healthy { pal.ok } else { pal.danger },
    );
}

// ============================================================================
// Header, Footer & Helpers
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn draw_header(
    fb: &mut Framebuffer,
    w: u32,
    h: u32,
    mode_tag: &str,
    spec: &SystemSpec,
    state: &MenuState,
    pal: &Palette,
    m: Metrics,
    paint: bool,
) -> Option<Rect> {
    let tag_h = m.micro.line_height() + 4;
    let tag_y = h.saturating_sub(tag_h) / 2;

    let abort_rect = state.countdown_ms.map(|_| {
        let sz = (tag_h + 2).clamp(18, 24);
        Rect {
            x: w.saturating_sub(m.pad + sz),
            y: h.saturating_sub(sz) / 2,
            w: sz,
            h: sz,
        }
    });

    if !paint {
        return abort_rect;
    }

    fb.fill_rect(0, 0, w, h, pal.header);
    fb.hline(0, h.saturating_sub(1), w, pal.border);

    let x = m.pad;
    let brand_style = if m.compact {
        TextStyle::BodyBold
    } else {
        m.hero
    };
    let by = h.saturating_sub(brand_style.line_height()) / 2;
    let bw = draw_text(fb, "SBM", x, by, brand_style, 1, pal.accent);

    let tag_x = x + bw + 10;
    let tag_w = draw_pill(fb, tag_x, tag_y, mode_tag, pal.focus, pal, m);

    let host_x = tag_x + tag_w + 10;
    let status = match state.countdown_ms {
        Some(ms) => format!("BOOT IN {}s", ms.div_ceil(1000)),
        None => spec
            .discovery_status
            .clone()
            .unwrap_or_else(|| "READY".into()),
    };
    let status_col = if let Some(ms) = state.countdown_ms {
        pal.accent.mix(pal.focus, autoboot_pulse(ms) / 3, 255)
    } else if spec.discovery_status.is_some() {
        pal.warn
    } else {
        pal.ok
    };
    let status_w = measure_text(&status, m.micro_bold, 0) + 12;
    let status_right = abort_rect
        .map(|ar| ar.x.saturating_sub(5))
        .unwrap_or_else(|| w.saturating_sub(m.pad));
    let status_x = status_right.saturating_sub(status_w);

    if status_x > host_x + 40 {
        draw_text_fit(
            fb,
            &spec.host_label,
            host_x,
            h.saturating_sub(m.micro.line_height()) / 2,
            m.micro,
            0,
            status_x.saturating_sub(host_x + 12),
            pal.muted,
        );
    }

    draw_pill(fb, status_x, tag_y, &status, status_col, pal, m);

    if let Some(ar) = abort_rect {
        let hov = state.cursor.is_some_and(|(cx, cy)| ar.contains(cx, cy));
        draw_abort_x_button(fb, ar, hov, pal);
    }

    if let Some(left) = state.countdown_ms
        && state.countdown_total_ms > 0
    {
        let fill_w = (w as u64 * left as u64 / state.countdown_total_ms as u64) as u32;
        fb.fill_rect(0, h.saturating_sub(2), fill_w, 2, pal.accent);
    }

    abort_rect
}

fn draw_footer(
    fb: &mut Framebuffer,
    r: Rect,
    state: &MenuState,
    pal: &Palette,
    m: Metrics,
    paint: bool,
) -> Vec<Rect> {
    if paint {
        fb.fill_rect(r.x, r.y, r.w, r.h, pal.header);
        fb.hline(r.x, r.y, r.w, pal.border);
    }
    let btn_h = r.h.saturating_sub(10).clamp(20, 32);
    let btn_y = r.y + r.h.saturating_sub(btn_h) / 2;
    let btn_w = if m.compact {
        84
    } else if m.wide {
        126
    } else {
        104
    };
    let gap = 6u32;
    let total_w = btn_w * 3 + gap * 2;
    let start_x = r.x + r.w.saturating_sub(m.pad + total_w);

    let hint = if m.compact {
        "1-4/F4 layout · F2 scen · F6 disc · T theme · F1 help"
    } else {
        "1-4/F4 layout · F2 scenario · F6 discovery · T theme · C Core · F1 help"
    };

    if paint {
        let hint_w = start_x.saturating_sub(r.x + m.pad + 10);
        draw_text_fit(
            fb,
            hint,
            r.x + m.pad,
            r.y + r.h.saturating_sub(m.micro.line_height()) / 2,
            m.micro,
            0,
            hint_w,
            pal.muted,
        );
    }

    let mut actions = Vec::with_capacity(3);
    for (idx, label) in ACTIONS.iter().enumerate() {
        let bx = start_x + (idx as u32) * (btn_w + gap);
        let br = Rect {
            x: bx,
            y: btn_y,
            w: btn_w,
            h: btn_h,
        };
        actions.push(br);
        if paint {
            let selected = state.focus == Focus::Actions && state.action == idx;
            let fill = if selected {
                pal.surface_selected
            } else {
                pal.surface_alt
            };
            let border = if selected { pal.focus } else { pal.border };
            let text_col = if idx == 2 {
                pal.danger
            } else if selected {
                pal.focus
            } else {
                pal.text
            };
            fb.round_panel(
                br.x,
                br.y,
                br.w,
                br.h,
                6,
                if selected { 2 } else { 1 },
                fill,
                border,
            );
            let fit = fit_mono(label, m.micro_bold, 0, br.w.saturating_sub(10));
            let lw = measure_text(&fit, m.micro_bold, 0);
            draw_text(
                fb,
                &fit,
                br.x + br.w.saturating_sub(lw) / 2,
                br.y + br.h.saturating_sub(m.micro.line_height()) / 2,
                m.micro_bold,
                0,
                text_col,
            );
        }
    }
    actions
}

fn draw_eyebrow(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    max_w: u32,
    text: &str,
    color: Rgba,
    m: Metrics,
) {
    draw_text_fit(fb, text, x, y, m.micro_bold, 1, max_w, color);
}

fn draw_pill(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    label: &str,
    color: Rgba,
    pal: &Palette,
    m: Metrics,
) -> u32 {
    let w = measure_text(label, m.micro_bold, 0) + 10;
    let h = m.micro.line_height() + 4;
    fb.round_panel(x, y, w, h, 4, 1, pal.surface_alt, color);
    draw_text(fb, label, x + 5, y + 2, m.micro_bold, 0, color);
    w
}

fn draw_badge_solid(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    label: &str,
    bg: Rgba,
    fg: Rgba,
    m: Metrics,
) -> u32 {
    let w = measure_text(label, m.micro_bold, 0) + 10;
    let h = m.micro.line_height() + 4;
    fb.round_rect(x, y, w, h, 4, bg);
    draw_text(fb, label, x + 5, y + 2, m.micro_bold, 0, fg);
    w
}

fn draw_cursor(fb: &mut Framebuffer, x: u32, y: u32, color: Rgba) {
    fb.fill_rect(x.saturating_sub(1), y, 2, 14, color);
    fb.fill_rect(x, y.saturating_sub(1), 10, 2, color);
}

fn bus_color(bus: &str, removable: bool, pal: &Palette) -> Rgba {
    if removable || bus.eq_ignore_ascii_case("usb") {
        pal.part_enc
    } else if bus.eq_ignore_ascii_case("nvme") {
        pal.accent
    } else {
        pal.part_rec
    }
}

fn part_short_tag(part: &PartitionView) -> &'static str {
    if part.encrypted.is_some() {
        return "ENC";
    }
    match part.kind {
        PartitionKind::Esp => "ESP",
        PartitionKind::Windows => "WIN",
        PartitionKind::Linux => "LNX",
        PartitionKind::Luks => "LUKS",
        PartitionKind::Recovery => "REC",
        PartitionKind::Msr => "MSR",
        PartitionKind::Swap => "SWAP",
        PartitionKind::Data => "DATA",
        PartitionKind::Apfs => "APFS",
        PartitionKind::Other => "PART",
    }
}

fn format_mib(mib: u64) -> String {
    if mib >= 1024 && mib.is_multiple_of(1024) {
        format!("{} GiB", mib / 1024)
    } else if mib >= 1024 {
        format!("{}.{} GiB", mib / 1024, (mib % 1024) * 10 / 1024)
    } else {
        format!("{mib} MiB")
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    const TIB: u64 = GIB * 1024;
    if bytes >= TIB {
        format!("{}.{} TiB", bytes / TIB, (bytes % TIB) * 10 / TIB)
    } else if bytes >= GIB {
        format!("{} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{} KiB", bytes / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn format_sectors(sectors: u64) -> String {
    if sectors >= 1_000_000_000 {
        format!(
            "{}.{}G",
            sectors / 1_000_000_000,
            (sectors % 1_000_000_000) / 100_000_000
        )
    } else if sectors >= 1_000_000 {
        format!("{}M", sectors / 1_000_000)
    } else if sectors >= 1_000 {
        format!("{}K", sectors / 1_000)
    } else {
        format!("{sectors}")
    }
}

fn short_cpu_name(full: &str) -> String {
    full.replace("(R)", "")
        .replace("(TM)", "")
        .replace("Intel ", "")
        .replace("AMD ", "")
        .trim()
        .to_string()
}

fn short_gpu_name(full: &str) -> String {
    full.split(" (").next().unwrap_or(full).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_spec(pending: bool, unavailable: bool) -> SystemSpec {
        let cpu = if pending {
            ProbeState::Pending
        } else if unavailable {
            ProbeState::Unavailable
        } else {
            ProbeState::Ready(CpuSpec {
                model: Some("AMD Ryzen 9 9950X".into()),
                arch: "x86_64".into(),
                cores: Some(16),
            })
        };
        let memory = if pending {
            ProbeState::Pending
        } else if unavailable {
            ProbeState::Unavailable
        } else {
            ProbeState::Ready(MemorySpec { total_mib: 32768 })
        };
        let security = if pending {
            ProbeState::Pending
        } else if unavailable {
            ProbeState::Unavailable
        } else {
            ProbeState::Ready(SecuritySpec {
                secure_boot: SecureBootSummary::Enabled,
                tpm_present: Some(true),
            })
        };
        let esp_start = 2048u64;
        let esp_len = 512 * 2048;
        let win_start = esp_start + esp_len + 2048 * 1024; // 1 GiB unallocated gap!
        let win_len = 1_000 * 2_097_152;
        SystemSpec {
            host_label: "ThinkPad X1 Carbon".into(),
            cpu,
            memory,
            board: ProbeState::Ready(BoardSpec {
                vendor: Some("LENOVO".into()),
                product: Some("ThinkPad X1".into()),
            }),
            gpu: ProbeState::Ready(vec!["Intel Arc 140V".into()]),
            firmware: ProbeState::Ready(FirmwareSpec {
                vendor: Some("Lenovo (EDK II)".into()),
                version: "0.1.0".into(),
                arch: "x86_64".into(),
            }),
            display: DisplaySpec {
                width: 1280,
                height: 800,
            },
            security,
            network: ProbeState::Ready(vec![NicSpec {
                name: "wlan0".into(),
                wireless: true,
                up: true,
                address: Some("192.168.1.42/24".into()),
            }]),
            storage: ProbeState::Ready(vec![StorageDevice {
                designator: "NVME0".into(),
                device_name: "nvme0n1".into(),
                bus: "nvme".into(),
                removable: false,
                model: Some("Samsung SSD 990 PRO 2TB".into()),
                size_bytes: 2_000_000_000_000,
                sector_size: 512,
                total_sectors: 2_000_000_000_000 / 512,
                regions: vec![
                    DiskRegion {
                        start_lba: esp_start,
                        end_lba: esp_start + esp_len,
                        size_bytes: esp_len * 512,
                        kind: RegionKind::Partition(PartitionView {
                            index: 1,
                            label: "EFI".into(),
                            kind: PartitionKind::Esp,
                            fs: Some("FAT32".into()),
                            free_bytes: Some(318 * 1024 * 1024),
                            encrypted: None,
                            is_recovery: false,
                            detected_os: None,
                            loader_count: 2,
                            boot_targets: vec![BootTargetView {
                                card_index: 1,
                                title: "SBM Core Slot A".into(),
                                family: "Recovery".into(),
                                hint: "recovery".into(),
                                loader_file: "sbm-core.efi".into(),
                                esp_part: Some(1),
                                os_part: None,
                                healthy: true,
                                is_recovery: true,
                                note: None,
                            }],
                        }),
                    },
                    DiskRegion {
                        start_lba: esp_start + esp_len,
                        end_lba: win_start,
                        size_bytes: (win_start - (esp_start + esp_len)) * 512,
                        kind: RegionKind::Unallocated,
                    },
                    DiskRegion {
                        start_lba: win_start,
                        end_lba: win_start + win_len,
                        size_bytes: win_len * 512,
                        kind: RegionKind::Partition(PartitionView {
                            index: 2,
                            label: "Windows".into(),
                            kind: PartitionKind::Windows,
                            fs: Some("NTFS".into()),
                            free_bytes: None,
                            encrypted: Some("BitLocker".into()),
                            is_recovery: false,
                            detected_os: Some("Windows".into()),
                            loader_count: 0,
                            boot_targets: vec![BootTargetView {
                                card_index: 0,
                                title: "Windows".into(),
                                family: "Windows".into(),
                                hint: "windows".into(),
                                loader_file: "bootmgfw.efi".into(),
                                esp_part: Some(1),
                                os_part: Some(2),
                                healthy: true,
                                is_recovery: false,
                                note: Some("BITLOCKER".into()),
                            }],
                        }),
                    },
                ],
            }]),
            policy_note: Some("DEFAULT: WINDOWS".into()),
            discovery_status: None,
        }
    }

    #[test]
    fn all_experimental_layouts_render_and_hit_test_across_resolutions() {
        for (w, h) in [(800, 600), (1280, 800), (1920, 1080)] {
            for mode in LayoutMode::EXPERIMENTS {
                for dark in [true, false] {
                    let mut spec = sample_spec(false, false);
                    spec.display = DisplaySpec {
                        width: w,
                        height: h,
                    };
                    let state = MenuState::new(0, dark, Some(8_000));
                    let mut fb = Framebuffer::new(w, h);
                    let map = paint_experimental(&mut fb, mode, &spec, &state);
                    assert!(map.zones.len() >= 2, "{mode:?} {w}x{h}");
                    assert_eq!(map.actions.len(), 3, "{mode:?} {w}x{h}");
                    for zone in &map.zones {
                        assert!(!zone.segments.is_empty());
                        for rect in &zone.segments {
                            assert!(rect.w > 0 && rect.h > 0);
                            assert!(rect.x + rect.w <= w && rect.y + rect.h <= h);
                            assert_eq!(
                                map.hit_test(rect.x + rect.w / 2, rect.y + rect.h / 2),
                                Hit::Card(zone.card_index)
                            );
                        }
                    }
                    for (idx, rect) in map.actions.iter().enumerate() {
                        assert_eq!(
                            map.hit_test(rect.x + rect.w / 2, rect.y + rect.h / 2),
                            Hit::Action(idx)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn pending_and_unavailable_states_still_expose_boot_targets() {
        for (pending, unavailable) in [(true, false), (false, true)] {
            let spec = sample_spec(pending, unavailable);
            let state = MenuState::new(0, true, None);
            let map = layout_experimental(LayoutMode::Bento, 1280, 800, &spec, &state);
            assert!(map.zones.len() >= 2);
            assert_eq!(map.actions.len(), 3);
        }
    }
}
