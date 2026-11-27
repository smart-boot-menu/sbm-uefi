use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use sbm_types::{CpuArch, FirmwarePlatform, PlatformCapabilities, SecureBootState};
use uefi::boot::{OpenProtocolAttributes, OpenProtocolParams, ScopedProtocol};
use uefi::proto::ProtocolPointer;
use uefi::proto::console::serial::Serial;
use uefi::runtime::{ResetType, VariableAttributes, VariableVendor};
use uefi::{CStr16, CString16, Handle, Status, boot, cstr16, guid, runtime, system};

static SERIAL_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);

pub struct Clock {
    origin: u64,
    ticks_per_us: u64,
}

impl Clock {
    pub fn start() -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            let hz = cntfrq();
            Self {
                origin: cntpct(),
                ticks_per_us: (hz / 1_000_000).max(1),
            }
        }
        #[cfg(target_arch = "x86_64")]
        {
            let origin = rdtsc();
            let before = rdtsc();
            boot::stall(core::time::Duration::from_millis(1));
            let after = rdtsc();
            Self {
                origin,
                ticks_per_us: (after.saturating_sub(before) / 1_000).max(1),
            }
        }
    }

    pub fn elapsed_us(&self) -> u64 {
        ticks().saturating_sub(self.origin) / self.ticks_per_us
    }

    pub fn hz(&self) -> u64 {
        self.ticks_per_us.saturating_mul(1_000_000)
    }
}

pub fn cpu_arch() -> CpuArch {
    #[cfg(target_arch = "x86_64")]
    {
        CpuArch::X86_64
    }
    #[cfg(target_arch = "aarch64")]
    {
        CpuArch::Aarch64
    }
}

pub fn mark(text: &str) {
    emit(&alloc::format!("SBM_MARK {text}\n"));
}

pub fn lab_marker(name: &str, fields: &str) {
    let line = if fields.is_empty() {
        alloc::format!("SBM-LAB-MARKER: {name}\n")
    } else {
        alloc::format!("SBM-LAB-MARKER: {name} {fields}\n")
    };
    emit(&line);
}

fn emit(line: &str) {
    let bytes = line.as_bytes();
    if !write_serial(bytes) {
        let _ = stdout_line(line);
    }
    debugcon(bytes);
}

pub fn open_shared<P>(handle: Handle) -> uefi::Result<ScopedProtocol<P>>
where
    P: ProtocolPointer + ?Sized,
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

pub fn protocol_present<P>() -> bool
where
    P: ProtocolPointer + ?Sized,
{
    boot::get_handle_for_protocol::<P>().is_ok()
}

pub fn c16_to_string(text: &CStr16) -> String {
    text.iter().copied().map(char::from).collect()
}

pub fn ucs2_bytes(text: &str) -> Option<Vec<u8>> {
    let coded = CString16::try_from(text).ok()?;
    let units = coded.to_u16_slice_with_nul();
    let mut bytes = Vec::with_capacity(units.len() * 2);
    for unit in units {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    Some(bytes)
}

pub fn firmware_unix() -> Option<i64> {
    let time = runtime::get_time().ok()?;
    sbm_boot::unix_timestamp(
        i32::from(time.year()),
        u32::from(time.month()),
        u32::from(time.day()),
        u32::from(time.hour()),
        u32::from(time.minute()),
        u32::from(time.second()),
    )
}

pub fn firmware_summary() -> String {
    let vendor = c16_to_string(system::firmware_vendor());
    alloc::format!("{vendor} rev {:#x}", system::firmware_revision())
}

pub fn secure_boot_state() -> SecureBootState {
    match (
        read_u8(cstr16!("SetupMode")),
        read_u8(cstr16!("SecureBoot")),
    ) {
        (Some(1), _) => SecureBootState::SetupMode,
        (_, Some(1)) => SecureBootState::Enabled,
        (_, Some(0)) => SecureBootState::Disabled,
        _ => SecureBootState::Unsupported,
    }
}

pub fn boot_entries() -> Vec<Vec<u8>> {
    let Ok((order, _)) =
        runtime::get_variable_boxed(cstr16!("BootOrder"), &VariableVendor::GLOBAL_VARIABLE)
    else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for chunk in order.as_chunks::<2>().0 {
        let id = u16::from_le_bytes(*chunk);
        let Ok(name) = CString16::try_from(alloc::format!("Boot{id:04X}").as_str()) else {
            continue;
        };
        if let Ok((value, _)) = runtime::get_variable_boxed(&name, &VariableVendor::GLOBAL_VARIABLE)
        {
            entries.push(value.into_vec());
        }
    }
    entries
}

pub fn nvram_accepts_volatile_write() -> bool {
    let Ok(name) = CString16::try_from("SbmProbe") else {
        return false;
    };
    let vendor = sbm_vendor();
    let attributes = VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;
    let wrote = runtime::set_variable(&name, &vendor, attributes, b"1").is_ok();
    let _ = runtime::delete_variable(&name, &vendor);
    wrote
}

pub fn publish_selection(id: &str) {
    let Ok(name) = CString16::try_from("LoaderEntrySelected") else {
        return;
    };
    let Some(bytes) = ucs2_bytes(id) else {
        return;
    };
    let attributes = VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;
    let _ = runtime::set_variable(&name, &sbm_vendor(), attributes, &bytes);
    let _ = runtime::set_variable(&name, &systemd_vendor(), attributes, &bytes);
}

pub fn firmware_ui_supported() -> bool {
    read_u64(cstr16!("OsIndicationsSupported")).is_some_and(|bits| bits & 1 != 0)
}

pub fn reboot_to_firmware_ui() -> Result<(), &'static str> {
    if !firmware_ui_supported() {
        return Err("firmware setup is not offered by this firmware");
    }
    let bits = read_u64(cstr16!("OsIndications")).unwrap_or(0) | 1;
    runtime::set_variable(
        cstr16!("OsIndications"),
        &VariableVendor::GLOBAL_VARIABLE,
        VariableAttributes::NON_VOLATILE
            | VariableAttributes::BOOTSERVICE_ACCESS
            | VariableAttributes::RUNTIME_ACCESS,
        &bits.to_le_bytes(),
    )
    .map_err(|_| "could not set OsIndications")?;
    runtime::reset(ResetType::COLD, Status::SUCCESS, None);
}

