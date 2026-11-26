use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::parse::{count_exact, count_prefix, log_text, metric, require_targets};

pub struct Guest {
    pub name: &'static str,
    pub triple: &'static str,
    pub qemu: &'static str,
    pub tag: &'static str,
    pub bls_arch: &'static str,
    pub code: &'static str,
    pub vars_template: &'static str,
    pub timeout: Duration,
    pub machine: &'static str,
    pub cpu: &'static str,
    pub debugcon: bool,
}

pub const AARCH64: Guest = Guest {
    name: "aarch64",
    triple: "aarch64-unknown-uefi",
    qemu: "qemu-system-aarch64",
    tag: "aa64",
    bls_arch: "aa64",
    code: "/opt/homebrew/share/qemu/edk2-aarch64-code.fd",
    vars_template: "/opt/homebrew/share/qemu/edk2-arm-vars.fd",
    timeout: Duration::from_secs(90),
    machine: "virt,accel=hvf,gic-version=3",
    cpu: "host",
    debugcon: false,
};

pub const X86_64: Guest = Guest {
    name: "x86_64",
    triple: "x86_64-unknown-uefi",
    qemu: "qemu-system-x86_64",
    tag: "x64",
    bls_arch: "x64",
    code: "/opt/homebrew/share/qemu/edk2-x86_64-code.fd",
    vars_template: "/opt/homebrew/share/qemu/edk2-i386-vars.fd",
    timeout: Duration::from_secs(180),
    machine: "q35,accel=tcg",
    cpu: "qemu64",
    debugcon: true,
};

pub struct Measured {
    pub summary: String,
}

pub fn run(workspace: &Path, guest: &Guest, esp: &Path) -> Result<Measured, String> {
    let attempts = [
        Attempt {
            label: "primary",
            disk: disk_args(guest, true),
            graphics: graphics_args(guest),
            display: "none",
        },
        Attempt {
            label: "alternate-disk",
            disk: disk_args(guest, false),
            graphics: graphics_args(guest),
            display: "cocoa",
        },
    ];
    let mut errors = Vec::new();
    for attempt in attempts {
        match run_attempt(workspace, guest, esp, &attempt) {
            Ok(measured) => return Ok(measured),
            Err(err) => errors.push(format!("{}: {err}", attempt.label)),
        }
    }
    Err(errors.join("\n"))
}

struct Attempt {
    label: &'static str,
    disk: Vec<String>,
    graphics: Vec<String>,
    display: &'static str,
}

