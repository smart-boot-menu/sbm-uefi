#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(feature = "experimental-layouts")]
pub mod disk_wrap;
mod draw;
mod font8x8;
#[cfg(feature = "experimental-layouts")]
pub mod hardware;
mod layout;
#[cfg(feature = "experimental-layouts")]
pub mod logo;
mod paint;
mod state;
mod theme;
pub mod typography;
pub mod vector;

pub use draw::{Framebuffer, Rgba};
pub use layout::{Hit, Layout, Rect, font_px, hit_test, layout_menu};
#[cfg(feature = "experimental-layouts")]
pub use logo::{LogoImage, draw_logo_image};
pub use paint::{
    CardView, paint_diagnostics, paint_diagnostics_with, paint_error, paint_error_with, paint_menu,
    paint_menu_with, paint_splash, paint_splash_with,
};
pub use state::{Focus, Input, MenuState, Outcome, Screen, step};
pub use theme::Theme;
pub use typography::{
    ATKINSON_MONO_REGULAR, ATKINSON_NEXT_REGULAR, ATKINSON_NEXT_SEMIBOLD, BUNDLED_FONT_BYTES,
    CacheStats, FaceSlot, FontLoadError, FontMode, LineMetrics, TextMetrics, TextRenderer,
    TextRole, UiScale, role_pixel_size,
};
#[cfg(feature = "experimental-layouts")]
pub use vector::draw_vector_qr;
pub use vector::{
    PathOp, VectorPath, draw_vector_chevron, draw_vector_chevron_h, draw_vector_close_x,
    draw_vector_cpu_logo, draw_vector_emblem, draw_vector_gpu_logo, draw_vector_hamburger,
    draw_vector_lock, fill_circle, fill_ortho_polygon, fill_path, fill_rounded_panel,
    fill_rounded_rect, ortho_outer_vertices, stroke_ortho_polygon,
};
