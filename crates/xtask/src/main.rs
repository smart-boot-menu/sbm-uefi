mod esp;
mod parse;
mod pe;
mod qemu;
mod sim;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::esp::assemble;
use crate::pe::set_nx_compat;
use crate::qemu::{AARCH64, Guest, X86_64};

fn main() {
    qemu::install_cancel_handlers();
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn native_guest() -> &'static Guest {
    if cfg!(target_arch = "aarch64") {
        &AARCH64
    } else {
        &X86_64
    }
}

fn resolve_guests(spec: Option<&str>) -> Result<Vec<&'static Guest>, String> {
    match spec {
        None | Some("native") => Ok(vec![native_guest()]),
        Some("all") | Some("both") => Ok(vec![&AARCH64, &X86_64]),
        Some("aarch64") | Some("arm64") => Ok(vec![&AARCH64]),
        Some("x86_64") | Some("x86-64") | Some("amd64") => Ok(vec![&X86_64]),
        Some(other) => Err(format!(
            "unknown architecture {other} (expected native|aarch64|x86_64|all)"
        )),
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None => {
            // Fast default: build and smoke-test native architecture only.
            let guest = native_guest();
            build_guests(&[guest])?;
            qemu_one(guest, false)?;
        }
        Some("all") => {
            let guests = vec![&AARCH64, &X86_64];
            build_guests(&guests)?;
            for guest in guests {
                qemu_one(guest, false)?;
            }
        }
        Some("build") => {
            let spec = args.next();
            let guests = resolve_guests(spec.as_deref())?;
            build_guests(&guests)?;
        }
        Some("qemu") => {
            let spec = args.next();
            let guests = resolve_guests(spec.as_deref())?;
            build_guests(&guests)?;
            for guest in guests {
                qemu_one(guest, false)?;
            }
        }
        Some("preview") => sim::preview(&workspace(), args.collect())?,
        Some("sim") => sim::sim(&workspace(), args.collect())?,
        Some("core") => sim::core(&workspace(), args.collect())?,
        Some("help" | "-h" | "--help") => {
            println!(
                "cargo xtask [build|qemu|all] [native|aarch64|x86_64|all]\n\n{}",
                sim::HELP
            );
        }
        Some(other) => {
            return Err(format!(
                "unknown command {other} (expected build|qemu|all|preview|sim|core|help)"
            ));
        }
    }
    Ok(())
}

fn build_guests(guests: &[&Guest]) -> Result<(), String> {
    let root = workspace();
    for guest in guests {
        let mut cmd = Command::new(cargo());
        cmd.current_dir(&root).args([
            "build",
            "--release",
            "--target",
            guest.triple,
            "-p",
            "sbm-app",
            "-p",
            "sbm-fixture",
        ]);
        let status = cmd.status().map_err(|err| format!("cargo: {err}"))?;
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

fn qemu_one(guest: &Guest, rebuild: bool) -> Result<(), String> {
    let root = workspace();
    if rebuild {
        build_guests(&[guest])?;
    }
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
