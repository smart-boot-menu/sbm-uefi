use alloc::string::String;
use alloc::vec::Vec;
use core::mem::align_of;

use sbm_gfx::{Framebuffer, Rgba};
use uefi::proto::console::gop::{
    BltOp, BltPixel, BltRegion, GraphicsOutput, Mode, PixelBitmask, PixelFormat,
};
use uefi::proto::console::pointer::{AbsolutePointer, Pointer};
use uefi::{boot, proto::console::gop::FrameBuffer as GopFrame};

use crate::firmware::open_shared;

pub struct Screen {
    gop: uefi::boot::ScopedProtocol<GraphicsOutput>,
    format: PixelFormat,
    mask: Option<PixelBitmask>,
    stride: usize,
    width: u32,
    height: u32,
}

impl Screen {
    pub fn open() -> Option<Self> {
        let handles = boot::find_handles::<GraphicsOutput>().ok()?;
        let mut fallback = None;
        for handle in handles {
            let Ok(mut gop) = open_shared::<GraphicsOutput>(handle) else {
                continue;
            };
            choose_mode(&mut gop);
            let info = gop.current_mode_info();
            let (width, height) = info.resolution();
            let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
                continue;
            };
            if width == 0 || height == 0 {
                continue;
            }
            let screen = Self {
                format: info.pixel_format(),
                mask: info.pixel_bitmask(),
                stride: info.stride().max(width as usize),
                width,
                height,
                gop,
            };
            if width >= 640 && height >= 480 {
                return Some(screen);
            }
            fallback = Some(screen);
        }
        fallback
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn describe(&self) -> String {
        alloc::format!(
            "{}x{} {:?} stride={}",
            self.width,
            self.height,
            self.format,
            self.stride
        )
    }

    pub fn present(&mut self, fb: &Framebuffer) {
        if self.format == PixelFormat::BltOnly {
            self.present_blt(fb);
            return;
        }
        let format = self.format;
        let mask = self.mask;
        let width = self.width;
        let height = self.height;
        let stride = self.stride;
        let pixels = (width as usize).saturating_mul(height as usize);
        if fb.px.len() < pixels {
            return;
        }
        let mut frame = self.gop.frame_buffer();
        let aligned = (frame.as_mut_ptr() as usize).is_multiple_of(align_of::<u32>());
        for y in 0..height {
            for x in 0..width {
                let source = fb.px[(y * fb.width + x) as usize];
                let packed = pack_pixel(source, format, mask);
                let index = (y as usize * stride + x as usize) * 4;
                write_pixel(&mut frame, index, packed, aligned);
            }
        }
    }

    fn present_blt(&mut self, fb: &Framebuffer) {
        let count = (self.width as usize).saturating_mul(self.height as usize);
        if fb.px.len() < count {
            return;
        }
        let mut buffer = Vec::with_capacity(count);
        for pixel in fb.px.iter().take(count) {
            let color = Rgba(*pixel);
            buffer.push(BltPixel::new(color.r(), color.g(), color.b()));
        }
        let _ = self.gop.blt(BltOp::BufferToVideo {
            buffer: &buffer,
            src: BltRegion::Full,
            dest: (0, 0),
            dims: (self.width as usize, self.height as usize),
        });
    }
}

pub struct Mouse {
    relative: Option<uefi::boot::ScopedProtocol<Pointer>>,
    absolute: Option<uefi::boot::ScopedProtocol<AbsolutePointer>>,
    x: i32,
    y: i32,
    left_down: bool,
    active: bool,
}

pub struct PointerSample {
    pub x: u32,
    pub y: u32,
    pub moved: bool,
    pub clicked: bool,
    pub active: bool,
}

impl Mouse {
    pub fn open(width: u32, height: u32) -> Self {
        let mut relative = boot::find_handles::<Pointer>().ok().and_then(|handles| {
            handles
                .into_iter()
                .find_map(|handle| open_shared::<Pointer>(handle).ok())
        });
        let mut absolute = boot::find_handles::<AbsolutePointer>()
            .ok()
            .and_then(|handles| {
                handles
                    .into_iter()
                    .find_map(|handle| open_shared::<AbsolutePointer>(handle).ok())
            });
        if let Some(pointer) = relative.as_mut() {
            let _ = pointer.reset(false);
        }
        if let Some(pointer) = absolute.as_mut() {
            let _ = pointer.reset(false);
        }
        Self {
            active: relative.is_some() || absolute.is_some(),
            relative,
            absolute,
            x: i32::try_from(width / 2).unwrap_or(0),
            y: i32::try_from(height / 2).unwrap_or(0),
            left_down: false,
        }
    }

