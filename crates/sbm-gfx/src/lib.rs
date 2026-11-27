#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(feature = "experimental-layouts")]
pub mod assets;
#[cfg(feature = "experimental-layouts")]
pub mod disk_wrap;
mod draw;
mod font8x8;
#[cfg(feature = "experimental-layouts")]
pub mod hardware;
mod layout;
mod paint;
mod state;
#[cfg(feature = "experimental-layouts")]
pub mod text;
mod theme;

pub use draw::{Framebuffer, Rgba};
pub use layout::{Hit, Layout, Rect, font_px, hit_test, layout_menu};
pub use paint::{CardView, paint_diagnostics, paint_error, paint_menu, paint_splash};
pub use state::{Focus, Input, MenuState, Outcome, Screen, step};
pub use theme::Theme;
