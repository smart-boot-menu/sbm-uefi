use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::pathutil::normalize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolError {
    NotFound,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
    pub is_dir: bool,
}

/// Result of [`Volume::stream`]: the size reported by the open file handle
/// and how many bytes were actually delivered to the sink.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Streamed {
    pub file_size: u64,
    pub bytes_read: u64,
    pub reads: u32,
}

pub trait Volume {
    fn partition_guid(&self) -> &str;
    fn removable(&self) -> bool;
    fn label(&self) -> &str;
    fn exists(&self, path: &str) -> bool;
    fn read(&self, path: &str) -> Result<Vec<u8>, VolError>;
    fn read_prefix(&self, path: &str, max: usize) -> Result<Vec<u8>, VolError>;
    fn read_at(&self, path: &str, offset: u64, max: usize) -> Result<Vec<u8>, VolError>;
    fn metadata(&self, path: &str) -> Result<FileMeta, VolError>;
    fn list(&self, path: &str) -> Result<Vec<DirEntry>, VolError>;
    /// Open `path` once and deliver its bytes in order through `buf`, which
    /// bounds memory use. Stops early only on end of file or error.
    fn stream(
        &self,
        path: &str,
        buf: &mut [u8],
        sink: &mut dyn FnMut(&[u8]),
    ) -> Result<Streamed, VolError>;
    /// Create or replace `path` and flush it to the medium before returning.
    fn write(&self, path: &str, data: &[u8]) -> Result<(), VolError>;
    /// Rename within one directory and flush. `to` must not exist.
    fn rename(&self, from: &str, to: &str) -> Result<(), VolError>;
    fn remove(&self, path: &str) -> Result<(), VolError>;
}

/// In-memory volume for host tests and the simulator. Fault hooks model a
/// medium that refuses writes or returns fewer bytes than the file size.
pub struct MemoryVolume {
    guid: String,
    removable: bool,
    label: String,
    files: RefCell<BTreeMap<String, Vec<u8>>>,
    read_only: core::cell::Cell<bool>,
    short_reads: RefCell<BTreeMap<String, u64>>,
}

impl MemoryVolume {
    pub fn new(guid: &str, removable: bool, label: &str) -> Self {
        Self {
            guid: guid.to_string(),
            removable,
            label: label.to_string(),
            files: RefCell::new(BTreeMap::new()),
            read_only: core::cell::Cell::new(false),
            short_reads: RefCell::new(BTreeMap::new()),
        }
    }

    /// Make every later write, rename and remove fail with `Io`.
    pub fn set_read_only(&self, read_only: bool) {
        self.read_only.set(read_only);
    }

    /// Make `stream` deliver at most `limit` bytes of `path`.
    pub fn truncate_reads(&self, path: &str, limit: u64) {
        self.short_reads.borrow_mut().insert(normalize(path), limit);
    }

    fn writable(&self) -> Result<(), VolError> {
        if self.read_only.get() {
            Err(VolError::Io)
        } else {
            Ok(())
        }
    }
}

impl Volume for MemoryVolume {
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
        let path = normalize(path);
        let files = self.files.borrow();
        if files.contains_key(&path) {
            return true;
        }
        let prefix = child_prefix(&path);
        files.keys().any(|key| key.starts_with(&prefix))
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, VolError> {
        self.files
            .borrow()
            .get(&normalize(path))
            .cloned()
            .ok_or(VolError::NotFound)
    }

    fn read_prefix(&self, path: &str, max: usize) -> Result<Vec<u8>, VolError> {
        let mut data = self.read(path)?;
        if data.len() > max {
            data.truncate(max);
        }
        Ok(data)
    }

    fn read_at(&self, path: &str, offset: u64, max: usize) -> Result<Vec<u8>, VolError> {
        let data = self.read(path)?;
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        if start >= data.len() {
            return Ok(Vec::new());
        }
        let end = start.saturating_add(max).min(data.len());
        Ok(data[start..end].to_vec())
    }

