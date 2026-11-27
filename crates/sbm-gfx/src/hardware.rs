//! Experimental hardware-bento, schematic, and storage-first layouts with
//! paragraph-wrapped 1D-LBA-to-2D orthogonal-polygon disk widgets.
//!
//! Built directly on the unified `no_std` + `alloc` [`Framebuffer`],
//! [`TextRenderer`] (Atkinson Hyperlegible Next & Mono), [`Theme`], and
//! `sbm_gfx::vector` CPU rasterizer shared with the production menu. Isolated
//! behind `experimental-layouts` so `sbm-app` keeps Classic as default until
//! a new dashboard direction is approved.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::disk_wrap::{WrapInput, WrappedRegion, hatch_polygon, layout_disk_wrap};
use crate::draw::{Framebuffer, Rgba};
use crate::layout::{Hit, Rect};
pub use crate::logo::LogoImage;
use crate::logo::draw_logo_image;
use crate::state::{Focus, MenuState};
use crate::theme::Theme;
use crate::typography::{FontMode, TextRenderer, TextRole, UiScale};
use crate::vector::{
    draw_vector_chevron, draw_vector_chevron_h, draw_vector_close_x, draw_vector_cpu_logo,
    draw_vector_emblem, draw_vector_gpu_logo, draw_vector_hamburger, draw_vector_lock,
    draw_vector_qr, fill_circle, fill_ortho_polygon, fill_rounded_panel, fill_rounded_rect,
    stroke_ortho_polygon,
};

/// Canonical project URL encoded in the Brand widget's scannable QR code.
pub const BRAND_QR_URL: &str = "https://g-AI.org/Smart-Boot-Manager";

/// How storage devices display their contents in experimental hardware dashboards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StorageDetail {
    /// Approachable default: clean rectangular OS cards inside each bootable
    /// drive (max 4 slots per drive, with a 4th hamburger overflow tile when
    /// a drive hosts more than 4 OS targets). Non-OS partitions and unallocated
    /// space are hidden.
    #[default]
    Compact,
    /// Technical partition topology: full LBA-ordered paragraph-wrapped
    /// orthogonal polygons for every partition and explicit hatched unallocated
    /// region across all drives.
    Detailed,
}

impl StorageDetail {
    pub const ALL: [Self; 2] = [Self::Compact, Self::Detailed];

    pub fn id(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Detailed => "detailed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Detailed => "Detailed",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::Compact => Self::Detailed,
            Self::Detailed => Self::Compact,
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "compact" | "simple" | "os" => Some(Self::Compact),
            "detailed" | "full" | "lba" | "partitions" => Some(Self::Detailed),
            _ => None,
        }
    }
}

/// View options passed to experimental dashboard layout & painting routines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExperimentalOptions {
    /// Horizontal scroll offset (pixels) for the top hardware widget strip.
    pub scroll_x: u32,
    /// Vertical scroll offset (pixels) for the storage workspace.
    pub scroll_y: u32,
    pub high_contrast: bool,
    pub detail: StorageDetail,
    /// Index in `spec.storage` of the drive whose Compact-mode hamburger
    /// overflow menu (`>4` OS targets) is currently open.
    pub overflow_drive: Option<usize>,
    /// Controlled by the `?` button: reveals optional helper text, keyboard
    /// shortcut hints, and verbose explanatory captions when `true`.
    pub show_hints: bool,
}

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
    /// Software version displayed in the Brand/QR widget (0SFS `YY.MM.DD#N` / `YY.MM.DD LTS`).
    pub sbm_version: String,
    pub cpu: ProbeState<CpuSpec>,
    /// Optional decoded hardware badge cached for the detected CPU (`None` falls back to vector icon).
    pub cpu_logo: Option<LogoImage>,
    pub memory: ProbeState<MemorySpec>,
    pub board: ProbeState<BoardSpec>,
    pub gpu: ProbeState<Vec<String>>,
    /// Optional decoded hardware badge cached for the primary GPU (`None` falls back to vector icon).
    pub gpu_logo: Option<LogoImage>,
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
/// autoboot abort `[X]` buttons, Compact/Detailed toggle, overflow menu, and
/// drive-list scrollbar controls).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExperimentalHit {
    None,
    Card(usize),
    Action(usize),
    /// Clicking either the storage-bar `[X]` or the flashing target's `[X]`.
    AbortAutoboot,
    /// Clicking the vertical storage scrollbar up/down arrow buttons (signed pixel delta).
    ScrollDelta(i32),
    /// Clicking on the vertical storage scrollbar track to jump to a scroll offset.
    ScrollTo(u32),
    /// Clicking the horizontal top-grid scrollbar left/right arrow buttons (signed pixel delta).
    ScrollXDelta(i32),
    /// Clicking on the horizontal top-grid scrollbar track to jump to a scroll offset.
    ScrollXTo(u32),
    /// Clicking the `[ Compact | Detailed ]` segmented toggle above the storage section.
    SetDetail(StorageDetail),
    /// Clicking the `[ ? ]` toggle button to show/hide optional helper text.
    ToggleHints,
    /// Clicking the 4th-slot hamburger overflow tile on a drive with >4 OS targets.
    ToggleOverflow(usize),
    /// Clicking the overflow popover's `[X]` button or clicking outside the popover.
    CloseOverflow,
}

/// Interactive hit regions produced by any layout so mouse + keyboard stay
/// wired to `MenuState` and `step()`.
#[derive(Clone, Debug, Default)]
pub struct LayoutHitMap {
    pub zones: Vec<CardHitZone>,
    pub actions: Vec<Rect>,
    /// Clickable `[X]` abort-autoboot buttons (storage bar countdown + flashing target).
    pub abort_buttons: Vec<Rect>,
    /// Clickable `[ Compact | Detailed ]` toggle segments right above the storage section.
    pub detail_compact: Option<Rect>,
    pub detail_detailed: Option<Rect>,
    /// Clickable `[ ? ]` button toggling optional helper/hint text.
    pub help_button: Option<Rect>,
    /// Clickable 4th-slot hamburger overflow tiles `(drive_index, rect)` in Compact mode.
    pub overflow_buttons: Vec<(usize, Rect)>,
    /// Open Compact-mode overflow popover geometry & interactive rows.
    pub overflow_panel: Option<Rect>,
    pub overflow_close: Option<Rect>,
    pub overflow_zones: Vec<CardHitZone>,
    /// Top hardware widget strip viewport & horizontal scrollbar geometry.
    pub hw_viewport: Option<Rect>,
    pub hw_scroll_left: Option<Rect>,
    pub hw_scroll_right: Option<Rect>,
    pub hw_scroll_track: Option<Rect>,
    pub hw_scroll_thumb: Option<Rect>,
    pub scroll_x: u32,
    pub max_scroll_x: u32,
    /// Storage workspace vertical scrollbar geometry.
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
        if let Some(panel) = self.overflow_panel {
            for zone in &self.overflow_zones {
                if zone.segments.iter().any(|s| s.contains(x, y)) {
                    return Hit::Card(zone.card_index);
                }
            }
            if panel.contains(x, y) {
                return Hit::None;
            }
        }
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

