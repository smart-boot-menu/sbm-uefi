use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use sbm_boot::{DirEntry, FileMeta, VolError, Volume, normalize};
use uefi::proto::device_path::build::{self, DevicePathBuilder};
use uefi::proto::device_path::media::{self, PartitionSignature};
use uefi::proto::device_path::{DevicePath, DeviceSubType, DeviceType};
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::block::BlockIO;
use uefi::proto::media::file::{Directory, File, FileAttribute, FileInfo, FileMode, FileType};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::{CString16, Handle, boot};

use crate::firmware::{self, c16_to_string, open_shared};

pub struct Disk {
    pub boot: bool,
    pub volume: UefiVolume,
}

pub struct HostImage {
    pub path: Option<String>,
    pub fs_handle: Option<Handle>,
    pub device: Option<Handle>,
}

pub struct UefiVolume {
    fs: RefCell<uefi::boot::ScopedProtocol<SimpleFileSystem>>,
    guid: String,
    removable: bool,
    label: String,
    device_path: Vec<u8>,
}

impl UefiVolume {
    pub fn device_path(&self) -> &[u8] {
        &self.device_path
    }

    fn open(handle: Handle) -> Result<Self, ()> {
        let fs = open_shared::<SimpleFileSystem>(handle).map_err(|_| ())?;
        let device_path = device_path_bytes(handle);
        let mut inspected = inspect_path(&device_path);
        if let Ok(block) = open_shared::<BlockIO>(handle)
            && block.media().is_removable_media()
        {
            inspected.removable = true;
        }
        let label = if inspected.removable { "USB" } else { "ESP" };
        Ok(Self {
            fs: RefCell::new(fs),
            guid: inspected.guid,
            removable: inspected.removable,
            label: String::from(label),
            device_path,
        })
    }

    fn with_root<T>(
        &self,
        body: impl FnOnce(&mut Directory) -> Result<T, VolError>,
    ) -> Result<T, VolError> {
        let mut proto = self.fs.borrow_mut();
        let mut root = proto.open_volume().map_err(|_| VolError::Io)?;
        body(&mut root)
    }
}

impl Volume for UefiVolume {
    fn partition_guid(&self) -> &str {
        &self.guid
    }

