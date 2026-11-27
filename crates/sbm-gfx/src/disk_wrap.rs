//! Paragraph-style 1D-LBA-to-2D wrapped orthogonal-polygon disk layout engine.
//!
//! Models a physical disk as a single one-dimensional Logical Block Address
//! (LBA) sequence wrapped left-to-right, top-to-bottom across rows like text in
//! a paragraph. Partitions and unallocated gaps stay strictly in ascending LBA
//! order and may wrap across multiple rows into single connected rectilinear
//! polygons (rectangles, L-shapes, inverted-L shapes, Z-shapes, or multi-row
//! ribbons) with merged internal borders.
//!
//! Informed by:
//! - Bederson, Shneiderman & Wattenberg (2002), *Ordered and Quantum Treemaps*
//!   (strip ordering, readability/direction-change preservation, quantum minimum
//!   areas, ragged-edge justification).
//! - Baudel & Broeksema (2012), *Capturing the Design Space of Sequential
//!   Space-Filling Layouts* (Order / Size / Score / Phrase decomposition).
//! - de Berg, Speckmann & van der Weele (2011), *Treemaps with Bounded Aspect
//!   Ratio* (relaxing single-rectangle constraints to connected rectilinear
//!   polygons to bound distortion while preserving exact order).

use alloc::vec;
use alloc::vec::Vec;

use crate::draw::{Framebuffer, Rgba};
use crate::layout::Rect;

/// Input descriptor for one LBA-ordered disk region (partition or unallocated gap).
#[derive(Clone, Copy, Debug)]
pub struct WrapInput {
    /// Exact capacity in bytes (independent of visual geometry).
    pub size_bytes: u64,
    /// Minimum linear width (in pixels along a row) needed for readability / interaction.
    pub min_width: u32,
    /// Minimum inscribed height needed for legible label / boot-target content.
    pub min_height: u32,
    /// True if this region carries selectable boot target(s).
    pub interactive: bool,
}

/// One laid-out partition or unallocated region as a connected orthogonal polygon.
#[derive(Clone, Debug)]
pub struct WrappedRegion {
    /// Exact index in the LBA-sorted input slice.
    pub index: usize,
    /// Contiguous row segments ordered top-to-bottom; adjacent segments overlap
    /// horizontally so the union forms one edge-connected orthogonal polygon.
    pub segments: Vec<Rect>,
    /// Largest axis-aligned inscribed rectangle inside the polygon for content placement.
    pub label_rect: Rect,
    /// Total rendered pixel area across all segments.
    pub pixel_area: u64,
    /// Exact original byte capacity preserved independently of visual geometry.
    pub actual_bytes: u64,
    /// True when minimum readable constraints enlarged this region above its raw byte share.
    pub clamped: bool,
}

impl WrappedRegion {
    /// Hit-test anywhere inside the connected orthogonal polygon.
    pub fn contains(&self, x: u32, y: u32) -> bool {
        self.segments.iter().any(|s| s.contains(x, y))
    }

    /// True if this region wraps across two or more rows (L / Z / multi-row polygon).
    pub fn is_polygonal(&self) -> bool {
        self.segments.len() >= 2
    }

    /// Verify that every consecutive pair of row segments overlaps horizontally
    /// so the polygon is strictly edge-connected.
    pub fn is_connected(&self) -> bool {
        for pair in self.segments.windows(2) {
            let a = pair[0];
            let b = pair[1];
            let overlap_start = a.x.max(b.x);
            let overlap_end = (a.x + a.w).min(b.x + b.w);
            if overlap_end <= overlap_start {
                return false;
            }
        }
        true
    }
}

/// Diagnostic quality metrics for evaluating candidate row counts / breaks.
#[derive(Clone, Copy, Debug, Default)]
pub struct WrapMetrics {
    /// L1 capacity distortion in permille (`0` = exact proportional areas, `1000` = worst).
    pub distortion_permille: u32,
    /// Fraction of container area filled in permille (`1000` = 100% space-filling).
    pub fill_permille: u32,
    /// True iff every multi-row region is strictly edge-connected.
    pub all_connected: bool,
}

