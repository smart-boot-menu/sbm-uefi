mod esp;
mod parse;
mod pe;
mod qemu;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::esp::assemble;
use crate::pe::set_nx_compat;
use crate::qemu::{AARCH64, Guest, X86_64};

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("all") => {
            build()?;
            qemu_one(&AARCH64)?;
            qemu_one(&X86_64)?;
        }
        Some("build") => build()?,
        Some("qemu") => match args.next().as_deref() {
            None => {
                qemu_one(&AARCH64)?;
                qemu_one(&X86_64)?;
            }
            Some("aarch64") => qemu_one(&AARCH64)?,
            Some("x86_64") => qemu_one(&X86_64)?,
            Some(other) => return Err(format!("unknown architecture {other}")),
        },
        Some(other) => return Err(format!("unknown command {other}")),
    }
    Ok(())
}

fn build() -> Result<(), String> {
    let root = workspace();
    for guest in [&AARCH64, &X86_64] {
        let status = Command::new(cargo())
            .current_dir(&root)
            .args([
                "build",
                "--release",
                "--target",
                guest.triple,
                "-p",
                "sbm-app",
                "-p",
                "sbm-fixture",
            ])
            .status()
            .map_err(|err| format!("cargo: {err}"))?;
        if !status.success() {
            return Err(format!("release build failed for {}", guest.triple));
        }
        let menu = root
            .join("target")
            .join(guest.triple)
            .join("release/sbm.efi");
        let fixture = root
            .join("target")
            .join(guest.triple)
            .join("release/sbm-fixture.efi");
        let mut menu_bytes =
            fs::read(&menu).map_err(|err| format!("read {}: {err}", menu.display()))?;
        if !menu_bytes.windows(7).any(|window| window == b"sbat,1,") {
            return Err(format!("{} is missing the SBAT section", menu.display()));
        }
        set_nx_compat(&mut menu_bytes)?;
        let artifacts = root.join("artifacts");
        fs::create_dir_all(&artifacts).map_err(|err| err.to_string())?;
        let menu_out = artifacts.join(format!("sbm-{}.efi", guest.name));
        let fixture_out = artifacts.join(format!("sbm-fixture-{}.efi", guest.name));
        fs::write(&menu_out, &menu_bytes).map_err(|err| err.to_string())?;
        fs::copy(&fixture, &fixture_out).map_err(|err| err.to_string())?;
        let size = menu_bytes.len();
        if size > 5 * 1024 * 1024 {
            return Err(format!(
                "{} is {size} bytes, above the 5 MiB limit",
                menu_out.display()
            ));
        }
        println!("{} {} bytes", menu_out.display(), size);
    }
    Ok(())
}

fn qemu_one(guest: &Guest) -> Result<(), String> {
    let root = workspace();
    build()?;
    let menu = fs::read(
        root.join("artifacts")
            .join(format!("sbm-{}.efi", guest.name)),
    )
    .map_err(|err| err.to_string())?;
    let fixture = fs::read(
        root.join("artifacts")
            .join(format!("sbm-fixture-{}.efi", guest.name)),
    )
    .map_err(|err| err.to_string())?;
    let esp = assemble(
        &root.join("build"),
        guest.name,
        guest.tag,
        guest.bls_arch,
        &menu,
        &fixture,
    )?;
    let measured = qemu::run(&root, guest, &esp.dir)?;
    println!("{}", measured.summary);
    Ok(())
}

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cargo() -> String {
    env::var("CARGO").unwrap_or_else(|_| String::from("cargo"))
}
