//! Unified `no_std` CPU vector rendering backend powered by `ab_glyph_rasterizer`.
//!
//! Serves typography outlines, rounded UI surfaces, irregular wrapped partition
//! polygons (`ortho_polygon`), and resolution-independent vector emblems through
//! a single analytical coverage rasterizer without extra full-screen pixmap
//! allocations.

use alloc::vec::Vec;

use ab_glyph_rasterizer::{Point, Rasterizer};

use crate::draw::{Framebuffer, Rgba};
use crate::layout::Rect;

/// Cubic Bézier handle ratio for a 90-degree circular arc: `4 * (sqrt(2) - 1) / 3`.
const KAPPA: f32 = 0.552_284_8;

/// Compact mathematical vector path command.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathOp {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

/// Resolution-independent vector path composed of lines, quadratic Béziers,
/// and cubic Béziers.
#[derive(Clone, Debug, Default)]
pub struct VectorPath {
    ops: Vec<PathOp>,
}

impl VectorPath {
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            ops: Vec::with_capacity(cap),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn ops(&self) -> &[PathOp] {
        &self.ops
    }

    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.ops.push(PathOp::MoveTo(x, y));
        self
    }

    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.ops.push(PathOp::LineTo(x, y));
        self
    }

    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) -> &mut Self {
        self.ops.push(PathOp::QuadTo(cx, cy, x, y));
        self
    }

    pub fn cubic_to(
        &mut self,
        c1x: f32,
        c1y: f32,
        c2x: f32,
        c2y: f32,
        x: f32,
        y: f32,
    ) -> &mut Self {
        self.ops.push(PathOp::CubicTo(c1x, c1y, c2x, c2y, x, y));
        self
    }

    pub fn close(&mut self) -> &mut Self {
        self.ops.push(PathOp::Close);
        self
    }

    /// Append a clockwise axis-aligned rectangle.
    pub fn push_rect(&mut self, x: f32, y: f32, w: f32, h: f32) -> &mut Self {
        if w <= 0.0 || h <= 0.0 {
            return self;
        }
        self.move_to(x, y)
            .line_to(x + w, y)
            .line_to(x + w, y + h)
            .line_to(x, y + h)
            .close()
    }

    /// Append a clockwise rounded rectangle using smooth cubic corner arcs.
    pub fn push_rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32) -> &mut Self {
        if w <= 0.0 || h <= 0.0 {
            return self;
        }
        let r = radius.max(0.0).min(w * 0.5).min(h * 0.5);
        if r <= 0.01 {
            return self.push_rect(x, y, w, h);
        }
        let k = r * KAPPA;
        let x1 = x + w;
        let y1 = y + h;
        self.move_to(x + r, y)
            .line_to(x1 - r, y)
            .cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r)
            .line_to(x1, y1 - r)
            .cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1)
            .line_to(x + r, y1)
            .cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r)
            .line_to(x, y + r)
            .cubic_to(x, y + r - k, x + r - k, y, x + r, y)
            .close()
    }

    /// Append a counter-clockwise rounded rectangle (subtractive winding hole).
    pub fn push_rounded_rect_ccw(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
    ) -> &mut Self {
        if w <= 0.0 || h <= 0.0 {
            return self;
        }
        let r = radius.max(0.0).min(w * 0.5).min(h * 0.5);
        let x1 = x + w;
        let y1 = y + h;
        if r <= 0.01 {
            return self
                .move_to(x, y)
                .line_to(x, y1)
                .line_to(x1, y1)
                .line_to(x1, y)
                .close();
        }
        let k = r * KAPPA;
        self.move_to(x + r, y)
            .cubic_to(x + r - k, y, x, y + r - k, x, y + r)
            .line_to(x, y1 - r)
            .cubic_to(x, y1 - r + k, x + r - k, y1, x + r, y1)
            .line_to(x1 - r, y1)
            .cubic_to(x1 - r + k, y1, x1, y1 - r + k, x1, y1 - r)
            .line_to(x1, y + r)
            .cubic_to(x1, y + r - k, x1 - r + k, y, x1 - r, y)
            .close()
    }

    /// Append a clockwise circle using 4 cubic Bézier arcs.
    pub fn push_circle(&mut self, cx: f32, cy: f32, r: f32) -> &mut Self {
        if r <= 0.0 {
            return self;
        }
        let k = r * KAPPA;
        self.move_to(cx, cy - r)
            .cubic_to(cx + k, cy - r, cx + r, cy - k, cx + r, cy)
            .cubic_to(cx + r, cy + k, cx + k, cy + r, cx, cy + r)
            .cubic_to(cx - k, cy + r, cx - r, cy + k, cx - r, cy)
            .cubic_to(cx - r, cy - k, cx - k, cy - r, cx, cy - r)
            .close()
    }

    /// Append a counter-clockwise circle (subtractive winding hole for rings).
    pub fn push_circle_ccw(&mut self, cx: f32, cy: f32, r: f32) -> &mut Self {
        if r <= 0.0 {
            return self;
        }
        let k = r * KAPPA;
        self.move_to(cx, cy - r)
            .cubic_to(cx - k, cy - r, cx - r, cy - k, cx - r, cy)
            .cubic_to(cx - r, cy + k, cx - k, cy + r, cx, cy + r)
            .cubic_to(cx + k, cy + r, cx + r, cy + k, cx + r, cy)
            .cubic_to(cx + r, cy - k, cx + k, cy - r, cx, cy - r)
            .close()
    }

    /// Append an antialiased ring (outer CW circle + inner CCW hole).
    pub fn push_ring(&mut self, cx: f32, cy: f32, outer_r: f32, thickness: f32) -> &mut Self {
        if outer_r <= 0.0 || thickness <= 0.0 {
            return self;
        }
        self.push_circle(cx, cy, outer_r);
        let inner_r = outer_r - thickness;
        if inner_r > 0.05 {
            self.push_circle_ccw(cx, cy, inner_r);
        }
        self
    }

    /// Append an arbitrary closed polygon with optional quadratic corner rounding.
    pub fn push_polygon(&mut self, verts: &[(f32, f32)], corner_radius: f32) -> &mut Self {
        let n = verts.len();
        if n < 3 {
            return self;
        }
        if corner_radius <= 0.01 {
            self.move_to(verts[0].0, verts[0].1);
            for &(x, y) in &verts[1..] {
                self.line_to(x, y);
            }
            return self.close();
        }
        for i in 0..n {
            let prev = verts[(i + n - 1) % n];
            let curr = verts[i];
            let next = verts[(i + 1) % n];
            let (d1x, d1y) = (prev.0 - curr.0, prev.1 - curr.1);
            let (d2x, d2y) = (next.0 - curr.0, next.1 - curr.1);
            let len1 = libm::hypotf(d1x, d1y);
            let len2 = libm::hypotf(d2x, d2y);
            let r = corner_radius.min(len1 * 0.45).min(len2 * 0.45);
            if r <= 0.01 || len1 <= 0.01 || len2 <= 0.01 {
                if i == 0 {
                    self.move_to(curr.0, curr.1);
                } else {
                    self.line_to(curr.0, curr.1);
                }
            } else {
                let p_in = (curr.0 + d1x * (r / len1), curr.1 + d1y * (r / len1));
                let p_out = (curr.0 + d2x * (r / len2), curr.1 + d2y * (r / len2));
                if i == 0 {
                    self.move_to(p_in.0, p_in.1);
                } else {
                    self.line_to(p_in.0, p_in.1);
                }
                self.quad_to(curr.0, curr.1, p_out.0, p_out.1);
            }
        }
        self.close()
    }

    /// Build a closed vector path for a rounded rectangle.
    pub fn rounded_rect(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Self {
        let mut path = Self::with_capacity(12);
        path.push_rounded_rect(x, y, w, h, radius);
        path
    }

    /// Build a closed vector path for an antialiased rounded border frame.
    pub fn rounded_frame(x: f32, y: f32, w: f32, h: f32, radius: f32, thickness: f32) -> Self {
        let mut path = Self::with_capacity(24);
        path.push_rounded_rect(x, y, w, h, radius);
        let t = thickness.max(0.0);
        if w > t * 2.0 && h > t * 2.0 {
            path.push_rounded_rect_ccw(
                x + t,
                y + t,
                w - t * 2.0,
                h - t * 2.0,
                (radius - t).max(0.0),
            );
        }
        path
    }

    /// Build a connected outer-perimeter vector path from top-to-bottom
    /// rectilinear row segments (such as wrapped LBA partition polygons).
    ///
    /// Internal shared seams between adjacent segments are merged away.
    pub fn ortho_polygon(segments: &[Rect], corner_radius: f32) -> Self {
        let verts = ortho_outer_vertices(segments);
        let mut path = Self::with_capacity(verts.len() * 2 + 2);
        path.push_polygon(&verts, corner_radius);
        path
    }

    /// Build an antialiased outer-perimeter border frame for a connected
    /// rectilinear polygon (`segments`), omitting internal seams.
    pub fn ortho_polygon_frame(segments: &[Rect], thickness: f32, corner_radius: f32) -> Self {
        let outer = ortho_outer_vertices(segments);
        if outer.len() < 4 || thickness <= 0.0 {
            return Self::new();
        }
        if let Some(mut inner_verts) = inset_ortho_polygon(&outer, thickness) {
            let mut path = Self::with_capacity(outer.len() * 4 + 4);
            path.push_polygon(&outer, corner_radius);
            inner_verts.reverse();
            path.push_polygon(&inner_verts, (corner_radius - thickness).max(0.0));
            return path;
        }
        // Fallback for polygons containing very short step ledges (< 2 * thickness):
        // build explicit inward edge strips + reflex corner fills so the interior
        // is never filled solid while still cancelling all shared internal seams.
        let n = outer.len();
        let mut path = Self::with_capacity(n * 6);
        let t = thickness;
        for i in 0..n {
            let a = outer[i];
            let b = outer[(i + 1) % n];
            let dx = b.0 - a.0;
            let dy = b.1 - a.1;
            if dx.abs() >= dy.abs() {
                if dx > 0.0 {
                    path.push_rect(a.0, a.1, dx, t);
                } else if dx < 0.0 {
                    path.push_rect(b.0, a.1 - t, -dx, t);
                }
            } else if dy > 0.0 {
                path.push_rect(a.0 - t, a.1, t, dy);
            } else if dy < 0.0 {
                path.push_rect(a.0, b.1, t, -dy);
            }
            // Close tiny reflex-corner gap when consecutive edges turn concave.
            let prev = outer[(i + n - 1) % n];
            let (e1x, e1y) = (a.0 - prev.0, a.1 - prev.1);
            let (e2x, e2y) = (dx, dy);
            if e1x * e2y - e1y * e2x < 0.0 {
                let (n1x, n1y) = ortho_inward_normal(prev, a);
                let (n2x, n2y) = ortho_inward_normal(a, b);
                let rx = a.0 + (n1x + n2x).min(0.0) * t;
                let ry = a.1 + (n1y + n2y).min(0.0) * t;
                path.push_rect(rx, ry, t, t);
            }
        }
        path
    }

    /// Axis-aligned bounding box `(min_x, min_y, max_x, max_y)` of control points.
    pub fn control_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut push = |x: f32, y: f32| {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        };
        for &op in &self.ops {
            match op {
                PathOp::MoveTo(x, y) | PathOp::LineTo(x, y) => push(x, y),
                PathOp::QuadTo(cx, cy, x, y) => {
                    push(cx, cy);
                    push(x, y);
                }
                PathOp::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                    push(c1x, c1y);
                    push(c2x, c2y);
                    push(x, y);
                }
                PathOp::Close => {}
            }
        }
        if min_x.is_finite() && max_x >= min_x && max_y >= min_y {
            Some((min_x, min_y, max_x, max_y))
        } else {
            None
        }
    }
}

