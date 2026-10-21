#!/usr/bin/env python3
"""Compare the egui app and the Tauri prototype on this machine.

Both run against the egui app's --demo daemon (sample chats, one answer
streaming, one change waiting for approval), so they see the same data.
With CWW_PAINT_MARK set, each prints "cww-paint <page> <unix micros>" once
its first frame with the daemon's data is drawn.

  bench.py --egui PATH/cww-app --tauri PATH/cww-desktop-tauri [--runs 7]

Measures, per app:
  - launch to first paint of the start page (Folders), warm and, on Linux,
    cold (every file the app's processes mapped is evicted from the page
    cache first with posix_fadvise, which needs no root but leaves pages
    other running programs map, like libc or the GPU driver),
  - memory of every process the app runs after 10 s idle: RSS and PSS on
    Linux, RSS and the physical footprint on macOS (where WebKit's
    processes are launchd's children, so they're found as the
    com.apple.WebKit processes that appeared with the app),
  - CPU time while idle and while an answer streams,
  - for Tauri, in-page checks: approving a change, scrolling a long
    transcript, and typing (the page's own events; no system input).
Stops only processes it started, by PID.
"""

import argparse
import glob
import json
import os
import re
import signal
import statistics
import subprocess
import sys
import tempfile
import threading
import time

MAC = sys.platform == "darwin"
CLK_TCK = os.sysconf("SC_CLK_TCK")


# Processes -------------------------------------------------------------------


def ps_table():
    """pid -> (ppid, rss KiB, cpu seconds, command) on macOS."""
    out = subprocess.run(["ps", "-axo", "pid=,ppid=,rss=,time=,comm="], capture_output=True, text=True).stdout
    table = {}
    for line in out.splitlines():
        parts = line.split(None, 4)
        if len(parts) < 5:
            continue
        pid, ppid, rss, cputime, comm = parts
        seconds = 0.0
        for part in cputime.replace("-", ":").split(":"):
            seconds = seconds * 60 + float(part)
        table[int(pid)] = (int(ppid), int(rss), seconds, comm)
    return table


def webkit_pids():
    return {pid for pid, row in ps_table().items() if "com.apple.WebKit" in row[3]} if MAC else set()


def descendants(pid):
    parents = {}
    if MAC:
        for child, row in ps_table().items():
            parents.setdefault(row[0], []).append(child)
    else:
        for stat in glob.glob("/proc/[0-9]*/stat"):
            try:
                with open(stat) as f:
                    fields = f.read().rsplit(")", 1)[1].split()
                parents.setdefault(int(fields[1]), []).append(int(stat.split("/")[2]))
            except (OSError, IndexError, ValueError):
                pass
    tree, todo = [], [pid]
    while todo:
        p = todo.pop()
        tree.append(p)
        todo.extend(parents.get(p, []))
    return tree


def name(pid):
    if MAC:
        return os.path.basename(ps_table().get(pid, (0, 0, 0, "?"))[3])
    try:
        with open(f"/proc/{pid}/comm") as f:
            return f.read().strip()
    except OSError:
        return "?"


def cpu_seconds(pids):
    if MAC:
        table = ps_table()
        return sum(table[p][2] for p in pids if p in table)
    total = 0
    for pid in pids:
        try:
            with open(f"/proc/{pid}/stat") as f:
                fields = f.read().rsplit(")", 1)[1].split()
            total += int(fields[11]) + int(fields[12])  # utime + stime
        except (OSError, IndexError):
            pass
    return total / CLK_TCK


def footprint_mib(pid):
    """macOS: the physical footprint Activity Monitor shows as Memory."""
    out = subprocess.run(["footprint", "-p", str(pid)], capture_output=True, text=True).stdout
    m = re.search(r"Footprint:\s+([\d.]+)\s+(KB|MB|GB)", out)
    if not m:
        return 0.0
    return float(m.group(1)) * {"KB": 1 / 1024, "MB": 1, "GB": 1024}[m.group(2)]