    fn metadata(&self, path: &str) -> Result<FileMeta, VolError> {
        let path = normalize(path);
        let files = self.files.borrow();
        if let Some(data) = files.get(&path) {
            return Ok(FileMeta {
                size: data.len() as u64,
                is_dir: false,
            });
        }
        let prefix = child_prefix(&path);
        if files.keys().any(|key| key.starts_with(&prefix)) {
            return Ok(FileMeta {
                size: 0,
                is_dir: true,
            });
        }
        Err(VolError::NotFound)
    }

    fn list(&self, path: &str) -> Result<Vec<DirEntry>, VolError> {
        let dir = normalize(path);
        let files = self.files.borrow();
        let mut names: BTreeMap<String, bool> = BTreeMap::new();
        for key in files.keys() {
            let rest = if dir == "\\" {
                key.strip_prefix('\\')
            } else {
                let prefix = child_prefix(&dir);
                key.strip_prefix(&prefix)
            };
            let Some(rest) = rest else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            let mut parts = rest.split('\\');
            let Some(name) = parts.next() else {
                continue;
            };
            let is_dir = parts.next().is_some();
            let entry = names.entry(name.to_string()).or_insert(false);
            if is_dir {
                *entry = true;
            }
        }
        Ok(names
            .into_iter()
            .map(|(name, is_dir)| DirEntry { name, is_dir })
            .collect())
    }

    fn stream(
        &self,
        path: &str,
        buf: &mut [u8],
        sink: &mut dyn FnMut(&[u8]),
    ) -> Result<Streamed, VolError> {
        let path = normalize(path);
        let files = self.files.borrow();
        let data = files.get(&path).ok_or(VolError::NotFound)?;
        let limit = self
            .short_reads
            .borrow()
            .get(&path)
            .copied()
            .unwrap_or(u64::MAX);
        let mut done = Streamed {
            file_size: data.len() as u64,
            ..Streamed::default()
        };
        if buf.is_empty() {
            return Err(VolError::Io);
        }
        let available = usize::try_from(limit).unwrap_or(usize::MAX).min(data.len());
        for chunk in data[..available].chunks(buf.len()) {
            buf[..chunk.len()].copy_from_slice(chunk);
            sink(&buf[..chunk.len()]);
            done.bytes_read += chunk.len() as u64;
            done.reads += 1;
        }
        Ok(done)
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), VolError> {
        self.writable()?;
        self.files
            .borrow_mut()
            .insert(normalize(path), data.to_vec());
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), VolError> {
        self.writable()?;
        let mut files = self.files.borrow_mut();
        if files.contains_key(&normalize(to)) {
            return Err(VolError::Io);
        }
        let data = files.remove(&normalize(from)).ok_or(VolError::NotFound)?;
        files.insert(normalize(to), data);
        Ok(())
    }

    fn remove(&self, path: &str) -> Result<(), VolError> {
        self.writable()?;
        if self.files.borrow_mut().remove(&normalize(path)).is_some() {
            Ok(())
        } else {
            Err(VolError::NotFound)
        }
    }
}

fn child_prefix(path: &str) -> String {
    if path == "\\" {
        String::from("\\")
    } else {
        alloc::format!("{path}\\")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_inferred_directories() {
        let vol = MemoryVolume::new("guid", false, "esp");
        vol.write(r"\EFI\Microsoft\Boot\bootmgfw.efi", b"pe")
            .unwrap();
        let efi = vol.list(r"\EFI").unwrap();
        assert_eq!(efi.len(), 1);
        assert!(efi[0].is_dir && efi[0].name == "Microsoft");
        let boot = vol.list(r"\EFI\Microsoft\Boot").unwrap();
        assert_eq!(boot[0].name, "bootmgfw.efi");
        assert!(!boot[0].is_dir);
        assert!(vol.exists(r"/EFI/Microsoft"));
        assert!(vol.metadata(r"\EFI\Microsoft").unwrap().is_dir);
    }
}