/// Trace the exact clockwise outer perimeter of horizontally overlapping
/// top-to-bottom rectilinear row segments, merging collinear edges.
pub fn ortho_outer_vertices(segments: &[Rect]) -> Vec<(f32, f32)> {
    let valid: Vec<Rect> = segments
        .iter()
        .copied()
        .filter(|s| s.w > 0 && s.h > 0)
        .collect();
    if valid.is_empty() {
        return Vec::new();
    }
    let mut raw = Vec::with_capacity(valid.len() * 4);
    // Right boundary top-to-bottom.
    for s in &valid {
        let x1 = s.x.saturating_add(s.w) as f32;
        let y0 = s.y as f32;
        let y1 = s.y.saturating_add(s.h) as f32;
        raw.push((x1, y0));
        raw.push((x1, y1));
    }
    // Left boundary bottom-to-top.
    for s in valid.iter().rev() {
        let x0 = s.x as f32;
        let y0 = s.y as f32;
        let y1 = s.y.saturating_add(s.h) as f32;
        raw.push((x0, y1));
        raw.push((x0, y0));
    }
    simplify_ortho_ring(raw)
}

fn simplify_ortho_ring(raw: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let mut dedup: Vec<(f32, f32)> = Vec::with_capacity(raw.len());
    for pt in raw {
        if dedup
            .last()
            .is_none_or(|prev| (prev.0 - pt.0).abs() > 0.01 || (prev.1 - pt.1).abs() > 0.01)
        {
            dedup.push(pt);
        }
    }
    if dedup.len() > 1 {
        let first = dedup[0];
        if let Some(&last) = dedup.last()
            && (first.0 - last.0).abs() <= 0.01
            && (first.1 - last.1).abs() <= 0.01
        {
            dedup.pop();
        }
    }
    let mut changed = true;
    while changed && dedup.len() >= 3 {
        changed = false;
        let n = dedup.len();
        let mut next = Vec::with_capacity(n);
        for i in 0..n {
            let prev = dedup[(i + n - 1) % n];
            let curr = dedup[i];
            let after = dedup[(i + 1) % n];
            let same_x = (prev.0 - curr.0).abs() <= 0.01 && (curr.0 - after.0).abs() <= 0.01;
            let same_y = (prev.1 - curr.1).abs() <= 0.01 && (curr.1 - after.1).abs() <= 0.01;
            if same_x || same_y {
                changed = true;
            } else {
                next.push(curr);
            }
        }
        dedup = next;
    }
    dedup
}

