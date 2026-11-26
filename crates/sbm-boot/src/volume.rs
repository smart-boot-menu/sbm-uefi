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

pub trait Volume {
    fn partition_guid(&self) -> &str;
    fn removable(&self) -> bool;
    fn label(&self) -> &str;
    fn exists(&self, path: &str) -> bool;
    fn read(&self, path: &str) -> Result<Vec<u8>, VolError>;
    fn read_prefix(&self, path: &str, max: usize) -> Result<Vec<u8>, VolError>;
    fn metadata(&self, path: &str) -> Result<FileMeta, VolError>;
    fn list(&self, path: &str) -> Result<Vec<DirEntry>, VolError>;
    fn write(&self, path: &str, data: &[u8]) -> Result<(), VolError>;
    fn rename(&self, from: &str, to: &str) -> Result<(), VolError>;
    fn remove(&self, path: &str) -> Result<(), VolError>;
}

pub struct MemoryVolume {
    guid: String,
    removable: bool,
    label: String,
    files: RefCell<BTreeMap<String, Vec<u8>>>,
}

impl MemoryVolume {
    pub fn new(guid: &str, removable: bool, label: &str) -> Self {
        Self {
            guid: guid.to_string(),
            removable,
            label: label.to_string(),
            files: RefCell::new(BTreeMap::new()),
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

    fn write(&self, path: &str, data: &[u8]) -> Result<(), VolError> {
        self.files
            .borrow_mut()
            .insert(normalize(path), data.to_vec());
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), VolError> {
        let mut files = self.files.borrow_mut();
        let data = files.remove(&normalize(from)).ok_or(VolError::NotFound)?;
        files.insert(normalize(to), data);
        Ok(())
    }

    fn remove(&self, path: &str) -> Result<(), VolError> {
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
