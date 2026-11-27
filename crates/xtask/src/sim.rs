//! Native development commands. They drive `sbm-preview` (this workspace) and
//! `sbm-sim` + `sbm-ui` (the sibling `sbm-core` checkout). Nothing here touches
//! firmware, disks, or boot settings on the host.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use crate::cargo;

pub const HELP: &str = "\
Native SBM development (SIMULATED; never touches host disks or NVRAM)

  cargo xtask sim [OPTIONS]        interactive UEFI menu window; Manage Computer opens native Core
      --scenario NAME              dual-boot (default), see `cargo xtask sim --list`
      --layout NAME                classic (default), bento, schematic, storage
      --font MODE                  next-mono (default), next, bitmap
      --contrast MODE              normal (default), high
      --scale PCT                  UI scale 75..=200 (default 100)
      --discovery MODE             instant (default), progressive, slow, sparse   --delay-ms N
      --theme dark|light           --display window|WxH   --size WxH
      --no-core                    skip building sbm-core (Manage Computer shows an error)
      --watch                      rebuild and relaunch on change (needs watchexec)
      --release                    optimized build
  cargo xtask preview [OPTIONS]    headless PNG screenshots into build/preview
      --scenario NAME | --all-scenarios   --screen menu|splash|diagnostics|error|all
      --layout NAME                classic, bento, schematic, storage, experiments, all
      --font MODE                  next-mono, next, bitmap, all
      --contrast MODE              normal, high, both   --scale 75..=200
      --discovery MODE             instant, progressive, slow, sparse   --delay-ms N  --at-ms N
      --theme dark|light|both  --size WxH | --sizes 800x600,1280x800,1920x1080|all
      --width N --height N  --keys Right,Enter,wait:500  --selected N  --out FILE|DIR  --open
  cargo xtask core [OPTIONS]       native Core (real sbm-ui) for a scenario, without the menu
      --scenario NAME
  cargo xtask core build           build sbm-sim and sbm-ui --features native-sim
  cargo xtask core fixtures        regenerate sbm-core/crates/sbm-sim/tests/fixtures";

pub fn core_root(root: &Path) -> PathBuf {
    env::var_os("SBM_CORE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../sbm-core"))
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let status = cmd.status().map_err(|err| format!("{what}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed ({status})"))
    }
}

fn profile_args(release: bool) -> Vec<&'static str> {
    if release {
        vec!["--release"]
    } else {
        Vec::new()
    }
}

fn core_build_cmd(root: &Path, release: bool) -> Result<Command, String> {
    let core = core_root(root);
    if !core.join("crates/sbm-sim").is_dir() {
        return Err(format!(
            "{} has no crates/sbm-sim; check out sbm-core next to sbm-uefi or set SBM_CORE_DIR",
            core.display()
        ));
    }
    let mut cmd = Command::new(cargo());
    cmd.current_dir(&core)
        .args([
            "build",
            "-p",
            "sbm-sim",
            "-p",
            "sbm-ui",
            "--features",
            "sbm-ui/native-sim",
        ])
        .args(profile_args(release));
    Ok(cmd)
}

pub fn core_bin(root: &Path, release: bool) -> PathBuf {
    core_root(root)
        .join("target")
        .join(if release { "release" } else { "debug" })
}

fn preview_build_cmd(root: &Path, release: bool, window: bool) -> Command {
    let mut cmd = Command::new(cargo());
    cmd.current_dir(root).args(["build", "-p", "sbm-preview"]);
    if window {
        cmd.args(["--features", "window"]);
    }
    cmd.args(profile_args(release));
    cmd
}

fn preview_bin(root: &Path, release: bool) -> PathBuf {
    root.join("target")
        .join(if release { "release" } else { "debug" })
        .join("sbm-preview")
}

pub fn sim(root: &Path, args: Vec<String>) -> Result<(), String> {
    let mut release = false;
    let mut with_core = true;
    let mut watch = false;
    let mut pass = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--release" => release = true,
            "--no-core" => with_core = false,
            "--watch" => watch = true,
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(());
            }
            _ => pass.push(arg),
        }
    }
    if watch {
        return watch_and_rerun(root, release, with_core, &pass);
    }
    if pass.iter().any(|arg| arg == "--list") {
        run(
            &mut preview_build_cmd(root, release, false),
            "build sbm-preview",
        )?;
        return run(
            Command::new(preview_bin(root, release)).args(&pass),
            "sbm-preview",
        );
    }

    // Build the menu and Core in parallel; Core only matters once Manage Computer is chosen.
    let core = if with_core {
        let mut cmd = core_build_cmd(root, release)?;
        Some(thread::spawn(move || {
            run(&mut cmd, "build sbm-core (sbm-sim, sbm-ui)")
        }))
    } else {
        None
    };
    let menu = run(
        &mut preview_build_cmd(root, release, true),
        "build sbm-preview",
    );
    if let Some(core) = core {
        core.join().map_err(|_| "core build thread panicked")??;
    }
    menu?;
    let mut cmd = Command::new(preview_bin(root, release));
    cmd.args(&pass);
    if with_core {
        cmd.arg("--core-bin").arg(core_bin(root, release));
    }
    run(&mut cmd, "sbm-preview")
}