/// Compute exact inward offset of a clockwise orthogonal polygon by `t` pixels.
fn inset_ortho_polygon(verts: &[(f32, f32)], t: f32) -> Option<Vec<(f32, f32)>> {
    let n = verts.len();
    if n < 4 {
        return None;
    }
    // Reject inset if any edge is too short to sustain a non-self-intersecting inset.
    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        if libm::hypotf(b.0 - a.0, b.1 - a.1) <= t * 2.0 + 0.5 {
            return None;
        }
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let prev = verts[(i + n - 1) % n];
        let curr = verts[i];
        let next = verts[(i + 1) % n];
        let (n1x, n1y) = ortho_inward_normal(prev, curr);
        let (n2x, n2y) = ortho_inward_normal(curr, next);
        out.push((curr.0 + (n1x + n2x) * t, curr.1 + (n1y + n2y) * t));
    }
    Some(out)
}

/// Inward unit normal for a clockwise axis-aligned edge in screen space (+y down).
fn ortho_inward_normal(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    if dx.abs() >= dy.abs() {
        if dx >= 0.0 { (0.0, 1.0) } else { (0.0, -1.0) }
    } else if dy >= 0.0 {
        (-1.0, 0.0)
    } else {
        (1.0, 0.0)
    }
}

/// Rasterize and composite `path` directly onto `fb` using `ab_glyph_rasterizer`.
///
/// Honors `fb.clip_bounds()` and `fb.y_bias()` for pixel-exact scrolled viewports,
/// allocating only the visible clipped bounding box of `path` (0 allocations when off-screen).
pub fn fill_path(fb: &mut Framebuffer, path: &VectorPath, color: Rgba) {
    let Some((min_x, min_y, max_x, max_y)) = path.control_bounds() else {
        return;
    };
    let (cx0, cy0, cx1, cy1) = fb.clip_bounds();
    let x_bias = fb.x_bias() as f32;
    let y_bias = fb.y_bias() as f32;
    let rx0 = libm::floorf(min_x + x_bias) as i32;
    let rx1 = libm::ceilf(max_x + x_bias) as i32;
    let vis_x0 = rx0.max(cx0 as i32);
    let vis_y0 = (libm::floorf(min_y + y_bias) as i32).max(cy0 as i32);
    let vis_x1 = rx1.min(cx1 as i32);
    let vis_y1 = (libm::ceilf(max_y + y_bias) as i32).min(cy1 as i32);
    if vis_x1 <= vis_x0 || vis_y1 <= vis_y0 {
        return;
    }
    // Keep the full horizontal span of the path inside the rasterizer so
    // `ab_glyph_rasterizer` never receives negative X coordinates (which would
    // corrupt winding or overflow) and never wraps past `w` onto the next row.
    let w = (rx1 - rx0 + 2).max(1) as usize;
    let h = (vis_y1 - vis_y0) as usize;
    let ox = rx0 as f32 - x_bias;
    let oy = vis_y0 as f32 - y_bias;

    let mut raster = Rasterizer::new(w, h);
    let mut start = Point { x: 0.0, y: 0.0 };
    let mut cur = Point { x: 0.0, y: 0.0 };
    let mut has_point = false;

    for &op in &path.ops {
        match op {
            PathOp::MoveTo(x, y) => {
                let p = Point {
                    x: x - ox,
                    y: y - oy,
                };
                start = p;
                cur = p;
                has_point = true;
            }
            PathOp::LineTo(x, y) => {
                let p = Point {
                    x: x - ox,
                    y: y - oy,
                };
                if has_point {
                    raster.draw_line(cur, p);
                } else {
                    start = p;
                    has_point = true;
                }
                cur = p;
            }
            PathOp::QuadTo(cx, cy, x, y) => {
                let c = Point {
                    x: cx - ox,
                    y: cy - oy,
                };
                let p = Point {
                    x: x - ox,
                    y: y - oy,
                };
                if has_point {
                    raster.draw_quad(cur, c, p);
                }
                cur = p;
            }
            PathOp::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                let c1 = Point {
                    x: c1x - ox,
                    y: c1y - oy,
                };
                let c2 = Point {
                    x: c2x - ox,
                    y: c2y - oy,
                };
                let p = Point {
                    x: x - ox,
                    y: y - oy,
                };
                if has_point {
                    raster.draw_cubic(cur, c1, c2, p);
                }
                cur = p;
            }
            PathOp::Close => {
                if has_point && (cur.x != start.x || cur.y != start.y) {
                    raster.draw_line(cur, start);
                }
                cur = start;
            }
        }
    }

    let base_y = vis_y0 as u32;
    raster.for_each_pixel_2d(|dx, dy, cov| {
        let sx = rx0 + dx as i32;
        if sx < vis_x0 || sx >= vis_x1 {
            return;
        }
        let alpha = (cov.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        if alpha > 0 {
            fb.blend_pixel_screen(sx as u32, base_y + dy, color, alpha);
        }
    });
}