/// Complete wrapped layout for one physical disk's LBA sequence.
#[derive(Clone, Debug)]
pub struct WrapLayout {
    pub rows: usize,
    pub regions: Vec<WrappedRegion>,
    pub metrics: WrapMetrics,
}

/// Compute the optimal paragraph-wrapped LBA orthogonal-polygon layout inside `bounds`.
///
/// Hard invariants guaranteed for any non-empty input:
/// 1. Output `regions` preserve exact input LBA order (`regions[i].index == i`).
/// 2. Every input partition and unallocated gap appears exactly once.
/// 3. Every multi-row region is a single edge-connected orthogonal polygon.
/// 4. Segments tile `bounds` cleanly without overlaps or dead holes.
pub fn layout_disk_wrap(bounds: Rect, items: &[WrapInput]) -> WrapLayout {
    if items.is_empty() || bounds.w == 0 || bounds.h == 0 {
        return WrapLayout {
            rows: 0,
            regions: Vec::new(),
            metrics: WrapMetrics {
                distortion_permille: 0,
                fill_permille: 0,
                all_connected: true,
            },
        };
    }

    let n = items.len();
    let max_min_h = items
        .iter()
        .map(|it| it.min_height)
        .max()
        .unwrap_or(28)
        .max(20);
    // Single-region or 2-region tiny disks rarely benefit from multi-row wrapping;
    // multi-region disks evaluate K = 1..=k_max to balance distortion, readability,
    // and connected L/Z polygon ergonomics.
    let k_max = if n == 1 {
        1
    } else {
        let by_h = (bounds.h / max_min_h).clamp(1, 5) as usize;
        let by_n = (n + 1).min(5);
        by_h.min(by_n).max(1)
    };

    let mut best: Option<(i64, WrapLayout)> = None;
    for k in 1..=k_max {
        let candidate = wrap_for_rows(bounds, items, k);
        let score = score_layout(bounds, items, &candidate);
        match &best {
            Some((best_score, _)) if *best_score <= score => {}
            _ => best = Some((score, candidate)),
        }
    }

    best.map(|(_, layout)| layout)
        .unwrap_or_else(|| wrap_for_rows(bounds, items, 1))
}

/// Multi-objective score (lower is better), aligned with priority order:
/// (3) minimum readable/interactive area -> (4) capacity fidelity ->
/// (5) space efficiency -> (6) label-box quality & polygon compactness.
fn score_layout(bounds: Rect, items: &[WrapInput], layout: &WrapLayout) -> i64 {
    let mut cost: i64 = 0;

    // Hard connectivity guard.
    if !layout.metrics.all_connected {
        cost += 1_000_000;
    }

    // Priority 3: Minimum readable & interactive area.
    for (item, reg) in items.iter().zip(&layout.regions) {
        let lw = reg.label_rect.w;
        let lh = reg.label_rect.h;
        if lw < item.min_width {
            let deficit = (item.min_width - lw) as i64;
            let mult = if item.interactive { 80 } else { 35 };
            cost += deficit * mult;
        }
        if lh < item.min_height {
            let deficit = (item.min_height - lh) as i64;
            let mult = if item.interactive { 90 } else { 40 };
            cost += deficit * mult;
        }
        // Very narrow slivers hurt readability sharply.
        if lw < 36 {
            cost += (36 - lw) as i64 * 140;
        }
    }

    // Priority 4: Capacity proportion fidelity (L1 area distortion in permille).
    cost += (layout.metrics.distortion_permille as i64) * 14;

    // Priority 5: Area fill efficiency.
    cost += (1000u32.saturating_sub(layout.metrics.fill_permille) as i64) * 30;

    // Priority 6: Label-box aspect ratio, visual continuity & polygon seam quality.
    for (item, reg) in items.iter().zip(&layout.regions) {
        let lw = reg.label_rect.w.max(1) as i64;
        let lh = reg.label_rect.h.max(1) as i64;
        // Prefer comfortable horizontal label boxes (aspect ratio between ~1.2 and ~4.5).
        let ratio_x10 = if lw >= lh {
            (lw * 10) / lh
        } else {
            (lh * 10) / lw
        };
        if ratio_x10 > 65 {
            cost += (ratio_x10 - 65) * 4;
        }
        if lh > lw && item.interactive {
            // Tall skinny column hurts horizontal OS names.
            cost += ((lh - lw) * 18) / lh.max(1);
        }
        // Reward generous shared horizontal seam on multi-row polygons so L/Z shapes
        // feel unmistakably unified rather than barely touching.
        for pair in reg.segments.windows(2) {
            let overlap = pair[0]
                .x
                .saturating_add(pair[0].w)
                .min(pair[1].x.saturating_add(pair[1].w))
                .saturating_sub(pair[0].x.max(pair[1].x));
            let target_seam = (bounds.w / 8).clamp(24, 60);
            if overlap < target_seam {
                cost += (target_seam - overlap) as i64 * 20;
            }
        }
    }

    // Mild preference against gratuitous extra rows when fewer rows already score well.
    if layout.rows > 1 && items.len() <= 2 {
        cost += (layout.rows as i64 - 1) * 900;
    }

    cost
}

