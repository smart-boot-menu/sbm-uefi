//! Reproducible before/after typography & vector rendering benchmark.
//!
//! Run with:
//! `cargo run -p sbm-gfx --release --example bench_typography`

use std::hint::black_box;
use std::time::Instant;

use sbm_gfx::typography::{BUNDLED_FONT_BYTES, FontMode, TextRenderer, TextRole, UiScale};
use sbm_gfx::{CardView, Framebuffer, MenuState, Rgba, paint_menu_with, paint_splash_with};

fn sample_cards() -> Vec<CardView> {
    vec![
        CardView {
            title: "Windows".into(),
            subtitle: "Windows · bootmgfw.efi".into(),
            hint: "windows".into(),
        },
        CardView {
            title: "Fedora Linux".into(),
            subtitle: "Linux · fedora.efi".into(),
            hint: "linux".into(),
        },
        CardView {
            title: "systemd-boot".into(),
            subtitle: "Linux · systemd-bootaa64.efi".into(),
            hint: "systemd".into(),
        },
        CardView {
            title: "Ubuntu".into(),
            subtitle: "Linux · shimaa64.efi".into(),
            hint: "ubuntu".into(),
        },
        CardView {
            title: "SBM Core Slot A".into(),
            subtitle: "Recovery · sbm-core-aarch64.efi".into(),
            hint: "recovery".into(),
        },
        CardView {
            title: "SBM Core Slot B".into(),
            subtitle: "Recovery · sbm-core-aarch64.efi".into(),
            hint: "recovery".into(),
        },
    ]
}

fn median_us(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.total_cmp(b));
    let n = samples.len();
    if n % 2 == 1 {
        samples[n / 2]
    } else {
        (samples[n / 2 - 1] + samples[n / 2]) * 0.5
    }
}