struct Running {
    child: Option<Child>,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn run_attempt(
    workspace: &Path,
    guest: &Guest,
    esp: &Path,
    attempt: &Attempt,
) -> Result<Measured, String> {
    let build = workspace.join("build");
    fs::create_dir_all(&build).map_err(|err| err.to_string())?;
    let serial_path = build.join(format!("serial-{}.txt", guest.name));
    let debug_path = build.join(format!("debugcon-{}.txt", guest.name));
    let err_path = build.join(format!("qemu-{}.err", guest.name));
    let sock_path = build.join(format!("monitor-{}.sock", guest.name));
    let vars_path = build.join(format!("vars-{}.fd", guest.name));
    let _ = fs::remove_file(&serial_path);
    let _ = fs::remove_file(&debug_path);
    let _ = fs::remove_file(&sock_path);
    fs::copy(guest.vars_template, &vars_path).map_err(|err| format!("copy vars: {err}"))?;
    let esp = fs::canonicalize(esp).map_err(|err| format!("esp path: {err}"))?;
    let image = build.join(format!("esp-{}.img", guest.name));
    crate::esp::create_fat_image(&esp, &image)?;
    let mut command = Command::new(qemu_bin(guest.qemu));
    command
        .arg("-machine")
        .arg(guest.machine)
        .arg("-cpu")
        .arg(guest.cpu)
        .arg("-m")
        .arg("1024")
        .arg("-smp")
        .arg("1")
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=0,readonly=on,file={}",
            guest.code
        ))
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,unit=1,file={}",
            display(&vars_path)
        ))
        .arg("-drive")
        .arg(format!(
            "file={},format=raw,if=none,id=esp",
            display(&image)
        ))
        .args(&attempt.disk)
        .args(&attempt.graphics)
        .arg("-serial")
        .arg(format!("file:{}", display(&serial_path)))
        .arg("-display")
        .arg(attempt.display)
        .arg("-monitor")
        .arg(format!("unix:{},server,nowait", display(&sock_path)))
        .arg("-nic")
        .arg("none")
        .arg("-no-reboot");
    if guest.debugcon {
        command
            .arg("-debugcon")
            .arg(format!("file:{}", display(&debug_path)))
            .arg("-global")
            .arg("isa-debugcon.iobase=0x402");
    }
    if guest.name == "aarch64" {
        command.args([
            "-device",
            "qemu-xhci,id=xhci",
            "-device",
            "usb-kbd,bus=xhci.0",
        ]);
    }
    let err_file = File::create(&err_path).map_err(|err| err.to_string())?;
    command.stdout(Stdio::null()).stderr(Stdio::from(err_file));
    let child = command
        .spawn()
        .map_err(|err| format!("spawn {}: {err}", guest.qemu))?;
    let mut running = Running { child: Some(child) };
    let ready = wait_log(
        &serial_path,
        &debug_path,
        "SBM_MARK menu_ready",
        guest.timeout,
    );
    let log = ready?;
    if !log.contains("SBM_MARK gop ") || log.contains("SBM_MARK gop missing") {
        return Err(format!("firmware did not open GOP\n{log}"));
    }
    let monitor = connect_monitor(&sock_path)?;
    let dark_ppm = build.join(format!("{}-dark.ppm", guest.name));
    screendump(&monitor, &dark_ppm)?;
    mon_cmd(&monitor, "sendkey t")?;
    wait_count(
        &serial_path,
        &debug_path,
        "key t",
        1,
        Duration::from_secs(20),
    )?;
    thread::sleep(Duration::from_millis(400));
    let light_ppm = build.join(format!("{}-light.ppm", guest.name));
    screendump(&monitor, &light_ppm)?;
    mon_cmd(&monitor, "sendkey ret")?;
    wait_count(
        &serial_path,
        &debug_path,
        "chainload_returned",
        1,
        Duration::from_secs(30),
    )?;
    mon_cmd(&monitor, "sendkey c")?;
    wait_count(
        &serial_path,
        &debug_path,
        "chainload_returned",
        2,
        Duration::from_secs(30),
    )?;
    let _ = mon_cmd(&monitor, "quit");
    if let Some(mut child) = running.child.take() {
        let _ = child.wait();
    }
    let serial = fs::read_to_string(&serial_path).unwrap_or_default();
    let debugcon = fs::read_to_string(&debug_path).unwrap_or_default();
    let log = log_text(&serial, &debugcon);
    require_targets(log)?;
    if count_exact(log, "fixture_started") < 2 {
        return Err(format!("expected two fixture starts\n{log}"));
    }
    if count_prefix(log, "chainload_returned") < 2 {
        return Err(format!("expected two chainload returns\n{log}"));
    }
    let handoff = crate::esp::read_fat_file(&image, "EFI/sbm/handoff/current.json");
    let attempt_file = crate::esp::read_fat_file(&image, "EFI/sbm/state/last_attempt.json");
    let marker = crate::esp::read_fat_file(&image, "EFI/sbm/test/chainload-ok.txt");
    let handoff_ok = handoff.as_ref().is_ok_and(|bytes| bytes.starts_with(b"{"));
    let attempt_ok = attempt_file
        .as_ref()
        .is_ok_and(|bytes| bytes.starts_with(b"{"));
    if !handoff_ok || !attempt_ok {
        return Err(format!(
            "persistence files missing (handoff {handoff_ok}, attempt {attempt_ok})\n{log}"
        ));
    }
    convert_png(
        workspace,
        &dark_ppm,
        &format!("screenshots/{}-menu-dark.png", guest.name),
    )?;
    convert_png(
        workspace,
        &light_ppm,
        &format!("screenshots/{}-menu-light.png", guest.name),
    )?;
    let summary = format!(
        "arch={}\nattempt={}\nefi_bytes={}\nfirst_frame_us={}\ndiscovery_us={}\nmenu_ready_us={}\nbackbuffer={}\nfixture_started={}\nchainload_returned={}\nhandoff={}\nattempt_file={}\nchainload_ok={}\n",
        guest.name,
        attempt.label,
        fs::metadata(
            workspace
                .join("artifacts")
                .join(format!("sbm-{}.efi", guest.name))
        )
        .map(|meta| meta.len())
        .unwrap_or(0),
        metric(log, "first_frame").ok_or_else(|| format!("missing first_frame\n{log}"))?,
        metric(log, "discovery_done").ok_or_else(|| format!("missing discovery_done\n{log}"))?,
        metric(log, "menu_ready").ok_or_else(|| format!("missing menu_ready\n{log}"))?,
        backbuffer(log),
        count_exact(log, "fixture_started"),
        count_prefix(log, "chainload_returned"),
        handoff_ok,
        attempt_ok,
        marker.is_ok(),
    );
    fs::write(build.join(format!("measure-{}.txt", guest.name)), &summary)
        .map_err(|err| err.to_string())?;
    fs::write(
        build.join(format!("acceptance-{}.json", guest.name)),
        summary_json(&summary),
    )
    .map_err(|err| err.to_string())?;
    Ok(Measured { summary })
}

fn summary_json(summary: &str) -> String {
    let mut body = String::from("{\n");
    let mut first = true;
    for line in summary.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !first {
            body.push_str(",\n");
        }
        first = false;
        body.push_str("  \"");
        body.push_str(key);
        body.push_str("\": ");
        if value == "true" || value == "false" || value.chars().all(|ch| ch.is_ascii_digit()) {
            body.push_str(value);
        } else {
            body.push('"');
            body.push_str(value);
            body.push('"');
        }
    }
    body.push_str("\n}\n");
    body
}