    fn removable(&self) -> bool {
        self.removable
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn exists(&self, path: &str) -> bool {
        self.metadata(path).is_ok()
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, VolError> {
        self.with_root(|root| read_file(root, path, None))
    }

    fn read_prefix(&self, path: &str, max: usize) -> Result<Vec<u8>, VolError> {
        self.with_root(|root| read_file(root, path, Some(max)))
    }

    fn metadata(&self, path: &str) -> Result<FileMeta, VolError> {
        let path = normalize(path);
        if path == "\\" {
            return Ok(FileMeta {
                size: 0,
                is_dir: true,
            });
        }
        self.with_root(|root| {
            let name = cstr(&path)?;
            let handle = root
                .open(&name, FileMode::Read, FileAttribute::empty())
                .map_err(|_| VolError::NotFound)?;
            match handle.into_type().map_err(|_| VolError::Io)? {
                FileType::Regular(mut file) => info_of(&mut file),
                FileType::Dir(mut dir) => {
                    let mut meta = info_of(&mut dir)?;
                    meta.is_dir = true;
                    Ok(meta)
                }
            }
        })
    }

    fn list(&self, path: &str) -> Result<Vec<DirEntry>, VolError> {
        self.with_root(|root| list_dir(root, path))
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), VolError> {
        self.with_root(|root| write_file(root, path, data))
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), VolError> {
        let data = self.read(from)?;
        self.write(to, &data)?;
        self.remove(from)
    }

    fn remove(&self, path: &str) -> Result<(), VolError> {
        self.with_root(|root| {
            let name = cstr(&normalize(path))?;
            let handle = root
                .open(&name, FileMode::ReadWrite, FileAttribute::empty())
                .map_err(|_| VolError::NotFound)?;
            match handle.into_type().map_err(|_| VolError::Io)? {
                FileType::Regular(file) => file.delete().map_err(|_| VolError::Io),
                FileType::Dir(_) => Err(VolError::Io),
            }
        })
    }
}

pub fn host_image() -> HostImage {
    let Ok(image) = open_shared::<LoadedImage>(boot::image_handle()) else {
        return HostImage {
            path: None,
            fs_handle: None,
            device: None,
        };
    };
    let device = image.device();
    let path = image.file_path().and_then(file_path_string);
    let fs_handle = device.and_then(|handle| {
        let Ok(path) = open_shared::<DevicePath>(handle) else {
            return None;
        };
        let mut remaining = &*path;
        boot::locate_device_path::<SimpleFileSystem>(&mut remaining).ok()
    });
    HostImage {
        path,
        fs_handle,
        device,
    }
}

pub fn scan_disks(host: &HostImage) -> Vec<Disk> {
    let Ok(handles) = boot::find_handles::<SimpleFileSystem>() else {
        return Vec::new();
    };
    let mut disks = Vec::new();
    for handle in handles {
        let Ok(volume) = UefiVolume::open(handle) else {
            continue;
        };
        let boot = host.fs_handle == Some(handle) || host.device == Some(handle);
        disks.push(Disk { boot, volume });
    }
    if !disks.iter().any(|disk| disk.boot)
        && let Some(disk) = disks.first_mut()
    {
        disk.boot = true;
        firmware::mark("boot_volume fallback");
    }
    disks
}

pub fn load_from_device_path(volume_path: &[u8], file: &uefi::CStr16) -> uefi::Result<Handle> {
    let volume =
        <&DevicePath>::try_from(volume_path).map_err(|_| uefi::Status::INVALID_PARAMETER)?;
    let mut storage = Vec::new();
    let mut builder = DevicePathBuilder::with_vec(&mut storage);
    for node in volume.node_iter() {
        if node.device_type() == DeviceType::END || is_file_node(node) {
            continue;
        }
        builder = builder
            .push(&node)
            .map_err(|_| uefi::Status::INVALID_PARAMETER)?;
    }
    builder = builder
        .push(&build::media::FilePath { path_name: file })
        .map_err(|_| uefi::Status::INVALID_PARAMETER)?;
    let path = builder
        .finalize()
        .map_err(|_| uefi::Status::INVALID_PARAMETER)?;
    boot::load_image(
        boot::image_handle(),
        uefi::boot::LoadImageSource::FromDevicePath {
            device_path: path,
            boot_policy: uefi::proto::BootPolicy::ExactMatch,
        },
    )
}

fn list_dir(root: &mut Directory, path: &str) -> Result<Vec<DirEntry>, VolError> {
    let path = normalize(path);
    if path == "\\" {
        return read_entries(root);
    }
    let name = cstr(&path)?;
    let handle = root
        .open(&name, FileMode::Read, FileAttribute::empty())
        .map_err(|_| VolError::NotFound)?;
    match handle.into_type().map_err(|_| VolError::Io)? {
        FileType::Dir(mut dir) => read_entries(&mut dir),
        FileType::Regular(_) => Err(VolError::Io),
    }
}

fn read_entries(dir: &mut Directory) -> Result<Vec<DirEntry>, VolError> {
    let mut entries = Vec::new();
    loop {
        match dir.read_entry_boxed() {
            Ok(Some(info)) => {
                let name = c16_to_string(info.file_name());
                if name.is_empty() || name == "." || name == ".." {
                    continue;
                }
                entries.push(DirEntry {
                    is_dir: info.is_directory(),
                    name,
                });
            }
            Ok(None) => return Ok(entries),
            Err(_) => return Err(VolError::Io),
        }
    }
}

fn read_file(root: &mut Directory, path: &str, max: Option<usize>) -> Result<Vec<u8>, VolError> {
    let path = normalize(path);
    let name = cstr(&path)?;
    let handle = root
        .open(&name, FileMode::Read, FileAttribute::empty())
        .map_err(|_| VolError::NotFound)?;
    let FileType::Regular(mut file) = handle.into_type().map_err(|_| VolError::Io)? else {
        return Err(VolError::Io);
    };
    let info = file
        .get_boxed_info::<FileInfo>()
        .map_err(|_| VolError::Io)?;
    let size = usize::try_from(info.file_size()).unwrap_or(usize::MAX);
    let want = match max {
        Some(limit) => size.min(limit),
        None if size > 32 * 1024 * 1024 => return Err(VolError::Io),
        None => size,
    };
    let mut buffer = alloc::vec![0u8; want];
    if want == 0 {
        return Ok(buffer);
    }
    let read = file.read(&mut buffer).map_err(|_| VolError::Io)?;
    buffer.truncate(read);
    Ok(buffer)
}

fn write_file(root: &mut Directory, path: &str, data: &[u8]) -> Result<(), VolError> {
    let path = normalize(path);
    ensure_parents(root, &path)?;
    let name = cstr(&path)?;
    if let Ok(handle) = root.open(&name, FileMode::ReadWrite, FileAttribute::empty()) {
        if let Ok(FileType::Regular(file)) = handle.into_type() {
            let _ = file.delete();
        } else {
            return Err(VolError::Io);
        }
    }
    let handle = root
        .open(&name, FileMode::CreateReadWrite, FileAttribute::empty())
        .map_err(|_| VolError::Io)?;
    let FileType::Regular(mut file) = handle.into_type().map_err(|_| VolError::Io)? else {
        return Err(VolError::Io);
    };
    file.write(data).map(|_| ()).map_err(|_| VolError::Io)
}

fn ensure_parents(root: &mut Directory, path: &str) -> Result<(), VolError> {
    let parts: Vec<&str> = path.split('\\').filter(|part| !part.is_empty()).collect();
    if parts.len() <= 1 {
        return Ok(());
    }
    let mut current = String::new();
    for part in &parts[..parts.len() - 1] {
        current.push('\\');
        current.push_str(part);
        let name = cstr(&current)?;
        if root
            .open(&name, FileMode::Read, FileAttribute::empty())
            .is_ok()
        {
            continue;
        }
        root.open(&name, FileMode::CreateReadWrite, FileAttribute::DIRECTORY)
            .map_err(|_| VolError::Io)?;
    }
    Ok(())
}

fn info_of(file: &mut impl File) -> Result<FileMeta, VolError> {
    let info = file
        .get_boxed_info::<FileInfo>()
        .map_err(|_| VolError::Io)?;
    Ok(FileMeta {
        size: info.file_size(),
        is_dir: info.is_directory(),
    })
}

fn cstr(path: &str) -> Result<CString16, VolError> {
    CString16::try_from(path).map_err(|_| VolError::Io)
}

struct Inspected {
    guid: String,
    removable: bool,
}

fn inspect_path(bytes: &[u8]) -> Inspected {
    let mut guid = String::from("noguid");
    let mut removable = false;
    let Ok(path) = <&DevicePath>::try_from(bytes) else {
        return Inspected { guid, removable };
    };
    for node in path.node_iter() {
        if node.device_type() == DeviceType::MESSAGING
            && matches!(
                node.sub_type(),
                DeviceSubType::MESSAGING_USB
                    | DeviceSubType::MESSAGING_USB_CLASS
                    | DeviceSubType::MESSAGING_USB_WWID
            )
        {
            removable = true;
        }
        if node.device_type() == DeviceType::MEDIA && node.sub_type() == DeviceSubType::MEDIA_CD_ROM
        {
            removable = true;
        }
        if let Ok(drive) = <&media::HardDrive>::try_from(node)
            && let PartitionSignature::Guid(value) = drive.partition_signature()
        {
            guid = alloc::format!("{value}");
        }
    }
    Inspected { guid, removable }
}

fn device_path_bytes(handle: Handle) -> Vec<u8> {
    open_shared::<DevicePath>(handle)
        .map(|path| path.as_bytes().to_vec())
        .unwrap_or_default()
}

fn file_path_string(path: &DevicePath) -> Option<String> {
    let mut combined = String::new();
    for node in path.node_iter() {
        if !is_file_node(node) {
            continue;
        }
        let Ok(file) = <&media::FilePath>::try_from(node) else {
            continue;
        };
        let mut part = String::new();
        for unit in file.path_name() {
            if unit == 0 {
                break;
            }
            if let Some(ch) = char::from_u32(u32::from(unit)) {
                part.push(ch);
            }
        }
        if part.is_empty() {
            continue;
        }
        if !combined.is_empty() && !combined.ends_with('\\') && !part.starts_with('\\') {
            combined.push('\\');
        }
        combined.push_str(&part);
    }
    if combined.is_empty() {
        None
    } else {
        Some(normalize(&combined))
    }
}

fn is_file_node(node: &uefi::proto::device_path::DevicePathNode) -> bool {
    node.device_type() == DeviceType::MEDIA && node.sub_type() == DeviceSubType::MEDIA_FILE_PATH
}