def memory(pids):
    """(RSS, PSS or footprint) in MiB, summed over pids, and per process."""
    rss = second = 0
    detail = []
    for pid in pids:
        if MAC:
            row = ps_table().get(pid)
            if not row:
                continue
            r, p = row[1] / 1024, footprint_mib(pid)
        else:
            try:
                with open(f"/proc/{pid}/smaps_rollup") as f:
                    text = f.read()
            except OSError:
                continue
            r = int(re.search(r"^Rss:\s+(\d+)", text, re.M).group(1)) / 1024
            p = int(re.search(r"^Pss:\s+(\d+)", text, re.M).group(1)) / 1024
        rss += r
        second += p
        detail.append((name(pid), round(r, 1), round(p, 1)))
    return rss, second, detail


def mapped_files(pids):
    files = set()
    for pid in pids:
        try:
            with open(f"/proc/{pid}/maps") as f:
                for line in f:
                    parts = line.split(None, 5)
                    if len(parts) == 6 and parts[5].startswith("/") and not parts[5].startswith(("/dev/", "/memfd")):
                        files.add(parts[5].strip().replace(" (deleted)", ""))
        except OSError:
            pass
    return files


def evict(files):
    for path in files:
        try:
            fd = os.open(path, os.O_RDONLY)
        except OSError:
            continue
        try:
            os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
        finally:
            os.close(fd)


# Launching ---------------------------------------------------------------------