/// Draw an antialiased filled rounded rectangle via the unified vector backend.
pub fn fill_rounded_rect(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    radius: u32,
    color: Rgba,
) {
    if w == 0 || h == 0 {
        return;
    }
    let path = VectorPath::rounded_rect(x as f32, y as f32, w as f32, h as f32, radius as f32);
    fill_path(fb, &path, color);
}

/// Draw an antialiased rounded panel with crisp border and interior fill.
#[allow(clippy::too_many_arguments)]
pub fn fill_rounded_panel(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    radius: u32,
    thickness: u32,
    fill: Rgba,
    border: Rgba,
) {
    if w == 0 || h == 0 {
        return;
    }
    let fill_path_geom =
        VectorPath::rounded_rect(x as f32, y as f32, w as f32, h as f32, radius as f32);
    fill_path(fb, &fill_path_geom, fill);
    if thickness > 0 {
        let frame = VectorPath::rounded_frame(
            x as f32,
            y as f32,
            w as f32,
            h as f32,
            radius as f32,
            thickness as f32,
        );
        fill_path(fb, &frame, border);
    }
}

/// Fill a connected rectilinear partition polygon with smooth antialiased corners.
pub fn fill_ortho_polygon(
    fb: &mut Framebuffer,
    segments: &[Rect],
    corner_radius: f32,
    color: Rgba,
) {
    let path = VectorPath::ortho_polygon(segments, corner_radius);
    fill_path(fb, &path, color);
}

/// Stroke the merged outer perimeter of a connected rectilinear partition polygon.
pub fn stroke_ortho_polygon(
    fb: &mut Framebuffer,
    segments: &[Rect],
    thickness: f32,
    corner_radius: f32,
    color: Rgba,
) {
    let frame = VectorPath::ortho_polygon_frame(segments, thickness, corner_radius);
    fill_path(fb, &frame, color);
}

/// Draw a resolution-independent antialiased vector OS / boot-target emblem.
#[allow(clippy::too_many_arguments)]
pub fn draw_vector_emblem(
    fb: &mut Framebuffer,
    hint: &str,
    title: &str,
    x: u32,
    y: u32,
    size: u32,
    badge_bg: Rgba,
    fg: Rgba,
) {
    if size < 10 {
        return;
    }
    let xf = x as f32;
    let yf = y as f32;
    let s = size as f32;
    let r = (s * 0.24).max(3.0);
    let bg = VectorPath::rounded_rect(xf, yf, s, s, r);
    fill_path(fb, &bg, badge_bg);

    let lower = title.to_ascii_lowercase();
    let cx = xf + s * 0.5;
    let cy = yf + s * 0.5;
    let inset = s * 0.22;
    let inner = s - inset * 2.0;

    let mut glyph = VectorPath::new();
    if hint == "windows" || lower.contains("windows") {
        let gap = (inner * 0.14).max(1.5);
        let pane = (inner - gap) * 0.5;
        let pr = (pane * 0.12).max(0.8);
        glyph
            .push_rounded_rect(xf + inset, yf + inset, pane, pane, pr)
            .push_rounded_rect(xf + inset + pane + gap, yf + inset, pane, pane, pr)
            .push_rounded_rect(xf + inset, yf + inset + pane + gap, pane, pane, pr)
            .push_rounded_rect(
                xf + inset + pane + gap,
                yf + inset + pane + gap,
                pane,
                pane,
                pr,
            );
    } else if hint == "recovery" || lower.contains("sbm") || lower.contains("recovery") {
        let arm = (inner * 0.30).max(2.0);
        let cr = (arm * 0.25).max(1.0);
        glyph
            .push_rounded_rect(cx - arm * 0.5, yf + inset, arm, inner, cr)
            .push_rounded_rect(xf + inset, cy - arm * 0.5, inner, arm, cr);
    } else if lower.contains("ubuntu") {
        let ring_r = inner * 0.36;
        let stroke = (s * 0.10).max(1.8);
        let dot_r = (s * 0.095).max(1.5);
        glyph
            .push_ring(cx, cy, ring_r, stroke)
            .push_circle(cx - ring_r, cy, dot_r)
            .push_circle(cx + ring_r * 0.5, cy - ring_r * 0.866, dot_r)
            .push_circle(cx + ring_r * 0.5, cy + ring_r * 0.866, dot_r);
    } else if lower.contains("arch") {
        let top_y = yf + inset;
        let bot_y = yf + s - inset;
        let half_w = inner * 0.46;
        let notch_w = inner * 0.16;
        let notch_y = top_y + inner * 0.64;
        glyph
            .move_to(cx, top_y)
            .line_to(cx + half_w, bot_y)
            .line_to(cx + notch_w, bot_y)
            .line_to(cx, notch_y)
            .line_to(cx - notch_w, bot_y)
            .line_to(cx - half_w, bot_y)
            .close();
    } else if lower.contains("fedora") {
        let ring_r = inner * 0.44;
        let stroke = (s * 0.10).max(1.8);
        let bar = (s * 0.11).max(1.8);
        glyph
            .push_ring(cx, cy, ring_r, stroke)
            .push_rounded_rect(
                cx - bar * 0.5,
                yf + inset + bar * 0.6,
                bar,
                inner - bar * 1.2,
                bar * 0.3,
            )
            .push_rounded_rect(cx - bar * 1.2, cy - bar * 0.5, bar * 2.6, bar, bar * 0.3);
    } else if lower.contains("debian") {
        let r1 = inner * 0.46;
        let r2 = inner * 0.25;
        let stroke = (s * 0.095).max(1.6);
        glyph.push_ring(cx, cy, r1, stroke).push_ring(
            cx + s * 0.03,
            cy - s * 0.03,
            r2,
            stroke * 0.85,
        );
    } else if hint == "usb" {
        let bw = inner * 0.60;
        let bh = inner * 0.54;
        let bx = cx - bw * 0.5;
        let by = yf + s - inset - bh;
        let pw = bw * 0.64;
        let ph = inner - bh;
        let px = cx - pw * 0.5;
        glyph
            .push_rounded_rect(bx, by, bw, bh, (s * 0.07).max(1.0))
            .push_rect(px, yf + inset, pw, ph);
        fill_path(fb, &glyph, fg);
        // Subtractive pin holes in badge background color.
        let mut pins = VectorPath::new();
        let pin_w = (pw * 0.22).max(1.5);
        let pin_h = (ph * 0.50).max(1.5);
        pins.push_rect(px + pw * 0.16, yf + inset + ph * 0.25, pin_w, pin_h)
            .push_rect(px + pw * 0.62, yf + inset + ph * 0.25, pin_w, pin_h);
        fill_path(fb, &pins, badge_bg);
        return;
    } else {
        // Generic Linux / systemd / GRUB terminal prompt `>_`.
        let stroke = (s * 0.10).max(1.8);
        let left = xf + inset;
        let mid_x = cx - s * 0.02;
        let top = cy - inner * 0.30;
        let bot = cy + inner * 0.30;
        glyph
            .move_to(left, top)
            .line_to(mid_x, cy)
            .line_to(left, bot)
            .line_to(left, bot - stroke)
            .line_to(mid_x - stroke, cy)
            .line_to(left, top + stroke)
            .close()
            .push_rounded_rect(
                mid_x + stroke * 0.5,
                bot - stroke,
                inner * 0.44,
                stroke,
                stroke * 0.35,
            );
    }
    fill_path(fb, &glyph, fg);
}