/// Water-filling clamped proportional projection of byte weights onto total ribbon length `total_len`.
///
/// Minimizes L1/L2 capacity distortion subject to `len[i] >= min_len[i]`.
fn project_lengths(items: &[WrapInput], total_len: u32) -> (Vec<u32>, Vec<bool>) {
    let n = items.len();
    let total_len = total_len.max(n as u32);
    let raw_mins: Vec<u32> = items.iter().map(|it| it.min_width.max(1)).collect();
    let sum_mins: u64 = raw_mins.iter().map(|&v| v as u64).sum();

    // Scale minimums down proportionally only when even minimums exceed total ribbon length.
    let eff_mins: Vec<u32> = if sum_mins <= total_len as u64 {
        raw_mins
    } else {
        let mut scaled: Vec<u32> = raw_mins
            .iter()
            .map(|&m| ((m as u64 * total_len as u64) / sum_mins).max(1) as u32)
            .collect();
        // Ensure exact fit under total_len.
        let mut sum: u32 = scaled.iter().sum();
        while sum > total_len {
            if let Some((idx, _)) = scaled
                .iter()
                .enumerate()
                .filter(|(_, v)| **v > 1)
                .max_by_key(|(_, v)| **v)
            {
                scaled[idx] -= 1;
                sum -= 1;
            } else {
                break;
            }
        }
        scaled
    };

    let weights: Vec<u64> = items.iter().map(|it| it.size_bytes.max(1)).collect();
    let mut clamped = vec![false; n];
    let mut lens = vec![0u32; n];

    loop {
        let fixed_len: u32 = (0..n).filter(|&i| clamped[i]).map(|i| eff_mins[i]).sum();
        let free_len = total_len.saturating_sub(fixed_len);
        let free_weight: u128 = (0..n)
            .filter(|&i| !clamped[i])
            .map(|i| weights[i] as u128)
            .sum();

        if free_weight == 0 {
            lens[..n].copy_from_slice(&eff_mins[..n]);
            break;
        }

        let mut newly_clamped = false;
        for i in 0..n {
            if clamped[i] {
                lens[i] = eff_mins[i];
            } else {
                let share = ((weights[i] as u128 * free_len as u128) / free_weight) as u32;
                if share < eff_mins[i] {
                    clamped[i] = true;
                    lens[i] = eff_mins[i];
                    newly_clamped = true;
                } else {
                    lens[i] = share;
                }
            }
        }
        if !newly_clamped {
            break;
        }
    }

    // Distribute integer rounding remainder to the largest unclamped (or overall largest) region.
    let current_sum: u32 = lens.iter().sum();
    if current_sum < total_len {
        let diff = total_len - current_sum;
        let target = (0..n)
            .filter(|&i| !clamped[i])
            .max_by_key(|&i| weights[i])
            .or_else(|| (0..n).max_by_key(|&i| weights[i]))
            .unwrap_or(0);
        lens[target] = lens[target].saturating_add(diff);
    } else if current_sum > total_len {
        let mut excess = current_sum - total_len;
        while excess > 0 {
            if let Some(target) = (0..n).filter(|&i| lens[i] > 1).max_by_key(|&i| lens[i]) {
                lens[target] -= 1;
                excess -= 1;
            } else {
                break;
            }
        }
    }

    (lens, clamped)
}