pub fn reboot() -> ! {
    runtime::reset(ResetType::COLD, Status::SUCCESS, None);
}

pub fn shutdown() -> ! {
    runtime::reset(ResetType::SHUTDOWN, Status::SUCCESS, None);
}

pub fn ram_mib() -> Option<u64> {
    use uefi::mem::memory_map::MemoryMap;
    use uefi::mem::memory_map::MemoryType;
    let map = boot::memory_map(MemoryType::LOADER_DATA).ok()?;
    let mut bytes = 0u64;
    for descriptor in map.entries() {
        if is_ram(descriptor.ty) {
            bytes = bytes.saturating_add(descriptor.page_count.saturating_mul(4096));
        }
    }
    Some(bytes / (1024 * 1024))
}

pub fn platform(
    gop: bool,
    removable: bool,
    nvram: bool,
    tpm: bool,
    network: bool,
    ram: Option<u64>,
) -> PlatformCapabilities {
    PlatformCapabilities {
        arch: cpu_arch(),
        firmware: FirmwarePlatform::UefiNative,
        page_size_bytes: 4096,
        secure_boot_state: secure_boot_state(),
        tpm_present: tpm,
        nvram_writable: nvram,
        gop_framebuffer: gop,
        uefi_networking: network,
        can_chainload_efi: true,
        can_direct_chainload_macos: false,
        external_usb_boot_supported: removable,
        ram_total_mib: ram,
    }
}

pub use sbm_menu::Key as FirmwareKey;

pub fn poll_key() -> Option<FirmwareKey> {
    let key = system::with_stdin(|input| input.read_key().ok().flatten())?;
    match key {
        uefi::proto::console::text::Key::Special(code) => match code {
            uefi::proto::console::text::ScanCode::UP => Some(FirmwareKey::Up),
            uefi::proto::console::text::ScanCode::DOWN => Some(FirmwareKey::Down),
            uefi::proto::console::text::ScanCode::LEFT => Some(FirmwareKey::Left),
            uefi::proto::console::text::ScanCode::RIGHT => Some(FirmwareKey::Right),
            uefi::proto::console::text::ScanCode::ESCAPE => Some(FirmwareKey::Escape),
            _ => None,
        },
        uefi::proto::console::text::Key::Printable(ch) => match char::from(ch) {
            '\r' | '\n' => Some(FirmwareKey::Activate),
            '\t' => Some(FirmwareKey::Tab),
            other => Some(FirmwareKey::Char(other)),
        },
    }
}

fn read_u8(name: &CStr16) -> Option<u8> {
    let (data, _) = runtime::get_variable_boxed(name, &VariableVendor::GLOBAL_VARIABLE).ok()?;
    data.first().copied()
}

fn read_u64(name: &CStr16) -> Option<u64> {
    let (data, _) = runtime::get_variable_boxed(name, &VariableVendor::GLOBAL_VARIABLE).ok()?;
    let bytes: [u8; 8] = data.get(..8)?.try_into().ok()?;
    Some(u64::from_le_bytes(bytes))
}

fn sbm_vendor() -> VariableVendor {
    VariableVendor(guid!("d8f4a21c-7b6e-4c91-9e32-8a5c1f0e3b40"))
}

fn systemd_vendor() -> VariableVendor {
    VariableVendor(guid!("4a67b082-0a4c-41cf-b6c7-440b29bb8c4f"))
}

fn is_ram(kind: uefi::mem::memory_map::MemoryType) -> bool {
    use uefi::mem::memory_map::MemoryType;
    matches!(
        kind,
        MemoryType::CONVENTIONAL
            | MemoryType::LOADER_CODE
            | MemoryType::LOADER_DATA
            | MemoryType::BOOT_SERVICES_CODE
            | MemoryType::BOOT_SERVICES_DATA
            | MemoryType::RUNTIME_SERVICES_CODE
            | MemoryType::RUNTIME_SERVICES_DATA
            | MemoryType::ACPI_RECLAIM
    )
}

fn write_serial(bytes: &[u8]) -> bool {
    let Ok(handles) = boot::find_handles::<Serial>() else {
        return false;
    };
    if handles.is_empty() {
        return false;
    }
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
    let Ok(mut serial) = open_shared::<Serial>(handle) else {
        return false;
    };
    let mut mode = *serial.io_mode();
    if mode.timeout == 0 || mode.timeout > 20_000 {
        mode.timeout = 20_000;
        let _ = serial.set_attributes(&mode);
    }
    serial.write(bytes).is_ok()
}

fn stdout_line(text: &str) -> bool {
    let Ok(coded) = CString16::try_from(text) else {
        return false;
    };
    system::with_stdout(|output| output.output_string(&coded).is_ok())
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

#[cfg(target_arch = "aarch64")]
fn cntpct() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!(
            "mrs {value}, cntpct_el0",
            value = out(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

#[cfg(target_arch = "aarch64")]
fn cntfrq() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!(
            "mrs {value}, cntfrq_el0",
            value = out(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

#[cfg(target_arch = "x86_64")]
fn rdtsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn ticks() -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        cntpct()
    }
    #[cfg(target_arch = "x86_64")]
    {
        rdtsc()
    }
}