fn disk_args(guest: &Guest, primary: bool) -> Vec<String> {
    if primary {
        vec![
            String::from("-device"),
            String::from("virtio-blk-pci,drive=esp,bootindex=0"),
        ]
    } else if guest.name == "aarch64" {
        vec![
            String::from("-device"),
            String::from("virtio-blk-device,drive=esp,bootindex=0"),
        ]
    } else {
        vec![
            String::from("-device"),
            String::from("ide-hd,drive=esp,bootindex=0"),
        ]
    }
}

fn graphics_args(guest: &Guest) -> Vec<String> {
    if guest.name == "aarch64" {
        vec![String::from("-device"), String::from("ramfb")]
    } else {
        vec![String::from("-vga"), String::from("std")]
    }
}

fn qemu_bin(name: &str) -> PathBuf {
    let brew = PathBuf::from("/opt/homebrew/bin").join(name);
    if brew.exists() {
        brew
    } else {
        PathBuf::from(name)
    }
}

fn wait_log(
    serial: &Path,
    debugcon: &Path,
    needle: &str,
    timeout: Duration,
) -> Result<String, String> {
    let start = Instant::now();
    loop {
        let serial_text = fs::read_to_string(serial).unwrap_or_default();
        let debug_text = fs::read_to_string(debugcon).unwrap_or_default();
        let log = log_text(&serial_text, &debug_text).to_string();
        if log.contains(needle) {
            return Ok(log);
        }
        if start.elapsed() > timeout {
            return Err(format!(
                "timed out waiting for {needle}\nserial:\n{serial_text}\ndebugcon:\n{debug_text}"
            ));
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn wait_count(
    serial: &Path,
    debugcon: &Path,
    prefix: &str,
    want: usize,
    timeout: Duration,
) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let log = combined(serial, debugcon);
        let found = if prefix == "key t" {
            count_exact(&log, "key t")
        } else {
            count_prefix(&log, prefix)
        };
        if found >= want {
            return Ok(());
        }
        if start.elapsed() > timeout {
            return Err(format!("timed out waiting for {prefix} x{want}\n{log}"));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn combined(serial: &Path, debugcon: &Path) -> String {
    let serial_text = fs::read_to_string(serial).unwrap_or_default();
    let debug_text = fs::read_to_string(debugcon).unwrap_or_default();
    log_text(&serial_text, &debug_text).to_string()
}

fn connect_monitor(path: &Path) -> Result<UnixStream, String> {
    let start = Instant::now();
    let mut last = String::from("monitor socket was not created");
    while start.elapsed() < Duration::from_secs(10) {
        match UnixStream::connect(path) {
            Ok(stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let _ = read_prompt(&stream)?;
                return Ok(stream);
            }
            Err(err) => last = err.to_string(),
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(last)
}

fn screendump(stream: &UnixStream, path: &Path) -> Result<(), String> {
    let response = mon_cmd(stream, &format!("screendump {}", display(path)))?;
    if response.to_ascii_lowercase().contains("error")
        || response.to_ascii_lowercase().contains("could not")
    {
        return Err(format!("screendump failed: {response}"));
    }
    let len = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if len < 64 * 1024 {
        return Err(format!("screenshot is too small ({len} bytes)"));
    }
    Ok(())
}

fn mon_cmd(mut stream: &UnixStream, command: &str) -> Result<String, String> {
    let mut line = command.to_string();
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|err| format!("monitor write: {err}"))?;
    read_prompt(stream)
}

fn read_prompt(mut stream: &UnixStream) -> Result<String, String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(7).any(|window| window == b"(qemu) ") {
                    return Ok(String::from_utf8_lossy(&buf).into_owned());
                }
            }
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut => {}
            Err(err) => return Err(format!("monitor read: {err}")),
        }
    }
    Err(format!(
        "monitor prompt missing: {}",
        String::from_utf8_lossy(&buf)
    ))
}

fn convert_png(workspace: &Path, ppm: &Path, relative: &str) -> Result<(), String> {
    let dest = workspace.join(relative);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let status = Command::new("sips")
        .args([
            "-s",
            "format",
            "png",
            &display(ppm),
            "--out",
            &display(&dest),
        ])
        .status()
        .map_err(|err| format!("sips: {err}"))?;
    if !status.success() {
        return Err(format!("sips exited {status}"));
    }
    Ok(())
}

fn backbuffer(log: &str) -> String {
    for line in log.lines() {
        if let Some(value) = line.trim().strip_prefix("SBM_MARK backbuffer bytes=") {
            return value.trim().to_string();
        }
    }
    String::from("unknown")
}

fn display(path: &Path) -> String {
    path.display().to_string()
}