/// Draw a resolution-independent padlock icon (BitLocker / LUKS).
pub fn draw_vector_lock(fb: &mut Framebuffer, x: u32, y: u32, size: u32, color: Rgba) {
    let s = (size.max(10)) as f32;
    let xf = x as f32;
    let yf = y as f32;
    let body_w = s * 0.74;
    let body_h = s * 0.52;
    let bx = xf + (s - body_w) * 0.5;
    let by = yf + s - body_h;
    let mut path = VectorPath::new();
    path.push_rounded_rect(bx, by, body_w, body_h, (s * 0.12).max(1.5));
    let shackle_r = body_w * 0.34;
    let stroke = (s * 0.14).max(1.5);
    path.push_ring(xf + s * 0.5, by + stroke * 0.2, shackle_r, stroke);
    fill_path(fb, &path, color);
}

/// Draw an antialiased filled circle via the unified vector backend.
pub fn fill_circle(fb: &mut Framebuffer, cx: u32, cy: u32, radius: f32, color: Rgba) {
    if radius <= 0.0 {
        return;
    }
    let mut path = VectorPath::with_capacity(6);
    path.push_circle(cx as f32 + 0.5, cy as f32 + 0.5, radius);
    fill_path(fb, &path, color);
}

/// Draw a crisp antialiased close/abort `×` glyph centered in `(x, y, size, size)`.
pub fn draw_vector_close_x(fb: &mut Framebuffer, x: u32, y: u32, size: u32, color: Rgba) {
    if size < 5 {
        return;
    }
    let s = size as f32;
    let xf = x as f32;
    let yf = y as f32;
    let inset = (s * 0.27).max(2.0);
    let x0 = xf + inset;
    let y0 = yf + inset;
    let x1 = xf + s - inset;
    let y1 = yf + s - inset;
    let half = (s * 0.08).clamp(0.75, 1.45);

    let mut path = VectorPath::with_capacity(14);
    // Diagonal 1: top-left to bottom-right
    path.move_to(x0 + half, y0 - half)
        .line_to(x1 + half, y1 - half)
        .line_to(x1 - half, y1 + half)
        .line_to(x0 - half, y0 + half)
        .close();
    // Diagonal 2: top-right to bottom-left
    path.move_to(x1 + half, y0 + half)
        .line_to(x0 + half, y1 + half)
        .line_to(x0 - half, y1 - half)
        .line_to(x1 - half, y0 - half)
        .close();
    fill_path(fb, &path, color);
}

/// Draw a crisp antialiased triangle chevron (`▲` when `up == true`, `▼` otherwise).
pub fn draw_vector_chevron(fb: &mut Framebuffer, x: u32, y: u32, size: u32, up: bool, color: Rgba) {
    if size < 5 {
        return;
    }
    let s = size as f32;
    let xf = x as f32;
    let yf = y as f32;
    let cx = xf + s * 0.5;
    let inset_x = (s * 0.18).max(1.5);
    let h = (s * 0.46).max(3.0);
    let top = yf + (s - h) * 0.5;
    let bot = top + h;

    let mut path = VectorPath::with_capacity(4);
    if up {
        path.move_to(cx, top)
            .line_to(xf + s - inset_x, bot)
            .line_to(xf + inset_x, bot)
            .close();
    } else {
        path.move_to(xf + inset_x, top)
            .line_to(xf + s - inset_x, top)
            .line_to(cx, bot)
            .close();
    }
    fill_path(fb, &path, color);
}