fn watch_and_rerun(
    root: &Path,
    release: bool,
    with_core: bool,
    pass: &[String],
) -> Result<(), String> {
    if Command::new("watchexec").arg("--version").output().is_err() {
        return Err(
            "--watch uses watchexec (Apache-2.0), which is not installed.\n  brew install watchexec   or   cargo install --locked watchexec-cli"
                .into(),
        );
    }
    let core = core_root(root);
    let mut cmd = Command::new("watchexec");
    cmd.current_dir(root).args([
        "--restart",
        "--exts",
        "rs,toml",
        "--watch",
        "crates",
        "--watch",
        "sim",
    ]);
    if with_core && core.is_dir() {
        cmd.arg("--watch").arg(core.join("crates"));
    }
    cmd.args(["--", "cargo", "run", "-q", "-p", "xtask", "--", "sim"]);
    if release {
        cmd.arg("--release");
    }
    if !with_core {
        cmd.arg("--no-core");
    }
    cmd.args(pass);
    run(&mut cmd, "watchexec")
}

/// `cargo xtask preview`: headless screenshots. Accepts the original
/// e397e34 options (`--selected N`, `--open`) and the sbm-preview ones.
pub fn preview(root: &Path, args: Vec<String>) -> Result<(), String> {
    let mut open = false;
    let mut out = None;
    let mut keys = Vec::new();
    let mut pass = vec!["--headless".to_string()];
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--open" => open = true,
            "--selected" => {
                let n: usize = iter
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--selected requires an index")?;
                keys.extend(std::iter::repeat_n("Right".to_string(), n));
            }
            "--keys" => keys.push(iter.next().ok_or("--keys requires a list")?),
            "--out" => {
                let value = iter.next().ok_or("--out requires a path")?;
                out = Some(PathBuf::from(&value));
                pass.extend(["--out".to_string(), value]);
            }
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(());
            }
            _ => pass.push(arg),
        }
    }
    if !keys.is_empty() {
        pass.extend(["--keys".to_string(), keys.join(",")]);
    }
    run(
        &mut preview_build_cmd(root, false, false),
        "build sbm-preview",
    )?;
    run(
        Command::new(preview_bin(root, false)).args(&pass),
        "sbm-preview",
    )?;
    if open {
        let target = out.unwrap_or_else(|| root.join("build/preview"));
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = Command::new(opener).arg(target).status();
    }
    Ok(())
}

pub fn core(root: &Path, args: Vec<String>) -> Result<(), String> {
    let mut scenario = "dual-boot".to_string();
    let mut release = false;
    let mut iter = args.into_iter().peekable();
    match iter.peek().map(String::as_str) {
        Some("build") => return run(&mut core_build_cmd(root, false)?, "build sbm-core"),
        Some("fixtures") => return fixtures(root),
        _ => {}
    }
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--scenario" => scenario = iter.next().ok_or("--scenario requires a name")?,
            "--release" => release = true,
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(());
            }
            other => return Err(format!("unknown core option {other}\n\n{HELP}")),
        }
    }
    let mut core_cmd = core_build_cmd(root, release)?;
    let core = thread::spawn(move || run(&mut core_cmd, "build sbm-core"));
    let menu = run(
        &mut preview_build_cmd(root, release, false),
        "build sbm-preview",
    );
    core.join().map_err(|_| "core build thread panicked")??;
    menu?;
    let session = root.join("build/sim/core-session");
    run(
        Command::new(preview_bin(root, release))
            .args(["--scenario", &scenario, "--materialize"])
            .arg(&session),
        "materialize scenario",
    )?;
    run(
        Command::new(core_bin(root, release).join("sbm-sim"))
            .arg("--session")
            .arg(&session),
        "sbm-sim",
    )
}

fn fixtures(root: &Path) -> Result<(), String> {
    run(
        &mut preview_build_cmd(root, false, false),
        "build sbm-preview",
    )?;
    let dir = core_root(root).join("crates/sbm-sim/tests/fixtures");
    for scenario in ["dual-boot", "bitlocker"] {
        run(
            Command::new(preview_bin(root, false))
                .args(["--scenario", scenario, "--materialize"])
                .arg(dir.join(scenario)),
            "materialize fixture",
        )?;
    }
    Ok(())
}