fn main() {
    let cards = sample_cards();
    let state = MenuState::new(0, true, None);

    println!("=== Bundled Font Data ===");
    println!("total_bytes={BUNDLED_FONT_BYTES}");

    println!("\n=== Cold First-Frame Menu Render (median of 11 cold runs, us) ===");
    println!(
        "{:<10} {:>12} {:>14} {:>16} {:>10} {:>10}",
        "Res", "Bitmap8x8", "AtkinsonNext", "AtkinsonNextMono", "CacheEnt", "CacheBytes"
    );
    for &(w, h) in &[(800, 600), (1280, 800), (1920, 1080), (3840, 2160)] {
        let mut fb = Framebuffer::new(w, h);
        let mut results = [0.0f64; 3];
        let mut final_entries = 0usize;
        let mut final_bytes = 0usize;
        for (idx, mode) in [
            FontMode::Bitmap8x8,
            FontMode::AtkinsonNext,
            FontMode::AtkinsonNextMono,
        ]
        .into_iter()
        .enumerate()
        {
            let mut samples = Vec::with_capacity(11);
            for _ in 0..11 {
                let mut renderer = TextRenderer::new(mode, h, UiScale::default());
                let t0 = Instant::now();
                paint_menu_with(&mut fb, &cards, &state, false, &mut renderer);
                let dt = t0.elapsed().as_secs_f64() * 1e6;
                black_box(fb.px[0]);
                samples.push(dt);
                if mode == FontMode::AtkinsonNextMono {
                    let s = renderer.cache_stats();
                    final_entries = s.entries;
                    final_bytes = s.bytes;
                }
            }
            results[idx] = median_us(samples);
        }
        println!(
            "{:<10} {:>12.1} {:>14.1} {:>16.1} {:>10} {:>10}",
            format!("{w}x{h}"),
            results[0],
            results[1],
            results[2],
            final_entries,
            final_bytes
        );
    }

    println!("\n=== Cold Splash First-Frame Render (median of 11 cold runs, us) ===");
    for &(w, h) in &[(1024, 768), (1280, 800), (1920, 1080)] {
        let mut fb = Framebuffer::new(w, h);
        let mut bm = Vec::new();
        let mut vec = Vec::new();
        for _ in 0..11 {
            let mut r_bm = TextRenderer::new(FontMode::Bitmap8x8, h, UiScale::default());
            let t0 = Instant::now();
            paint_splash_with(&mut fb, true, false, &mut r_bm);
            bm.push(t0.elapsed().as_secs_f64() * 1e6);
            black_box(fb.px[0]);

            let mut r_vec = TextRenderer::new(FontMode::AtkinsonNextMono, h, UiScale::default());
            let t1 = Instant::now();
            paint_splash_with(&mut fb, true, false, &mut r_vec);
            vec.push(t1.elapsed().as_secs_f64() * 1e6);
            black_box(fb.px[0]);
        }
        println!(
            "{w}x{h}: Bitmap8x8={:.1} us  AtkinsonNextMono={:.1} us",
            median_us(bm),
            median_us(vec)
        );
    }

    println!("\n=== Repeated Full-Menu Render (50 frames): Uncached vs Bounded LRU Cache ===");
    for &(w, h) in &[(1024, 768), (1280, 800), (1920, 1080)] {
        let mut fb = Framebuffer::new(w, h);
        let mut uncached = TextRenderer::new(FontMode::AtkinsonNextMono, h, UiScale::default());
        uncached.set_cache_enabled(false);
        let t0 = Instant::now();
        for i in 0..50 {
            let mut st = state.clone();
            st.selected = i % cards.len();
            paint_menu_with(&mut fb, &cards, &st, false, &mut uncached);
            black_box(fb.px[0]);
        }
        let uncached_per_frame = (t0.elapsed().as_secs_f64() * 1e6) / 50.0;

        let mut cached = TextRenderer::new(FontMode::AtkinsonNextMono, h, UiScale::default());
        cached.set_cache_enabled(true);
        let t1 = Instant::now();
        for i in 0..50 {
            let mut st = state.clone();
            st.selected = i % cards.len();
            paint_menu_with(&mut fb, &cards, &st, false, &mut cached);
            black_box(fb.px[0]);
        }
        let cached_per_frame = (t1.elapsed().as_secs_f64() * 1e6) / 50.0;
        let stats = cached.cache_stats();
        println!(
            "{w}x{h}: uncached={uncached_per_frame:.1} us/frame  cached={cached_per_frame:.1} us/frame  speedup={:.2}x  cache={} entries / {} B (hits={} misses={} evictions={})",
            uncached_per_frame / cached_per_frame,
            stats.entries,
            stats.bytes,
            stats.hits,
            stats.misses,
            stats.evictions
        );
    }

    println!(
        "\n=== Isolated Text-Only Rendering (100 iterations of 6 mixed-role lines at 1280x800) ==="
    );
    let lines: [(TextRole, &str); 6] = [
        (TextRole::Display, "Smart Boot Menu 2026"),
        (TextRole::Heading, "Fedora Linux 43 Workstation"),
        (
            TextRole::Body,
            "Choose an operating system to start immediately",
        ),
        (
            TextRole::Caption,
            "Arrows move · Enter boot · Tab actions · T theme · D diagnostics",
        ),
        (
            TextRole::Technical,
            "\\EFI\\fedora\\shimx64.efi  0x001A4F80  NVMe0n1p1  512.0 GiB",
        ),
        (
            TextRole::Technical,
            "PARTUUID=8f3c2d10-9b4a-4e21-8c77-1a2b3c4d5e6f  SHA-256 verified",
        ),
    ];
    let mut fb = Framebuffer::new(1280, 800);
    let fg = Rgba::rgb(0xF2, 0xF5, 0xF8);

    for (label, mode, cache_on) in [
        ("Bitmap8x8", FontMode::Bitmap8x8, false),
        (
            "AtkinsonNextMono (uncached)",
            FontMode::AtkinsonNextMono,
            false,
        ),
        (
            "AtkinsonNextMono (cached)",
            FontMode::AtkinsonNextMono,
            true,
        ),
    ] {
        let mut r = TextRenderer::new(mode, 800, UiScale::default());
        r.set_cache_enabled(cache_on);
        let t0 = Instant::now();
        for _ in 0..100 {
            let mut y = 24u32;
            for &(role, s) in &lines {
                r.draw_top(&mut fb, s, 24, y, role, fg);
                y += r.line_metrics(role).line_height + 6;
            }
            black_box(fb.px[24 * 1280 + 24]);
        }
        let per_iter = (t0.elapsed().as_secs_f64() * 1e6) / 100.0;
        let st = r.cache_stats();
        println!(
            "{label:<30}: {per_iter:>7.1} us/pass  (cache={} entries, {} B, hits={}, misses={})",
            st.entries, st.bytes, st.hits, st.misses
        );
    }

    #[cfg(feature = "experimental-layouts")]
    bench_experimental_layouts();
}