/// Draw a crisp antialiased left/right chevron inside `(x, y, size, size)`.
pub fn draw_vector_chevron_h(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    size: u32,
    left: bool,
    color: Rgba,
) {
    if size < 5 {
        return;
    }
    let s = size as f32;
    let xf = x as f32;
    let yf = y as f32;
    let cy = yf + s * 0.5;
    let inset_y = (s * 0.18).max(1.5);
    let w = (s * 0.46).max(3.0);
    let x_left = xf + (s - w) * 0.5;
    let x_right = x_left + w;

    let mut path = VectorPath::with_capacity(4);
    if left {
        path.move_to(x_left, cy)
            .line_to(x_right, yf + inset_y)
            .line_to(x_right, yf + s - inset_y)
            .close();
    } else {
        path.move_to(x_right, cy)
            .line_to(x_left, yf + s - inset_y)
            .line_to(x_left, yf + inset_y)
            .close();
    }
    fill_path(fb, &path, color);
}

/// Draw a crisp antialiased 3-bar hamburger menu icon inside `(x, y, size, size)`.
pub fn draw_vector_hamburger(fb: &mut Framebuffer, x: u32, y: u32, size: u32, color: Rgba) {
    if size < 8 {
        return;
    }
    let inset_x = (size / 6).max(2);
    let bar_w = size.saturating_sub(inset_x * 2).max(4);
    let bar_h = (size / 8).clamp(2, 4);
    let gap = ((size.saturating_sub(bar_h * 3)) / 4).max(2);
    let total_h = bar_h * 3 + gap * 2;
    let y0 = y.saturating_add(size.saturating_sub(total_h) / 2);
    let r = bar_h / 2;
    for i in 0..3u32 {
        let by = y0.saturating_add(i.saturating_mul(bar_h + gap));
        fill_rounded_rect(fb, x.saturating_add(inset_x), by, bar_w, bar_h, r, color);
    }
}

/// Draw a resolution-independent CPU processor microchip logo inside a rounded
/// badge `(x, y, size, size)`: hollow silicon package, inner die core, and
/// protruding pins on all four edges.
pub fn draw_vector_cpu_logo(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    size: u32,
    badge_bg: Rgba,
    fg: Rgba,
) {
    if size < 12 {
        return;
    }
    let xf = x as f32;
    let yf = y as f32;
    let s = size as f32;
    let badge_r = (s * 0.24).max(3.0);
    let bg = VectorPath::rounded_rect(xf, yf, s, s, badge_r);
    fill_path(fb, &bg, badge_bg);

    let cx = xf + s * 0.5;
    let cy = yf + s * 0.5;
    let inset = s * 0.14;
    let inner = s - inset * 2.0;

    // Central silicon package (hollow frame + solid inner die).
    let body = inner * 0.62;
    let bx = cx - body * 0.5;
    let by = cy - body * 0.5;
    let body_r = (body * 0.15).max(1.2);
    let stroke = (s * 0.082).max(1.4);

    let mut glyph = VectorPath::with_capacity(64);
    glyph.push_rounded_rect(bx, by, body, body, body_r);
    let hole = body - stroke * 2.0;
    if hole > 2.2 {
        glyph.push_rounded_rect_ccw(
            bx + stroke,
            by + stroke,
            hole,
            hole,
            (body_r - stroke * 0.4).max(0.4),
        );
        let die = (hole * 0.54).max(1.6);
        glyph.push_rounded_rect(
            cx - die * 0.5,
            cy - die * 0.5,
            die,
            die,
            (die * 0.18).max(0.5),
        );
    }

    // Protruding contact pins (3 per side at >=20px, 2 per side at tiny sizes).
    let pin_len = ((inner - body) * 0.52).max(1.8);
    let pin_w = (s * 0.068).max(1.2);
    let pin_r = (pin_w * 0.35).max(0.4);
    let offsets_3 = [-0.28 * body, 0.0, 0.28 * body];
    let offsets_2 = [-0.20 * body, 0.20 * body];
    let offsets: &[f32] = if size >= 20 { &offsets_3 } else { &offsets_2 };

    for &off in offsets {
        // Top & bottom pins
        let px = cx + off - pin_w * 0.5;
        glyph.push_rounded_rect(px, by - pin_len + 0.4, pin_w, pin_len, pin_r);
        glyph.push_rounded_rect(px, by + body - 0.4, pin_w, pin_len, pin_r);
        // Left & right pins
        let py = cy + off - pin_w * 0.5;
        glyph.push_rounded_rect(bx - pin_len + 0.4, py, pin_len, pin_w, pin_r);
        glyph.push_rounded_rect(bx + body - 0.4, py, pin_len, pin_w, pin_r);
    }

    fill_path(fb, &glyph, fg);
}

