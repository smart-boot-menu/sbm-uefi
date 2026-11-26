#![no_main]
#![no_std]

extern crate alloc;

use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;

use uefi::CString16;
use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams};
use uefi::proto::console::serial::Serial;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::file::{File, FileAttribute, FileMode, FileType};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::{Handle, Status, boot, entry, helpers, system};

static SERIAL_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);

#[entry]
fn main() -> Status {
    let _ = helpers::init();
    let _ = boot::set_watchdog_timer(0, 0, None);
    emit(b"SBM_MARK fixture_started\n");
    let _ = record_chainload();
    boot::stall(Duration::from_millis(200));
    Status::SUCCESS
}

fn record_chainload() -> uefi::Result<()> {
    let image = open::<LoadedImage>(boot::image_handle())?;
    let device = image.device().ok_or(Status::UNSUPPORTED)?;
    let mut fs = open::<SimpleFileSystem>(device)?;
    let mut root = fs.open_volume()?;
    let sbm = cstr16(r"\EFI\sbm")?;
    let test = cstr16(r"\EFI\sbm\test")?;
    ensure_dir(&mut root, &sbm)?;
    ensure_dir(&mut root, &test)?;
    let path = cstr16(r"\EFI\sbm\test\chainload-ok.txt")?;
    if let Ok(handle) = root.open(&path, FileMode::ReadWrite, FileAttribute::empty())
        && let Ok(FileType::Regular(file)) = handle.into_type()
    {
        let _ = file.delete();
    }
    let handle = root.open(&path, FileMode::CreateReadWrite, FileAttribute::empty())?;
    let FileType::Regular(mut file) = handle.into_type()? else {
        return Err(Status::UNSUPPORTED.into());
    };
    file.write(b"chainload-ok\n")
        .map_err(|_| Status::DEVICE_ERROR)?;
    Ok(())
}

fn ensure_dir(
    root: &mut uefi::proto::media::file::Directory,
    path: &uefi::CStr16,
) -> uefi::Result<()> {
    if root
        .open(path, FileMode::Read, FileAttribute::empty())
        .is_ok()
    {
        return Ok(());
    }
    root.open(path, FileMode::CreateReadWrite, FileAttribute::DIRECTORY)?;
    Ok(())
}

fn cstr16(text: &str) -> uefi::Result<CString16> {
    CString16::try_from(text).map_err(|_| Status::INVALID_PARAMETER.into())
}

fn emit(bytes: &[u8]) {
    if !write_serial(bytes)
        && let Ok(text) = CString16::try_from(core::str::from_utf8(bytes).unwrap_or(""))
    {
        system::with_stdout(|output| {
            let _ = output.output_string(&text);
        });
    }
    debugcon(bytes);
}

fn write_serial(bytes: &[u8]) -> bool {
    let Ok(handles) = boot::find_handles::<Serial>() else {
        return false;
    };
    let slot = SERIAL_SLOT.load(Ordering::Relaxed);
    if slot < handles.len() && write_one(handles[slot], bytes) {
        return true;
    }
    for (index, handle) in handles.into_iter().enumerate() {
        if write_one(handle, bytes) {
            SERIAL_SLOT.store(index, Ordering::Relaxed);
            return true;
        }
    }
    false
}

fn write_one(handle: Handle, bytes: &[u8]) -> bool {
    let Ok(mut serial) = open::<Serial>(handle) else {
        return false;
    };
    let mut mode = *serial.io_mode();
    if mode.timeout == 0 || mode.timeout > 20_000 {
        mode.timeout = 20_000;
        let _ = serial.set_attributes(&mode);
    }
    serial.write(bytes).is_ok()
}

fn open<P>(handle: Handle) -> uefi::Result<uefi::boot::ScopedProtocol<P>>
where
    P: uefi::proto::ProtocolPointer + ?Sized,
{
    unsafe {
        boot::open_protocol::<P>(
            OpenProtocolParams {
                handle,
                agent: boot::image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }
}

fn debugcon(bytes: &[u8]) {
    #[cfg(target_arch = "x86_64")]
    for &byte in bytes {
        unsafe {
            core::arch::asm!(
                "out dx, al",
                in("dx") 0x402u16,
                in("al") byte,
                options(nomem, nostack, preserves_flags)
            );
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = bytes;
}
