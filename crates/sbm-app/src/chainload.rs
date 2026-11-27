//! `LoadImage`/`StartImage` by exact device path (Spec 03 §9).
//!
//! There is no buffer or pathless retry: a failed device-path load,
//! including `EFI_SECURITY_VIOLATION`, is final and reported to the picker.

use uefi::proto::loaded_image::LoadedImage;
use uefi::{Handle, Status, boot};

use sbm_menu::StartFailure;
use sbm_menu::flow::LaunchResult;

use crate::firmware::{self, open_shared, ucs2_bytes};
use crate::volume;

/// `on_loaded` runs after `LoadImage` succeeds and before `StartImage`.
pub fn chainload(
    device_path: &[u8],
    file: &str,
    options: Option<&str>,
    on_loaded: &mut dyn FnMut(),
) -> LaunchResult {
    let Ok(path) = uefi::CString16::try_from(file) else {
        return load_failed(Status::INVALID_PARAMETER);
    };
    let Some(full) = volume::file_device_path(device_path, &path) else {
        return load_failed(Status::INVALID_PARAMETER);
    };
    let handle = match load_exact(&full) {
        Ok(handle) => handle,
        Err(status) => {
            firmware::mark(&alloc::format!("chainload device-path {status:?}"));
            return load_failed(status);
        }
    };
    firmware::mark("chainload method=device-path");
    // Recovery passes `None`: the UKI keeps its signed embedded `.cmdline`.
    let bytes = match options.map(ucs2_bytes) {
        None => None,
        Some(Some(bytes)) => Some(bytes),
        Some(None) => {
            let _ = boot::unload_image(handle);
            return load_failed(Status::INVALID_PARAMETER);
        }
    };
    if let Some(bytes) = bytes.as_ref() {
        let Ok(size) = u32::try_from(bytes.len()) else {
            let _ = boot::unload_image(handle);
            return load_failed(Status::BAD_BUFFER_SIZE);
        };
        let Ok(mut image) = open_shared::<LoadedImage>(handle) else {
            let _ = boot::unload_image(handle);
            return load_failed(Status::UNSUPPORTED);
        };
        // SAFETY: `bytes` lives until this function returns, after the
        // image has run.
        unsafe { image.set_load_options(bytes.as_ptr(), size) };
    }
    on_loaded();
    let started = boot::start_image(handle);
    let _ = boot::unload_image(handle);
    drop(bytes);
    let status = match started {
        Ok(()) => Status::SUCCESS,
        Err(err) => err.status(),
    };
    LaunchResult::Returned {
        success: status == Status::SUCCESS,
        failure: failure(status),
        status: alloc::format!("{status:?}"),
    }
}

/// `EFI_BOOT_SERVICES.LoadImage` called directly. Firmware may return a
/// handle together with `EFI_SECURITY_VIOLATION`; the UEFI specification
/// requires the caller to unload it, which `uefi::boot::load_image` cannot
/// do because it discards the handle on error.
fn load_exact(path: &[u8]) -> Result<Handle, Status> {
    let Some(st) = uefi::table::system_table_raw() else {
        return Err(Status::NOT_READY);
    };
    // SAFETY: the system table pointer stays valid until ExitBootServices,
    // which the picker never calls.
    let bs = unsafe { st.as_ref().boot_services };
    if bs.is_null() {
        return Err(Status::NOT_READY);
    }
    let mut raw = core::ptr::null_mut();
    // SAFETY: `path` is a finalized device path that lives across the call;
    // no source buffer is passed, so firmware reads the file itself.
    let status = unsafe {
        ((*bs).load_image)(
            // FALSE: EFI_BOOT_POLICY exact match, no boot-option expansion.
            false.into(),
            boot::image_handle().as_ptr(),
            path.as_ptr().cast(),
            core::ptr::null(),
            0,
            &mut raw,
        )
    };
    // SAFETY: firmware either left the out-parameter null or set a handle.
    let handle = unsafe { Handle::from_ptr(raw) };
    if status == Status::SUCCESS {
        return handle.ok_or(Status::LOAD_ERROR);
    }
    if let Some(handle) = handle {
        let _ = boot::unload_image(handle);
        firmware::mark("chainload unloaded rejected image");
    }
    Err(status)
}

fn load_failed(status: Status) -> LaunchResult {
    LaunchResult::LoadFailed {
        failure: failure(status),
        status: alloc::format!("{status:?}"),
    }
}

pub fn failure(status: Status) -> StartFailure {
    match status {
        Status::SECURITY_VIOLATION | Status::ACCESS_DENIED => StartFailure::SecurityViolation,
        Status::NOT_FOUND => StartFailure::NotFound,
        Status::UNSUPPORTED | Status::INCOMPATIBLE_VERSION => StartFailure::Unsupported,
        _ => StartFailure::Other,
    }
}