class Run:
    """One launch: the process, its WebKit helpers, and what it prints."""

    def __init__(self, cmd, env):
        self.marks = {}
        self.webkit_before = webkit_pids()
        self.started = time.time_ns() // 1000
        self.proc = subprocess.Popen(cmd, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
        self.event = threading.Condition()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.proc.stderr:
            m = re.match(r"cww-paint (\w+) (\d+)", line)
            if m:
                with self.event:
                    self.marks[m.group(1)] = int(m.group(2))
                    self.event.notify_all()
            m = re.match(r"cww-report (\w+) (.*)", line)
            if m:
                with self.event:
                    self.marks["report-" + m.group(1)] = m.group(2).strip()
                    self.event.notify_all()

    def wait_mark(self, what, timeout=30):
        with self.event:
            self.event.wait_for(lambda: what in self.marks, timeout)
        return self.marks.get(what)

    def processes(self):
        return descendants(self.proc.pid) + sorted(webkit_pids() - self.webkit_before)

    def stop(self):
        tree = self.processes()
        self.proc.send_signal(signal.SIGTERM)
        try:
            self.proc.wait(5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
        time.sleep(0.5)
        for pid in tree[1:]:
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                continue
            if not MAC:  # macOS's WebKit services belong to launchd, which ends them
                os.kill(pid, signal.SIGKILL)


class Demo:
    """The egui app's --demo daemon, in the tray, for the Tauri runs."""

    def __init__(self, egui):
        self.proc = subprocess.Popen([egui, "--demo", "--background"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.home = os.path.join(tempfile.gettempdir(), f"cww-demo-{self.proc.pid}-0")
        for _ in range(200):
            if os.path.exists(f"{self.home}/run/cww.sock"):
                return
            time.sleep(0.05)
        raise SystemExit("the demo daemon didn't start")

    def stop(self):
        self.proc.terminate()
        self.proc.wait(5)


OPEN_CHAT = json.dumps({"server": "https://chatwithwork.com", "account": "482139075", "chat": 12})


def launch(app, args, chat=None, autotest=None):
    """Starts one app on Folders, or on a chat; returns (Run, Demo or None)."""
    env = dict(os.environ, CWW_PAINT_MARK="1", CWW_CRASH_REPORTS="0")
    if app == "egui":
        cmd = [args.egui, "--demo"]
        if chat:
            cmd += ["--open-chat", OPEN_CHAT]
        return Run(cmd, env), None
    demo = Demo(args.egui)
    env["CWW_HOME"] = demo.home
    if not MAC:
        # WebKitGTK's DMA-BUF renderer dies with a Wayland protocol error
        # under NVIDIA's explicit sync on Hyprland without this.
        env.setdefault("__NV_DISABLE_EXPLICIT_SYNC", "1")
    if chat:
        env["CWW_OPEN_CHAT"] = chat
    if autotest:
        env["CWW_AUTOTEST"] = autotest
    return Run([args.tauri], env), demo


def finish(run, demo):
    run.stop()
    if demo:
        demo.stop()
    time.sleep(1)


# Measurements ------------------------------------------------------------------


def start_times(app, args, cold, files):
    times = []
    for _ in range(args.runs):
        if cold:
            evict(files[app])
            time.sleep(0.5)
        run, demo = launch(app, args)
        mark = run.wait_mark("folders")
        if mark is None:
            print(f"  {app}: no paint mark", file=sys.stderr)
        else:
            times.append((mark - run.started) / 1000)
        time.sleep(1.5)
        if not MAC:
            files[app] |= mapped_files(run.processes())
        finish(run, demo)
    return times


def idle_and_stream(app, args):
    out = {}
    run, demo = launch(app, args)
    run.wait_mark("folders")
    time.sleep(10)
    out["rss"], out["pss"], out["processes"] = memory(run.processes())
    c0 = cpu_seconds(run.processes())
    time.sleep(10)
    out["idle_cpu_pct"] = (cpu_seconds(run.processes()) - c0) / 10 * 100
    finish(run, demo)
    # Chat 12's answer streams for about 4 s once it's opened.
    run, demo = launch(app, args, chat="12")
    mark = run.wait_mark("chat")
    c0 = cpu_seconds(run.processes())
    t0 = time.monotonic()
    time.sleep(5)
    out["stream_cpu_s"] = cpu_seconds(run.processes()) - c0
    out["stream_cpu_pct"] = out["stream_cpu_s"] / (time.monotonic() - t0) * 100
    time.sleep(5)
    out["chat_rss"], out["chat_pss"], _ = memory(run.processes())
    out["chat_paint_ms"] = (mark - run.started) / 1000 if mark else None
    finish(run, demo)
    return out


def autotests(args):
    out = {}
    for test, chat in [("approve", "5"), ("scroll", "11"), ("type", "11")]:
        run, demo = launch("tauri", args, chat=chat, autotest=test)
        out[test] = run.wait_mark("report-" + test, timeout=20)
        finish(run, demo)
    return out


def summary(times):
    if not times:
        return "n/a"
    return f"median {statistics.median(times):.0f} ms (min {min(times):.0f}, max {max(times):.0f}, n={len(times)})"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--egui", required=True)
    parser.add_argument("--tauri", required=True)
    parser.add_argument("--runs", type=int, default=7)
    args = parser.parse_args()
    files = {"egui": set(), "tauri": set()}
    results = {}
    second = "footprint" if MAC else "PSS"
    for app in ["egui", "tauri"]:
        print(f"{app}: warm starts", file=sys.stderr)
        warm = start_times(app, args, False, files)
        cold = []
        if not MAC:
            print(f"{app}: cold starts", file=sys.stderr)
            cold = start_times(app, args, True, files)
        print(f"{app}: memory and CPU", file=sys.stderr)
        results[app] = {"warm_ms": warm, "cold_ms": cold, **idle_and_stream(app, args)}
    print("tauri: in-page checks", file=sys.stderr)
    results["tauri"]["autotests"] = autotests(args)
    for app, r in results.items():
        print(f"\n== {app}")
        print(f"start to first paint, warm: {summary(r['warm_ms'])}")
        if r["cold_ms"]:
            print(f"start to first paint, cold: {summary(r['cold_ms'])}")
        if r["chat_paint_ms"]:
            print(f"start on a chat to first paint (warm): {r['chat_paint_ms']:.0f} ms")
        print(f"idle on Folders: RSS {r['rss']:.0f} MiB, {second} {r['pss']:.0f} MiB, CPU {r['idle_cpu_pct']:.2f}%")
        for proc in r["processes"]:
            print(f"    {proc[0]:<28} RSS {proc[1]:>7} MiB  {second} {proc[2]:>7} MiB")
        print(f"after streaming a chat: RSS {r['chat_rss']:.0f} MiB, {second} {r['chat_pss']:.0f} MiB")
        print(f"streaming answer (5 s): {r['stream_cpu_s']:.2f} CPU s, {r['stream_cpu_pct']:.1f}% of one core")
        for test, line in r.get("autotests", {}).items():
            print(f"{test}: {line}")
    print("\n" + json.dumps(results, indent=1, default=str))


if __name__ == "__main__":
    main()