#[cfg(feature = "experimental-layouts")]
fn bench_experimental_layouts() {
    use sbm_gfx::hardware::{
        BoardSpec, BootTargetView, CpuSpec, DiskRegion, DisplaySpec, ExperimentalOptions,
        FirmwareSpec, LayoutMode, MemorySpec, NicSpec, PartitionKind, PartitionView, ProbeState,
        RegionKind, SecureBootSummary, SecuritySpec, StorageDetail, StorageDevice, SystemSpec,
        paint_experimental_with,
    };

    let make_spec = |w: u32, h: u32| SystemSpec {
        host_label: "Workstation LBA Stress".into(),
        cpu: ProbeState::Ready(CpuSpec {
            model: Some("AMD Ryzen 9 9950X".into()),
            arch: "x86_64".into(),
            cores: Some(16),
        }),
        memory: ProbeState::Ready(MemorySpec { total_mib: 65536 }),
        board: ProbeState::Ready(BoardSpec {
            vendor: Some("ASUS".into()),
            product: Some("ProArt X870E".into()),
        }),
        gpu: ProbeState::Ready(vec!["AMD Radeon RX 9070 XT".into()]),
        firmware: ProbeState::Ready(FirmwareSpec {
            vendor: Some("American Megatrends".into()),
            version: "0.1.0".into(),
            arch: "x86_64".into(),
        }),
        display: DisplaySpec {
            width: w,
            height: h,
        },
        security: ProbeState::Ready(SecuritySpec {
            secure_boot: SecureBootSummary::Enabled,
            tpm_present: Some(true),
        }),
        network: ProbeState::Ready(vec![NicSpec {
            name: "eth0".into(),
            wireless: false,
            up: true,
            address: Some("192.168.50.14/24".into()),
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
                    start_lba: 2048,
                    end_lba: 1_050_624,
                    size_bytes: 536_870_912,
                    kind: RegionKind::Partition(PartitionView {
                        index: 1,
                        label: "EFI".into(),
                        kind: PartitionKind::Esp,
                        fs: Some("FAT32".into()),
                        free_bytes: Some(320 * 1024 * 1024),
                        encrypted: None,
                        is_recovery: false,
                        detected_os: None,
                        loader_count: 4,
                        boot_targets: vec![BootTargetView {
                            card_index: 3,
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
                    start_lba: 1_050_624,
                    end_lba: 135_268_352,
                    size_bytes: 68_719_476_736,
                    kind: RegionKind::Unallocated,
                },
                DiskRegion {
                    start_lba: 135_268_352,
                    end_lba: 1_700_000_000,
                    size_bytes: 801_142_603_776,
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
                DiskRegion {
                    start_lba: 1_700_000_000,
                    end_lba: 3_100_000_000,
                    size_bytes: 716_800_000_000,
                    kind: RegionKind::Partition(PartitionView {
                        index: 3,
                        label: "fedora".into(),
                        kind: PartitionKind::Linux,
                        fs: Some("Btrfs".into()),
                        free_bytes: None,
                        encrypted: Some("LUKS2".into()),
                        is_recovery: false,
                        detected_os: Some("Fedora Linux 45".into()),
                        loader_count: 0,
                        boot_targets: vec![BootTargetView {
                            card_index: 1,
                            title: "Fedora Linux 45".into(),
                            family: "Linux".into(),
                            hint: "linux".into(),
                            loader_file: "shimx64.efi".into(),
                            esp_part: Some(1),
                            os_part: Some(3),
                            healthy: true,
                            is_recovery: false,
                            note: None,
                        }],
                    }),
                },
                DiskRegion {
                    start_lba: 3_100_000_000,
                    end_lba: 3_900_000_000,
                    size_bytes: 409_600_000_000,
                    kind: RegionKind::Partition(PartitionView {
                        index: 4,
                        label: "arch".into(),
                        kind: PartitionKind::Linux,
                        fs: Some("ext4".into()),
                        free_bytes: None,
                        encrypted: None,
                        is_recovery: false,
                        detected_os: Some("Arch Linux".into()),
                        loader_count: 0,
                        boot_targets: vec![
                            BootTargetView {
                                card_index: 2,
                                title: "Arch Linux".into(),
                                family: "Linux".into(),
                                hint: "linux".into(),
                                loader_file: "vmlinuz-linux".into(),
                                esp_part: Some(1),
                                os_part: Some(4),
                                healthy: true,
                                is_recovery: false,
                                note: None,
                            },
                            BootTargetView {
                                card_index: 4,
                                title: "systemd-boot".into(),
                                family: "Linux".into(),
                                hint: "systemd".into(),
                                loader_file: "systemd-bootx64.efi".into(),
                                esp_part: Some(1),
                                os_part: Some(4),
                                healthy: true,
                                is_recovery: false,
                                note: None,
                            },
                        ],
                    }),
                },
            ],
        }]),
        policy_note: Some("DEFAULT: WINDOWS".into()),
        discovery_status: None,
    };

    println!(
        "\n=== Experimental LBA Paragraph Dashboards (1280x800 & 1920x1080, AtkinsonNextMono) ==="
    );
    println!(
        "{:<10} {:<10} {:>12} {:>12} {:>10} {:>10}",
        "Layout", "Res", "Cold (us)", "Warm (us)", "CacheEnt", "CacheBytes"
    );
    let state = MenuState::new(0, true, Some(8_000));
    for detail in StorageDetail::ALL {
        println!("\n--- Storage detail: {} ---", detail.id());
        let opts = ExperimentalOptions {
            detail,
            ..ExperimentalOptions::default()
        };
        for mode in LayoutMode::EXPERIMENTS {
            for &(w, h) in &[(1280u32, 800u32), (1920, 1080)] {
                let spec = make_spec(w, h);
                let mut fb = Framebuffer::new(w, h);
                let mut cold_samples = Vec::with_capacity(9);
                for _ in 0..9 {
                    let mut tr =
                        TextRenderer::new(FontMode::AtkinsonNextMono, h, UiScale::default());
                    let t0 = Instant::now();
                    let _ = paint_experimental_with(&mut fb, mode, &spec, &state, opts, &mut tr);
                    cold_samples.push(t0.elapsed().as_secs_f64() * 1e6);
                    black_box(fb.px[0]);
                }
                let mut tr = TextRenderer::new(FontMode::AtkinsonNextMono, h, UiScale::default());
                let _ = paint_experimental_with(&mut fb, mode, &spec, &state, opts, &mut tr);
                let t0 = Instant::now();
                for i in 0..30 {
                    let mut st = state.clone();
                    st.selected = i % 5;
                    let _ = paint_experimental_with(&mut fb, mode, &spec, &st, opts, &mut tr);
                    black_box(fb.px[0]);
                }
                let warm_us = (t0.elapsed().as_secs_f64() * 1e6) / 30.0;
                let st = tr.cache_stats();
                println!(
                    "{:<10} {:<10} {:>12.1} {:>12.1} {:>10} {:>10}",
                    mode.id(),
                    format!("{w}x{h}"),
                    median_us(cold_samples),
                    warm_us,
                    st.entries,
                    st.bytes
                );
            }
        }
    }
}
