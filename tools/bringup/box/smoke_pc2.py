#!/usr/bin/env python3
"""PC2 smoke test over neomaster's debug connection (doc/debug-connection.md).

Starts `neomaster --headless` on the real PC2 with the debug socket, TCP stream and a recording,
then checks, from the debug records alone:

  1. hello: headless, PC2 over USB
  2. the PC2 opens (pc2_session open) and answers the start-up (usb_in, pc2_in)
  3. the debug commands answer (ping, state)
  4. what the link and the PC2 say while we listen (ml_in / ml_out / ir / pc2_in counts)
  5. core loop timings (handle_us)
  6. optional, --interactive: a Beo4 key at the BM5's IR eye shows up as an `ir` record
  7. optional, --panel-recording FILE (from nm-panel-probe): the real panel's reports, replayed
     into neomaster as `panel_raw` injections, decode to the same inputs neomaster's own decoder
     produced in the probe

Nothing is put on the link: no telegram or PC2 injection, no standby. The role's own start-up
announcements are what neomaster always sends.

Standard library only (the BM5 has python3). Run as root (USB). Writes OUT/pc2-summary.json,
OUT/pc2-record.jsonl (neomaster's own recording) and OUT/neomaster-stderr.log.
"""

import argparse
import json
import os
import signal
import socket
import subprocess
import sys
import threading
import time


class Debug:
    """A debug-socket connection that collects every record in the background."""

    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self.file = self.sock.makefile("rwb", buffering=0)
        self.records = []
        self.lock = threading.Lock()
        self.cv = threading.Condition(self.lock)
        self.thread = threading.Thread(target=self._read, daemon=True)
        self.thread.start()

    def _read(self):
        for line in self.file:
            try:
                rec = json.loads(line)
            except ValueError:
                continue
            rec["_rx"] = time.monotonic()
            with self.cv:
                self.records.append(rec)
                self.cv.notify_all()

    def send(self, cmd):
        self.file.write((json.dumps(cmd) + "\n").encode())

    def wait(self, pred, timeout, since=0):
        """First record at index >= since matching pred, or None after timeout."""
        deadline = time.monotonic() + timeout
        with self.cv:
            i = since
            while True:
                while i < len(self.records):
                    if pred(self.records[i]):
                        return self.records[i]
                    i += 1
                left = deadline - time.monotonic()
                if left <= 0:
                    return None
                self.cv.wait(left)

    def count(self):
        with self.lock:
            return len(self.records)

    def kinds(self, since=0):
        with self.lock:
            out = {}
            for r in self.records[since:]:
                out[r.get("kind")] = out.get(r.get("kind"), 0) + 1
            return out

    def of(self, kind, since=0):
        with self.lock:
            return [r for r in self.records[since:] if r.get("kind") == kind]


def pct(values, p):
    if not values:
        return None
    v = sorted(values)
    return v[min(len(v) - 1, round(p / 100 * (len(v) - 1)))]


class Checks:
    def __init__(self):
        self.items = []

    def add(self, name, status, detail=""):
        self.items.append({"check": name, "status": status, "detail": detail})
        mark = {"pass": "PASS", "fail": "FAIL", "warn": "WARN", "skip": "SKIP"}[status]
        print(f"[{mark}] {name}" + (f": {detail}" if detail else ""), flush=True)


