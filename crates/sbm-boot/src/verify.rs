//! Launch-time integrity verification of the recovery UKI (Spec 03 §9).
//!
//! The image is streamed once from a single open file handle through a
//! caller-owned bounded buffer. Verification fails closed on any I/O error,
//! a short read, or a digest mismatch. A match proves the bytes equal the
//! manifest digest; it is not a signature check.

use alloc::string::String;

use sha2::{Digest, Sha256};

use crate::pathutil::hex_encode;
use crate::volume::{VolError, Volume};

/// Chunk size used by the picker. Large sequential reads keep FAT and block
/// I/O overhead low; the buffer is allocated once and reused.
pub const VERIFY_CHUNK_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub file_size: u64,
    pub bytes_read: u64,
    pub reads: u32,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// No digest was published for the entry.
    NoDigest,
    Missing,
    Io,
    Truncated {
        file_size: u64,
        bytes_read: u64,
    },
    Mismatch {
        expected: String,
        actual: String,
    },
}

impl VerifyError {
    pub fn describe(&self) -> String {
        match self {
            VerifyError::NoDigest => String::from("no SHA-256 digest is published for the image"),
            VerifyError::Missing => String::from("the image is missing"),
            VerifyError::Io => String::from("the image could not be read completely"),
            VerifyError::Truncated {
                file_size,
                bytes_read,
            } => alloc::format!("only {bytes_read} of {file_size} bytes could be read"),
            VerifyError::Mismatch { .. } => {
                String::from("its SHA-256 digest does not match the recovery manifest")
            }
        }
    }
}

pub fn verify_image(
    vol: &dyn Volume,
    path: &str,
    expected_hex: Option<&str>,
    buf: &mut [u8],
) -> Result<Verified, VerifyError> {
    let expected = expected_hex
        .filter(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or(VerifyError::NoDigest)?;
    let mut hasher = Sha256::new();
    let streamed = vol
        .stream(path, buf, &mut |chunk| hasher.update(chunk))
        .map_err(|err| match err {
            VolError::NotFound => VerifyError::Missing,
            VolError::Io => VerifyError::Io,
        })?;
    if streamed.file_size == 0 || streamed.bytes_read != streamed.file_size {
        return Err(VerifyError::Truncated {
            file_size: streamed.file_size,
            bytes_read: streamed.bytes_read,
        });
    }
    let digest = hex_encode(&hasher.finalize());
    if !digest.eq_ignore_ascii_case(expected) {
        return Err(VerifyError::Mismatch {
            expected: expected.to_ascii_lowercase(),
            actual: digest,
        });
    }
    Ok(Verified {
        file_size: streamed.file_size,
        bytes_read: streamed.bytes_read,
        reads: streamed.reads,
        digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pathutil::sha256;
    use crate::volume::MemoryVolume;

    const PATH: &str = r"\EFI\sbm\recovery\slot-a\sbm-core-aarch64.efi";

    fn volume(bytes: &[u8]) -> MemoryVolume {
        let vol = MemoryVolume::new("guid", false, "ESP");
        vol.write(PATH, bytes).unwrap();
        vol
    }

    #[test]
    fn streams_once_and_matches() {
        let image = alloc::vec![7u8; 3 * 1024 + 5];
        let hash = hex_encode(&sha256(&image));
        let vol = volume(&image);
        let mut buf = alloc::vec![0u8; 1024];
        let done = verify_image(&vol, PATH, Some(&hash.to_ascii_uppercase()), &mut buf).unwrap();
        assert_eq!(done.bytes_read, image.len() as u64);
        assert_eq!(done.reads, 4);
        assert_eq!(done.digest, hash);
    }

    #[test]
    fn fails_closed() {
        let image = alloc::vec![1u8; 4096];
        let hash = hex_encode(&sha256(&image));
        let vol = volume(&image);
        let mut buf = alloc::vec![0u8; 512];
        assert_eq!(
            verify_image(&vol, PATH, None, &mut buf),
            Err(VerifyError::NoDigest)
        );
        assert_eq!(
            verify_image(&vol, PATH, Some("abc"), &mut buf),
            Err(VerifyError::NoDigest)
        );
        assert_eq!(
            verify_image(&vol, r"\missing.efi", Some(&hash), &mut buf),
            Err(VerifyError::Missing)
        );
        let tampered = volume(&[2u8; 4096]);
        assert!(matches!(
            verify_image(&tampered, PATH, Some(&hash), &mut buf),
            Err(VerifyError::Mismatch { .. })
        ));
        vol.truncate_reads(PATH, 1000);
        assert_eq!(
            verify_image(&vol, PATH, Some(&hash), &mut buf),
            Err(VerifyError::Truncated {
                file_size: 4096,
                bytes_read: 1000
            })
        );
    }
}
