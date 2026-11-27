#!/usr/bin/env python3
"""Measure the native SBM simulator: builds, startup, memory and idle CPU.

Usage: python3 sim/measure.py [--cold N] [--warm N] [--runs N] [--skip-cold]

Cold builds use fresh CARGO_TARGET_DIRs under $TMPDIR so the normal target/
caches stay intact (crates are already downloaded; network time is excluded).
Opens real windows briefly. Prints a Markdown report on stdout.
"""

import argparse
import functools
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

print = functools.partial(print, flush=True)  # noqa: A001

UEFI = Path(__file__).resolve().parents[1]
CORE = Path(os.environ.get("SBM_CORE_DIR", UEFI.parent / "sbm-core"))
PREVIEW = UEFI / "target/debug/sbm-preview"
CORE_BIN = CORE / "target/debug"
UEFI_BUILD = ["cargo", "build", "-q", "-p", "sbm-preview", "--features", "window"]
CORE_BUILD = ["cargo", "build", "-q", "-p", "sbm-sim", "-p", "sbm-ui", "--features", "sbm-ui/native-sim"]


def timed(cmd, cwd, env=None):
    started = time.perf_counter()
    subprocess.run(cmd, cwd=cwd, env=env, check=True)
    return time.perf_counter() - started


def stats(values, unit, scale=1.0, digits=2):
    values = [v * scale for v in values]
    med = statistics.median(values)
    return (
        f"median {med:.{digits}f} {unit}, min {min(values):.{digits}f}, "
        f"max {max(values):.{digits}f} (N={len(values)})"
    )


def cold(n, tmp):
    out = {}
    for name, cmd, cwd in [("sbm-preview (window)", UEFI_BUILD, UEFI), ("sbm-sim + sbm-ui", CORE_BUILD, CORE)]:
        runs = []
        for i in range(n):
            target = Path(tmp) / f"cold-{name.split()[0]}-{i}"
            env = dict(os.environ, CARGO_TARGET_DIR=str(target))
            runs.append(timed(cmd, cwd, env))
            shutil.rmtree(target, ignore_errors=True)
            print(f"  cold {name} run {i + 1}: {runs[-1]:.1f} s", file=sys.stderr)
        out[name] = runs
    return out


def warm(n):
    edits = [
        ("sbm-preview session.rs", UEFI / "crates/sbm-preview/src/session.rs", UEFI_BUILD, UEFI),
        ("sbm-gfx paint.rs (renderer)", UEFI / "crates/sbm-gfx/src/paint.rs", UEFI_BUILD, UEFI),
        ("sbm-ui main.rs", CORE / "crates/sbm-ui/src/main.rs", CORE_BUILD, CORE),
        ("sbm-runtime ipc.rs", CORE / "crates/sbm-runtime/src/ipc.rs", CORE_BUILD, CORE),
    ]
    out = {}
    for name, path, cmd, cwd in edits:
        timed(cmd, cwd)
        runs = []
        for _ in range(n):
            path.touch()
            runs.append(timed(cmd, cwd))
        out[name] = runs
        print(f"  warm {name}: {statistics.median(runs):.2f} s", file=sys.stderr)
    return out


