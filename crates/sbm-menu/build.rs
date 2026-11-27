//! Computes Smart Boot Menu's compile-time software version from Git history
//! using the 0SFS / FOSS Earth versioning rule:
//!
//! - Commits: `YY.MM.DD#N` (UTC commit date and 1-based commit count on that day)
//! - Long-term support releases tagged `YY.MM.DD-LTS`: `YY.MM.DD LTS`
//!
//! Set `SBM_VERSION` in the environment to override (e.g. source tarball builds
//! without `.git`). Falls back to `CARGO_PKG_VERSION` if Git history is unavailable.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION_LOOKBACK: &str = "400";

fn main() {
    println!("cargo:rerun-if-env-changed=SBM_VERSION");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let dir = Path::new(&manifest_dir);
    emit_git_rerun_hints(dir);

    let version = env::var("SBM_VERSION")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| git_lts_tag(dir))
        .or_else(|| git_commit_version(dir))
        .unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".into()));

    println!("cargo:rustc-env=SBM_VERSION={version}");
}

fn git_output(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn emit_git_rerun_hints(dir: &Path) {
    let Some(git_dir_raw) = git_output(dir, &["rev-parse", "--git-dir"]) else {
        return;
    };
    let git_dir = PathBuf::from(&git_dir_raw);
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        dir.join(git_dir)
    };
    let head = git_dir.join("HEAD");
    if head.exists() {
        println!("cargo:rerun-if-changed={}", head.display());
        if let Ok(head_text) = std::fs::read_to_string(&head)
            && let Some(ref_rel) = head_text.trim().strip_prefix("ref: ")
        {
            let ref_path = git_dir.join(ref_rel.trim());
            if ref_path.exists() {
                println!("cargo:rerun-if-changed={}", ref_path.display());
            }
        }
    }
    let packed = git_dir.join("packed-refs");
    if packed.exists() {
        println!("cargo:rerun-if-changed={}", packed.display());
    }
}

fn git_lts_tag(dir: &Path) -> Option<String> {
    let tags = git_output(dir, &["tag", "--points-at", "HEAD"])?;
    for tag in tags.lines().map(str::trim) {
        if is_lts_tag(tag) {
            return Some(format!("{} LTS", &tag[..8]));
        }
    }
    None
}

fn is_lts_tag(tag: &str) -> bool {
    let b = tag.as_bytes();
    b.len() == 12
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2] == b'.'
        && b[3].is_ascii_digit()
        && b[4].is_ascii_digit()
        && b[5] == b'.'
        && b[6].is_ascii_digit()
        && b[7].is_ascii_digit()
        && &b[8..] == b"-LTS"
}

fn git_commit_version(dir: &Path) -> Option<String> {
    let raw = git_output(
        dir,
        &["log", &format!("-n{VERSION_LOOKBACK}"), "--format=%ct"],
    )?;
    let mut days = Vec::new();
    for line in raw.lines() {
        if let Ok(secs) = line.trim().parse::<i64>() {
            days.push(utc_ymd_from_unix(secs));
        }
    }
    let head = *days.first()?;
    let count = days.iter().filter(|&&d| d == head).count();
    let (year, month, day) = head;
    Some(format!(
        "{:02}.{:02}.{:02}#{count}",
        year.rem_euclid(100),
        month,
        day
    ))
}

/// Convert Unix epoch seconds to UTC `(year, month, day)` using Howard Hinnant's
/// proleptic Gregorian civil calendar formula.
fn utc_ymd_from_unix(unix_secs: i64) -> (i64, u32, u32) {
    let days = unix_secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}
