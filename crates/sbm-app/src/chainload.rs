use alloc::vec::Vec;

use uefi::boot::LoadImageSource;
use uefi::proto::loaded_image::LoadedImage;
use uefi::{Status, boot};

use crate::firmware::{self, open_shared, ucs2_bytes};
use crate::volume;

pub enum ChainOutcome {
    LoadFailed(Status),
    Returned(Status),
}

pub fn chainload(
    device_path: &[u8],
    file: &str,
    options: Option<&str>,
    read_image: impl FnOnce() -> Option<Vec<u8>>,
) -> ChainOutcome {
    let Ok(path) = uefi::CString16::try_from(file) else {
        return ChainOutcome::LoadFailed(Status::INVALID_PARAMETER);
    };
    let option_bytes = options.and_then(ucs2_bytes);
    let loaded = if device_path.is_empty() {
        None
    } else {
        match volume::load_from_device_path(device_path, &path) {
            Ok(handle) => {
                firmware::mark("chainload method=device-path");
                Some(handle)
            }
            Err(err) => {
                firmware::mark(&alloc::format!("chainload device-path {err:?}"));
                None
            }
        }
    };
    let handle = if let Some(handle) = loaded {
        handle
    } else {
        let Some(image) = read_image() else {
            firmware::mark("chainload method=buffer-unavailable");
            return ChainOutcome::LoadFailed(Status::LOAD_ERROR);
        };
        match boot::load_image(
            boot::image_handle(),
            LoadImageSource::FromBuffer {
                buffer: &image,
                file_path: None,
            },
        ) {
            Ok(handle) => {
                firmware::mark("chainload method=buffer");
                handle
            }
            Err(err) => return ChainOutcome::LoadFailed(err.status()),
        }
    };
    if let Some(bytes) = option_bytes.as_ref()
        && let Ok(mut image) = open_shared::<LoadedImage>(handle)
    {
        let Ok(size) = u32::try_from(bytes.len()) else {
            let _ = boot::unload_image(handle);
            return ChainOutcome::LoadFailed(Status::BAD_BUFFER_SIZE);
        };
        unsafe {
            image.set_load_options(bytes.as_ptr(), size);
        }
    }
    let started = boot::start_image(handle);
    let _ = boot::unload_image(handle);
    match started {
        Ok(()) => ChainOutcome::Returned(Status::SUCCESS),
        Err(err) => ChainOutcome::Returned(err.status()),
    }
}