/// Draw a resolution-independent discrete GPU graphics-card logo inside a
/// rounded badge `(x, y, size, size)`: horizontal card shroud, left L-shaped
/// PCIe I/O bracket, keyed bottom PCIe x16 edge connector, and cooling fans.
pub fn draw_vector_gpu_logo(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    size: u32,
    badge_bg: Rgba,
    fg: Rgba,
) {
    if size < 12 {
        return;
    }
    let xf = x as f32;
    let yf = y as f32;
    let s = size as f32;
    let badge_r = (s * 0.24).max(3.0);
    let bg = VectorPath::rounded_rect(xf, yf, s, s, badge_r);
    fill_path(fb, &bg, badge_bg);

    let cy = yf + s * 0.5;
    let inset = s * 0.13;
    let inner = s - inset * 2.0;

    let bt = (s * 0.068).max(1.3);
    let card_x = xf + inset + bt * 1.45;
    let card_w = (inner - bt * 1.45).max(6.0);
    let card_h = inner * 0.56;
    let card_y = cy - card_h * 0.54;
    let card_r = (card_h * 0.16).max(1.2);

    // 1. Shroud + left L-bracket + keyed bottom PCIe x16 edge fingers.
    let mut body = VectorPath::with_capacity(32);
    body.push_rounded_rect(card_x, card_y, card_w, card_h, card_r);
    // Left I/O mounting bracket stem + top lip
    let brk_x = card_x - bt * 0.85;
    let brk_y = card_y - bt * 1.15;
    body.push_rounded_rect(brk_x, brk_y, bt, card_h + bt * 1.95, bt * 0.3);
    body.push_rounded_rect(xf + inset, brk_y, bt * 1.55, bt, bt * 0.3);
    // Keyed PCIe x16 edge fingers along the bottom edge
    let pcie_h = (s * 0.088).max(1.6);
    let pcie_y = card_y + card_h - 0.4;
    let pcie_r = (pcie_h * 0.28).max(0.4);
    body.push_rounded_rect(
        card_x + card_w * 0.10,
        pcie_y,
        card_w * 0.48,
        pcie_h,
        pcie_r,
    );
    body.push_rounded_rect(
        card_x + card_w * 0.66,
        pcie_y,
        card_w * 0.19,
        pcie_h,
        pcie_r,
    );
    fill_path(fb, &body, fg);

    // 2. Cut out circular fan apertures in badge_bg, then draw fan hubs + blades in fg.
    let fan_cy = card_y + card_h * 0.5;
    let dual_fan = size >= 22;
    let (centers_buf, count, fan_r): ([(f32, f32); 2], usize, f32) = if dual_fan {
        let r = (card_h * 0.34).min(card_w * 0.185);
        (
            [
                (card_x + card_w * 0.28, fan_cy),
                (card_x + card_w * 0.72, fan_cy),
            ],
            2,
            r,
        )
    } else {
        let r = (card_h * 0.36).min(card_w * 0.28);
        ([(card_x + card_w * 0.50, fan_cy), (0.0, 0.0)], 1, r)
    };

    if fan_r >= 1.8 {
        let mut holes = VectorPath::with_capacity(12);
        for &(fcx, fcy) in &centers_buf[..count] {
            holes.push_circle(fcx, fcy, fan_r);
        }
        fill_path(fb, &holes, badge_bg);

        let hub_r = (fan_r * 0.35).max(1.05);
        let blade_hw = (fan_r * 0.14).clamp(0.45, 1.0);
        // 3 evenly spaced turbine spokes (30°, 150°, 270°) — pure no_std constants.
        const SPOKES: [(f32, f32); 3] = [(0.866_025_4, 0.5), (-0.866_025_4, 0.5), (0.0, -1.0)];
        let mut hubs = VectorPath::with_capacity(32);
        for &(fcx, fcy) in &centers_buf[..count] {
            hubs.push_circle(fcx, fcy, hub_r);
            if fan_r >= 3.0 {
                let tip_r = fan_r * 0.96;
                for &(dx, dy) in &SPOKES {
                    let px = -dy * blade_hw;
                    let py = dx * blade_hw;
                    let tx = fcx + dx * tip_r;
                    let ty = fcy + dy * tip_r;
                    hubs.move_to(fcx + px, fcy + py)
                        .line_to(tx + px * 0.7, ty + py * 0.7)
                        .line_to(tx - px * 0.7, ty - py * 0.7)
                        .line_to(fcx - px, fcy - py)
                        .close();
                }
            }
        }
        fill_path(fb, &hubs, fg);
    }
}