    /// Full hit test including `[X]` autoboot abort buttons, Compact/Detailed
    /// toggle, overflow menu, and scrollbar controls.
    pub fn hit_test_experimental(&self, x: u32, y: u32) -> ExperimentalHit {
        if let Some(panel) = self.overflow_panel {
            if self.overflow_close.is_some_and(|r| r.contains(x, y)) {
                return ExperimentalHit::CloseOverflow;
            }
            if self
                .abort_buttons
                .iter()
                .any(|r| panel.contains(r.x, r.y) && r.contains(x, y))
            {
                return ExperimentalHit::AbortAutoboot;
            }
            for zone in &self.overflow_zones {
                if zone.segments.iter().any(|s| s.contains(x, y)) {
                    return ExperimentalHit::Card(zone.card_index);
                }
            }
            if panel.contains(x, y) {
                return ExperimentalHit::None;
            }
            if self.detail_compact.is_some_and(|r| r.contains(x, y)) {
                return ExperimentalHit::SetDetail(StorageDetail::Compact);
            }
            if self.detail_detailed.is_some_and(|r| r.contains(x, y)) {
                return ExperimentalHit::SetDetail(StorageDetail::Detailed);
            }
            if self.help_button.is_some_and(|r| r.contains(x, y)) {
                return ExperimentalHit::ToggleHints;
            }
            return ExperimentalHit::CloseOverflow;
        }
        // Abort `[X]` buttons always take highest priority so clicking `[X]` on
        // the flashing target aborts autoboot instead of launching it.
        if self.abort_buttons.iter().any(|r| r.contains(x, y)) {
            return ExperimentalHit::AbortAutoboot;
        }
        if self.help_button.is_some_and(|r| r.contains(x, y)) {
            return ExperimentalHit::ToggleHints;
        }
        if self.detail_compact.is_some_and(|r| r.contains(x, y)) {
            return ExperimentalHit::SetDetail(StorageDetail::Compact);
        }
        if self.detail_detailed.is_some_and(|r| r.contains(x, y)) {
            return ExperimentalHit::SetDetail(StorageDetail::Detailed);
        }
        for &(drive_idx, rect) in &self.overflow_buttons {
            if rect.contains(x, y) {
                return ExperimentalHit::ToggleOverflow(drive_idx);
            }
        }
        if self.hw_scroll_left.is_some_and(|r| r.contains(x, y)) {
            return ExperimentalHit::ScrollXDelta(-160);
        }
        if self.hw_scroll_right.is_some_and(|r| r.contains(x, y)) {
            return ExperimentalHit::ScrollXDelta(160);
        }
        if let Some(track) = self.hw_scroll_track
            && track.contains(x, y)
            && self.max_scroll_x > 0
        {
            let thumb_w = self.hw_scroll_thumb.map(|t| t.w).unwrap_or(28);
            let travel = track.w.saturating_sub(thumb_w).max(1);
            let rel = x.saturating_sub(track.x + thumb_w / 2).min(travel);
            let target = ((rel as u64 * self.max_scroll_x as u64) / travel as u64) as u32;
            return ExperimentalHit::ScrollXTo(target.min(self.max_scroll_x));
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
    high_contrast: bool,
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
    fn resolve(dark: bool, high_contrast: bool) -> Self {
        let base = Theme::resolve(dark, high_contrast);
        if high_contrast {
            if dark {
                Self {
                    high_contrast: true,
                    bg: base.bg,
                    header: base.header,
                    surface: base.card,
                    surface_alt: Rgba::rgb(0x11, 0x16, 0x1F),
                    surface_hover: Rgba::rgb(0x18, 0x24, 0x38),
                    surface_selected: base.card_selected,
                    unalloc_bg: Rgba::rgb(0x05, 0x07, 0x0A),
                    unalloc_hatch: Rgba::rgb(0x46, 0x52, 0x62),
                    tint_cpu: base.card,
                    tint_mem: base.card,
                    tint_gpu: base.card,
                    tint_net: base.card,
                    tint_sec: base.card,
                    border: base.border,
                    border_strong: Rgba::rgb(0xFF, 0xFF, 0xFF),
                    grid_dot: Rgba::rgb(0x36, 0x42, 0x52),
                    text: base.text,
                    muted: base.muted,
                    eyebrow: Rgba::rgb(0xE6, 0xEE, 0xF8),
                    accent: Rgba::rgb(0xFF, 0xD1, 0x66),
                    focus: base.accent,
                    ok: base.ok,
                    warn: Rgba::rgb(0xFF, 0xCC, 0x4D),
                    danger: base.danger,
                    part_esp: Rgba::rgb(0xFF, 0xD1, 0x66),
                    part_win: Rgba::rgb(0x7A, 0xB8, 0xFF),
                    part_linux: Rgba::rgb(0x5E, 0xF2, 0xB0),
                    part_enc: Rgba::rgb(0xDF, 0xA8, 0xFF),
                    part_rec: Rgba::rgb(0x6E, 0xE2, 0xFF),
                    part_data: Rgba::rgb(0xB8, 0xC4, 0xD4),
                    part_dim: Rgba::rgb(0x98, 0xA6, 0xB8),
                    on_accent: Rgba::rgb(0x00, 0x00, 0x00),
                }
            } else {
                Self {
                    high_contrast: true,
                    bg: base.bg,
                    header: base.header,
                    surface: base.card,
                    surface_alt: Rgba::rgb(0xF2, 0xF5, 0xF9),
                    surface_hover: Rgba::rgb(0xE2, 0xEC, 0xFC),
                    surface_selected: base.card_selected,
                    unalloc_bg: Rgba::rgb(0xF5, 0xF7, 0xFA),
                    unalloc_hatch: Rgba::rgb(0x6E, 0x7A, 0x88),
                    tint_cpu: base.card,
                    tint_mem: base.card,
                    tint_gpu: base.card,
                    tint_net: base.card,
                    tint_sec: base.card,
                    border: base.border,
                    border_strong: Rgba::rgb(0x00, 0x00, 0x00),
                    grid_dot: Rgba::rgb(0x9A, 0xA6, 0xB4),
                    text: base.text,
                    muted: base.muted,
                    eyebrow: Rgba::rgb(0x18, 0x20, 0x28),
                    accent: Rgba::rgb(0x8A, 0x4B, 0x00),
                    focus: base.accent,
                    ok: base.ok,
                    warn: Rgba::rgb(0x8A, 0x4B, 0x00),
                    danger: base.danger,
                    part_esp: Rgba::rgb(0x8A, 0x4B, 0x00),
                    part_win: Rgba::rgb(0x00, 0x3B, 0x99),
                    part_linux: Rgba::rgb(0x00, 0x5C, 0x36),
                    part_enc: Rgba::rgb(0x52, 0x1A, 0x9E),
                    part_rec: Rgba::rgb(0x00, 0x4E, 0x7C),
                    part_data: Rgba::rgb(0x2F, 0x39, 0x44),
                    part_dim: Rgba::rgb(0x49, 0x54, 0x62),
                    on_accent: Rgba::rgb(0xFF, 0xFF, 0xFF),
                }
            }
        } else if dark {
            Self {
                high_contrast: false,
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
                high_contrast: false,
                bg: Rgba::rgb(0xF1, 0xF3, 0xEF),
                header: base.header,
                surface: base.card,
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
                border: base.border,
                border_strong: Rgba::rgb(0x73, 0x84, 0x94),
                grid_dot: Rgba::rgb(0xD5, 0xDD, 0xE3),
                text: Rgba::rgb(0x10, 0x16, 0x1D),
                muted: base.muted,
                eyebrow: Rgba::rgb(0x43, 0x52, 0x60),
                accent: Rgba::rgb(0xB3, 0x72, 0x00),
                focus: base.accent,
                ok: base.ok,
                warn: Rgba::rgb(0xB4, 0x53, 0x09),
                danger: base.danger,
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

    fn border_px(&self) -> u32 {
        if self.high_contrast { 2 } else { 1 }
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
        if self.high_contrast {
            if selected {
                self.surface_selected
            } else if hovered {
                self.surface_hover
            } else {
                self.surface
            }
        } else if selected {
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

/// Responsive layout metrics derived directly from the active [`TextRenderer`]
/// and `UiScale` so experimental dashboards scale cleanly across 75%..=200%
/// and 800×600..=3840×2160.
#[derive(Clone, Copy)]
struct Metrics {
    cap_h: u32,
    tech_h: u32,
    body_h: u32,
    head_h: u32,
    footer_h: u32,
    pad: u32,
    gap: u32,
    radius: u32,
    scale_pct: u32,
    compact: bool,
    wide: bool,
}

impl Metrics {
    fn for_renderer(w: u32, h: u32, renderer: &TextRenderer<'_>) -> Self {
        let cap_h = renderer.line_metrics(TextRole::Caption).line_height;
        let tech_h = renderer.line_metrics(TextRole::Technical).line_height;
        let body_h = renderer.line_metrics(TextRole::Body).line_height;
        let head_h = renderer.line_metrics(TextRole::Heading).line_height;
        let scale_pct = u32::from(renderer.scale().percent());
        let wide = w >= 1800 && h >= 980;
        let compact = w < 1000 || h < 680;
        let base_pad = if wide {
            14
        } else if compact {
            7
        } else {
            10
        };
        let base_gap = if wide {
            12
        } else if compact {
            6
        } else {
            8
        };
        let pad = (base_pad * scale_pct / 100).clamp(6, 22);
        let gap = (base_gap * scale_pct / 100).clamp(5, 18);
        let radius = if wide {
            12
        } else if compact {
            8
        } else {
            10
        };
        let footer_h = body_h.max(cap_h + 6).saturating_add(16).clamp(34, 80);
        Self {
            cap_h,
            tech_h,
            body_h,
            head_h,
            footer_h,
            pad,
            gap,
            radius,
            scale_pct,
            compact,
            wide,
        }
    }

    fn scaled(&self, base: u32) -> u32 {
        (base * self.scale_pct / 100).max(1)
    }
}

/// Compute hit-map without rendering pixels (default typography/contrast, Compact view).
pub fn layout_experimental(
    mode: LayoutMode,
    width: u32,
    height: u32,
    spec: &SystemSpec,
    state: &MenuState,
) -> LayoutHitMap {
    layout_experimental_scrolled(mode, width, height, spec, state, 0)
}

/// Compute hit-map at a specific vertical drive-list scroll offset (default typography/contrast, Compact view).
pub fn layout_experimental_scrolled(
    mode: LayoutMode,
    width: u32,
    height: u32,
    spec: &SystemSpec,
    state: &MenuState,
    scroll_y: u32,
) -> LayoutHitMap {
    let mut renderer = TextRenderer::new(FontMode::AtkinsonNextMono, height, UiScale::default());
    layout_experimental_with(
        mode,
        width,
        height,
        spec,
        state,
        ExperimentalOptions {
            scroll_y,
            ..ExperimentalOptions::default()
        },
        &mut renderer,
    )
}

/// Compute hit-map honoring the active [`TextRenderer`] scale/font and [`ExperimentalOptions`].
#[allow(clippy::too_many_arguments)]
pub fn layout_experimental_with(
    mode: LayoutMode,
    width: u32,
    height: u32,
    spec: &SystemSpec,
    state: &MenuState,
    opts: ExperimentalOptions,
    renderer: &mut TextRenderer<'_>,
) -> LayoutHitMap {
    renderer.set_display_height(height);
    let mut dummy = Framebuffer::new(0, 0);
    render_mode(
        &mut dummy, width, height, mode, spec, state, opts, renderer, false,
    )
}

/// Render an experimental layout and return its interactive hit-map (default typography/contrast, Compact view).
pub fn paint_experimental(
    fb: &mut Framebuffer,
    mode: LayoutMode,
    spec: &SystemSpec,
    state: &MenuState,
) -> LayoutHitMap {
    paint_experimental_scrolled(fb, mode, spec, state, 0)
}

/// Render an experimental layout at a vertical scroll offset (default typography/contrast, Compact view).
pub fn paint_experimental_scrolled(
    fb: &mut Framebuffer,
    mode: LayoutMode,
    spec: &SystemSpec,
    state: &MenuState,
    scroll_y: u32,
) -> LayoutHitMap {
    let mut renderer = TextRenderer::new(FontMode::AtkinsonNextMono, fb.height, UiScale::default());
    paint_experimental_with(
        fb,
        mode,
        spec,
        state,
        ExperimentalOptions {
            scroll_y,
            ..ExperimentalOptions::default()
        },
        &mut renderer,
    )
}

/// Render an experimental layout using the unified [`TextRenderer`] and vector backend.
#[allow(clippy::too_many_arguments)]
pub fn paint_experimental_with(
    fb: &mut Framebuffer,
    mode: LayoutMode,
    spec: &SystemSpec,
    state: &MenuState,
    opts: ExperimentalOptions,
    renderer: &mut TextRenderer<'_>,
) -> LayoutHitMap {
    let (w, h) = (fb.width, fb.height);
    renderer.set_display_height(h);
    let map = render_mode(fb, w, h, mode, spec, state, opts, renderer, true);
    if let Some((x, y)) = state.cursor {
        let pal = Palette::resolve(state.dark, opts.high_contrast);
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
    opts: ExperimentalOptions,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> LayoutHitMap {
    let pal = Palette::resolve(state.dark, opts.high_contrast);
    let m = Metrics::for_renderer(w, h, renderer);
    let mut hit = match mode {
        LayoutMode::Bento | LayoutMode::Classic => {
            paint_bento(fb, w, h, spec, state, &pal, m, opts, renderer, paint)
        }
        LayoutMode::Schematic => {
            paint_schematic(fb, w, h, spec, state, &pal, m, opts, renderer, paint)
        }
        LayoutMode::Storage => {
            paint_storage_first(fb, w, h, spec, state, &pal, m, opts, renderer, paint)
        }
    };
    if opts.detail == StorageDetail::Compact
        && let Some(drive_idx) = opts.overflow_drive
    {
        draw_overflow_popover(
            fb, w, h, spec, drive_idx, state, opts, &pal, m, renderer, paint, &mut hit,
        );
    }
    hit
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

#[derive(Clone, Copy, Default)]
struct HwStripHit {
    viewport: Option<Rect>,
    scroll_left: Option<Rect>,
    scroll_right: Option<Rect>,
    scroll_track: Option<Rect>,
    scroll_thumb: Option<Rect>,
    scroll_x: u32,
    max_scroll_x: u32,
}

impl HwStripHit {
    fn apply(self, hit: &mut LayoutHitMap) {
        hit.hw_viewport = self.viewport;
        hit.hw_scroll_left = self.scroll_left;
        hit.hw_scroll_right = self.scroll_right;
        hit.hw_scroll_track = self.scroll_track;
        hit.hw_scroll_thumb = self.scroll_thumb;
        hit.scroll_x = self.scroll_x;
        hit.max_scroll_x = self.max_scroll_x;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_h_scrollbar(
    fb: &mut Framebuffer,
    bar: Rect,
    viewport: Rect,
    content_w: u32,
    eff_scroll_x: u32,
    max_scroll_x: u32,
    state: &MenuState,
    pal: &Palette,
    paint: bool,
    hw_hit: &mut HwStripHit,
) {
    let btn_w = bar.h.clamp(14, 20);
    let left_rect = Rect {
        x: bar.x,
        y: bar.y,
        w: btn_w,
        h: bar.h,
    };
    let right_rect = Rect {
        x: bar.x + bar.w.saturating_sub(btn_w),
        y: bar.y,
        w: btn_w,
        h: bar.h,
    };
    let track_x = left_rect.x + left_rect.w + 3;
    let track_w = right_rect.x.saturating_sub(track_x + 3).max(20);
    let track_rect = Rect {
        x: track_x,
        y: bar.y,
        w: track_w,
        h: bar.h,
    };
    let thumb_w = ((track_w as u64 * viewport.w as u64) / content_w.max(1) as u64)
        .clamp(28u64.min(track_w as u64), track_w as u64) as u32;
    let travel = track_w.saturating_sub(thumb_w);
    let thumb_x = track_x
        + if max_scroll_x > 0 {
            ((travel as u64 * eff_scroll_x as u64) / max_scroll_x as u64) as u32
        } else {
            0
        };
    let thumb_rect = Rect {
        x: thumb_x,
        y: bar.y + 2,
        w: thumb_w,
        h: bar.h.saturating_sub(4).max(6),
    };

    hw_hit.scroll_left = Some(left_rect);
    hw_hit.scroll_right = Some(right_rect);
    hw_hit.scroll_track = Some(track_rect);
    hw_hit.scroll_thumb = Some(thumb_rect);

    if !paint {
        return;
    }

    let l_hov = state.cursor.is_some_and(|(x, y)| left_rect.contains(x, y));
    let r_hov = state.cursor.is_some_and(|(x, y)| right_rect.contains(x, y));
    let t_hov = state.cursor.is_some_and(|(x, y)| thumb_rect.contains(x, y));
    let bp = pal.border_px();

    draw_bento_panel(fb, track_rect, pal.surface_alt, pal.border, 5, bp);
    draw_bento_panel(
        fb,
        left_rect,
        if l_hov {
            pal.surface_hover
        } else {
            pal.surface
        },
        if eff_scroll_x > 0 {
            pal.border_strong
        } else {
            pal.border
        },
        4,
        bp,
    );
    draw_vector_chevron_h(
        fb,
        left_rect.x + 2,
        left_rect.y + 2,
        left_rect.h.saturating_sub(4),
        true,
        if eff_scroll_x > 0 {
            if l_hov { pal.focus } else { pal.text }
        } else {
            pal.border
        },
    );
    draw_bento_panel(
        fb,
        right_rect,
        if r_hov {
            pal.surface_hover
        } else {
            pal.surface
        },
        if eff_scroll_x < max_scroll_x {
            pal.border_strong
        } else {
            pal.border
        },
        4,
        bp,
    );
    draw_vector_chevron_h(
        fb,
        right_rect.x + 2,
        right_rect.y + 2,
        right_rect.h.saturating_sub(4),
        false,
        if eff_scroll_x < max_scroll_x {
            if r_hov { pal.focus } else { pal.text }
        } else {
            pal.border
        },
    );
    let thumb_col = if t_hov { pal.focus } else { pal.border_strong };
    draw_rrect(fb, thumb_rect, 4, thumb_col);

    // Subtle edge overflow accent bars along left/right viewport edges.
    if eff_scroll_x > 0 {
        fb.vline(
            viewport.x,
            viewport.y + 8,
            viewport.h.saturating_sub(16),
            pal.focus,
        );
    }
    if eff_scroll_x < max_scroll_x {
        fb.vline(
            viewport.x + viewport.w.saturating_sub(1),
            viewport.y + 8,
            viewport.h.saturating_sub(16),
            pal.accent,
        );
    }
}

fn brand_tile_min_width(
    tile_h: u32,
    spec: &SystemSpec,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) -> u32 {
    let pad = m.pad.clamp(8, 14);
    let qr_sz = tile_h.saturating_sub(pad * 2).clamp(44, 88);
    let text_w = renderer
        .measure("Smart-Boot-Manager", TextRole::Body)
        .width
        .max(renderer.measure("g-AI.org/", TextRole::Micro).width)
        .max(
            renderer
                .measure(&spec.sbm_version, TextRole::Technical)
                .width,
        );
    qr_sz + pad * 2 + 12 + text_w + 8
}

#[allow(clippy::too_many_arguments)]
fn paint_bento(
    fb: &mut Framebuffer,
    w: u32,
    h: u32,
    spec: &SystemSpec,
    state: &MenuState,
    pal: &Palette,
    m: Metrics,
    opts: ExperimentalOptions,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> LayoutHitMap {
    if paint {
        fb.fill(pal.bg);
    }
    let footer_h = m.footer_h;
    let x0 = m.pad;
    let inner_w = w.saturating_sub(m.pad * 2);
    let y0 = (m.pad / 2).max(4);

    let min_tile_h = (m.cap_h + m.body_h + m.tech_h + 28).max(m.scaled(82));
    let max_tile_h = (h * 20 / 100).max(86);
    let tile_h = min_tile_h.min(max_tile_h).max(76);

    let (hw_hit, strip_total_h) = draw_bento_hw_grid(
        fb,
        Rect {
            x: x0,
            y: y0,
            w: inner_w,
            h: tile_h,
        },
        spec,
        state,
        opts,
        pal,
        m,
        renderer,
        paint,
    );
    let storage_y = y0.saturating_add(strip_total_h).saturating_add(m.gap);

    let footer_y = h.saturating_sub(footer_h);
    let storage_h = footer_y.saturating_sub(storage_y).saturating_sub(m.gap);
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
        StorageStyle::Bento,
        opts,
        renderer,
        paint,
    );
    hw_hit.apply(&mut hit);
    let (actions, help_btn) = draw_footer(
        fb,
        Rect {
            x: 0,
            y: footer_y,
            w,
            h: footer_h,
        },
        state,
        opts,
        pal,
        m,
        renderer,
        paint,
    );
    hit.actions = actions;
    hit.help_button = help_btn;
    hit
}

/// Horizontally scrollable top hardware strip: Brand/QR/Version + CPU + GPU + RAM +
/// Display/Firmware + Security + Network. Zero redundant category eyebrows.
#[allow(clippy::too_many_arguments)]
fn draw_bento_hw_grid(
    fb: &mut Framebuffer,
    area: Rect,
    spec: &SystemSpec,
    state: &MenuState,
    opts: ExperimentalOptions,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> (HwStripHit, u32) {
    let brand_w = brand_tile_min_width(area.h, spec, m, renderer);
    let base_widths: [u32; 7] = [
        brand_w,
        m.scaled(248).max(196),
        m.scaled(270).max(208),
        m.scaled(124).max(100),
        m.scaled(176).max(140),
        m.scaled(150).max(120),
        m.scaled(186).max(148),
    ];
    let total_gaps = m.gap.saturating_mul(6);
    let natural_w: u32 = base_widths.iter().sum::<u32>().saturating_add(total_gaps);
    let scrollable_x = natural_w > area.w;

    // Stretch tiles proportionally to fill wide screens cleanly when all 7 fit.
    let widths: [u32; 7] = if !scrollable_x && area.w > total_gaps {
        let avail = area.w.saturating_sub(total_gaps);
        let sum_base = base_widths.iter().sum::<u32>().max(1);
        let mut out = [0u32; 7];
        let mut used = 0u32;
        for i in 0..7 {
            if i == 6 {
                out[i] = avail.saturating_sub(used);
            } else {
                let w = ((avail as u64 * base_widths[i] as u64) / sum_base as u64) as u32;
                out[i] = w;
                used = used.saturating_add(w);
            }
        }
        out
    } else {
        base_widths
    };

    let content_w: u32 = widths.iter().sum::<u32>().saturating_add(total_gaps);
    let max_scroll_x = content_w.saturating_sub(area.w);
    let eff_scroll_x = opts.scroll_x.min(max_scroll_x);

    let mut hw_hit = HwStripHit {
        viewport: Some(area),
        scroll_x: eff_scroll_x,
        max_scroll_x,
        ..HwStripHit::default()
    };

    if paint {
        fb.set_viewport_xy(area.x, area.y, area.w, area.h, -(eff_scroll_x as i32), 0);
        let mut x = area.x;
        for (idx, &tw) in widths.iter().enumerate() {
            let r = Rect {
                x,
                y: area.y,
                w: tw,
                h: area.h,
            };
            // Skip tiles completely outside the horizontal viewport.
            let screen_x0 = (r.x as i64) - (eff_scroll_x as i64);
            let screen_x1 = screen_x0 + (r.w as i64);
            if screen_x1 > area.x as i64 && screen_x0 < (area.x + area.w) as i64 {
                match idx {
                    0 => draw_brand_tile(fb, r, spec, pal, m, renderer),
                    1 => draw_cpu_tile(fb, r, spec, pal, m, renderer),
                    2 => draw_gpu_tile(fb, r, spec, pal, m, renderer),
                    3 => draw_memory_tile(fb, r, spec, pal, m, renderer),
                    4 => draw_board_fw_tile(fb, r, spec, pal, m, renderer),
                    5 => draw_security_tile(fb, r, spec, pal, m, renderer),
                    _ => draw_network_tile(fb, r, spec, pal, m, renderer),
                }
            }
            x = x.saturating_add(tw).saturating_add(m.gap);
        }
        fb.clear_viewport();
    }

    let total_h = if scrollable_x {
        let sb_h = if m.compact { 12 } else { 14 };
        let sb_gap = 4u32;
        let sb_rect = Rect {
            x: area.x,
            y: area.y.saturating_add(area.h).saturating_add(sb_gap),
            w: area.w,
            h: sb_h,
        };
        draw_h_scrollbar(
            fb,
            sb_rect,
            area,
            content_w,
            eff_scroll_x,
            max_scroll_x,
            state,
            pal,
            paint,
            &mut hw_hit,
        );
        area.h.saturating_add(sb_gap).saturating_add(sb_h)
    } else {
        area.h
    };

    (hw_hit, total_h)
}

fn draw_bento_panel(
    fb: &mut Framebuffer,
    r: Rect,
    fill: Rgba,
    border: Rgba,
    radius: u32,
    border_px: u32,
) {
    fill_rounded_panel(fb, r.x, r.y, r.w, r.h, radius, border_px, fill, border);
}

fn draw_rrect(fb: &mut Framebuffer, r: Rect, radius: u32, fill: Rgba) {
    fill_rounded_rect(fb, r.x, r.y, r.w, r.h, radius, fill);
}

/// Brand identity widget replacing the old top bar: scannable QR code linking to
/// `https://g-AI.org/Smart-Boot-Manager`, `g-AI.org/` (small gray),
/// `Smart-Boot-Manager` (normal white/ink), and the 0SFS software version.
fn draw_brand_tile(
    fb: &mut Framebuffer,
    r: Rect,
    spec: &SystemSpec,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) {
    draw_bento_panel(
        fb,
        r,
        pal.header,
        pal.border_strong,
        m.radius,
        pal.border_px(),
    );
    let pad = m.pad.clamp(7, 12);
    let qr_sz = r.h.saturating_sub(pad * 2).clamp(44, 88);
    let qx = r.x + pad;
    let qy = r.y + r.h.saturating_sub(qr_sz) / 2;
    let qr_drawn = draw_vector_qr(
        fb,
        qx,
        qy,
        qr_sz,
        BRAND_QR_URL,
        Rgba::rgb(0xFF, 0xFF, 0xFF),
        Rgba::rgb(0x0B, 0x10, 0x14),
    );

    let tx = if qr_drawn { qx + qr_sz + 10 } else { r.x + pad };
    let tw = r.x.saturating_add(r.w).saturating_sub(tx + pad);
    if tw < 24 {
        return;
    }

    let micro_h = renderer.line_metrics(TextRole::Micro).line_height;
    let top_pad = (pad / 2).max(3);

    // Half-size gray domain prefix pinned close to the top edge of the tile
    let micro_y = r.y + top_pad;
    renderer.draw_truncated(fb, "g-AI.org/", tx, micro_y, tw, TextRole::Micro, pal.muted);

    // Center the product name + 0SFS version cleanly in the space below `g-AI.org/`
    let body_area_y = micro_y.saturating_add(micro_h).saturating_add(2);
    let body_area_h =
        r.y.saturating_add(r.h)
            .saturating_sub(top_pad)
            .saturating_sub(body_area_y);
    let gap2 = 5u32;
    let main_h = m.body_h.saturating_add(gap2).saturating_add(m.tech_h);
    let mut y = body_area_y.saturating_add(body_area_h.saturating_sub(main_h) / 2);

    // Normal size white (high-contrast text) product name: `Smart-Boot-Manager`
    renderer.draw_truncated(
        fb,
        "Smart-Boot-Manager",
        tx,
        y,
        tw,
        TextRole::Body,
        pal.text,
    );
    y += m.body_h + gap2;

    // 0SFS software version (`YY.MM.DD#N` / `YY.MM.DD LTS`), plus transient probing badge if active
    let ver = if spec.sbm_version.is_empty() {
        match &spec.firmware {
            ProbeState::Ready(fw) => fw.version.as_str(),
            _ => "",
        }
    } else {
        spec.sbm_version.as_str()
    };
    if let Some(status) = spec
        .discovery_status
        .as_deref()
        .filter(|s| s.starts_with("PROBING"))
    {
        let ver_w = renderer.measure(ver, TextRole::Technical).width;
        renderer.draw_truncated(fb, ver, tx, y, tw, TextRole::Technical, pal.accent);
        let sx = tx + ver_w + 8;
        let rem = tx.saturating_add(tw).saturating_sub(sx);
        if rem > 28 {
            renderer.draw_truncated(fb, status, sx, y, rem, TextRole::Caption, pal.warn);
        }
    } else {
        renderer.draw_truncated(fb, ver, tx, y, tw, TextRole::Technical, pal.accent);
    }
}

fn draw_cpu_tile(
    fb: &mut Framebuffer,
    r: Rect,
    spec: &SystemSpec,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) {
    draw_bento_panel(fb, r, pal.tint_cpu, pal.border, m.radius, pal.border_px());
    let pad = m.pad.clamp(7, 12);
    let lx = r.x + pad;
    let logo_sz = if let (ProbeState::Ready(_), Some(logo)) = (&spec.cpu, spec.cpu_logo.as_ref()) {
        let sz = r.h.saturating_sub(pad * 2).clamp(44, 68);
        let ly = r.y + r.h.saturating_sub(sz) / 2;
        if !draw_logo_image(fb, logo, lx, ly, sz, sz, 6) {
            draw_vector_cpu_logo(fb, lx, ly, sz, pal.accent, pal.on_accent);
        }
        sz
    } else {
        let sz = r.h.saturating_sub(pad * 2 + 12).clamp(26, 44);
        let ly = r.y + r.h.saturating_sub(sz) / 2;
        draw_vector_cpu_logo(fb, lx, ly, sz, pal.accent, pal.on_accent);
        sz
    };

    let tx = lx + logo_sz + 10;
    let tw = r.x.saturating_add(r.w).saturating_sub(tx + pad);
    if tw < 24 {
        return;
    }

    match &spec.cpu {
        ProbeState::Pending => {
            let py = r.y + r.h.saturating_sub(m.cap_h + 6) / 2;
            draw_pill(fb, tx, py, "PROBING..", pal.warn, pal, m, renderer);
        }
        ProbeState::Unavailable => {
            let py = r.y + r.h.saturating_sub(m.body_h) / 2;
            renderer.draw_truncated(fb, "CPU n/a", tx, py, tw, TextRole::Body, pal.muted);
        }
        ProbeState::Ready(cpu) => {
            let hero = match cpu.cores {
                Some(c) => format!("{c}C · {}", cpu.arch.to_ascii_uppercase()),
                None => cpu.arch.to_ascii_uppercase(),
            };
            let model = cpu.model.as_deref().unwrap_or("Model n/a");
            let model_col = if cpu.model.is_some() {
                pal.text
            } else {
                pal.muted
            };
            let board = match &spec.board {
                ProbeState::Ready(b) => b.product.as_deref().or(b.vendor.as_deref()),
                _ => None,
            };
            let has_board = board.is_some() && r.h >= m.body_h + m.cap_h * 2 + 24;
            let block_h = m.body_h + 3 + m.cap_h + if has_board { 3 + m.cap_h } else { 0 };
            let mut y = r.y + r.h.saturating_sub(block_h) / 2;
            renderer.draw_truncated(fb, &hero, tx, y, tw, TextRole::Body, pal.accent);
            y += m.body_h + 3;
            renderer.draw_truncated(fb, model, tx, y, tw, TextRole::Caption, model_col);
            if let Some(b) = board
                && has_board
            {
                y += m.cap_h + 3;
                renderer.draw_truncated(fb, b, tx, y, tw, TextRole::Caption, pal.muted);
            }
        }
    }
}

fn draw_gpu_tile(
    fb: &mut Framebuffer,
    r: Rect,
    spec: &SystemSpec,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) {
    draw_bento_panel(fb, r, pal.tint_gpu, pal.border, m.radius, pal.border_px());
    let pad = m.pad.clamp(7, 12);
    let lx = r.x + pad;
    let logo_sz = match (&spec.gpu, spec.gpu_logo.as_ref()) {
        (ProbeState::Ready(gpus), Some(logo)) if !gpus.is_empty() => {
            let sz = r.h.saturating_sub(pad * 2).clamp(44, 68);
            let ly = r.y + r.h.saturating_sub(sz) / 2;
            if !draw_logo_image(fb, logo, lx, ly, sz, sz, 6) {
                draw_vector_gpu_logo(fb, lx, ly, sz, pal.focus, pal.on_accent);
            }
            sz
        }
        _ => {
            let sz = r.h.saturating_sub(pad * 2 + 12).clamp(26, 44);
            let ly = r.y + r.h.saturating_sub(sz) / 2;
            let badge_bg = match &spec.gpu {
                ProbeState::Ready(gpus) if !gpus.is_empty() => pal.focus,
                ProbeState::Pending => pal.warn,
                _ => pal.border_strong,
            };
            draw_vector_gpu_logo(fb, lx, ly, sz, badge_bg, pal.on_accent);
            sz
        }
    };

    let tx = lx + logo_sz + 10;
    let tw = r.x.saturating_add(r.w).saturating_sub(tx + pad);
    if tw < 24 {
        return;
    }

    match &spec.gpu {
        ProbeState::Pending => {
            let py = r.y + r.h.saturating_sub(m.cap_h + 6) / 2;
            draw_pill(fb, tx, py, "PROBING..", pal.warn, pal, m, renderer);
        }
        ProbeState::Unavailable => {
            let py = r.y + r.h.saturating_sub(m.body_h) / 2;
            renderer.draw_truncated(fb, "Not exposed", tx, py, tw, TextRole::Body, pal.muted);
        }
        ProbeState::Ready(gpus) if gpus.is_empty() => {
            let block_h = m.body_h + 3 + m.cap_h;
            let y = r.y + r.h.saturating_sub(block_h) / 2;
            renderer.draw_truncated(fb, "GOP only", tx, y, tw, TextRole::Body, pal.muted);
            renderer.draw_truncated(
                fb,
                "No PCI GPU",
                tx,
                y + m.body_h + 3,
                tw,
                TextRole::Caption,
                pal.muted,
            );
        }
        ProbeState::Ready(gpus) => {
            if gpus.len() > 1 {
                let block_h = m.body_h + 3 + m.cap_h;
                let y = r.y + r.h.saturating_sub(block_h) / 2;
                renderer.draw_truncated(fb, &gpus[0], tx, y, tw, TextRole::Body, pal.text);
                renderer.draw_truncated(
                    fb,
                    &format!("+{} · {}", gpus.len() - 1, gpus[1]),
                    tx,
                    y + m.body_h + 3,
                    tw,
                    TextRole::Caption,
                    pal.focus,
                );
            } else {
                let single_w = renderer.measure(&gpus[0], TextRole::Body).width;
                if single_w <= tw {
                    let y = r.y + r.h.saturating_sub(m.body_h) / 2;
                    renderer.draw_truncated(fb, &gpus[0], tx, y, tw, TextRole::Body, pal.text);
                } else {
                    let block_h = (m.body_h * 2 + 2).min(r.h.saturating_sub(12));
                    let y = r.y + r.h.saturating_sub(block_h) / 2;
                    renderer.draw_wrapped(fb, &gpus[0], tx, y, tw, 2, TextRole::Body, pal.text);
                }
            }
        }
    }
}

fn draw_memory_tile(
    fb: &mut Framebuffer,
    r: Rect,
    spec: &SystemSpec,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) {
    draw_bento_panel(fb, r, pal.tint_mem, pal.border, m.radius, pal.border_px());
    let pad = m.pad.clamp(7, 12);
    let px = r.x + pad;
    let tw = r.w.saturating_sub(pad * 2);
    if tw < 24 {
        return;
    }
    match &spec.memory {
        ProbeState::Pending => {
            let py = r.y + r.h.saturating_sub(m.cap_h + 6) / 2;
            draw_pill(fb, px, py, "PROBING..", pal.warn, pal, m, renderer);
        }
        ProbeState::Unavailable => {
            let py = r.y + r.h.saturating_sub(m.body_h) / 2;
            renderer.draw_truncated(fb, "RAM n/a", px, py, tw, TextRole::Body, pal.muted);
        }
        ProbeState::Ready(mem) => {
            let block_h = m.head_h + 2 + m.cap_h;
            let y = r.y + r.h.saturating_sub(block_h) / 2;
            let label = format_mib(mem.total_mib);
            renderer.draw_truncated(fb, &label, px, y, tw, TextRole::Heading, pal.text);
            renderer.draw_truncated(
                fb,
                "RAM",
                px,
                y + m.head_h + 2,
                tw,
                TextRole::Caption,
                pal.muted,
            );
        }
    }
}

fn draw_board_fw_tile(
    fb: &mut Framebuffer,
    r: Rect,
    spec: &SystemSpec,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) {
    draw_bento_panel(fb, r, pal.surface, pal.border, m.radius, pal.border_px());
    let pad = m.pad.clamp(7, 12);
    let px = r.x + pad;
    let tw = r.w.saturating_sub(pad * 2);
    if tw < 24 {
        return;
    }
    let disp = format!("{}×{}", spec.display.width, spec.display.height);
    let block_h = m.body_h + 3 + m.cap_h + 3 + m.tech_h;
    let mut y = r.y + r.h.saturating_sub(block_h.min(r.h.saturating_sub(10))) / 2;
    renderer.draw_truncated(fb, &disp, px, y, tw, TextRole::Body, pal.focus);
    y += m.body_h + 3;
    match &spec.firmware {
        ProbeState::Pending => {
            renderer.draw_truncated(fb, "UEFI..", px, y, tw, TextRole::Caption, pal.warn);
        }
        ProbeState::Unavailable => {
            renderer.draw_truncated(fb, "UEFI n/a", px, y, tw, TextRole::Caption, pal.muted);
        }
        ProbeState::Ready(fw) => {
            let v = fw.vendor.as_deref().unwrap_or("UEFI");
            renderer.draw_truncated(fb, v, px, y, tw, TextRole::Caption, pal.text);
            y += m.cap_h + 3;
            if y + m.tech_h <= r.y + r.h - 4 {
                renderer.draw_truncated(
                    fb,
                    &format!("UEFI · {}", fw.arch),
                    px,
                    y,
                    tw,
                    TextRole::Technical,
                    pal.muted,
                );
            }
        }
    }
}

fn draw_security_tile(
    fb: &mut Framebuffer,
    r: Rect,
    spec: &SystemSpec,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) {
    draw_bento_panel(fb, r, pal.tint_sec, pal.border, m.radius, pal.border_px());
    let pad = m.pad.clamp(7, 12);
    let px = r.x + pad;
    let tw = r.w.saturating_sub(pad * 2);
    if tw < 24 {
        return;
    }
    match &spec.security {
        ProbeState::Pending => {
            let py = r.y + r.h.saturating_sub(m.cap_h + 6) / 2;
            draw_pill(fb, px, py, "PROBING..", pal.warn, pal, m, renderer);
        }
        ProbeState::Unavailable => {
            let py = r.y + r.h.saturating_sub(m.body_h) / 2;
            renderer.draw_truncated(fb, "SEC n/a", px, py, tw, TextRole::Body, pal.muted);
        }
        ProbeState::Ready(sec) => {
            let sb_col = match sec.secure_boot {
                SecureBootSummary::Enabled => pal.ok,
                SecureBootSummary::Disabled => pal.warn,
                SecureBootSummary::SetupMode => pal.accent,
                SecureBootSummary::Unsupported => pal.muted,
            };
            let tpm = match sec.tpm_present {
                Some(true) => ("TPM 2.0", pal.ok),
                Some(false) => ("No TPM", pal.muted),
                None => ("TPM n/a", pal.muted),
            };
            let pill_h = m.cap_h + 6;
            let has_note = spec.policy_note.is_some() && r.h >= pill_h + m.body_h + m.cap_h + 18;
            let block_h = pill_h + 5 + m.body_h + if has_note { 3 + m.cap_h } else { 0 };
            let mut y = r.y + r.h.saturating_sub(block_h) / 2;
            draw_pill(fb, px, y, sec.secure_boot.badge(), sb_col, pal, m, renderer);
            y += pill_h + 5;
            renderer.draw_truncated(fb, tpm.0, px, y, tw, TextRole::Body, tpm.1);
            if let Some(note) = &spec.policy_note
                && has_note
            {
                y += m.body_h + 3;
                renderer.draw_truncated(fb, note, px, y, tw, TextRole::Caption, pal.warn);
            }
        }
    }
}

fn draw_network_tile(
    fb: &mut Framebuffer,
    r: Rect,
    spec: &SystemSpec,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) {
    draw_bento_panel(fb, r, pal.tint_net, pal.border, m.radius, pal.border_px());
    let pad = m.pad.clamp(7, 12);
    let px = r.x + pad;
    let tw = r.w.saturating_sub(pad * 2);
    if tw < 24 {
        return;
    }
    match &spec.network {
        ProbeState::Pending => {
            let py = r.y + r.h.saturating_sub(m.cap_h + 6) / 2;
            draw_pill(fb, px, py, "PROBING..", pal.warn, pal, m, renderer);
        }
        ProbeState::Unavailable => {
            let py = r.y + r.h.saturating_sub(m.body_h) / 2;
            renderer.draw_truncated(fb, "No SNP", px, py, tw, TextRole::Body, pal.muted);
        }
        ProbeState::Ready(nics) if nics.is_empty() => {
            let py = r.y + r.h.saturating_sub(m.body_h) / 2;
            renderer.draw_truncated(fb, "Offline", px, py, tw, TextRole::Body, pal.muted);
        }
        ProbeState::Ready(nics) => {
            let show = nics.len().min(2);
            let mut block_h = 0u32;
            for (i, nic) in nics.iter().take(show).enumerate() {
                block_h += m.body_h;
                if nic.address.is_some() {
                    block_h += 2 + m.tech_h;
                }
                if i + 1 < show {
                    block_h += 4;
                }
            }
            let mut y = r.y + r.h.saturating_sub(block_h.min(r.h.saturating_sub(8))) / 2;
            for nic in nics.iter().take(show) {
                let kind = if nic.wireless { "WIFI" } else { "ETH" };
                let st = if nic.up { "UP" } else { "DOWN" };
                let col = if nic.up { pal.ok } else { pal.muted };
                if y + m.body_h > r.y + r.h - 4 {
                    break;
                }
                renderer.draw_truncated(
                    fb,
                    &format!("{} {kind} {st}", nic.name),
                    px,
                    y,
                    tw,
                    TextRole::Body,
                    col,
                );
                y += m.body_h + 2;
                if let Some(addr) = &nic.address
                    && y + m.tech_h <= r.y + r.h - 4
                {
                    renderer.draw_truncated(fb, addr, px, y, tw, TextRole::Technical, pal.muted);
                    y += m.tech_h + 4;
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
    opts: ExperimentalOptions,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> LayoutHitMap {
    if paint {
        fb.fill(pal.bg);
        let grid_step = if m.compact { 16 } else { 20 };
        fb.dot_grid(0, 0, w, h, grid_step, pal.grid_dot);
    }

    let footer_h = m.footer_h;
    let x0 = m.pad;
    let inner_w = w.saturating_sub(m.pad * 2);
    let top_y = (m.pad / 2).max(4);
    let footer_y = h.saturating_sub(footer_h);

    let ic_h = (m.cap_h + m.body_h + m.tech_h + 28)
        .max(m.scaled(82))
        .min((h * 20 / 100).max(86));
    let (hw_hit, ic_total_h) = draw_schematic_ics(
        fb,
        Rect {
            x: x0,
            y: top_y,
            w: inner_w,
            h: ic_h,
        },
        spec,
        state,
        opts,
        pal,
        m,
        renderer,
        paint,
    );

    let bus_band_h = if m.compact { 14 } else { 18 };
    let spine_y = top_y + ic_total_h + bus_band_h / 2;

    if paint {
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
        fb.vline(hub_x, top_y + ic_total_h, bus_band_h / 2, pal.accent);
        fb.vline(hub_x + 1, top_y + ic_total_h, bus_band_h / 2, pal.accent);
        fill_circle(fb, hub_x, spine_y, 3.2, pal.accent);
    }

    let storage_y = top_y + ic_total_h + bus_band_h;
    let storage_h = footer_y.saturating_sub(storage_y).saturating_sub(m.gap);
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
        StorageStyle::Schematic { spine_y },
        opts,
        renderer,
        paint,
    );
    hw_hit.apply(&mut hit);
    let (actions, help_btn) = draw_footer(
        fb,
        Rect {
            x: 0,
            y: footer_y,
            w,
            h: footer_h,
        },
        state,
        opts,
        pal,
        m,
        renderer,
        paint,
    );
    hit.actions = actions;
    hit.help_button = help_btn;
    hit
}

#[allow(clippy::too_many_arguments)]
fn draw_schematic_ics(
    fb: &mut Framebuffer,
    area: Rect,
    spec: &SystemSpec,
    state: &MenuState,
    opts: ExperimentalOptions,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> (HwStripHit, u32) {
    let link_w: u32 = if m.compact { 10 } else { 14 };
    let brand_w = brand_tile_min_width(area.h, spec, m, renderer);
    let base_widths: [u32; 7] = [
        brand_w,
        m.scaled(242).max(192),
        m.scaled(264).max(204),
        m.scaled(124).max(98),
        m.scaled(172).max(138),
        m.scaled(148).max(118),
        m.scaled(182).max(144),
    ];
    let total_links = link_w.saturating_mul(6);
    let natural_w: u32 = base_widths.iter().sum::<u32>().saturating_add(total_links);
    let scrollable_x = natural_w > area.w;

    let widths: [u32; 7] = if !scrollable_x && area.w > total_links {
        let avail = area.w.saturating_sub(total_links);
        let sum_base = base_widths.iter().sum::<u32>().max(1);
        let mut out = [0u32; 7];
        let mut used = 0u32;
        for i in 0..7 {
            if i == 6 {
                out[i] = avail.saturating_sub(used);
            } else {
                let w = ((avail as u64 * base_widths[i] as u64) / sum_base as u64) as u32;
                out[i] = w;
                used = used.saturating_add(w);
            }
        }
        out
    } else {
        base_widths
    };

    let content_w: u32 = widths.iter().sum::<u32>().saturating_add(total_links);
    let max_scroll_x = content_w.saturating_sub(area.w);
    let eff_scroll_x = opts.scroll_x.min(max_scroll_x);

    let mut hw_hit = HwStripHit {
        viewport: Some(area),
        scroll_x: eff_scroll_x,
        max_scroll_x,
        ..HwStripHit::default()
    };

    if paint {
        let mid_y = area.y + area.h / 2;
        fb.set_viewport_xy(area.x, area.y, area.w, area.h, -(eff_scroll_x as i32), 0);
        let mut x = area.x;
        for (idx, &bw) in widths.iter().enumerate() {
            let r = Rect {
                x,
                y: area.y,
                w: bw,
                h: area.h,
            };
            let screen_x0 = (r.x as i64) - (eff_scroll_x as i64);
            let screen_x1 = screen_x0 + (r.w as i64) + (link_w as i64);
            if screen_x1 > area.x as i64 && screen_x0 < (area.x + area.w) as i64 {
                match idx {
                    0 => draw_brand_tile(fb, r, spec, pal, m, renderer),
                    1 => draw_cpu_tile(fb, r, spec, pal, m, renderer),
                    2 => draw_gpu_tile(fb, r, spec, pal, m, renderer),
                    3 => draw_memory_tile(fb, r, spec, pal, m, renderer),
                    4 => draw_board_fw_tile(fb, r, spec, pal, m, renderer),
                    5 => draw_security_tile(fb, r, spec, pal, m, renderer),
                    _ => draw_network_tile(fb, r, spec, pal, m, renderer),
                }
                fb.corner_ticks(r.x, r.y, r.w, r.h, 6, pal.accent);
                if idx + 1 < 7 {
                    let lx = r.x + r.w;
                    fb.hline(lx, mid_y, link_w, pal.border_strong);
                    fill_circle(fb, lx, mid_y, 2.2, pal.accent);
                    fill_circle(fb, lx + link_w, mid_y, 2.2, pal.accent);
                }
            }
            x = x.saturating_add(bw).saturating_add(link_w);
        }
        fb.clear_viewport();
    }

    let total_h = if scrollable_x {
        let sb_h = if m.compact { 12 } else { 14 };
        let sb_gap = 4u32;
        let sb_rect = Rect {
            x: area.x,
            y: area.y.saturating_add(area.h).saturating_add(sb_gap),
            w: area.w,
            h: sb_h,
        };
        draw_h_scrollbar(
            fb,
            sb_rect,
            area,
            content_w,
            eff_scroll_x,
            max_scroll_x,
            state,
            pal,
            paint,
            &mut hw_hit,
        );
        area.h.saturating_add(sb_gap).saturating_add(sb_h)
    } else {
        area.h
    };

    (hw_hit, total_h)
}

// ============================================================================
// Experiment C: Storage-First Full-Width LBA Paragraph Canvases + Compact Strip
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
    opts: ExperimentalOptions,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> LayoutHitMap {
    if paint {
        fb.fill(pal.bg);
    }
    let footer_h = m.footer_h;
    let strip_h = (m.cap_h + m.body_h + m.tech_h + 22)
        .max(m.scaled(72))
        .min((h * 16 / 100).max(74));

    let x0 = m.pad;
    let inner_w = w.saturating_sub(m.pad * 2);
    let strip_y = (m.pad / 2).max(4);

    let (hw_hit, strip_total_h) = draw_bento_hw_grid(
        fb,
        Rect {
            x: x0,
            y: strip_y,
            w: inner_w,
            h: strip_h,
        },
        spec,
        state,
        opts,
        pal,
        m,
        renderer,
        paint,
    );

    let storage_y = strip_y.saturating_add(strip_total_h).saturating_add(m.gap);
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
        opts,
        renderer,
        paint,
    );
    hw_hit.apply(&mut hit);
    let (actions, help_btn) = draw_footer(
        fb,
        Rect {
            x: 0,
            y: footer_y,
            w,
            h: footer_h,
        },
        state,
        opts,
        pal,
        m,
        renderer,
        paint,
    );
    hit.actions = actions;
    hit.help_button = help_btn;
    hit
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
fn draw_empty_storage_panel(
    fb: &mut Framebuffer,
    area: Rect,
    message: &str,
    color: Rgba,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) {
    draw_bento_panel(fb, area, pal.surface, pal.border, m.radius, pal.border_px());
    renderer.draw_truncated(
        fb,
        message,
        area.x + m.pad,
        area.y + m.pad,
        area.w.saturating_sub(m.pad * 2),
        TextRole::Heading,
        color,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_scrollbar(
    fb: &mut Framebuffer,
    work: Rect,
    content_viewport: Rect,
    content_h: u32,
    eff_scroll_y: u32,
    max_scroll_y: u32,
    sb_w: u32,
    sb_gap: u32,
    state: &MenuState,
    pal: &Palette,
    paint: bool,
    hit: &mut LayoutHitMap,
) {
    let sb_x = work.x + content_viewport.w + sb_gap;
    let btn_h = sb_w.clamp(14, 20);
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
        .clamp(22u64.min(track_h as u64), track_h as u64) as u32;
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

    if !paint {
        return;
    }

    let up_hov = state.cursor.is_some_and(|(x, y)| up_rect.contains(x, y));
    let dn_hov = state.cursor.is_some_and(|(x, y)| down_rect.contains(x, y));
    let th_hov = state.cursor.is_some_and(|(x, y)| thumb_rect.contains(x, y));
    let bp = pal.border_px();

    draw_bento_panel(fb, track_rect, pal.surface_alt, pal.border, 5, bp);
    draw_bento_panel(
        fb,
        up_rect,
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
        4,
        bp,
    );
    draw_vector_chevron(
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
    draw_bento_panel(
        fb,
        down_rect,
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
        4,
        bp,
    );
    draw_vector_chevron(
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
    let thumb_col = if th_hov { pal.focus } else { pal.border_strong };
    draw_rrect(fb, thumb_rect, 4, thumb_col);

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

#[allow(clippy::too_many_arguments)]
fn draw_storage_workspace(
    fb: &mut Framebuffer,
    area: Rect,
    spec: &SystemSpec,
    state: &MenuState,
    pal: &Palette,
    m: Metrics,
    style: StorageStyle,
    opts: ExperimentalOptions,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> LayoutHitMap {
    let mut hit = LayoutHitMap {
        cols: 1,
        ..LayoutHitMap::default()
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
    let bar_h = (m.cap_h + 10).clamp(22, 32);
    let bar_gap = if m.compact { 4 } else { 6 };
    let label_h = bar_h.saturating_add(bar_gap);
    let bar_rect = Rect {
        x: area.x.saturating_add(bus_gutter),
        y: area.y,
        w: area.w.saturating_sub(bus_gutter),
        h: bar_h,
    };
    let work = Rect {
        x: area.x.saturating_add(bus_gutter),
        y: area.y.saturating_add(label_h),
        w: area.w.saturating_sub(bus_gutter),
        h: area.h.saturating_sub(label_h),
    };

    let all_devices = match &spec.storage {
        ProbeState::Pending => {
            let (cr, dr, bar_abort) = draw_storage_section_bar(
                fb,
                bar_rect,
                Some("Probing storage..."),
                state,
                opts,
                pal,
                m,
                renderer,
                paint,
            );
            hit.detail_compact = cr;
            hit.detail_detailed = dr;
            if let Some(ar) = bar_abort {
                hit.abort_buttons.push(ar);
            }
            if paint {
                draw_empty_storage_panel(
                    fb,
                    work,
                    "PROBING STORAGE DEVICES...",
                    pal.warn,
                    pal,
                    m,
                    renderer,
                );
            }
            return hit;
        }
        ProbeState::Unavailable => {
            let (cr, dr, bar_abort) = draw_storage_section_bar(
                fb,
                bar_rect,
                Some("Storage not exposed"),
                state,
                opts,
                pal,
                m,
                renderer,
                paint,
            );
            hit.detail_compact = cr;
            hit.detail_detailed = dr;
            if let Some(ar) = bar_abort {
                hit.abort_buttons.push(ar);
            }
            if paint {
                draw_empty_storage_panel(
                    fb,
                    work,
                    "No storage devices exposed by firmware",
                    pal.muted,
                    pal,
                    m,
                    renderer,
                );
            }
            return hit;
        }
        ProbeState::Ready(list) if list.is_empty() => {
            let (cr, dr, bar_abort) = draw_storage_section_bar(
                fb,
                bar_rect,
                Some("No drives"),
                state,
                opts,
                pal,
                m,
                renderer,
                paint,
            );
            hit.detail_compact = cr;
            hit.detail_detailed = dr;
            if let Some(ar) = bar_abort {
                hit.abort_buttons.push(ar);
            }
            if paint {
                draw_empty_storage_panel(
                    fb,
                    work,
                    "No physical storage devices discovered",
                    pal.muted,
                    pal,
                    m,
                    renderer,
                );
            }
            return hit;
        }
        ProbeState::Ready(list) => list,
    };

    // In Compact mode, hide data-only drives that hold no bootable OS targets
    // whenever at least one bootable drive exists, keeping the menu calm and focused.
    let mut visible_devices: Vec<(usize, &StorageDevice)> = match opts.detail {
        StorageDetail::Detailed => all_devices.iter().enumerate().collect(),
        StorageDetail::Compact => all_devices
            .iter()
            .enumerate()
            .filter(|(_, d)| d.boot_target_count() > 0)
            .collect(),
    };
    let hidden_data_drives = if visible_devices.is_empty() {
        visible_devices = all_devices.iter().enumerate().collect();
        0
    } else {
        all_devices.len().saturating_sub(visible_devices.len())
    };
    hit.viewport_h = work.h;

    let n = visible_devices.len();
    let cols: usize = match style {
        // Storage-first always uses 1 full-width column so every disk has maximum
        // horizontal canvas for OS cards or LBA polygon wrapping.
        StorageStyle::FullWidthRack => 1,
        StorageStyle::Bento | StorageStyle::Schematic { .. } => {
            if n == 1 {
                1
            } else if n >= 4 && work.w >= 1650 && m.scale_pct <= 125 {
                3
            } else if m.scale_pct <= 150
                && ((n >= 3 && work.w >= 760) || (n == 2 && work.w >= 1100))
            {
                2
            } else {
                1
            }
        }
    };
    let used_cols = cols.min(n).max(1);
    hit.cols = used_cols;
    let rows = n.div_ceil(used_cols);

    // Compute natural comfortable height for every row based on active detail mode,
    // target richness, and active UI scale so scaled / multi-drive setups scroll cleanly.
    let mut row_weights = Vec::with_capacity(rows);
    let mut row_heights = Vec::with_capacity(rows);
    for row in 0..rows {
        let row_start = row * used_cols;
        let row_end = (row_start + used_cols).min(n);
        let row_devs = &visible_devices[row_start..row_end];
        let max_targets = row_devs
            .iter()
            .map(|(_, d)| d.boot_target_count())
            .max()
            .unwrap_or(0);
        match opts.detail {
            StorageDetail::Compact => {
                let slots = max_targets.clamp(1, 4) as u32;
                let weight = (8 + slots * 5).min(28);
                let base_h: u32 = match style {
                    StorageStyle::FullWidthRack => {
                        if m.compact {
                            118
                        } else if m.wide {
                            170
                        } else {
                            146
                        }
                    }
                    StorageStyle::Bento | StorageStyle::Schematic { .. } => {
                        if m.compact {
                            126
                        } else if m.wide {
                            184
                        } else {
                            154
                        }
                    }
                };
                let extra = if slots >= 3 && used_cols >= 2 {
                    if m.compact { 38 } else { 54 }
                } else {
                    0
                };
                row_weights.push(weight);
                row_heights.push(m.scaled(base_h.saturating_add(extra)));
            }
            StorageDetail::Detailed => {
                let max_score = row_devs
                    .iter()
                    .map(|(_, d)| {
                        let r_pts = d.regions.len().clamp(1, 10) as u32 * 3;
                        let t_pts = d.boot_target_count().min(4) as u32 * 6;
                        (8 + r_pts + t_pts).min(36)
                    })
                    .max()
                    .unwrap_or(10);
                let max_regions = row_devs
                    .iter()
                    .map(|(_, d)| d.regions.len())
                    .max()
                    .unwrap_or(1);
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
                row_heights.push(m.scaled(base_h.saturating_add(rich_bonus)));
            }
        }
    }

    let total_gaps = m.gap.saturating_mul(rows.saturating_sub(1) as u32);
    let natural_total_h: u32 = row_heights
        .iter()
        .copied()
        .sum::<u32>()
        .saturating_add(total_gaps);
    let scrollable = natural_total_h > work.h;

    // If everything fits comfortably inside `work.h`, stretch rows proportionally
    // to fill the canvas (in Compact mode, cap single-row drives with <=3 targets
    // so the drive chassis stays sleek instead of becoming an empty tall cave).
    if !scrollable {
        let total_weight: u32 = row_weights.iter().sum::<u32>().max(1);
        let mut avail_h = work.h.saturating_sub(total_gaps);
        if opts.detail == StorageDetail::Compact && rows == 1 {
            let max_slots = visible_devices
                .iter()
                .map(|(_, d)| d.boot_target_count().min(4))
                .max()
                .unwrap_or(1);
            let max_single_row_h = if max_slots >= 4 {
                m.scaled(360)
            } else {
                m.scaled(240)
            };
            avail_h = avail_h.min(max_single_row_h);
        }
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
    let eff_scroll_y = opts.scroll_y.min(max_scroll_y);
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

    {
        let hint_title = opts.show_hints.then(|| {
            let total_targets: usize = visible_devices
                .iter()
                .map(|(_, d)| d.boot_target_count())
                .sum();
            let scroll_hint = if scrollable {
                " · scroll vertically"
            } else {
                ""
            };
            match opts.detail {
                StorageDetail::Compact => {
                    let hidden_note = if hidden_data_drives > 0 {
                        format!(
                            " · {} data drive{} in Detailed",
                            hidden_data_drives,
                            if hidden_data_drives == 1 { "" } else { "s" }
                        )
                    } else {
                        String::new()
                    };
                    format!(
                        "{} drive{} · {} OS{hidden_note}{scroll_hint}",
                        visible_devices.len(),
                        if visible_devices.len() == 1 { "" } else { "s" },
                        total_targets,
                    )
                }
                StorageDetail::Detailed => format!(
                    "LBA order · {} drive{} · {} boot target{}{scroll_hint}",
                    visible_devices.len(),
                    if visible_devices.len() == 1 { "" } else { "s" },
                    total_targets,
                    if total_targets == 1 { "" } else { "s" },
                ),
            }
        });
        let (cr, dr, bar_abort) = draw_storage_section_bar(
            fb,
            bar_rect,
            hint_title.as_deref(),
            state,
            opts,
            pal,
            m,
            renderer,
            paint,
        );
        hit.detail_compact = cr;
        hit.detail_detailed = dr;
        if let Some(ar) = bar_abort {
            hit.abort_buttons.push(ar);
        }
    }

    let col_w =
        content_w.saturating_sub(m.gap.saturating_mul(used_cols as u32 - 1)) / used_cols as u32;

    let content_cursor = if opts.overflow_drive.is_some() {
        None
    } else {
        state.cursor.and_then(|(cx, cy)| {
            content_viewport
                .contains(cx, cy)
                .then_some((cx, cy.saturating_add(eff_scroll_y)))
        })
    };

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

        let row_start = row * used_cols;
        let row_end = (row_start + used_cols).min(n);
        if visible_devices[row_start..row_end]
            .iter()
            .any(|(_, dev)| dev.has_selected(state.selected))
        {
            hit.selected_drive_band = Some((row_top, row_bottom));
        }

        let visible_row =
            row_bottom > eff_scroll_y && row_top < eff_scroll_y.saturating_add(work.h);
        if visible_row {
            last_tap_y = row_mid_y;

            if paint && let StorageStyle::Schematic { .. } = style {
                let (_, row_dev) = visible_devices[row_start];
                let tap_col = bus_color(&row_dev.bus, row_dev.removable, pal);
                fb.hline(trunk_x, row_mid_y, work.x.saturating_sub(trunk_x), tap_col);
                fill_circle(fb, trunk_x, row_mid_y, 3.0, tap_col);
                fill_circle(fb, work.x, row_mid_y, 2.2, tap_col);
            }

            for col in 0..used_cols {
                let idx = row_start + col;
                if idx >= n {
                    break;
                }
                let (drive_idx, dev) = visible_devices[idx];
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
                        fill_circle(fb, drop_x, rect.y, 2.2, bus_col);
                    }
                    if col > 0 {
                        let prev_right = rect.x.saturating_sub(m.gap);
                        fb.hline(prev_right, row_mid_y, m.gap, pal.border_strong);
                        fill_circle(fb, prev_right, row_mid_y, 2.2, bus_col);
                        fill_circle(fb, rect.x, row_mid_y, 2.2, bus_col);
                    }
                }

                let (dev_zones, dev_aborts, overflow_btn) = match opts.detail {
                    StorageDetail::Compact => draw_compact_storage_device(
                        fb,
                        rect,
                        drive_idx,
                        dev,
                        state,
                        content_cursor,
                        opts.show_hints,
                        pal,
                        m,
                        style,
                        opts.overflow_drive == Some(drive_idx),
                        renderer,
                        paint,
                    ),
                    StorageDetail::Detailed => {
                        let (z, a) = draw_storage_device(
                            fb,
                            rect,
                            dev,
                            state,
                            content_cursor,
                            opts.show_hints,
                            pal,
                            m,
                            style,
                            renderer,
                            paint,
                        );
                        (z, a, None)
                    }
                };
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
                if let Some(btn_r) = overflow_btn
                    && let Some(sr) = shift_and_clip_rect(btn_r, eff_scroll_y, content_viewport)
                {
                    hit.overflow_buttons.push((drive_idx, sr));
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
            fill_circle(fb, trunk_x, spine_y, 3.2, pal.accent);
        } else {
            fb.clear_viewport();
        }
    }

    if scrollable {
        draw_scrollbar(
            fb,
            work,
            content_viewport,
            content_h,
            eff_scroll_y,
            max_scroll_y,
            sb_w,
            sb_gap,
            state,
            pal,
            paint,
            &mut hit,
        );
    }

    hit
}

/// Renders the storage section control bar directly above the drive cards:
/// right-aligned `[ Compact | Detailed ]` toggle scoped specifically to storage,
/// active autoboot countdown pill + `[X]` abort button when a policy timer runs,
/// and optional left-side hint summary when `?` (`show_hints`) is active.
#[allow(clippy::too_many_arguments)]
fn draw_storage_section_bar(
    fb: &mut Framebuffer,
    bar: Rect,
    title: Option<&str>,
    state: &MenuState,
    opts: ExperimentalOptions,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> (Option<Rect>, Option<Rect>, Option<Rect>) {
    if bar.w < 120 || bar.h < 16 {
        return (None, None, None);
    }

    let c_lbl = if bar.w < 500 { "Cmp" } else { "Compact" };
    let d_lbl = if bar.w < 500 { "Det" } else { "Detailed" };
    let c_w = renderer.measure(c_lbl, TextRole::Caption).width + 16;
    let d_w = renderer.measure(d_lbl, TextRole::Caption).width + 16;
    let toggle_w = c_w + d_w + 4;
    let show_toggle = toggle_w + 20 <= bar.w;

    let (toggle_x, detail_compact, detail_detailed) = if show_toggle {
        let tx = bar.x + bar.w.saturating_sub(toggle_w);
        let seg_h = bar.h.saturating_sub(4).max(12);
        let seg_y = bar.y + 2;
        (
            tx,
            Some(Rect {
                x: tx + 2,
                y: seg_y,
                w: c_w,
                h: seg_h,
            }),
            Some(Rect {
                x: tx + 2 + c_w,
                y: seg_y,
                w: d_w,
                h: seg_h,
            }),
        )
    } else {
        (bar.x + bar.w, None, None)
    };

    // Place active policy autoboot countdown + `[X]` abort right to the left of the toggle.
    let (abort_rect, countdown_x) = if let Some(ms) = state.countdown_ms {
        let sz = bar.h.saturating_sub(2).clamp(18, 26);
        let status = format!("BOOT IN {}s", ms.div_ceil(1000));
        let status_w = renderer.measure(&status, TextRole::Caption).width + 12;
        let ax = toggle_x.saturating_sub(sz + 8);
        let sx = ax.saturating_sub(status_w + 5);
        if sx > bar.x + 8 {
            (
                Some(Rect {
                    x: ax,
                    y: bar.y + bar.h.saturating_sub(sz) / 2,
                    w: sz,
                    h: sz,
                }),
                Some((sx, status, ms)),
            )
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    if !paint {
        return (detail_compact, detail_detailed, abort_rect);
    }

    let left_limit = countdown_x
        .as_ref()
        .map(|(sx, _, _)| *sx)
        .unwrap_or(toggle_x);
    if let Some(text) = title {
        let title_max_w = left_limit.saturating_sub(bar.x + 10);
        if title_max_w > 24 {
            let ty = bar.y + bar.h.saturating_sub(m.cap_h) / 2;
            draw_eyebrow(fb, bar.x, ty, title_max_w, text, pal.eyebrow, renderer);
        }
    }

    let hover_cursor = if opts.overflow_drive.is_some() {
        None
    } else {
        state.cursor
    };

    if let (Some(ar), Some((sx, status, ms))) = (abort_rect, countdown_x) {
        let status_col = pal.accent.mix(pal.focus, autoboot_pulse(ms) / 3, 255);
        let pill_h = m.cap_h + 6;
        let pill_y = bar.y + bar.h.saturating_sub(pill_h) / 2;
        draw_pill(fb, sx, pill_y, &status, status_col, pal, m, renderer);
        let hov = hover_cursor.is_some_and(|(cx, cy)| ar.contains(cx, cy));
        draw_abort_x_button(fb, ar, hov, pal);
        if state.countdown_total_ms > 0 {
            let fill_w = (bar.w as u64 * ms as u64 / state.countdown_total_ms as u64) as u32;
            fb.fill_rect(
                bar.x,
                bar.y + bar.h.saturating_sub(2),
                fill_w,
                2,
                pal.accent,
            );
        }
    }

    if let (Some(cr), Some(dr)) = (detail_compact, detail_detailed) {
        let outer = Rect {
            x: toggle_x,
            y: bar.y,
            w: toggle_w,
            h: bar.h,
        };
        draw_bento_panel(
            fb,
            outer,
            pal.surface_alt,
            pal.border_strong,
            6,
            pal.border_px(),
        );
        for (seg_r, lbl, mode_val) in [
            (cr, c_lbl, StorageDetail::Compact),
            (dr, d_lbl, StorageDetail::Detailed),
        ] {
            let active = opts.detail == mode_val;
            let hovered = hover_cursor.is_some_and(|(cx, cy)| seg_r.contains(cx, cy));
            if active {
                draw_rrect(fb, seg_r, 4, pal.focus);
                renderer.draw_centered(fb, lbl, seg_r, TextRole::Caption, pal.on_accent);
            } else {
                if hovered {
                    draw_rrect(fb, seg_r, 4, pal.surface_selected);
                }
                let col = if hovered { pal.text } else { pal.muted };
                renderer.draw_centered(fb, lbl, seg_r, TextRole::Caption, col);
            }
        }
    }

    (detail_compact, detail_detailed, abort_rect)
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

/// One bootable target inside a drive together with friendly partition metadata
/// for Compact-mode rendering (no raw partition indices or LBA jargon).
struct CompactTarget<'a> {
    target: &'a BootTargetView,
    size_bytes: Option<u64>,
    fs: Option<&'a str>,
    encrypted: Option<&'a str>,
}

/// Collect bootable targets hosted by `dev` for Compact mode:
/// actual OS targets first (in exact ascending LBA order), then standalone
/// recovery targets (in exact ascending LBA order). Non-OS partitions and
/// unallocated gaps are omitted.
fn compact_drive_targets(dev: &StorageDevice) -> Vec<CompactTarget<'_>> {
    let mut os_targets = Vec::new();
    let mut rec_targets = Vec::new();
    for reg in &dev.regions {
        let RegionKind::Partition(part) = &reg.kind else {
            continue;
        };
        for target in &part.boot_targets {
            let entry = CompactTarget {
                target,
                size_bytes: target.os_part.map(|_| reg.size_bytes),
                fs: if target.os_part.is_some() {
                    part.fs.as_deref()
                } else {
                    None
                },
                encrypted: part.encrypted.as_deref(),
            };
            if target.is_recovery {
                rec_targets.push(entry);
            } else {
                os_targets.push(entry);
            }
        }
    }
    os_targets.extend(rec_targets);
    os_targets
}

fn compact_target_subtitle(item: &CompactTarget<'_>) -> String {
    if !item.target.healthy {
        return "Unavailable · integrity error".into();
    }
    if item.target.is_recovery {
        return "Recovery & diagnostics".into();
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(bytes) = item.size_bytes {
        parts.push(format_bytes(bytes));
    }
    if let Some(fs) = item.fs {
        parts.push(fs.to_string());
    }
    if let Some(enc) = item.encrypted {
        parts.push(enc.to_string());
    }
    if parts.is_empty() {
        item.target.family.clone()
    } else {
        parts.join(" · ")
    }
}

/// Render a drive card in Compact mode: clean rectangular OS widgets only
/// (max 4 slots per drive; when >4 targets exist, the 4th slot is a hamburger
/// overflow button that reveals the remaining OSes).
#[allow(clippy::too_many_arguments)]
fn draw_compact_storage_device(
    fb: &mut Framebuffer,
    r: Rect,
    _drive_idx: usize,
    dev: &StorageDevice,
    state: &MenuState,
    cursor: Option<(u32, u32)>,
    show_hints: bool,
    pal: &Palette,
    m: Metrics,
    style: StorageStyle,
    overflow_open: bool,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> (Vec<CardHitZone>, Vec<Rect>, Option<Rect>) {
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
                fb.rect_outline(r.x, r.y, r.w, r.h, pal.border_px(), border);
                fb.corner_ticks(r.x, r.y, r.w, r.h, 7, bus_col);
            }
            StorageStyle::Bento | StorageStyle::FullWidthRack => {
                draw_bento_panel(fb, r, pal.surface, border, m.radius, pal.border_px());
                let inset = m.radius.min(r.h / 2);
                fb.fill_rect(
                    r.x + pal.border_px(),
                    r.y + inset,
                    3,
                    r.h.saturating_sub(inset * 2),
                    bus_col,
                );
            }
        }
    }

    let pad = if m.compact { 8 } else { 10 };
    let px = r.x + pad + 3;
    let inner_w = r.w.saturating_sub(pad * 2 + 4);
    let header_h = m.body_h.max(m.tech_h + 6) + 6;
    let y0 = r.y + pad.min(8);

    // 1. Clean Device Header Row: [NVME0] [NVME] Model   |   Capacity (no LBA sector noise)
    if paint {
        let cap = format_bytes(dev.size_bytes);
        let cap_w = renderer.measure(&cap, TextRole::Technical).width;
        let desig_w = draw_badge_solid(
            fb,
            px,
            y0,
            &dev.designator,
            bus_col,
            pal.on_accent,
            m,
            renderer,
        );
        let bus_tag = if dev.removable {
            format!("{} REM", dev.bus.to_ascii_uppercase())
        } else {
            dev.bus.to_ascii_uppercase()
        };
        let bus_w = draw_pill(
            fb,
            px + desig_w + 5,
            y0,
            &bus_tag,
            bus_col,
            pal,
            m,
            renderer,
        );
        let model_x = px + desig_w + bus_w + 12;
        let model_max = (px + inner_w)
            .saturating_sub(cap_w + 10)
            .saturating_sub(model_x);
        let model = dev.model.as_deref().unwrap_or("Storage device");
        let model_col = if dev.model.is_some() {
            pal.text
        } else {
            pal.muted
        };
        let model_y = y0 + header_h.saturating_sub(m.body_h + 4) / 2;
        renderer.draw_truncated(
            fb,
            model,
            model_x,
            model_y,
            model_max,
            TextRole::Body,
            model_col,
        );
        renderer.draw_top(
            fb,
            &cap,
            px + inner_w.saturating_sub(cap_w),
            y0 + header_h.saturating_sub(m.tech_h + 4) / 2,
            TextRole::Technical,
            pal.accent,
        );
    }

    let body_y = y0.saturating_add(header_h).saturating_add(2);
    let body_h =
        r.y.saturating_add(r.h)
            .saturating_sub(pad)
            .saturating_sub(body_y);
    let body = Rect {
        x: px,
        y: body_y,
        w: inner_w,
        h: body_h,
    };
    if body.w < 40 || body.h < 28 {
        return (zones, abort_buttons, None);
    }

    let targets = compact_drive_targets(dev);
    if targets.is_empty() {
        if paint {
            draw_bento_panel(fb, body, pal.surface_alt, pal.border, 8, pal.border_px());
            renderer.draw_centered(
                fb,
                "No operating systems found on this drive",
                body,
                TextRole::Caption,
                pal.muted,
            );
        }
        return (zones, abort_buttons, None);
    }

    // Max 4 slots per drive in Compact mode:
    // - 1..=4 targets -> render all directly.
    // - >4 targets    -> render first 3 OS cards + 4th hamburger overflow card.
    const MAX_SLOTS: usize = 4;
    let has_overflow = targets.len() > MAX_SLOTS;
    let direct_count = if has_overflow {
        MAX_SLOTS - 1
    } else {
        targets.len()
    };
    let slots = if has_overflow {
        MAX_SLOTS
    } else {
        direct_count
    };

    let gap: u32 = if m.compact { 6 } else { 8 };
    let t_cols: usize = match slots {
        1 => 1,
        2 => {
            if body.w >= m.scaled(340) {
                2
            } else {
                1
            }
        }
        3 => {
            if body.w >= m.scaled(640) {
                3
            } else if body.w >= m.scaled(340) {
                2
            } else {
                1
            }
        }
        _ => {
            if body.h >= m.scaled(190) && body.w >= m.scaled(340) {
                // Tall drive canvas: prefer a balanced 2x2 grid so every OS
                // card has generous horizontal room for titles, sizes, and badges.
                2
            } else if body.w >= m.scaled(860) {
                4
            } else if body.w >= m.scaled(340) {
                2
            } else {
                1
            }
        }
    };
    let t_rows = slots.div_ceil(t_cols);
    let row_gap_total = gap.saturating_mul(t_rows.saturating_sub(1) as u32);
    let max_card_h = if t_rows == 1 {
        m.scaled(136)
    } else {
        m.scaled(124)
    };
    let raw_card_h = body.h.saturating_sub(row_gap_total) / (t_rows as u32);
    let card_h = raw_card_h.min(max_card_h).max(34).min(body.h);
    let block_h = card_h
        .saturating_mul(t_rows as u32)
        .saturating_add(row_gap_total);
    let start_y = body.y.saturating_add(body.h.saturating_sub(block_h) / 2);

    let mut overflow_btn = None;
    for slot in 0..slots {
        let row = slot / t_cols;
        let col = slot % t_cols;
        let cols_in_row = if row + 1 == t_rows {
            slots - row * t_cols
        } else {
            t_cols
        };
        let col_gap_total = gap.saturating_mul(cols_in_row.saturating_sub(1) as u32);
        let cw = body.w.saturating_sub(col_gap_total) / (cols_in_row as u32);
        let cx = body.x.saturating_add((col as u32) * (cw + gap));
        let card_w = if col + 1 == cols_in_row {
            body.x.saturating_add(body.w).saturating_sub(cx)
        } else {
            cw
        };
        let cy = start_y.saturating_add((row as u32) * (card_h + gap));
        let card_r = Rect {
            x: cx,
            y: cy,
            w: card_w,
            h: card_h,
        };

        if slot < direct_count {
            let item = &targets[slot];
            let selected = state.focus == Focus::Cards && state.selected == item.target.card_index;
            let countdown = if selected { state.countdown_ms } else { None };
            let hovered = cursor.is_some_and(|(mx, my)| card_r.contains(mx, my));
            if let Some(ar) = draw_compact_os_card(
                fb, card_r, item, selected, hovered, countdown, cursor, show_hints, pal, m,
                renderer, paint,
            ) {
                abort_buttons.push(ar);
            }
            zones.push(CardHitZone {
                card_index: item.target.card_index,
                segments: vec![card_r],
                priority: true,
            });
        } else {
            // 4th slot: hamburger overflow card revealing targets[3..].
            let hidden = &targets[direct_count..];
            let selected_hidden = if state.focus == Focus::Cards {
                hidden
                    .iter()
                    .find(|it| it.target.card_index == state.selected)
            } else {
                None
            };
            let countdown = selected_hidden.and(state.countdown_ms);
            let hovered = cursor.is_some_and(|(mx, my)| card_r.contains(mx, my));
            if let Some(ar) = draw_compact_overflow_tile(
                fb,
                card_r,
                hidden,
                selected_hidden,
                overflow_open,
                hovered,
                countdown,
                cursor,
                pal,
                m,
                renderer,
                paint,
            ) {
                abort_buttons.push(ar);
            }
            overflow_btn = Some(card_r);
        }
    }

    (zones, abort_buttons, overflow_btn)
}

/// Clean rectangular OS widget shared by Compact drive cards and the overflow popover.
#[allow(clippy::too_many_arguments)]
fn draw_compact_os_card(
    fb: &mut Framebuffer,
    r: Rect,
    item: &CompactTarget<'_>,
    selected: bool,
    hovered: bool,
    countdown_ms: Option<u32>,
    cursor: Option<(u32, u32)>,
    show_hints: bool,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> Option<Rect> {
    let abort_rect = countdown_ms.map(|_| {
        let sz = r.h.saturating_sub(14).clamp(18, 24);
        Rect {
            x: r.x + r.w.saturating_sub(sz + 8),
            y: r.y + r.h.saturating_sub(sz) / 2,
            w: sz,
            h: sz,
        }
    });
    if !paint {
        return abort_rect;
    }

    let target = item.target;
    let pulse = countdown_ms.map(autoboot_pulse);
    let accent = if !target.healthy {
        pal.danger
    } else if target.is_recovery {
        pal.part_rec
    } else if target.hint == "windows" {
        pal.part_win
    } else {
        pal.part_linux
    };
    let fill = if let Some(p) = pulse {
        pal.surface_selected.mix(pal.accent, p * 75 / 255, 255)
    } else if selected {
        pal.surface_selected
    } else if hovered {
        pal.surface_alt.mix(accent, 18, 255)
    } else {
        pal.surface_alt
    };
    let border = if let Some(p) = pulse {
        pal.focus.mix(pal.accent, p, 255)
    } else if selected {
        pal.focus
    } else if hovered {
        pal.border_strong
    } else if !target.healthy {
        pal.danger
    } else {
        pal.border
    };
    let border_w = if selected || pal.high_contrast { 2 } else { 1 };
    draw_bento_panel(fb, r, fill, border, 8, border_w);

    let pad_x = if m.compact { 8 } else { 10 };
    let icon_sz = r.h.saturating_sub(14).clamp(20, 38);
    let ix = r.x + pad_x;
    let iy = r.y + r.h.saturating_sub(icon_sz) / 2;
    draw_vector_emblem(
        fb,
        &target.hint,
        &target.title,
        ix,
        iy,
        icon_sz,
        accent,
        pal.on_accent,
    );

    // Optional right-aligned status pill (`AUTO Ns` + `[X]`, or `ENTER TO BOOT` when selected).
    let mut right_limit = abort_rect
        .map(|ar| ar.x.saturating_sub(6))
        .unwrap_or_else(|| r.x + r.w.saturating_sub(pad_x));
    let tx = ix + icon_sz + pad_x;

    if let Some(ms) = countdown_ms {
        let pill_txt = format!("AUTO {}s", ms.div_ceil(1000));
        let pill_w = renderer.measure(&pill_txt, TextRole::Caption).width + 12;
        if right_limit.saturating_sub(tx) >= pill_w + 90 {
            let pill_h = m.cap_h + 6;
            let pill_x = right_limit.saturating_sub(pill_w);
            let pill_y = r.y + r.h.saturating_sub(pill_h) / 2;
            let pill_col = pal.accent.mix(pal.focus, pulse.unwrap_or(0) / 3, 255);
            draw_pill(fb, pill_x, pill_y, &pill_txt, pill_col, pal, m, renderer);
            right_limit = pill_x.saturating_sub(8);
        }
    } else if show_hints && selected && r.w >= m.scaled(270) {
        let cue = if r.w >= m.scaled(360) {
            "ENTER / CLICK: BOOT"
        } else {
            "SELECTED"
        };
        let pill_w = renderer.measure(cue, TextRole::Caption).width + 12;
        if right_limit.saturating_sub(tx) >= pill_w + 100 {
            let pill_h = m.cap_h + 6;
            let pill_x = right_limit.saturating_sub(pill_w);
            let pill_y = r.y + r.h.saturating_sub(pill_h) / 2;
            draw_pill(fb, pill_x, pill_y, cue, pal.focus, pal, m, renderer);
            right_limit = pill_x.saturating_sub(8);
        }
    } else if !target.healthy {
        let cue = "CORRUPT";
        let pill_w = renderer.measure(cue, TextRole::Caption).width + 12;
        if right_limit.saturating_sub(tx) >= pill_w + 80 {
            let pill_h = m.cap_h + 6;
            let pill_x = right_limit.saturating_sub(pill_w);
            let pill_y = r.y + r.h.saturating_sub(pill_h) / 2;
            draw_pill(fb, pill_x, pill_y, cue, pal.danger, pal, m, renderer);
            right_limit = pill_x.saturating_sub(8);
        }
    }

    let tw = right_limit.saturating_sub(tx);
    let title_role = if r.h >= m.head_h + m.cap_h + 16 && tw >= 110 {
        TextRole::Heading
    } else {
        TextRole::Body
    };
    let title_h = renderer.line_metrics(title_role).line_height;
    let title_text = if selected {
        format!("> {}", target.title)
    } else {
        target.title.clone()
    };
    let title_col = if countdown_ms.is_some() {
        pal.accent
    } else if selected {
        pal.focus
    } else if !target.healthy {
        pal.danger
    } else {
        pal.text
    };

    let sub = compact_target_subtitle(item);
    if r.h >= title_h + m.cap_h + 10 {
        let block_h = title_h + m.cap_h + 3;
        let ty = r.y + r.h.saturating_sub(block_h) / 2;
        renderer.draw_truncated(fb, &title_text, tx, ty, tw, title_role, title_col);
        let sy = ty + title_h + 3;
        let mut sx = tx;
        let mut sw = tw;
        if item.encrypted.is_some() && sw > 18 {
            let lock_sz = m.cap_h.clamp(10, 14);
            draw_vector_lock(
                fb,
                sx,
                sy + m.cap_h.saturating_sub(lock_sz) / 2,
                lock_sz,
                pal.part_win,
            );
            sx = sx.saturating_add(lock_sz + 4);
            sw = sw.saturating_sub(lock_sz + 4);
        }
        let sub_col = if !target.healthy {
            pal.danger
        } else {
            pal.muted
        };
        renderer.draw_truncated(fb, &sub, sx, sy, sw, TextRole::Caption, sub_col);
    } else {
        let ty = r.y + r.h.saturating_sub(title_h) / 2;
        renderer.draw_truncated(fb, &title_text, tx, ty, tw, title_role, title_col);
    }

    if let Some(ar) = abort_rect {
        let hov = cursor.is_some_and(|(mx, my)| ar.contains(mx, my));
        draw_abort_x_button(fb, ar, hov, pal);
    }
    abort_rect
}

/// 4th-slot hamburger overflow widget shown in Compact mode when a drive has >4 OS targets.
#[allow(clippy::too_many_arguments)]
fn draw_compact_overflow_tile(
    fb: &mut Framebuffer,
    r: Rect,
    hidden: &[CompactTarget<'_>],
    selected_hidden: Option<&CompactTarget<'_>>,
    overflow_open: bool,
    hovered: bool,
    countdown_ms: Option<u32>,
    cursor: Option<(u32, u32)>,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> Option<Rect> {
    let abort_rect = countdown_ms.map(|_| {
        let sz = r.h.saturating_sub(14).clamp(18, 24);
        Rect {
            x: r.x + r.w.saturating_sub(sz + 8),
            y: r.y + r.h.saturating_sub(sz) / 2,
            w: sz,
            h: sz,
        }
    });
    if !paint {
        return abort_rect;
    }

    let active = overflow_open || selected_hidden.is_some();
    let pulse = countdown_ms.map(autoboot_pulse);
    let fill = if let Some(p) = pulse {
        pal.surface_selected.mix(pal.accent, p * 75 / 255, 255)
    } else if active {
        pal.surface_selected
    } else if hovered {
        pal.surface_alt.mix(pal.focus, 22, 255)
    } else {
        pal.surface_alt
    };
    let border = if let Some(p) = pulse {
        pal.focus.mix(pal.accent, p, 255)
    } else if active {
        pal.focus
    } else if hovered {
        pal.border_strong
    } else {
        pal.border
    };
    draw_bento_panel(
        fb,
        r,
        fill,
        border,
        8,
        if active || pal.high_contrast { 2 } else { 1 },
    );

    let pad_x = if m.compact { 8 } else { 10 };
    let icon_sz = r.h.saturating_sub(14).clamp(20, 36);
    let ix = r.x + pad_x;
    let iy = r.y + r.h.saturating_sub(icon_sz) / 2;
    let badge_bg = if active { pal.focus } else { pal.border_strong };
    let bar_fg = if active { pal.on_accent } else { pal.text };
    fill_rounded_rect(fb, ix, iy, icon_sz, icon_sz, (icon_sz / 5).max(3), badge_bg);
    draw_vector_hamburger(fb, ix + 2, iy + 2, icon_sz.saturating_sub(4), bar_fg);

    let tx = ix + icon_sz + pad_x;
    let right_limit = abort_rect
        .map(|ar| ar.x.saturating_sub(6))
        .unwrap_or_else(|| r.x + r.w.saturating_sub(pad_x));
    let tw = right_limit.saturating_sub(tx);

    let heading = match selected_hidden {
        Some(sel) => format!(
            "> {} (+{})",
            sel.target.title,
            hidden.len().saturating_sub(1)
        ),
        None => format!("+{} more", hidden.len()),
    };
    let names: String = hidden
        .iter()
        .map(|it| it.target.title.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let sub = if overflow_open {
        format!("Showing: {names}")
    } else {
        names
    };

    let title_role = if r.h >= m.head_h + m.cap_h + 16 && tw >= 110 {
        TextRole::Heading
    } else {
        TextRole::Body
    };
    let title_h = renderer.line_metrics(title_role).line_height;
    let title_col = if countdown_ms.is_some() {
        pal.accent
    } else if active {
        pal.focus
    } else {
        pal.text
    };
    if r.h >= title_h + m.cap_h + 10 {
        let block_h = title_h + m.cap_h + 3;
        let ty = r.y + r.h.saturating_sub(block_h) / 2;
        renderer.draw_truncated(fb, &heading, tx, ty, tw, title_role, title_col);
        renderer.draw_truncated(
            fb,
            &sub,
            tx,
            ty + title_h + 3,
            tw,
            TextRole::Caption,
            pal.muted,
        );
    } else {
        let ty = r.y + r.h.saturating_sub(title_h) / 2;
        renderer.draw_truncated(fb, &heading, tx, ty, tw, title_role, title_col);
    }

    if let Some(ar) = abort_rect {
        let hov = cursor.is_some_and(|(mx, my)| ar.contains(mx, my));
        draw_abort_x_button(fb, ar, hov, pal);
    }
    abort_rect
}

/// Dim the rendered backdrop lightly behind a Compact-mode overflow popover.
fn dim_framebuffer(fb: &mut Framebuffer, dark: bool) {
    let scale: u32 = if dark { 140 } else { 196 };
    for px in &mut fb.px {
        let r = (((*px >> 16) & 0xFF) * scale) >> 8;
        let g = (((*px >> 8) & 0xFF) * scale) >> 8;
        let b = ((*px & 0xFF) * scale) >> 8;
        *px = (r << 16) | (g << 8) | b;
    }
}

/// Render the Compact-mode hamburger overflow popover showing the remaining OS
/// targets (`targets[3..]`) for `drive_idx`, populating `hit.overflow_*`.
#[allow(clippy::too_many_arguments)]
fn draw_overflow_popover(
    fb: &mut Framebuffer,
    w: u32,
    h: u32,
    spec: &SystemSpec,
    drive_idx: usize,
    state: &MenuState,
    opts: ExperimentalOptions,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
    hit: &mut LayoutHitMap,
) {
    let ProbeState::Ready(devices) = &spec.storage else {
        return;
    };
    let Some(dev) = devices.get(drive_idx) else {
        return;
    };
    let targets = compact_drive_targets(dev);
    if targets.len() <= 4 {
        return;
    }
    let overflow = &targets[3..];

    let pad = if m.compact { 12 } else { 16 };
    let header_h = m.head_h.max(m.tech_h + 8) + 8;
    let row_h = m.scaled(58).max(m.body_h + m.cap_h + 22);
    let row_gap = 8u32;
    let footer_h = if opts.show_hints { m.cap_h + 10 } else { 0 };

    let max_panel_w = w.saturating_sub(32).max(240);
    let panel_w = (w * 56 / 100)
        .max(m.scaled(400).min(max_panel_w))
        .min(max_panel_w);
    let max_panel_h = h.saturating_sub(48).max(160);
    let rows_h = (overflow.len() as u32)
        .saturating_mul(row_h)
        .saturating_add(row_gap.saturating_mul(overflow.len().saturating_sub(1) as u32));
    let desired_h = pad + header_h + 10 + rows_h + 12 + footer_h + pad;
    let panel_h = desired_h.min(max_panel_h);
    let panel = Rect {
        x: w.saturating_sub(panel_w) / 2,
        y: h.saturating_sub(panel_h) / 2,
        w: panel_w,
        h: panel_h,
    };

    let close_sz = header_h.saturating_sub(6).clamp(20, 28);
    let close_r = Rect {
        x: panel.x + panel.w.saturating_sub(pad + close_sz),
        y: panel.y + pad + header_h.saturating_sub(close_sz) / 2,
        w: close_sz,
        h: close_sz,
    };

    hit.overflow_panel = Some(panel);
    hit.overflow_close = Some(close_r);

    if paint {
        dim_framebuffer(fb, state.dark);
        fill_rounded_panel(
            fb, panel.x, panel.y, panel.w, panel.h, 12, 2, pal.header, pal.focus,
        );
        let bus_col = bus_color(&dev.bus, dev.removable, pal);
        let hx = panel.x + pad;
        let hy = panel.y + pad;
        let dw = draw_badge_solid(
            fb,
            hx,
            hy + header_h.saturating_sub(m.tech_h + 6) / 2,
            &dev.designator,
            bus_col,
            pal.on_accent,
            m,
            renderer,
        );
        let model = dev.model.as_deref().unwrap_or(&dev.device_name);
        let title = format!("{model} · {} more OS", overflow.len());
        let title_x = hx + dw + 10;
        let title_w = close_r.x.saturating_sub(title_x + 8);
        renderer.draw_truncated(
            fb,
            &title,
            title_x,
            hy + header_h.saturating_sub(m.head_h) / 2,
            title_w,
            TextRole::Heading,
            pal.text,
        );
        let close_hov = state
            .cursor
            .is_some_and(|(cx, cy)| close_r.contains(cx, cy));
        let close_bg = if close_hov {
            pal.danger
        } else {
            pal.surface_alt
        };
        let close_fg = if close_hov { pal.on_accent } else { pal.text };
        draw_bento_panel(fb, close_r, close_bg, pal.border_strong, 6, pal.border_px());
        draw_vector_close_x(
            fb,
            close_r.x + 2,
            close_r.y + 2,
            close_r.w.saturating_sub(4),
            close_fg,
        );
        fb.hline(
            panel.x + pad,
            hy + header_h + 3,
            panel.w.saturating_sub(pad * 2),
            pal.border,
        );
    }

    let mut cur_y = panel.y + pad + header_h + 10;
    let max_row_y = panel.y + panel.h.saturating_sub(pad + footer_h + 6);
    let inner_w = panel.w.saturating_sub(pad * 2);
    for item in overflow {
        if cur_y.saturating_add(34) > max_row_y {
            break;
        }
        let rh = row_h.min(max_row_y.saturating_sub(cur_y));
        let row_r = Rect {
            x: panel.x + pad,
            y: cur_y,
            w: inner_w,
            h: rh,
        };
        let selected = state.focus == Focus::Cards && state.selected == item.target.card_index;
        let countdown = if selected { state.countdown_ms } else { None };
        let hovered = state.cursor.is_some_and(|(mx, my)| row_r.contains(mx, my));
        if let Some(ar) = draw_compact_os_card(
            fb,
            row_r,
            item,
            selected,
            hovered,
            countdown,
            state.cursor,
            opts.show_hints,
            pal,
            m,
            renderer,
            paint,
        ) {
            hit.abort_buttons.push(ar);
        }
        hit.overflow_zones.push(CardHitZone {
            card_index: item.target.card_index,
            segments: vec![row_r],
            priority: true,
        });
        cur_y = cur_y.saturating_add(rh + row_gap);
    }

    if paint && opts.show_hints {
        let hint = "Click to select · Click again or Enter to boot · Esc or click outside to close";
        renderer.draw_truncated(
            fb,
            hint,
            panel.x + pad,
            panel.y + panel.h.saturating_sub(pad + m.cap_h),
            inner_w,
            TextRole::Caption,
            pal.muted,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_storage_device(
    fb: &mut Framebuffer,
    r: Rect,
    dev: &StorageDevice,
    state: &MenuState,
    cursor: Option<(u32, u32)>,
    show_hints: bool,
    pal: &Palette,
    m: Metrics,
    style: StorageStyle,
    renderer: &mut TextRenderer<'_>,
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
                fb.rect_outline(r.x, r.y, r.w, r.h, pal.border_px(), border);
                fb.corner_ticks(r.x, r.y, r.w, r.h, 7, bus_col);
            }
            StorageStyle::Bento | StorageStyle::FullWidthRack => {
                draw_bento_panel(fb, r, pal.surface, border, m.radius, pal.border_px());
                let inset = m.radius.min(r.h / 2);
                fb.fill_rect(
                    r.x + pal.border_px(),
                    r.y + inset,
                    3,
                    r.h.saturating_sub(inset * 2),
                    bus_col,
                );
            }
        }
    }

    let pad = if m.compact { 6 } else { 8 };
    let px = r.x + pad + 3;
    let inner_w = r.w.saturating_sub(pad * 2 + 4);
    let header_h = m.body_h.max(m.tech_h + 6) + 5;
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
        let cap_w = renderer.measure(&cap, TextRole::Technical).width;
        let desig_w = draw_badge_solid(
            fb,
            px,
            y0,
            &dev.designator,
            bus_col,
            pal.on_accent,
            m,
            renderer,
        );
        let bus_tag = if dev.removable {
            format!("{} REM", dev.bus.to_ascii_uppercase())
        } else {
            dev.bus.to_ascii_uppercase()
        };
        let bus_w = draw_pill(
            fb,
            px + desig_w + 5,
            y0,
            &bus_tag,
            bus_col,
            pal,
            m,
            renderer,
        );
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
        let model_y = y0 + header_h.saturating_sub(m.body_h + 4) / 2;
        renderer.draw_truncated(
            fb,
            model,
            model_x,
            model_y,
            model_max,
            TextRole::Body,
            model_col,
        );
        renderer.draw_top(
            fb,
            &cap,
            px + inner_w.saturating_sub(cap_w),
            y0 + header_h.saturating_sub(m.tech_h + 4) / 2,
            TextRole::Technical,
            pal.accent,
        );
    }

    // Reserve optional Selected Target / LBA Provenance footer inside tall single-disk cards.
    let map_y = y0.saturating_add(header_h);
    let mut map_bottom = r.y + r.h.saturating_sub(pad);
    let prov_h = m.tech_h + m.cap_h + 14;
    let show_provenance = !m.compact
        && map_bottom.saturating_sub(map_y) >= 180 + prov_h
        && dev.find_selected(state.selected).is_some();
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
            let (base_w, base_h, interactive) = match &reg.kind {
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
                RegionKind::Partition(_) if targets >= 2 => (
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
                RegionKind::Partition(_) if targets == 1 => (
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
                ),
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
                min_width: m.scaled(base_w).min(map_rect.w.max(1)),
                min_height: m.scaled(base_h).min(map_rect.h.max(1)),
                interactive,
            }
        })
        .collect();

    // 3. Compute connected paragraph-wrapped orthogonal polygon layout.
    let wrapped = layout_disk_wrap(map_rect, &inputs);

    // 4. Paint polygon fills, hatches, internal labels/logos/targets, then antialiased outer borders.
    let mut selected_outline: Option<(Vec<Rect>, Rgba, f32)> = None;
    let mut hovered_outline: Option<(Vec<Rect>, Rgba)> = None;
    let mut hovered_region: Option<&DiskRegion> = None;
    let countdown_pulse = state.countdown_ms.map(autoboot_pulse);
    let poly_radius = if m.compact { 2.0 } else { 3.0 };
    let base_stroke = pal.border_px() as f32;

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
                    fill_ortho_polygon(fb, &poly.segments, poly_radius, fill);
                    hatch_polygon(fb, &poly.segments, 8, pal.unalloc_hatch);
                    stroke_ortho_polygon(fb, &poly.segments, poly_radius, base_stroke, pal.border);
                    draw_unallocated_content(fb, reg, poly, pal, m, renderer);
                    if hovered {
                        hovered_outline = Some((poly.segments.clone(), pal.border_strong));
                    }
                }
                RegionKind::Partition(part) => {
                    let base_fill = pal.part_tint(part, is_selected, hovered);
                    // Flash the ENTIRE connected polygon smoothly while policy autoboot counts down!
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
                    fill_ortho_polygon(fb, &poly.segments, poly_radius, fill);

                    // Crisp bus/OS accent tab at the top-left of the partition's first segment
                    // so reading-order start of every L/Z polygon is visually anchored!
                    if let Some(first) = poly.segments.first() {
                        let accent = if is_autoboot_target {
                            pal.accent.mix(pal.focus, countdown_pulse.unwrap_or(0), 255)
                        } else {
                            pal.part_accent(part)
                        };
                        let tab_h = first.h.saturating_sub(4).max(1);
                        fb.fill_rect(first.x + 2, first.y + 2, 3, tab_h, accent);
                    }

                    // Antialiased outer polygon perimeter (shared internal row edges canceled!).
                    stroke_ortho_polygon(fb, &poly.segments, poly_radius, base_stroke, pal.border);

                    let (mut sub_zones, mut part_aborts) = draw_partition_content(
                        fb,
                        reg,
                        part,
                        poly,
                        state,
                        cursor,
                        is_selected,
                        hovered,
                        show_hints,
                        pal,
                        m,
                        renderer,
                        true,
                    );
                    zones.append(&mut sub_zones);
                    abort_buttons.append(&mut part_aborts);

                    if is_autoboot_target {
                        let pulse = countdown_pulse.unwrap_or(200);
                        let glow = pal.focus.mix(pal.accent, pulse, 255);
                        let thickness = if pulse > 140 { 3.0 } else { 2.0 };
                        selected_outline = Some((poly.segments.clone(), glow, thickness));
                    } else if is_selected {
                        let glow = if pal.high_contrast {
                            pal.focus
                        } else {
                            pal.border_strong.mix(
                                pal.focus,
                                160u32.saturating_sub(state.anim_ms.min(160)),
                                160,
                            )
                        };
                        selected_outline = Some((
                            poly.segments.clone(),
                            glow,
                            if pal.high_contrast { 3.0 } else { 2.0 },
                        ));
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
                show_hints,
                pal,
                m,
                renderer,
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
            stroke_ortho_polygon(fb, &segs, poly_radius, 2.0, col);
        }
        if let Some((segs, col, thickness)) = selected_outline {
            stroke_ortho_polygon(fb, &segs, poly_radius, thickness, col);
        }
        fb.rect_outline(
            map_rect.x,
            map_rect.y,
            map_rect.w,
            map_rect.h,
            pal.border_px(),
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
                draw_hover_lba_bar(fb, pr, dev, hreg, show_hints, pal, m, renderer);
            } else if let Some((sel_reg, sel_target)) = dev.find_selected(state.selected) {
                draw_provenance_bar(
                    fb,
                    pr,
                    dev,
                    sel_reg,
                    sel_target,
                    state.countdown_ms.is_some(),
                    show_hints,
                    pal,
                    m,
                    renderer,
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
    renderer: &mut TextRenderer<'_>,
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

    if lr.h >= m.cap_h + m.tech_h + 10 {
        let ty = lr.y + lr.h.saturating_sub(m.cap_h + m.tech_h + 2) / 2;
        renderer.draw_truncated(fb, "UNALLOC", tx, ty, tw, TextRole::Caption, pal.muted);
        renderer.draw_truncated(
            fb,
            &format!("{cap}{scale_tag}"),
            tx,
            ty + m.cap_h + 2,
            tw,
            TextRole::Technical,
            pal.eyebrow,
        );
    } else {
        let ty = lr.y + lr.h.saturating_sub(m.tech_h) / 2;
        renderer.draw_truncated(
            fb,
            &format!("FREE {cap}{scale_tag}"),
            tx,
            ty,
            tw,
            TextRole::Technical,
            pal.muted,
        );
    }
}

/// Render partition label, exact capacity, filesystem, vector OS emblem, encryption
/// lock, interactive boot action(s), and (when policy autoboot is active) an inline
/// `[×]` abort-autoboot button inside `poly.label_rect`.
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
    show_hints: bool,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
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
        let hdr_h = m.tech_h + 4;
        if paint {
            let hdr = format!("p{} · {cap}{scale_mark} · {fs}{wrap_mark}", part.index);
            renderer.draw_truncated(fb, &hdr, x0, y0, w, TextRole::Technical, accent);
        }
        let btn_top = y0 + hdr_h;
        let btn_avail = bottom.saturating_sub(btn_top);
        let n = part.boot_targets.len();
        let b_cols = if n >= 2 && w >= m.scaled(190) {
            n.min(2)
        } else {
            1
        };
        let b_rows = n.div_ceil(b_cols);
        let gap = 4u32;
        let col_w = w.saturating_sub(gap * (b_cols as u32 - 1)) / b_cols as u32;
        let max_bh = m.scaled(if m.wide { 42 } else { 32 });
        let row_h = (btn_avail.saturating_sub(gap * (b_rows as u32 - 1)) / b_rows as u32)
            .clamp(m.cap_h.max(16), max_bh.max(m.cap_h + 6));

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
            if let Some(abort_r) = draw_inline_target_pill(
                fb,
                br,
                target,
                t_sel,
                t_countdown,
                cursor,
                pal,
                m,
                renderer,
                paint,
            ) {
                abort_buttons.push(abort_r);
            }
        }
        return (sub_zones, abort_buttons);
    }

    // Case 2: Partition with exactly 1 boot target (entire polygon is one unified boot widget!).
    if let Some(target) = part.boot_targets.first() {
        let icon_sz = if lr.h >= m.scaled(54) && w >= m.scaled(130) {
            m.scaled(if m.wide { 28 } else { 24 }).clamp(18, 40)
        } else if lr.h >= m.scaled(34) && w >= m.scaled(90) {
            m.scaled(18).clamp(14, 28)
        } else {
            0
        };
        let mut tx = x0;
        let mut tw = w;
        if icon_sz > 0 {
            if paint {
                let iy = lr.y + lr.h.saturating_sub(icon_sz) / 2;
                let badge_bg = if !target.healthy { pal.danger } else { accent };
                draw_vector_emblem(
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

        let use_heading = lr.h >= m.head_h + m.tech_h + m.cap_h + 16 && tw >= m.scaled(110);
        let title_role = if use_heading {
            TextRole::Heading
        } else {
            TextRole::Body
        };
        let title_h = if use_heading { m.head_h } else { m.body_h };
        let is_autoboot = is_selected && countdown_ms.is_some();

        if lr.h >= m.tech_h + title_h + m.tech_h + 14 {
            // 3 rich lines: (1) pN · exact capacity · FS [+ lock], (2) OS Name + badge + [×], (3) EFI provenance
            let total_h = m.tech_h + title_h + m.tech_h + 6;
            let mut cy = lr.y + lr.h.saturating_sub(total_h) / 2;

            if paint {
                let mut mx = tx;
                if part.encrypted.is_some() {
                    draw_vector_lock(fb, mx, cy, m.tech_h, pal.part_enc);
                    mx += m.tech_h + 4;
                }
                let enc_tag = part
                    .encrypted
                    .as_deref()
                    .map(|e| format!(" · {e}"))
                    .unwrap_or_default();
                let meta = format!("p{} · {cap}{scale_mark} · {fs}{enc_tag}", part.index);
                renderer.draw_truncated(
                    fb,
                    &meta,
                    mx,
                    cy,
                    (tx + tw).saturating_sub(mx),
                    TextRole::Technical,
                    accent,
                );
            }
            cy += m.tech_h + 3;

            // Hero OS name line + safe boot status pill + clickable [×] abort button (only on policy countdown)
            let pill_h = m.cap_h + 6;
            let pill_y = cy + title_h.saturating_sub(pill_h) / 2;
            let mut right_edge = tx + tw;

            if let Some(left_ms) = countdown_ms
                && is_selected
            {
                let x_sz = pill_h.clamp(18, 28);
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

                if paint && right_edge > tx + m.scaled(130) {
                    let pulse = autoboot_pulse(left_ms);
                    let secs = left_ms.div_ceil(1000);
                    let badge = if hovered {
                        format!("BOOT NOW ({secs}s)")
                    } else {
                        format!("AUTO {secs}s")
                    };
                    let bw = renderer.measure(&badge, TextRole::Caption).width + 12;
                    let bx = right_edge.saturating_sub(bw);
                    let bcol = if hovered {
                        pal.focus
                    } else {
                        pal.accent.mix(pal.focus, pulse / 2, 255)
                    };
                    draw_pill(fb, bx, pill_y, &badge, bcol, pal, m, renderer);
                    right_edge = bx.saturating_sub(8);
                }
            } else if paint && show_hints {
                let boot_badge = if is_selected && tw >= m.scaled(185) {
                    Some("ENTER / CLICK: BOOT")
                } else if hovered && tw >= m.scaled(165) {
                    Some("CLICK TO SELECT")
                } else {
                    None
                };
                if let Some(b) = boot_badge {
                    let bw = renderer.measure(b, TextRole::Caption).width + 12;
                    let bx = right_edge.saturating_sub(bw);
                    draw_pill(
                        fb,
                        bx,
                        pill_y,
                        b,
                        if is_selected { pal.focus } else { accent },
                        pal,
                        m,
                        renderer,
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
                renderer.draw_truncated(
                    fb,
                    &format!("{prefix}{}", target.title),
                    tx,
                    cy,
                    right_edge.saturating_sub(tx),
                    title_role,
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
                renderer.draw_truncated(fb, &prov, tx, cy, tw, TextRole::Technical, pcol);
            }
        } else if lr.h >= title_h + m.tech_h + 8 {
            let total_h = title_h + m.tech_h + 3;
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
                renderer.draw_truncated(
                    fb,
                    &format!("{prefix}{}", target.title),
                    tx,
                    cy,
                    right_edge.saturating_sub(tx),
                    title_role,
                    title_col,
                );
                let sub = format!("p{} · {cap}{scale_mark} · {fs}", part.index);
                renderer.draw_truncated(
                    fb,
                    &sub,
                    tx,
                    cy + title_h + 3,
                    tw,
                    TextRole::Technical,
                    pal.muted,
                );
            }
        } else {
            let cy = lr.y + lr.h.saturating_sub(m.cap_h) / 2;
            let mut right_edge = tx + tw;
            if is_autoboot {
                let x_sz = lr.h.saturating_sub(4).clamp(14, 20);
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
                renderer.draw_truncated(
                    fb,
                    &format!("{} {cap}", target.title),
                    tx,
                    cy,
                    right_edge.saturating_sub(tx),
                    TextRole::Caption,
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

        // Live countdown progress stripe along the bottom of the flashing partition's label area.
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
        if renderer.measure(&full, TextRole::Technical).width <= w {
            full
        } else if renderer.measure(&with_fs, TextRole::Technical).width <= w {
            with_fs
        } else {
            base_cap
        }
    } else if let Some(free) = part.free_bytes {
        let full = format!("{base_cap} · {fs} · {} free", format_bytes(free));
        if renderer.measure(&full, TextRole::Technical).width <= w {
            full
        } else if renderer.measure(&with_fs, TextRole::Technical).width <= w {
            with_fs
        } else {
            base_cap
        }
    } else if renderer.measure(&with_fs, TextRole::Technical).width <= w {
        with_fs
    } else {
        base_cap
    };

    if lr.h >= m.cap_h + m.tech_h + 10 {
        let ty = lr.y + lr.h.saturating_sub(m.cap_h + m.tech_h + 2) / 2;
        let head = format!("p{} {title}", part.index);
        renderer.draw_truncated(fb, &head, x0, ty, w, TextRole::Caption, pal.text);
        renderer.draw_truncated(
            fb,
            &sub,
            x0,
            ty + m.cap_h + 2,
            w,
            TextRole::Technical,
            if part.loader_count > 0 {
                accent
            } else {
                pal.muted
            },
        );
    } else {
        let ty = lr.y + lr.h.saturating_sub(m.tech_h) / 2;
        renderer.draw_truncated(
            fb,
            &format!("p{} {cap}{scale_mark}", part.index),
            x0,
            ty,
            w,
            TextRole::Technical,
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
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> Option<Rect> {
    let abort_rect = countdown_ms.map(|_| {
        let sz = r.h.saturating_sub(6).clamp(14, 22);
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
    draw_bento_panel(
        fb,
        r,
        fill,
        border,
        5,
        if selected || pal.high_contrast { 2 } else { 1 },
    );

    let icon_sz = r.h.saturating_sub(8).clamp(12, 24);
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
    draw_vector_emblem(
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
    if r.h >= m.cap_h + m.tech_h + 6 {
        let ty = r.y + r.h.saturating_sub(m.cap_h + m.tech_h + 2) / 2;
        renderer.draw_truncated(
            fb,
            &format!("{prefix}{}", target.title),
            tx,
            ty,
            tw,
            TextRole::Caption,
            tcol,
        );
        let sub = match target.esp_part {
            Some(esp) => format!("EFI:p{esp} · {}", target.loader_file),
            None => target.loader_file.clone(),
        };
        renderer.draw_truncated(
            fb,
            &sub,
            tx,
            ty + m.cap_h + 2,
            tw,
            TextRole::Technical,
            pal.muted,
        );
    } else {
        let ty = r.y + r.h.saturating_sub(m.cap_h) / 2;
        renderer.draw_truncated(
            fb,
            &format!("{prefix}{}", target.title),
            tx,
            ty,
            tw,
            TextRole::Caption,
            tcol,
        );
    }

    if let Some(ar) = abort_rect {
        let hov = cursor.is_some_and(|(cx, cy)| ar.contains(cx, cy));
        draw_abort_x_button(fb, ar, hov, pal);
    }
    abort_rect
}

/// Draw a crisp clickable `[×]` button for aborting policy autoboot.
fn draw_abort_x_button(fb: &mut Framebuffer, r: Rect, hovered: bool, pal: &Palette) {
    let fill = if hovered {
        pal.danger
    } else {
        pal.surface.mix(pal.danger, 48, 255)
    };
    let border = if hovered { pal.text } else { pal.danger };
    let fg = if hovered { pal.on_accent } else { pal.danger };
    draw_bento_panel(fb, r, fill, border, 5, pal.border_px());
    let sz = r.w.min(r.h);
    draw_vector_close_x(
        fb,
        r.x + r.w.saturating_sub(sz) / 2,
        r.y + r.h.saturating_sub(sz) / 2,
        sz,
        fg,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_hover_lba_bar(
    fb: &mut Framebuffer,
    r: Rect,
    dev: &StorageDevice,
    reg: &DiskRegion,
    show_hints: bool,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) {
    draw_bento_panel(
        fb,
        r,
        pal.surface_alt,
        pal.border_strong,
        6,
        pal.border_px(),
    );
    let ix = r.x + 10;
    let iw = r.w.saturating_sub(20);
    let (line1, line2, col2) = match &reg.kind {
        RegionKind::Unallocated => (
            format!(
                "UNALLOCATED  ·  LBA {}..{}  ·  {} ({} × {}B)",
                reg.start_lba,
                reg.end_lba,
                format_bytes(reg.size_bytes),
                reg.end_lba.saturating_sub(reg.start_lba),
                dev.sector_size,
            ),
            if show_hints {
                Some("Unallocated gap in ascending LBA order".into())
            } else {
                None
            },
            pal.muted,
        ),
        RegionKind::Partition(p) => {
            let fs = p.fs.as_deref().unwrap_or(part_short_tag(p));
            let line1 = format!(
                "{}p{} {}  ·  {} ({fs})  ·  LBA {}..{}",
                dev.device_name,
                p.index,
                p.label,
                format_bytes(reg.size_bytes),
                reg.start_lba,
                reg.end_lba,
            );
            let line2 = if !p.boot_targets.is_empty() {
                let names: Vec<&str> = p.boot_targets.iter().map(|t| t.title.as_str()).collect();
                Some(if show_hints {
                    format!(
                        "{}  ·  Click to select · Click again or Enter to boot",
                        names.join(", ")
                    )
                } else {
                    names.join(", ")
                })
            } else if p.loader_count > 0 {
                Some(format!("ESP  ·  {} EFI loaders", p.loader_count))
            } else if let Some(free) = p.free_bytes {
                Some(format!("{} free", format_bytes(free)))
            } else if show_hints {
                Some("Non-bootable partition".into())
            } else {
                None
            };
            let col2 = if !p.boot_targets.is_empty() {
                pal.focus
            } else {
                pal.muted
            };
            (line1, line2, col2)
        }
    };
    if let Some(l2) = line2 {
        let ty = r.y + r.h.saturating_sub(m.tech_h + m.cap_h + 2) / 2;
        renderer.draw_truncated(fb, &line1, ix, ty, iw, TextRole::Technical, pal.text);
        renderer.draw_truncated(fb, &l2, ix, ty + m.tech_h + 2, iw, TextRole::Caption, col2);
    } else {
        let ty = r.y + r.h.saturating_sub(m.tech_h) / 2;
        renderer.draw_truncated(fb, &line1, ix, ty, iw, TextRole::Technical, pal.text);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_provenance_bar(
    fb: &mut Framebuffer,
    r: Rect,
    dev: &StorageDevice,
    reg: &DiskRegion,
    target: &BootTargetView,
    counting_down: bool,
    show_hints: bool,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) {
    draw_bento_panel(fb, r, pal.surface_alt, pal.border, 6, pal.border_px());
    let ix = r.x + 10;
    let iw = r.w.saturating_sub(20);
    let line1 = match (target.esp_part, target.os_part) {
        (Some(esp), Some(os)) if esp != os => format!(
            "{}  ·  EFI: {}p{} ({}) -> OS: {}p{}  ·  LBA {}..{} ({})",
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
            "{}  ·  EFI: {}p{} ({})  ·  LBA {}..{} ({})",
            target.title,
            dev.device_name,
            esp,
            target.loader_file,
            reg.start_lba,
            reg.end_lba,
            format_bytes(reg.size_bytes),
        ),
        (None, _) => format!(
            "{}  ·  {}  ·  LBA {}..{}",
            target.title, target.loader_file, reg.start_lba, reg.end_lba
        ),
    };
    let line2: Option<String> = match (&target.note, show_hints) {
        (Some(note), true) => {
            let hint = if counting_down {
                "Enter / click again to boot  ·  Esc / [X] to abort"
            } else {
                "Enter / click again to boot  ·  C for SBM Core"
            };
            Some(format!("{note}  ·  {hint}"))
        }
        (Some(note), false) => Some(note.clone()),
        (None, true) => Some(
            if counting_down {
                "Enter / click again to boot  ·  Esc / [X] to abort"
            } else {
                "Enter / click again to boot  ·  Tab for power actions"
            }
            .into(),
        ),
        (None, false) => None,
    };
    if let Some(l2) = line2 {
        let ty = r.y + r.h.saturating_sub(m.tech_h + m.cap_h + 2) / 2;
        renderer.draw_truncated(fb, &line1, ix, ty, iw, TextRole::Technical, pal.text);
        renderer.draw_truncated(
            fb,
            &l2,
            ix,
            ty + m.tech_h + 2,
            iw,
            TextRole::Caption,
            if !target.healthy {
                pal.danger
            } else if target.note.is_some() {
                pal.warn
            } else {
                pal.muted
            },
        );
    } else {
        let ty = r.y + r.h.saturating_sub(m.tech_h) / 2;
        renderer.draw_truncated(fb, &line1, ix, ty, iw, TextRole::Technical, pal.text);
    }
}

// ============================================================================
// Footer & Helpers
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn draw_footer(
    fb: &mut Framebuffer,
    r: Rect,
    state: &MenuState,
    opts: ExperimentalOptions,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
    paint: bool,
) -> (Vec<Rect>, Option<Rect>) {
    if paint {
        fb.fill_rect(r.x, r.y, r.w, r.h, pal.header);
        fb.hline(r.x, r.y, r.w, pal.border);
    }
    let btn_h =
        r.h.saturating_sub(10)
            .max(20)
            .min(r.h.saturating_sub(4).max(20));
    let btn_y = r.y + r.h.saturating_sub(btn_h) / 2;
    let max_label_w = ACTIONS
        .iter()
        .map(|l| renderer.measure(l, TextRole::Caption).width)
        .max()
        .unwrap_or(64);
    let btn_w = (max_label_w + 20)
        .max(if m.compact {
            84
        } else if m.wide {
            126
        } else {
            104
        })
        .min(r.w / 4);
    let gap = 6u32;
    let total_w = btn_w * 3 + gap * 2;
    let start_x = r.x + r.w.saturating_sub(m.pad + total_w);

    // Left-aligned `[ ? ]` toggle button to show/hide optional helper text.
    let help_w = btn_h.clamp(22, 32);
    let help_rect = Rect {
        x: r.x + m.pad,
        y: btn_y,
        w: help_w,
        h: btn_h,
    };
    let help_btn = (help_rect.x + help_rect.w + 8 <= start_x).then_some(help_rect);

    if paint {
        if let Some(hr) = help_btn {
            let hov = state.cursor.is_some_and(|(cx, cy)| hr.contains(cx, cy));
            let (fill, border, fg) = if opts.show_hints {
                (pal.focus, pal.focus, pal.on_accent)
            } else if hov {
                (pal.surface_hover, pal.border_strong, pal.text)
            } else {
                (pal.surface_alt, pal.border, pal.muted)
            };
            draw_bento_panel(
                fb,
                hr,
                fill,
                border,
                6,
                if opts.show_hints || pal.high_contrast {
                    2
                } else {
                    1
                },
            );
            renderer.draw_centered(fb, "?", hr, TextRole::Body, fg);
        }

        if opts.show_hints {
            let hint = if m.compact {
                "1-4 layout · V view · ? hints · T theme · +/- scale · F1 keys"
            } else {
                "1-4 layout · V compact/detailed · ? hints · F2 scen · F6 disc · F7 font · F8 HC · +/- scale · T theme · F1 keys"
            };
            let hint_x = help_btn.map(|hr| hr.x + hr.w + 10).unwrap_or(r.x + m.pad);
            let hint_w = start_x.saturating_sub(hint_x + 10);
            if hint_w > 24 {
                renderer.draw_truncated(
                    fb,
                    hint,
                    hint_x,
                    r.y + r.h.saturating_sub(m.cap_h) / 2,
                    hint_w,
                    TextRole::Caption,
                    pal.muted,
                );
            }
        }
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
            draw_bento_panel(
                fb,
                br,
                fill,
                border,
                6,
                if selected || pal.high_contrast { 2 } else { 1 },
            );
            renderer.draw_centered(fb, label, br, TextRole::Caption, text_col);
        }
    }
    (actions, help_btn)
}

fn draw_eyebrow(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    max_w: u32,
    text: &str,
    color: Rgba,
    renderer: &mut TextRenderer<'_>,
) {
    renderer.draw_truncated(fb, text, x, y, max_w, TextRole::Caption, color);
}

#[allow(clippy::too_many_arguments)]
fn draw_pill(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    label: &str,
    color: Rgba,
    pal: &Palette,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) -> u32 {
    let w = renderer.measure(label, TextRole::Caption).width + 12;
    let h = m.cap_h + 6;
    let rect = Rect { x, y, w, h };
    draw_bento_panel(fb, rect, pal.surface_alt, color, 4, pal.border_px());
    renderer.draw_centered(fb, label, rect, TextRole::Caption, color);
    w
}

#[allow(clippy::too_many_arguments)]
fn draw_badge_solid(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    label: &str,
    bg: Rgba,
    fg: Rgba,
    m: Metrics,
    renderer: &mut TextRenderer<'_>,
) -> u32 {
    let w = renderer.measure(label, TextRole::Technical).width + 12;
    let h = m.tech_h + 6;
    let rect = Rect { x, y, w, h };
    draw_rrect(fb, rect, 4, bg);
    renderer.draw_centered(fb, label, rect, TextRole::Technical, fg);
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
            sbm_version: "26.11.27#12".into(),
            cpu,
            cpu_logo: None,
            memory,
            board: ProbeState::Ready(BoardSpec {
                vendor: Some("LENOVO".into()),
                product: Some("ThinkPad X1".into()),
            }),
            gpu: ProbeState::Ready(vec!["Intel Arc 140V".into()]),
            gpu_logo: None,
            firmware: ProbeState::Ready(FirmwareSpec {
                vendor: Some("Lenovo (EDK II)".into()),
                version: "26.11.27#12".into(),
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

    #[test]
    fn unified_renderer_supports_high_contrast_scaling_and_font_modes() {
        let spec = sample_spec(false, false);
        let state = MenuState::new(0, true, Some(8_000));
        for font_mode in FontMode::ALL {
            for scale_pct in [75u16, 100, 150, 200] {
                let mut renderer = TextRenderer::new(font_mode, 800, UiScale::new(scale_pct));
                for mode in LayoutMode::EXPERIMENTS {
                    for detail in StorageDetail::ALL {
                        for high_contrast in [false, true] {
                            let mut fb = Framebuffer::new(1280, 800);
                            let map = paint_experimental_with(
                                &mut fb,
                                mode,
                                &spec,
                                &state,
                                ExperimentalOptions {
                                    high_contrast,
                                    detail,
                                    ..ExperimentalOptions::default()
                                },
                                &mut renderer,
                            );
                            assert!(
                                map.zones.len() >= 2,
                                "{font_mode:?} scale={scale_pct} {mode:?} {detail:?} hc={high_contrast}"
                            );
                            assert_eq!(map.actions.len(), 3);
                            assert!(!map.abort_buttons.is_empty());
                            assert!(map.detail_compact.is_some());
                            assert!(map.detail_detailed.is_some());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn compact_mode_caps_at_four_slots_with_hamburger_overflow_menu() {
        let mut spec = sample_spec(false, false);
        let ProbeState::Ready(ref mut devs) = spec.storage else {
            unreachable!();
        };
        // Add 3 more OS targets so the single drive hosts 5 total targets (>4).
        let base_end = devs[0].regions.last().unwrap().end_lba;
        for (i, name) in ["Arch Linux", "Fedora Linux", "Ubuntu"]
            .into_iter()
            .enumerate()
        {
            let start = base_end + (i as u64) * 20_000_000;
            let end = start + 18_000_000;
            devs[0].regions.push(DiskRegion {
                start_lba: start,
                end_lba: end,
                size_bytes: (end - start) * 512,
                kind: RegionKind::Partition(PartitionView {
                    index: 3 + i,
                    label: name.into(),
                    kind: PartitionKind::Linux,
                    fs: Some("ext4".into()),
                    free_bytes: None,
                    encrypted: None,
                    is_recovery: false,
                    detected_os: Some(name.into()),
                    loader_count: 0,
                    boot_targets: vec![BootTargetView {
                        card_index: 2 + i,
                        title: name.into(),
                        family: "Linux".into(),
                        hint: "linux".into(),
                        loader_file: "shimx64.efi".into(),
                        esp_part: Some(1),
                        os_part: Some(3 + i),
                        healthy: true,
                        is_recovery: false,
                        note: None,
                    }],
                }),
            });
        }
        // Also append a data-only second drive that must be hidden in Compact mode.
        devs.push(StorageDevice {
            designator: "SATA0".into(),
            device_name: "sda".into(),
            bus: "sata".into(),
            removable: false,
            model: Some("Archive SSD".into()),
            size_bytes: 1_000_000_000_000,
            sector_size: 512,
            total_sectors: 1_000_000_000_000 / 512,
            regions: vec![DiskRegion {
                start_lba: 2048,
                end_lba: 500_000_000,
                size_bytes: 256_000_000_000,
                kind: RegionKind::Partition(PartitionView {
                    index: 1,
                    label: "Archive".into(),
                    kind: PartitionKind::Data,
                    fs: Some("ext4".into()),
                    free_bytes: None,
                    encrypted: None,
                    is_recovery: false,
                    detected_os: None,
                    loader_count: 0,
                    boot_targets: vec![],
                }),
            }],
        });

        let state = MenuState::new(0, true, None);
        let mut renderer = TextRenderer::new(FontMode::AtkinsonNextMono, 800, UiScale::default());
        let mut fb = Framebuffer::new(1280, 800);
        let map = paint_experimental_with(
            &mut fb,
            LayoutMode::Bento,
            &spec,
            &state,
            ExperimentalOptions::default(),
            &mut renderer,
        );
        // 3 direct OS widgets + 1 hamburger overflow button = 4 total slots on drive 0.
        assert_eq!(map.zones.len(), 3, "only 3 direct OS widgets should render");
        assert_eq!(
            map.overflow_buttons.len(),
            1,
            "4th slot must be hamburger overflow"
        );
        let (drive_idx, btn) = map.overflow_buttons[0];
        assert_eq!(drive_idx, 0);
        assert_eq!(
            map.hit_test_experimental(btn.x + btn.w / 2, btn.y + btn.h / 2),
            ExperimentalHit::ToggleOverflow(0)
        );
        // Header Compact/Detailed toggle hits cleanly.
        let cr = map.detail_compact.unwrap();
        let dr = map.detail_detailed.unwrap();
        assert_eq!(
            map.hit_test_experimental(cr.x + cr.w / 2, cr.y + cr.h / 2),
            ExperimentalHit::SetDetail(StorageDetail::Compact)
        );
        assert_eq!(
            map.hit_test_experimental(dr.x + dr.w / 2, dr.y + dr.h / 2),
            ExperimentalHit::SetDetail(StorageDetail::Detailed)
        );

        // Opening the overflow popover exposes the remaining 2 targets + close button.
        let mut fb_open = Framebuffer::new(1280, 800);
        let open_map = paint_experimental_with(
            &mut fb_open,
            LayoutMode::Bento,
            &spec,
            &state,
            ExperimentalOptions {
                overflow_drive: Some(0),
                ..ExperimentalOptions::default()
            },
            &mut renderer,
        );
        assert!(open_map.overflow_panel.is_some());
        assert_eq!(
            open_map.overflow_zones.len(),
            2,
            "remaining 2 OSes must appear in popover"
        );
        let close = open_map.overflow_close.unwrap();
        assert_eq!(
            open_map.hit_test_experimental(close.x + close.w / 2, close.y + close.h / 2),
            ExperimentalHit::CloseOverflow
        );
        let z0 = open_map.overflow_zones[0].segments[0];
        assert_eq!(
            open_map.hit_test_experimental(z0.x + z0.w / 2, z0.y + z0.h / 2),
            ExperimentalHit::Card(open_map.overflow_zones[0].card_index)
        );
        // Detailed mode exposes all 5 targets directly without overflow popover.
        let det_map = layout_experimental_with(
            LayoutMode::Bento,
            1280,
            800,
            &spec,
            &state,
            ExperimentalOptions {
                detail: StorageDetail::Detailed,
                ..ExperimentalOptions::default()
            },
            &mut renderer,
        );
        assert!(det_map.overflow_buttons.is_empty());
        assert_eq!(det_map.zones.len(), 5);
    }

    #[test]
    fn top_grid_scrolls_horizontally_toggle_is_below_grid_and_help_btn_toggles_hints() {
        let spec = sample_spec(false, false);
        let state = MenuState::new(0, true, Some(5_000));
        let mut renderer = TextRenderer::new(FontMode::AtkinsonNextMono, 800, UiScale::default());

        let mut fb_clean = Framebuffer::new(1280, 800);
        let map = paint_experimental_with(
            &mut fb_clean,
            LayoutMode::Bento,
            &spec,
            &state,
            ExperimentalOptions::default(),
            &mut renderer,
        );

        // Top hardware strip overflows at 1280px and exposes horizontal scrollbar controls.
        let hw = map.hw_viewport.expect("top hardware viewport");
        assert!(
            map.max_scroll_x > 0,
            "7-widget top strip should scroll horizontally at 1280px"
        );
        let hr = map.hw_scroll_right.expect("right horizontal scroll button");
        assert_eq!(
            map.hit_test_experimental(hr.x + hr.w / 2, hr.y + hr.h / 2),
            ExperimentalHit::ScrollXDelta(160)
        );

        // Compact/Detailed toggle lives right above the storage section (below the top hardware grid).
        let cr = map.detail_compact.expect("Compact toggle");
        assert!(
            cr.y >= hw.y + hw.h,
            "Compact/Detailed toggle (y={}) must sit below the top hardware grid (bottom={})",
            cr.y,
            hw.y + hw.h
        );

        // Scrolling horizontally shifts pixels inside the top strip.
        let mut fb_scrolled = Framebuffer::new(1280, 800);
        let _ = paint_experimental_with(
            &mut fb_scrolled,
            LayoutMode::Bento,
            &spec,
            &state,
            ExperimentalOptions {
                scroll_x: map.max_scroll_x,
                ..ExperimentalOptions::default()
            },
            &mut renderer,
        );
        assert_ne!(fb_clean.px, fb_scrolled.px);

        // Clicking `?` button hits ToggleHints and reveals optional helper text.
        let hb = map.help_button.expect("`?` toggle button");
        assert_eq!(
            map.hit_test_experimental(hb.x + hb.w / 2, hb.y + hb.h / 2),
            ExperimentalHit::ToggleHints
        );
        let mut fb_hints = Framebuffer::new(1280, 800);
        let _ = paint_experimental_with(
            &mut fb_hints,
            LayoutMode::Bento,
            &spec,
            &state,
            ExperimentalOptions {
                show_hints: true,
                ..ExperimentalOptions::default()
            },
            &mut renderer,
        );
        assert_ne!(fb_clean.px, fb_hints.px);
    }

    #[test]
    fn cpu_and_gpu_tiles_render_cached_qoi_badges_when_ready() {
        use rapid_qoi::{Colors, Qoi};

        let make_badge = |r: u8, g: u8, b: u8| {
            let mut rgba = Vec::with_capacity(16 * 16 * 4);
            for _ in 0..(16 * 16) {
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
            let encoded = Qoi {
                width: 16,
                height: 16,
                colors: Colors::SrgbLinA,
            }
            .encode_alloc(&rgba)
            .expect("encode test badge");
            LogoImage::from_qoi(&encoded).expect("decode test badge")
        };

        let base_spec = sample_spec(false, false);
        let mut logo_spec = base_spec.clone();
        logo_spec.cpu_logo = Some(make_badge(240, 96, 18));
        logo_spec.gpu_logo = Some(make_badge(210, 24, 48));

        let state = MenuState::new(0, true, None);
        let mut renderer = TextRenderer::new(FontMode::AtkinsonNextMono, 800, UiScale::default());

        let mut fb_vec = Framebuffer::new(1280, 800);
        let _ = paint_experimental_with(
            &mut fb_vec,
            LayoutMode::Bento,
            &base_spec,
            &state,
            ExperimentalOptions::default(),
            &mut renderer,
        );

        let mut fb_real = Framebuffer::new(1280, 800);
        let _ = paint_experimental_with(
            &mut fb_real,
            LayoutMode::Bento,
            &logo_spec,
            &state,
            ExperimentalOptions::default(),
            &mut renderer,
        );
        assert_ne!(
            fb_vec.px, fb_real.px,
            "real CPU/GPU badges should visibly replace fallback vector glyphs"
        );

        // When CPU/GPU probes are Pending, badges must stay hidden until Ready.
        let mut pending_spec = sample_spec(true, false);
        pending_spec.gpu = ProbeState::Pending;
        let mut pending_with_logos = pending_spec.clone();
        pending_with_logos.cpu_logo = logo_spec.cpu_logo.clone();
        pending_with_logos.gpu_logo = logo_spec.gpu_logo.clone();

        let mut fb_p1 = Framebuffer::new(1280, 800);
        let _ = paint_experimental_with(
            &mut fb_p1,
            LayoutMode::Bento,
            &pending_spec,
            &state,
            ExperimentalOptions::default(),
            &mut renderer,
        );
        let mut fb_p2 = Framebuffer::new(1280, 800);
        let _ = paint_experimental_with(
            &mut fb_p2,
            LayoutMode::Bento,
            &pending_with_logos,
            &state,
            ExperimentalOptions::default(),
            &mut renderer,
        );
        assert_eq!(
            fb_p1.px, fb_p2.px,
            "pending probes must not display cached hardware badges prematurely"
        );
    }
}