    pub fn poll(&mut self, width: u32, height: u32) -> PointerSample {
        let mut moved = false;
        let mut saw_button = false;
        let mut down = false;
        let mut dx = 0i32;
        let mut dy = 0i32;
        if let Some(pointer) = self.relative.as_mut() {
            let resolution_x = pointer.mode().resolution_x;
            let resolution_y = pointer.mode().resolution_y;
            if let Ok(Some(state)) = pointer.read_state() {
                dx = scale(state.relative_movement_x, resolution_x);
                dy = scale(state.relative_movement_y, resolution_y);
                saw_button = true;
                down |= bool::from(state.left_button);
            }
        }
        if dx != 0 || dy != 0 {
            let max_x = i32::try_from(width.saturating_sub(1)).unwrap_or(0);
            let max_y = i32::try_from(height.saturating_sub(1)).unwrap_or(0);
            self.x = (self.x + dx).clamp(0, max_x);
            self.y = (self.y + dy).clamp(0, max_y);
            moved = true;
        }
        let mut absolute_x = None;
        let mut absolute_y = None;
        let mut absolute_down = false;
        if let Some(pointer) = self.absolute.as_mut() {
            let mode = *pointer.mode();
            if let Ok(Some(state)) = pointer.read_state() {
                absolute_x = Some(map_range(
                    state.current_x,
                    mode.absolute_min_x,
                    mode.absolute_max_x,
                    width,
                ));
                absolute_y = Some(map_range(
                    state.current_y,
                    mode.absolute_min_y,
                    mode.absolute_max_y,
                    height,
                ));
                absolute_down = state.active_buttons & 1 != 0;
                saw_button = true;
            }
        }
        if let (Some(x), Some(y)) = (absolute_x, absolute_y) {
            let next_x = i32::try_from(x).unwrap_or(self.x);
            let next_y = i32::try_from(y).unwrap_or(self.y);
            if next_x != self.x || next_y != self.y {
                moved = true;
            }
            self.x = next_x;
            self.y = next_y;
            down |= absolute_down;
        }
        let clicked = saw_button && down && !self.left_down;
        if saw_button {
            self.left_down = down;
        }
        PointerSample {
            x: u32::try_from(self.x.max(0)).unwrap_or(0),
            y: u32::try_from(self.y.max(0)).unwrap_or(0),
            moved,
            clicked,
            active: self.active,
        }
    }
}

fn choose_mode(gop: &mut GraphicsOutput) {
    let current = gop.current_mode_info();
    let (width, height) = current.resolution();
    if (1024..=1920).contains(&width) && (768..=1080).contains(&height) {
        return;
    }
    let mut best: Option<Mode> = None;
    let mut best_score = 0usize;
    for mode in gop.modes() {
        let (mode_width, mode_height) = mode.info().resolution();
        let score = if (800..=1920).contains(&mode_width) && (600..=1080).contains(&mode_height) {
            mode_width.saturating_mul(mode_height)
        } else {
            0
        };
        if score > best_score {
            best_score = score;
            best = Some(mode);
        }
    }
    if let Some(mode) = best.as_ref() {
        let _ = gop.set_mode(mode);
    }
}

fn pack_pixel(pixel: u32, format: PixelFormat, mask: Option<PixelBitmask>) -> u32 {
    match format {
        PixelFormat::Bgr => pixel,
        PixelFormat::Rgb => {
            let red = (pixel >> 16) & 0xff;
            let green = (pixel >> 8) & 0xff;
            let blue = pixel & 0xff;
            (blue << 16) | (green << 8) | red
        }
        PixelFormat::Bitmask => mask.map(|mask| pack_mask(pixel, mask)).unwrap_or(pixel),
        PixelFormat::BltOnly => pixel,
    }
}

fn pack_mask(pixel: u32, mask: PixelBitmask) -> u32 {
    let red = ((pixel >> 16) & 0xff) as u8;
    let green = ((pixel >> 8) & 0xff) as u8;
    let blue = pixel as u8;
    channel(mask.red, red) | channel(mask.green, green) | channel(mask.blue, blue)
}

fn channel(mask: u32, value: u8) -> u32 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let width = mask.count_ones();
    if width == 0 || width >= 31 {
        return 0;
    }
    let max = (1u32 << width) - 1;
    ((u32::from(value) * max) / 255) << shift
}

fn write_pixel(frame: &mut GopFrame<'_>, index: usize, value: u32, aligned: bool) {
    if index.saturating_add(4) > frame.size() {
        return;
    }
    unsafe {
        if aligned {
            frame.write_value(index, value);
        } else {
            for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
                frame.write_byte(index + offset, byte);
            }
        }
    }
}

fn scale(raw: i32, resolution: u64) -> i32 {
    let scaled = if resolution > 1 {
        i32::try_from(i64::from(raw) * 16 / resolution as i64).unwrap_or(0)
    } else {
        raw
    };
    scaled.clamp(-48, 48)
}

fn map_range(value: u64, min: u64, max: u64, span: u32) -> u32 {
    if span == 0 || max <= min {
        return 0;
    }
    let value = value.clamp(min, max) - min;
    let span = u128::from(span.saturating_sub(1));
    u32::try_from(u128::from(value) * span / u128::from(max - min)).unwrap_or(0)
}