/// Draw a crisp, camera-scannable ISO/IEC 18004 QR code encoding `data` inside
/// a rounded white quiet-zone square `(x, y, size, size)` using integer-pixel
/// modules (no subpixel blur). Returns `true` if the QR code fit and rendered.
#[cfg(feature = "experimental-layouts")]
pub fn draw_vector_qr(
    fb: &mut Framebuffer,
    x: u32,
    y: u32,
    size: u32,
    data: &str,
    light: Rgba,
    dark: Rgba,
) -> bool {
    let Ok(code) = qrcode::QrCode::with_error_correction_level(data.as_bytes(), qrcode::EcLevel::L)
    else {
        return false;
    };
    let mods = code.width() as u32;
    if mods == 0 || size < mods + 4 {
        return false;
    }
    // Use at least a 2-module quiet zone when space allows, falling back to 1-module
    // padding on tiny squares so modules stay integer-aligned and sharp.
    let quiet = if size >= mods + 4 { 2 } else { 1 };
    let total_mods = mods + quiet * 2;
    let module_px = (size / total_mods).max(1);
    let qr_px = mods * module_px;
    if qr_px + 2 > size {
        return false;
    }
    let radius = (size / 10).clamp(3, 8);
    fill_rounded_rect(fb, x, y, size, size, radius, light);
    let ox = x.saturating_add(size.saturating_sub(qr_px) / 2);
    let oy = y.saturating_add(size.saturating_sub(qr_px) / 2);
    for my in 0..mods {
        for mx in 0..mods {
            if code[(mx as usize, my as usize)] == qrcode::Color::Dark {
                fb.fill_rect(
                    ox.saturating_add(mx.saturating_mul(module_px)),
                    oy.saturating_add(my.saturating_mul(module_px)),
                    module_px,
                    module_px,
                    dark,
                );
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ortho_polygon_short_ledge_stroke_never_fills_interior() {
        // Two segments whose overhang ledge is only 2 px wide, stroked with thickness 3 px.
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
                w: 78,
                h: 30,
            },
        ];
        let mut fb = Framebuffer::new(110, 90);
        let bg = Rgba::rgb(0x10, 0x14, 0x1A);
        let fill = Rgba::rgb(0x20, 0x30, 0x40);
        let border = Rgba::rgb(0xFF, 0xCC, 0x00);
        fb.fill(bg);
        fill_ortho_polygon(&mut fb, &segs, 2.0, fill);
        stroke_ortho_polygon(&mut fb, &segs, 3.0, 2.0, border);
        // Interior must remain `fill`, never overwritten with `border`.
        assert_eq!(fb.pixel(40, 25), Some(fill));
        assert_eq!(fb.pixel(40, 55), Some(fill));
        // Merged internal seam must stay `fill`.
        assert_eq!(fb.pixel(40, 39), Some(fill));
        assert_eq!(fb.pixel(40, 40), Some(fill));
        // Outer top border must be `border`.
        assert_eq!(fb.pixel(40, 11), Some(border));
    }

    #[test]
    fn fill_path_honors_viewport_clip_and_scroll_bias() {
        let mut fb = Framebuffer::new(40, 40);
        let bg = Rgba::rgb(0, 0, 0);
        let fg = Rgba::rgb(80, 180, 255);
        fb.fill(bg);
        fb.set_viewport(5, 10, 30, 15, -50);
        // Content y=55..80 maps to screen y=5..30, clipped to screen y=10..25!
        fill_rounded_rect(&mut fb, 10, 55, 20, 25, 0, fg);
        fb.clear_viewport();
        assert_eq!(fb.pixel(15, 15), Some(fg));
        assert_eq!(fb.pixel(15, 8), Some(bg));
        assert_eq!(fb.pixel(15, 27), Some(bg));
    }

    #[test]
    fn rounded_panel_has_antialiased_edge_pixels() {
        let mut fb = Framebuffer::new(80, 50);
        let bg = Rgba::rgb(0x10, 0x14, 0x1A);
        let fill = Rgba::rgb(0x1C, 0x23, 0x30);
        let border = Rgba::rgb(0x4C, 0x8D, 0xFF);
        fb.fill(bg);
        fill_rounded_panel(&mut fb, 8, 8, 64, 34, 10, 2, fill, border);
        assert_eq!(fb.pixel(40, 25), Some(fill));
        // Antialiased pixels exist that are neither pure bg, pure fill, nor pure border.
        let blended = fb
            .px
            .iter()
            .filter(|&&p| p != bg.0 && p != fill.0 && p != border.0)
            .count();
        assert!(
            blended > 10,
            "expected antialiased edge pixels, got {blended}"
        );
    }

    #[test]
    fn ortho_polygon_traces_l_shape_and_cancels_internal_seam() {
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
        let verts = ortho_outer_vertices(&segs);
        // L-shape has exactly 6 vertices.
        assert_eq!(verts.len(), 6, "verts={verts:?}");

        let mut fb = Framebuffer::new(110, 90);
        let bg = Rgba::rgb(0x10, 0x14, 0x1A);
        let fill = Rgba::rgb(0x20, 0x30, 0x40);
        let border = Rgba::rgb(0xFF, 0xCC, 0x00);
        fb.fill(bg);
        fill_ortho_polygon(&mut fb, &segs, 4.0, fill);
        stroke_ortho_polygon(&mut fb, &segs, 2.0, 4.0, border);

        // Interior across the merged horizontal seam at (30, 39..=40) must stay `fill`.
        assert_eq!(fb.pixel(30, 39), Some(fill));
        assert_eq!(fb.pixel(30, 40), Some(fill));
        // Exposed overhang bottom edge at (75, 39) must be `border`.
        assert_eq!(fb.pixel(75, 39), Some(border));
        // Cutout region at (80, 55) must stay `bg`.
        assert_eq!(fb.pixel(80, 55), Some(bg));
    }

    #[test]
    fn vector_emblems_render_distinct_ink() {
        for (hint, title) in [
            ("windows", "Windows"),
            ("linux", "Fedora Linux"),
            ("linux", "Ubuntu"),
            ("linux", "Arch Linux"),
            ("linux", "Debian"),
            ("recovery", "SBM Core"),
            ("usb", "USB Drive"),
            ("grub", "GRUB"),
        ] {
            let mut fb = Framebuffer::new(48, 48);
            let bg = Rgba::rgb(0x10, 0x14, 0x1A);
            let accent = Rgba::rgb(0x4C, 0x8D, 0xFF);
            let fg = Rgba::rgb(0xFF, 0xFF, 0xFF);
            fb.fill(bg);
            draw_vector_emblem(&mut fb, hint, title, 4, 4, 40, accent, fg);
            assert!(
                fb.px.contains(&fg.0),
                "emblem {hint}/{title} had no foreground ink"
            );
        }
    }

    #[test]
    fn cpu_and_gpu_vector_logos_render_distinct_ink() {
        let bg = Rgba::rgb(0x10, 0x14, 0x1A);
        let badge = Rgba::rgb(0xE8, 0xB6, 0x47);
        let fg = Rgba::rgb(0x0B, 0x10, 0x14);
        for size in [16u32, 24, 34] {
            let mut fb_cpu = Framebuffer::new(size + 8, size + 8);
            fb_cpu.fill(bg);
            draw_vector_cpu_logo(&mut fb_cpu, 4, 4, size, badge, fg);
            assert!(
                fb_cpu.px.contains(&badge.0) && fb_cpu.px.contains(&fg.0),
                "cpu logo size {size} missing badge or foreground ink"
            );

            let mut fb_gpu = Framebuffer::new(size + 8, size + 8);
            fb_gpu.fill(bg);
            draw_vector_gpu_logo(&mut fb_gpu, 4, 4, size, badge, fg);
            assert!(
                fb_gpu.px.contains(&badge.0) && fb_gpu.px.contains(&fg.0),
                "gpu logo size {size} missing badge or foreground ink"
            );
            assert_ne!(
                fb_cpu.px, fb_gpu.px,
                "cpu and gpu logos must be visually distinct at size {size}"
            );
        }
    }

    #[cfg(feature = "experimental-layouts")]
    #[test]
    fn qr_code_renders_finder_patterns_inside_quiet_zone() {
        let mut fb = Framebuffer::new(96, 96);
        let bg = Rgba::rgb(0x10, 0x14, 0x1A);
        let light = Rgba::rgb(0xFF, 0xFF, 0xFF);
        let dark = Rgba::rgb(0x0B, 0x10, 0x14);
        fb.fill(bg);
        assert!(draw_vector_qr(
            &mut fb,
            8,
            8,
            72,
            "https://g-AI.org/Smart-Boot-Manager",
            light,
            dark,
        ));
        let light_count = fb.px.iter().filter(|&&p| p == light.0).count();
        let dark_count = fb.px.iter().filter(|&&p| p == dark.0).count();
        assert!(
            light_count > 1000,
            "expected white quiet zone and light modules"
        );
        assert!(dark_count > 500, "expected dark QR code modules");
    }
}