/// Lay out `items` across up to `k` rows using paragraph-style wrapping with
/// adaptive row breaks and guaranteed horizontal overlap for multi-row polygons.
fn wrap_for_rows(bounds: Rect, items: &[WrapInput], k: usize) -> WrapLayout {
    let n = items.len();
    let w = bounds.w;
    let total_len = w.saturating_mul(k as u32);
    let (vis_len, clamped) = project_lengths(items, total_len);

    // Precompute suffix sums of minimum widths so wrapping never starves trailing regions.
    let mut suffix_min = vec![0u32; n + 1];
    for i in (0..n).rev() {
        let floor = (items[i].min_width / 2).max(16).min(vis_len[i]);
        suffix_min[i] = suffix_min[i + 1].saturating_add(floor);
    }

    let min_seam = (w / 10).clamp(18, 48).min(w / 2);
    let min_stub = (w / 12).clamp(22, 52).min(w / 2);

    // Each row stores `(item_index, x0, x1)` relative to `0..w`.
    let mut row_spans: Vec<Vec<(usize, u32, u32)>> = vec![Vec::new(); k];
    let mut cur_row: usize = 0;
    let mut cur_x: u32 = 0;

    let mut i = 0;
    while i < n {
        // Last available row: pack all remaining items cleanly onto `cur_row`.
        if cur_row + 1 >= k {
            let rem_w = w.saturating_sub(cur_x);
            let rem_slice = &vis_len[i..];
            let sum_rem: u64 = rem_slice.iter().map(|&v| v as u64).sum::<u64>().max(1);
            let mut x = cur_x;
            for (offset, &len) in rem_slice.iter().enumerate() {
                let idx = i + offset;
                let x1 = if idx + 1 == n {
                    w
                } else {
                    let seg_w = ((len as u64 * rem_w as u64) / sum_rem).max(1) as u32;
                    let left_items = (n - 1 - idx) as u32;
                    x.saturating_add(seg_w).min(w.saturating_sub(left_items))
                };
                if x1 > x {
                    row_spans[cur_row].push((idx, x, x1));
                    x = x1;
                }
            }
            break;
        }

        let rem = w.saturating_sub(cur_x);
        // Cap length if necessary so trailing regions still have room on remaining rows.
        let total_remaining_ribbon = rem.saturating_add(w.saturating_mul((k - 1 - cur_row) as u32));
        let max_allowed = total_remaining_ribbon
            .saturating_sub(suffix_min[i + 1])
            .max(1);
        let need = vis_len[i].min(max_allowed);

        if need <= rem {
            // Fits on the current row.
            let x1 = cur_x.saturating_add(need);
            row_spans[cur_row].push((i, cur_x, x1));
            cur_x = x1;
            if cur_x >= w && i + 1 < n {
                cur_row += 1;
                cur_x = 0;
            }
            i += 1;
            continue;
        }

        // `need > rem`: candidate multi-row wrap across the right boundary!
        let spill = need.saturating_sub(rem);
        let connected_wrap =
            rem >= min_stub && (spill >= w || spill >= cur_x.saturating_add(min_seam));

        if connected_wrap {
            // Emit top segment on `cur_row`, any full middle rows, and bottom tail.
            row_spans[cur_row].push((i, cur_x, w));
            cur_row += 1;
            let mut left = spill;
            while left > w && cur_row + 1 < k {
                let ribbon_after = w.saturating_mul((k - 1 - cur_row) as u32);
                if ribbon_after < suffix_min[i + 1] {
                    break;
                }
                row_spans[cur_row].push((i, 0, w));
                left = left.saturating_sub(w);
                cur_row += 1;
            }
            let tail_max = w
                .saturating_add(w.saturating_mul((k - 1 - cur_row) as u32))
                .saturating_sub(suffix_min[i + 1])
                .min(w)
                .max(1);
            let tail = left.min(tail_max).max(1);
            row_spans[cur_row].push((i, 0, tail));
            cur_x = tail;
            if cur_x >= w && cur_row + 1 < k && i + 1 < n {
                cur_row += 1;
                cur_x = 0;
            }
            i += 1;
        } else if cur_x > 0 {
            // Adaptive row break: justify `cur_row` to full width and start item `i` cleanly
            // at `x = 0` on `cur_row + 1` (so any multi-row wrap becomes a connected L-shape!).
            justify_row(&mut row_spans[cur_row], w);
            cur_row += 1;
            cur_x = 0;
            // Re-evaluate item `i` at `(cur_row, 0)`.
        } else {
            // Already at `cur_x == 0`, so `need > w`: guaranteed connected L-shape!
            row_spans[cur_row].push((i, 0, w));
            cur_row += 1;
            let mut left = need.saturating_sub(w);
            while left > w && cur_row + 1 < k {
                let ribbon_after = w.saturating_mul((k - 1 - cur_row) as u32);
                if ribbon_after < suffix_min[i + 1] {
                    break;
                }
                row_spans[cur_row].push((i, 0, w));
                left = left.saturating_sub(w);
                cur_row += 1;
            }
            let tail_max = w
                .saturating_add(w.saturating_mul((k - 1 - cur_row) as u32))
                .saturating_sub(suffix_min[i + 1])
                .min(w)
                .max(1);
            let tail = left.min(tail_max).max(1);
            row_spans[cur_row].push((i, 0, tail));
            cur_x = tail;
            if cur_x >= w && cur_row + 1 < k && i + 1 < n {
                cur_row += 1;
                cur_x = 0;
            }
            i += 1;
        }
    }

    // Drop any unused trailing rows and justify the final row to 100% width.
    while row_spans.len() > 1 && row_spans.last().is_some_and(Vec::is_empty) {
        row_spans.pop();
    }
    if let Some(last_row) = row_spans.last_mut() {
        justify_row(last_row, w);
    }

    let used_rows = row_spans.len().max(1);
    // Compute crisp integer row vertical bounds covering 100% of `bounds.h`.
    let mut row_ys = Vec::with_capacity(used_rows + 1);
    for r in 0..=used_rows {
        let y = bounds.y + ((bounds.h as u64 * r as u64) / used_rows as u64) as u32;
        row_ys.push(y);
    }

    // Assemble per-region segments in exact LBA order.
    let mut segs_by_item: Vec<Vec<Rect>> = vec![Vec::new(); n];
    for (r, spans) in row_spans.iter().enumerate() {
        let y0 = row_ys[r];
        let rh = row_ys[r + 1].saturating_sub(y0);
        for &(item_idx, x0, x1) in spans {
            if x1 > x0 && rh > 0 {
                segs_by_item[item_idx].push(Rect {
                    x: bounds.x.saturating_add(x0),
                    y: y0,
                    w: x1 - x0,
                    h: rh,
                });
            }
        }
    }

    let total_bytes: u128 = items.iter().map(|it| it.size_bytes.max(1) as u128).sum();
    let bounds_area: u64 = (bounds.w as u64).saturating_mul(bounds.h as u64).max(1);
    let mut total_poly_area: u64 = 0;
    let mut all_connected = true;
    let mut l1_num: u128 = 0;

    let mut regions = Vec::with_capacity(n);
    for (idx, segs) in segs_by_item.into_iter().enumerate() {
        let pixel_area: u64 = segs.iter().map(|s| s.w as u64 * s.h as u64).sum();
        total_poly_area = total_poly_area.saturating_add(pixel_area);
        let label_rect = best_label_rect(&segs);
        let reg = WrappedRegion {
            index: idx,
            segments: segs,
            label_rect,
            pixel_area,
            actual_bytes: items[idx].size_bytes,
            clamped: clamped[idx],
        };
        if !reg.is_connected() {
            all_connected = false;
        }
        // L1 share difference scaled to 10000.
        let area_share = (pixel_area as u128 * 10_000) / bounds_area as u128;
        let byte_share = (items[idx].size_bytes.max(1) as u128 * 10_000) / total_bytes.max(1);
        l1_num = l1_num.saturating_add(area_share.abs_diff(byte_share));
        regions.push(reg);
    }

    WrapLayout {
        rows: used_rows,
        regions,
        metrics: WrapMetrics {
            distortion_permille: (l1_num / 20) as u32,
            fill_permille: ((total_poly_area.min(bounds_area) * 1000) / bounds_area) as u32,
            all_connected,
        },
    }
}