def load_panel_recording(path):
    reports, inputs = [], []
    with open(path) as f:
        for line in f:
            try:
                r = json.loads(line)
            except ValueError:
                continue
            if r.get("kind") == "panel_in" and r.get("raw"):
                reports.append((r["t"], r["raw"]))
            elif r.get("kind") == "panel_input":
                inputs.append((r["t"], r["input"]))
    return reports, inputs


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--neomaster", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--role", default="link-source")
    ap.add_argument("--tcp", default="7155", help="also stream read-only on this TCP port ('' for none)")
    ap.add_argument("--listen", type=float, default=30.0, help="seconds to watch the link")
    ap.add_argument("--interactive", action="store_true")
    ap.add_argument("--panel-recording")
    ap.add_argument("--replay-max", type=float, default=120.0, help="seconds of the panel recording to replay")
    a = ap.parse_args()

    os.makedirs(a.out, exist_ok=True)
    # Unix socket paths are limited to about 100 bytes; keep it short and out of the results.
    sock = f"/tmp/nm-smoke-{os.getpid()}.sock"
    if os.path.exists(sock):
        os.unlink(sock)
    cmd = [a.neomaster, "--headless", "--role", a.role, "--debug-socket", sock,
           "--debug-record", os.path.join(a.out, "pc2-record.jsonl")]
    if a.tcp:
        cmd += ["--debug-tcp", a.tcp]
    env = dict(os.environ, RUST_LOG=os.environ.get("RUST_LOG", "info"))
    stderr = open(os.path.join(a.out, "neomaster-stderr.log"), "wb")
    print("$ " + " ".join(cmd), flush=True)
    proc = subprocess.Popen(cmd, stdout=stderr, stderr=subprocess.STDOUT, env=env)
    checks = Checks()
    summary = {"command": cmd, "role": a.role}
    t_start = time.monotonic()

    try:
        for _ in range(100):
            if os.path.exists(sock) or proc.poll() is not None:
                break
            time.sleep(0.1)
        if proc.poll() is not None:
            checks.add("neomaster starts", "fail", f"exited with {proc.returncode}; see neomaster-stderr.log")
            return finish(checks, summary, a.out)
        d = Debug(sock)
        hello = d.wait(lambda r: r.get("kind") == "hello", 5)
        if not hello:
            checks.add("hello", "fail", "no hello on the debug socket")
            return finish(checks, summary, a.out)
        summary["hello"] = {k: v for k, v in hello.items() if not k.startswith("_")}
        ok = hello.get("mode") == "headless" and hello.get("pc2_device") == "Usb"
        checks.add("hello", "pass" if ok else "fail", f"mode={hello.get('mode')} role={hello.get('role')} pc2={hello.get('pc2_device')} {hello.get('protocol')} {hello.get('version')}")
        d.send({"cmd": "filter", "kinds": ["all"]})

        # 2. the PC2 opens and answers
        opened = d.wait(lambda r: r.get("kind") == "pc2_session" and r.get("state") == "open", 15)
        if opened:
            secs = opened["_rx"] - t_start
            summary["pc2_open_s"] = round(secs, 2)
            checks.add("PC2 opens", "pass", f"mode {opened.get('mode')} after {secs:.1f} s")
        else:
            logs = [r.get("msg", "") for r in d.of("log") if r.get("level") in ("WARN", "ERROR")]
            if not logs:
                stderr.flush()
                with open(os.path.join(a.out, "neomaster-stderr.log"), errors="replace") as f:
                    logs = [l.strip() for l in f if " WARN " in l or " ERROR " in l]
            checks.add("PC2 opens", "fail", "; ".join(logs[-3:]) or "no pc2_session open in 15 s")
        usb = d.wait(lambda r: r.get("kind") == "usb_in", 10)
        checks.add("PC2 sends USB packets", "pass" if usb else "fail", usb.get("raw", "") if usb else "no usb_in in 10 s")
        ev = d.wait(lambda r: r.get("kind") == "pc2_in", 10)
        checks.add("PC2 reports decode", "pass" if ev else "warn", str(ev.get("event"))[:100] if ev else "no pc2_in in 10 s")

        # 3. commands
        n = d.count()
        d.send({"cmd": "ping"})
        pong = d.wait(lambda r: r.get("kind") in ("ok", "error"), 3, n)
        checks.add("ping", "pass" if pong and pong["kind"] == "ok" else "fail", "" if pong else "no answer")
        n = d.count()
        d.send({"cmd": "state"})
        st = d.wait(lambda r: r.get("kind") == "state", 3, n)
        if st:
            summary["state"] = st.get("state")
            checks.add("state", "pass", json.dumps(st.get("state"))[:160])
        else:
            checks.add("state", "fail", "no state record")
        d.send({"cmd": "mark", "text": "smoke: listening"})

        # 4. listen
        n0 = d.count()
        print(f"Listening to the link for {a.listen:.0f} s ...", flush=True)
        time.sleep(a.listen)
        k = d.kinds(n0)
        summary["listen"] = {"seconds": a.listen, "kinds": k}
        ml = k.get("ml_in", 0) + k.get("ml_out", 0)
        checks.add("Masterlink traffic", "pass" if ml else "warn",
                   f"{k.get('ml_in', 0)} in, {k.get('ml_out', 0)} out, {k.get('pc2_refused', 0)} refused, {k.get('ir', 0)} IR in {a.listen:.0f} s"
                   + ("" if ml else " (a quiet link is not an error; switch a room on to see some)"))
        bad = [r for r in d.of("ml_in") if r.get("checksum_ok") is False]
        if bad:
            checks.add("telegram checksums", "warn", f"{len(bad)} bad checksums")

        # 5. timings
        handle = [r.get("handle_us", 0) for r in d.of("timing")]
        summary["core_handle_us"] = {"n": len(handle), "p50": pct(handle, 50), "p99": pct(handle, 99), "max": max(handle) if handle else None}
        if handle:
            checks.add("core loop", "pass" if pct(handle, 99) < 5000 else "warn",
                       f"{len(handle)} turns, handle p50 {pct(handle, 50)} us, p99 {pct(handle, 99)} us, max {max(handle)} us")
        else:
            checks.add("core loop", "warn", "no timing records")

        # 6. IR
        if a.interactive:
            n = d.count()
            print("\n>>> Press a key on a Beo4 remote, pointed at the BM5's IR eye (20 s)", flush=True)
            ir = d.wait(lambda r: r.get("kind") == "ir", 20, n)
            checks.add("Beo4 IR via the PC2", "pass" if ir else "fail",
                       f"key {ir.get('key')} code {ir.get('code')} type {ir.get('key_type')}" if ir else "no ir record in 20 s")
        else:
            checks.add("Beo4 IR via the PC2", "skip", "needs --interactive")

        # 7. panel replay
        if a.panel_recording and os.path.exists(a.panel_recording):
            reports, probe_inputs = load_panel_recording(a.panel_recording)
            reports = [(t, raw) for t, raw in reports if t - reports[0][0] <= a.replay_max] if reports else []
            if not reports:
                checks.add("panel replay", "skip", "the recording has no panel reports")
            else:
                n = d.count()
                d.send({"cmd": "mark", "text": "smoke: panel replay"})
                t0, w0 = reports[0][0], time.monotonic()
                for t, raw in reports:
                    delay = (t - t0) - (time.monotonic() - w0)
                    if delay > 0:
                        time.sleep(delay)
                    d.send({"cmd": "panel_raw", "hex": raw})
                time.sleep(1.5)
                got = [r["input"] for r in d.of("panel_input", n)]
                errors = [r for r in d.of("error", n)]
                fx = d.kinds(n).get("ui_effect", 0)
                # Long presses depend on the hold time, so leave them out; the rest must match
                # what nm-panel-probe decoded from the same reports, in order.
                t_end = reports[-1][0]
                want = [i for t, i in probe_inputs if t <= t_end + 0.05 and not i.startswith("LongPress")]
                have = [i for i in got if not i.startswith("LongPress")]
                same = have == want
                if not same:
                    first = next((j for j, (x, y) in enumerate(zip(have, want)) if x != y), min(len(have), len(want)))
                    summary["panel_replay_first_difference"] = {"index": first, "neomaster": have[first:first + 3], "probe": want[first:first + 3]}
                summary["panel_replay"] = {"reports": len(reports), "probe_inputs": len(want), "neomaster_inputs": len(got), "ui_effects": fx, "errors": len(errors)}
                checks.add("panel replay through neomaster", "pass" if same and not errors else "warn",
                           f"{len(reports)} reports → {len(got)} inputs (probe decoded {len(want)} in that window, LongPress aside), {fx} UI effects, {len(errors)} errors")
        else:
            checks.add("panel replay through neomaster", "skip", "no panel recording")

        summary["kinds_total"] = d.kinds()
    finally:
        if proc.poll() is None:
            proc.send_signal(signal.SIGINT)
            try:
                proc.wait(10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
        summary["exit_code"] = proc.returncode
        stderr.close()
        if os.path.exists(sock):
            os.unlink(sock)
    return finish(checks, summary, a.out)


def finish(checks, summary, out):
    summary["checks"] = checks.items
    summary["ok"] = all(c["status"] in ("pass", "warn", "skip") for c in checks.items)
    with open(os.path.join(out, "pc2-summary.json"), "w") as f:
        json.dump(summary, f, indent=2, default=str)
    return 0 if summary["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
