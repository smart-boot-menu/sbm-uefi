#![no_main]
#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use sbm_boot::{decode_attempt, decode_handoff, hex_encode, sha256};
use sbm_menu::flow::{ATTEMPT_PATH, HANDOFF_PATH, HANDOFF_VERSION, HandoffVar};
use sbm_protocol::{BootAttemptStatus, is_uuid_v4};

use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;

use uefi::CString16;
use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams};
use uefi::proto::console::serial::Serial;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::file::{File, FileAttribute, FileMode, FileType};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::runtime::{self, VariableAttributes, VariableVendor};
use uefi::{Handle, Status, boot, entry, guid, helpers, system};

static SERIAL_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Unsigned `.cmdline` section, the same name systemd-stub reads.
/// Recovery uses the embedded value and receives no external options.
#[used]
#[unsafe(link_section = ".cmdline")]
static EMBEDDED_CMDLINE: [u8; 13] = *b"console=tty0\0";

#[entry]
fn main() -> Status {
    let _ = helpers::init();
    let _ = boot::set_watchdog_timer(0, 0, None);
    emit(b"SBM_MARK fixture_started\n");
    if let Err(error) = record_chainload() {
        return error.status();
    }
    boot::stall(Duration::from_millis(200));
    Status::SUCCESS
}

fn record_chainload() -> uefi::Result<()> {
    let image = open::<LoadedImage>(boot::image_handle())?;
    let device = image.device().ok_or(Status::UNSUPPORTED)?;
    let mut fs = open::<SimpleFileSystem>(device)?;
    let mut root = fs.open_volume()?;
    if handoff_var(HandoffVar::LoaderEntrySelected)
        .is_some_and(|id| id == sbm_boot::RECOVERY_TARGET_ID)
    {
        validate_recovery(&image, &mut root).ok_or(Status::COMPROMISED_DATA)?;
    }
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

fn validate_recovery(
    image: &LoadedImage,
    root: &mut uefi::proto::media::file::Directory,
) -> Option<()> {
    if image
        .load_options_as_bytes()
        .is_some_and(|options| !options.is_empty())
    {
        return None;
    }
    let partuuid = handoff_var(HandoffVar::LoaderDevicePartUuid)?;
    let attempt_id = handoff_var(HandoffVar::SbmBootAttemptId)?;
    let digest = handoff_var(HandoffVar::SbmHandoffSha256)?;
    let version = handoff_var(HandoffVar::SbmHandoffVersion)?;
    let handoff_bytes = read_record(root, HANDOFF_PATH)?;
    let attempt = decode_attempt(&read_record(root, ATTEMPT_PATH)?).ok()?;
    let handoff = decode_handoff(&handoff_bytes).ok()?;
    if version != HANDOFF_VERSION
        || !is_uuid_v4(&attempt_id)
        || attempt_id != handoff.attempt_id
        || attempt_id != attempt.attempt_id
        || attempt.target_id.as_str() != sbm_boot::RECOVERY_TARGET_ID
        || attempt.status != BootAttemptStatus::Started
        || digest != hex_encode(&sha256(&handoff_bytes))
    {
        return None;
    }
    emit(alloc::format!("SBM_MARK fixture_recovery_valid esp={partuuid}\n").as_bytes());
    Some(())
}

fn handoff_var(var: HandoffVar) -> Option<String> {
    let name = CString16::try_from(var.name()).ok()?;
    let vendor = if var.systemd_vendor() {
        VariableVendor(guid!("4a67b082-0a4c-41cf-b6c7-440b29bb8c4f"))
    } else {
        VariableVendor(guid!("d8f4a21c-7b6e-4c91-9e32-8a5c1f0e3b40"))
    };
    let (bytes, attrs) = runtime::get_variable_boxed(&name, &vendor).ok()?;
    if attrs != VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS {
        return None;
    }
    let (pairs, remainder) = bytes.as_chunks::<2>();
    let (end, text) = pairs.split_last()?;
    if !remainder.is_empty() || *end != [0, 0] {
        return None;
    }
    let units: alloc::vec::Vec<_> = text.iter().copied().map(u16::from_le_bytes).collect();
    String::from_utf16(&units).ok()
}

fn read_record(
    root: &mut uefi::proto::media::file::Directory,
    path: &str,
) -> Option<alloc::vec::Vec<u8>> {
    let path = CString16::try_from(path).ok()?;
    let handle = root
        .open(&path, FileMode::Read, FileAttribute::empty())
        .ok()?;
    let FileType::Regular(mut file) = handle.into_type().ok()? else {
        return None;
    };
    let mut bytes = vec![0; 65536];
    let count = file.read(&mut bytes).ok()?;
    bytes.truncate(count);
    Some(bytes)
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
