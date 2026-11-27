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
}