/// Stretch `spans` on a single row linearly from `[0, cur_end]` to `[0, target_w]`,
/// keeping every span non-empty and monotonic.
fn justify_row(spans: &mut [(usize, u32, u32)], target_w: u32) {
    if spans.is_empty() {
        return;
    }
    let cur_end = spans.last().map(|s| s.2).unwrap_or(0);
    if cur_end == 0 || cur_end >= target_w {
        if let Some(last) = spans.last_mut() {
            last.2 = target_w;
        }
        return;
    }
    let count = spans.len();
    let mut prev_x = 0u32;
    for (idx, span) in spans.iter_mut().enumerate() {
        let new_x1 = if idx + 1 == count {
            target_w
        } else {
            let scaled = ((span.2 as u64 * target_w as u64) / cur_end as u64) as u32;
            let remain = (count - 1 - idx) as u32;
            scaled.max(prev_x + 1).min(target_w.saturating_sub(remain))
        };
        span.1 = prev_x;
        span.2 = new_x1;
        prev_x = new_x1;
    }
}

/// Find the optimal axis-aligned inscribed rectangle inside a connected row-polygon
/// for placing OS logos, names, exact capacity, and boot actions.
pub fn best_label_rect(segments: &[Rect]) -> Rect {
    if segments.is_empty() {
        return Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
    }
    let mut best = segments[0];
    let mut best_score = label_rect_score(best);

    for a in 0..segments.len() {
        let mut x0 = segments[a].x;
        let mut x1 = segments[a].x.saturating_add(segments[a].w);
        let y0 = segments[a].y;
        for seg in segments.iter().skip(a) {
            x0 = x0.max(seg.x);
            x1 = x1.min(seg.x.saturating_add(seg.w));
            if x1 <= x0 {
                break;
            }
            let y1 = seg.y.saturating_add(seg.h);
            let cand = Rect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1.saturating_sub(y0),
            };
            let sc = label_rect_score(cand);
            if sc > best_score {
                best_score = sc;
                best = cand;
            }
        }
    }
    best
}