def cpu_seconds(pid):
    text = subprocess.run(["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    if not text:
        return None
    parts = [float(p) for p in text.replace("-", ":").split(":")]
    total = 0.0
    for p in parts:
        total = total * 60 + p
    return total


def rss_mib(pid):
    text = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return int(text) / 1024 if text else None


def first_frames(n):
    internal, external = [], []
    for _ in range(n):
        started = time.perf_counter()
        proc = subprocess.Popen(
            [str(PREVIEW), "--scenario", "windows-only", "--exit-after-ms", "1500", "--core-bin", "/nonexistent"],
            stdout=subprocess.PIPE, text=True,
        )
        line = proc.stdout.readline()
        external.append(time.perf_counter() - started)
        proc.wait()
        match = re.search(r"first frame ([0-9.]+) ms", line)
        if match:
            internal.append(float(match.group(1)) / 1000)
    return internal, external


def idle_window(scenario, settle, window, runs):
    rss, cpu = [], []
    for _ in range(runs):
        proc = subprocess.Popen(
            [str(PREVIEW), "--scenario", scenario, "--exit-after-ms", str(int((settle + window + 1) * 1000)),
             "--core-bin", "/nonexistent"],
            stdout=subprocess.DEVNULL,
        )
        time.sleep(settle)
        a = cpu_seconds(proc.pid)
        time.sleep(window)
        b = cpu_seconds(proc.pid)
        rss.append(rss_mib(proc.pid))
        proc.wait()
        cpu.append((b - a) / window * 100)
    return rss, cpu


def core_runs(runs, settle, window):
    session = Path(tempfile.mkdtemp(prefix="sbm-measure-core-"))
    subprocess.run([str(PREVIEW), "--scenario", "multi-disk", "--materialize", str(session)],
                   check=True, stdout=subprocess.DEVNULL)
    success = session / "esp/EFI/sbm/state/boot-success.json"
    ready, rss, cpu = [], [], []
    for _ in range(runs):
        success.unlink(missing_ok=True)
        started = time.perf_counter()
        sim = subprocess.Popen([str(CORE_BIN / "sbm-sim"), "--session", str(session)],
                               stderr=subprocess.DEVNULL)
        while not success.exists() and time.perf_counter() - started < 30:
            time.sleep(0.005)
        ready.append(time.perf_counter() - started)
        ui = None
        for _ in range(100):
            out = subprocess.run(["pgrep", "-P", str(sim.pid), "sbm-ui"], capture_output=True, text=True).stdout.split()
            if out:
                ui = int(out[0])
                break
            time.sleep(0.02)
        time.sleep(settle)
        a, sa = cpu_seconds(ui), cpu_seconds(sim.pid)
        time.sleep(window)
        b, sb = cpu_seconds(ui), cpu_seconds(sim.pid)
        rss.append((rss_mib(ui), rss_mib(sim.pid)))
        cpu.append(((b - a) / window * 100, (sb - sa) / window * 100))
        subprocess.run(["kill", str(ui)])
        sim.wait()
    shutil.rmtree(session, ignore_errors=True)
    return ready, rss, cpu


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cold", type=int, default=3)
    parser.add_argument("--warm", type=int, default=10)
    parser.add_argument("--runs", type=int, default=10)
    parser.add_argument("--skip-cold", action="store_true")
    parser.add_argument("--skip-warm", action="store_true")
    args = parser.parse_args()

    print("## Native simulator measurements\n")
    host = subprocess.run(["uname", "-mrs"], capture_output=True, text=True).stdout.strip()
    rustc = subprocess.run(["rustc", "--version"], capture_output=True, text=True).stdout.strip()
    print(f"- Host: {host}; {os.cpu_count()} logical CPUs; {rustc}")
    print("- Profile: dev (debug), the profile `cargo xtask sim` uses\n")

    with tempfile.TemporaryDirectory(prefix="sbm-measure-") as tmp:
        if not args.skip_cold:
            print("### Cold builds (fresh target dir, crates already downloaded)\n")
            for name, runs in cold(args.cold, tmp).items():
                print(f"- {name}: {stats(runs, 's', digits=1)}")
            print()

    subprocess.run(UEFI_BUILD, cwd=UEFI, check=True)
    subprocess.run(CORE_BUILD, cwd=CORE, check=True)

    if not args.skip_warm:
        print("### Warm rebuilds (touch one file, rebuild)\n")
        for name, runs in warm(args.warm).items():
            print(f"- {name}: {stats(runs, 's')}")
        print()

    print("### UEFI menu window startup\n")
    internal, external = first_frames(args.runs)
    print(f"- Process start to first presented frame (in-process clock): {stats(internal, 'ms', 1000, 1)}")
    print(f"- Spawn to first frame reported (includes exec and dyld): {stats(external, 'ms', 1000, 1)}\n")

    print("### UEFI menu window memory and idle CPU (8 s sample after 3 s settle)\n")
    for scenario, label in [("windows-only", "menu idle, no countdown"), ("dual-boot", "countdown running")]:
        rss, cpu = idle_window(scenario, 3, 4 if scenario == "dual-boot" else 8, max(3, args.runs // 2))
        print(f"- {label} ({scenario}): RSS {stats(rss, 'MiB', digits=1)}; CPU {stats(cpu, '%', digits=2)}")
    print()

    print("### Native Core (real sbm-ui + sbm-sim, multi-disk scenario)\n")
    ready, rss, cpu = core_runs(max(3, args.runs // 2), 3, 8)
    print(f"- sbm-sim start to sbm-ui UiReady (boot-success.json written): {stats(ready, 'ms', 1000, 0)}")
    print(f"- sbm-ui RSS: {stats([r[0] for r in rss], 'MiB', digits=1)}; idle CPU {stats([c[0] for c in cpu], '%')}")
    print(f"- sbm-sim RSS: {stats([r[1] for r in rss], 'MiB', digits=1)}; idle CPU {stats([c[1] for c in cpu], '%')}")


if __name__ == "__main__":
    main()