fn label_rect_score(r: Rect) -> u64 {
    // Cap effective height at 120px for primary ranking so multi-row L/Z polygons
    // prefer their widest comfortably-tall horizontal band (clean left-margin alignment
    // for OS emblems, names, and BOOT badges) rather than narrow tall chimneys,
    // while breaking ties by true area.
    let eff_h = (r.h as u64).min(120);
    let primary = (r.w as u64).saturating_mul(eff_h);
    let bonus = if r.w >= 110 && r.h >= 34 {
        primary.saturating_mul(12) / 10
    } else {
        primary
    };
    let area = (r.w as u64).saturating_mul(r.h as u64);
    bonus.saturating_mul(1024).saturating_add(area / 64)
}

/// Fill all segments of a connected orthogonal polygon with a subtle diagonal hatch
/// (used for unallocated LBA regions).
pub fn hatch_polygon(fb: &mut Framebuffer, segments: &[Rect], step: u32, color: Rgba) {
    for &seg in segments {
        fb.fill_hatch(seg.x, seg.y, seg.w, seg.h, step, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::{fill_ortho_polygon, stroke_ortho_polygon};

    fn mk(gb: u64, min_w: u32, interactive: bool) -> WrapInput {
        WrapInput {
            size_bytes: gb * 1024 * 1024,
            min_width: min_w,
            min_height: 36,
            interactive,
        }
    }

    #[test]
    fn preserves_exact_order_uniqueness_connectivity_and_full_fill() {
        // Dual-boot with tiny ESP, tiny MSR, huge Windows, tiny WinRE, huge Linux, swap, gap
        let items = [
            mk(512, 80, true),        // ESP 512 MiB
            mk(16, 60, false),        // MSR 16 MiB
            mk(1_000_000, 140, true), // Windows 1 TB
            mk(1024, 70, false),      // WinRE 1 GiB
            mk(40_000, 65, false),    // Unallocated 40 GiB
            mk(800_000, 140, true),   // Linux 800 GiB
            mk(16_384, 65, false),    // Swap 16 GiB
        ];
        let bounds = Rect {
            x: 10,
            y: 20,
            w: 900,
            h: 240,
        };
        let layout = layout_disk_wrap(bounds, &items);
        assert_eq!(layout.regions.len(), items.len());
        assert!(
            layout.metrics.all_connected,
            "every multi-row polygon must be connected"
        );
        assert!(
            layout.metrics.fill_permille >= 995,
            "fill = {}",
            layout.metrics.fill_permille
        );

        // Verify exact LBA order, no overlaps, and every region hit-tests cleanly.
        for (i, reg) in layout.regions.iter().enumerate() {
            assert_eq!(reg.index, i);
            assert!(!reg.segments.is_empty());
            assert!(reg.is_connected());
            assert!(reg.label_rect.w > 0 && reg.label_rect.h > 0);
            assert!(reg.contains(
                reg.label_rect.x + reg.label_rect.w / 2,
                reg.label_rect.y + reg.label_rect.h / 2
            ));
        }

        // Verify at least one large partition actually wrapped into an irregular polygon!
        assert!(
            layout.regions.iter().any(|r| r.is_polygonal()),
            "expected multi-row irregular polygon wrap for large OS partitions"
        );
    }

    #[test]
    fn handles_many_small_partitions_and_multiple_gaps_without_dropping_any() {
        let mut items = Vec::new();
        for i in 0..14 {
            let size_mib = match i {
                0 => 100,     // tiny ESP
                3 => 900_000, // huge data
                7 => 250_000, // medium OS
                _ => 512,     // many small partitions & gaps
            };
            items.push(mk(size_mib, 60, i == 0 || i == 7));
        }
        let bounds = Rect {
            x: 0,
            y: 0,
            w: 760,
            h: 220,
        };
        let layout = layout_disk_wrap(bounds, &items);
        assert_eq!(layout.regions.len(), 14);
        assert!(layout.metrics.all_connected);
        for (i, r) in layout.regions.iter().enumerate() {
            assert_eq!(r.index, i);
            assert!(!r.segments.is_empty(), "region {i} dropped");
        }
    }

    #[test]
    fn merged_polygon_border_cancels_internal_seam() {
        let mut fb = Framebuffer::new(100, 100);
        let fill = Rgba::rgb(0x20, 0x30, 0x40);
        let border = Rgba::rgb(0xFF, 0xCC, 0x00);
        let segs = [
            Rect {
                x: 10,
                y: 10,
                w: 80,
                h: 30,
            },
            Rect {
                x: 10,
                y: 40,
                w: 50,
                h: 30,
            },
        ];
        fill_ortho_polygon(&mut fb, &segs, 0.0, fill);
        stroke_ortho_polygon(&mut fb, &segs, 1.0, 0.0, border);
        // Shared horizontal seam at x=30, y=39..40 must stay `fill` (merged!).
        assert_eq!(fb.pixel(30, 39), Some(fill));
        assert_eq!(fb.pixel(30, 40), Some(fill));
        // Exposed overhang bottom of top segment at x=75, y=39 must be `border`!
        assert_eq!(fb.pixel(75, 39), Some(border));
        // Outer top and left edges must be `border`!
        assert_eq!(fb.pixel(30, 10), Some(border));
        assert_eq!(fb.pixel(10, 50), Some(border));
    }
}
