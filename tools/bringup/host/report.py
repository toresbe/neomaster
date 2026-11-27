#!/usr/bin/env python3
"""Turn a bring-up results directory into summary.md (to stdout).

    python3 tools/bringup/host/report.py RESULTS_DIR > RESULTS_DIR/summary.md
"""

import json
import os
import re
import sys

FRAME_US = 16_667


def jsonl(path):
    out = []
    if os.path.exists(path):
        with open(path) as f:
            for line in f:
                try:
                    out.append(json.loads(line))
                except ValueError:
                    pass
    return out


def load(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def f1(x, unit=""):
    if x is None:
        return "–"
    if isinstance(x, (int, float)):
        return f"{x:,.1f}{unit}" if abs(x) < 100 else f"{x:,.0f}{unit}"
    return str(x)


def table(headers, rows):
    out = ["| " + " | ".join(headers) + " |", "|" + "|".join("---" for _ in headers) + "|"]
    out += ["| " + " | ".join(str(c) for c in r) + " |" for r in rows]
    return "\n".join(out)


def main(d):
    md = [f"# BM5 bring-up results: {os.path.basename(os.path.abspath(d))}", ""]
    version = open(os.path.join(d, "VERSION")).read().strip() if os.path.exists(os.path.join(d, "VERSION")) else "?"
    md.append(f"Kit `{version}`.")
    md.append("")

    # ---------------------------------------------------------------- preflight
    pre = {}
    if os.path.exists(os.path.join(d, "preflight.txt")):
        for line in open(os.path.join(d, "preflight.txt")):
            if ": " in line:
                k, v = line.rstrip("\n").split(": ", 1)
                pre[k] = v
        keys = ["debian", "kernel", "glibc", "cpu", "mem_mb", "xserver", "openchrome", "videoram", "exa_offscreen", "dev_fb", "pc2", "panel", "alsa_cards", "package_manager_running"]
        md += ["## The box", "", table(["", ""], [[k, pre.get(k, "–")] for k in keys if k in pre]), ""]

    # ---------------------------------------------------------------- xbench
    xb = jsonl(os.path.join(d, "xbench.jsonl"))
    if xb:
        md += ["## Rendering", ""]
        errors = [r for r in xb if r.get("test") == "error"]
        for e in errors:
            md.append(f"- **{e.get('label')} / {e.get('backend')} failed:** {e.get('error')}")
        runs = []
        for r in xb:
            key = (r.get("label"), r.get("backend"))
            if key not in runs and r.get("test") not in ("error",):
                runs.append(key)

        def get(label, backend, test, **params):
            for r in xb:
                if r.get("label") == label and r.get("backend") == backend and r.get("test") == test and all(r.get("params", {}).get(k) == v for k, v in params.items()):
                    return r
            return None

        # The decision table: can each path hold 60 fps, and with how much CPU left over?
        rows = []
        for label, backend in runs:
            for test in ("strip60", "full60"):
                r = get(label, backend, test)
                if not r:
                    continue
                rows.append([f"{label} / {backend}", test, r["params"].get("format"), f1(r.get("p50_us"), " us"), f1(r.get("p99_us"), " us"),
                             f"{r.get('missed')}/{r.get('frames')}", f1(r.get("fps")), f1(r.get("system_busy_pct"), " %")])
        if rows:
            md += ["### 60 fps loops", "",
                   "Frame time is draw plus a read-back sync, so it is the time until the pixels are done. "
                   f"The budget is {FRAME_US / 1000:.1f} ms; system busy is the whole box (X server included) while the loop runs, "
                   "which is what is left for the player and the rest of neomaster on the single-core C7.", "",
                   table(["setup / backend", "loop", "covers", "p50", "p99", "missed", "fps", "system busy"], rows), ""]

        rows = []
        for label, backend in runs:
            def per(fmt, size, test="covers"):
                r = get(label, backend, test, format=fmt, size=size)
                return f1(r.get("us_per_cover")) if r else "–"
            pres = get(label, backend, "present", region="screen")
            dim = get(label, backend, "fills", kind="dim 1024x768 x4")
            gl = get(label, backend, "glyphs")
            sync = get(label, backend, "sync")
            rows.append([f"{label} / {backend}", f1(sync.get("p50_us")) if sync else "–", f1(pres.get("p50_us")) if pres else "–",
                         per("xrgb8888", 96), per("rgb565", 96), per("argb8888", 96), per("rgb565", 48), per("rgb565", 96, "fades"),
                         f1(dim.get("mpix_per_s")) if dim else "–", f1(gl.get("us_per_glyph")) if gl else "–"])
        md += ["### Primitives", "",
               "Per-cover times are net of the sync round trip, from batches of 64. Fades are 50 % alpha.", "",
               table(["setup / backend", "sync us", "present 1024x768 us", "xrgb 96 us", "rgb565 96 us", "argb 96 us", "rgb565 48 us", "fade rgb565 96 us", "dim Mpix/s", "us/glyph"], rows), ""]

        rows = []
        for r in xb:
            if r.get("test") == "upload":
                rows.append([f"{r['label']} / {r['backend']}", r["params"]["format"], f1(r.get("covers_per_s")), f1(r.get("mb_per_s"))])
        if rows:
            md += ["### Cover uploads (PutImage, 96 px)", "", table(["setup", "format", "covers/s", "MB/s"], rows), ""]

        rows = []
        for r in xb:
            if r.get("test") == "vram":
                k = r.get("knee")
                curve = r.get("curve") or []
                last = curve[-1] if curve else {}
                first = curve[0] if curve else {}
                rows.append([f"{r['label']}", r["params"]["format"], f"{k['covers']} covers ({k['mb']} MB)" if k else f"none up to {last.get('covers')} covers ({last.get('mb')} MB)",
                             f1(first.get("us_per_cover")), f1(last.get("us_per_cover"))])
        if rows:
            md += ["### Video memory (96 px covers resident before EXA starts migrating)", "",
                   "The knee is the first count where compositing every cover once costs over three times as much per cover as with few.", "",
                   table(["setup", "format", "knee", "us/cover at start", "us/cover at end"], rows), ""]

        rows = []
        for r in xb:
            if r.get("test") == "ui":
                rows.append([r["label"], r["params"]["view"], f1(r["scene"].get("p50_us"), " us"), f1(r["render"].get("p50_us"), " us"),
                             f1(r["render"].get("p99_us"), " us"), f"{r.get('missed')}/{r.get('frames')}", f1(r.get("system_busy_pct"), " %")])
        if rows:
            md += ["### neomaster's own UI and XRender backend (fake library)", "", table(["setup", "view", "scene p50", "render p50", "render p99", "missed", "system busy"], rows), ""]

    # ---------------------------------------------------------------- player
    pl = jsonl(os.path.join(d, "player.jsonl"))
    if pl:
        md += ["## Sendspin player", ""]
        rows = []
        for r in pl:
            st = r.get("steady") or {}
            rows.append([r.get("label"), "yes" if r.get("ok") else "**no**", f1(r.get("connect_ms"), " ms"),
                         f1((r.get("play_to_stream") or {}).get("p50_us", 0) / 1000 if (r.get("play_to_stream") or {}).get("p50_us") else None, " ms"),
                         f1((st.get("process_cpu_pct") or {}).get("mean"), " %"), f1((st.get("process_cpu_pct") or {}).get("max"), " %"),
                         f1((st.get("system_busy_pct") or {}).get("mean"), " %"), f1(((st.get("rss_kb") or {}).get("max") or 0) / 1024, " MB"),
                         st.get("warnings", "–"), len(r.get("errors") or [])])
        md += ["Stream start is when the first audio reaches the player; sound follows the lead time (200 ms) later.", "",
               table(["run", "ok", "connect", "play → stream p50", "player CPU mean", "max", "system busy", "RSS max", "warnings", "errors"], rows), ""]
        for r in pl:
            if r.get("first_warnings"):
                md += [f"First warnings ({r.get('label')}):", "", "```", *r["first_warnings"][:8], "```", ""]
        lp = os.path.join(d, "latency-probe-200.txt")
        if os.path.exists(lp):
            m = re.search(r"median start-to-sound (.*), skip-to-sound (.*)", open(lp).read())
            if m:
                md += [f"Scheduled sound (latency_probe, lead 200 ms): start {m.group(1)}, skip {m.group(2)}.", ""]

    # ---------------------------------------------------------------- panel, PC2
    pan = load(os.path.join(d, "panel-summary.json"))
    if pan:
        md += ["## Panel (USB 0cd4:1112)", ""]
        if not pan.get("found"):
            md.append("**Not found.**")
        else:
            ri = pan.get("report_interval") or {}
            md.append(f"{'All steps passed' if pan.get('ok') else '**Not all steps passed**'}; {pan.get('reports')} reports, "
                      f"interval p50 {f1(ri.get('p50_us'))} us, {pan.get('out_reports_ok')} output reports accepted, "
                      f"{len(pan.get('out_errors') or [])} refused.")
            md += ["", table(["step", "ok", "secs", "inputs"], [[s.get("step"), "yes" if s.get("ok") else "**no**", s.get("secs", "–"), s.get("inputs", "–")] for s in pan.get("steps", [])])]
        md.append("")
    pc2 = load(os.path.join(d, "pc2", "pc2-summary.json"))
    if pc2:
        md += ["## PC2 over the debug connection", "", table(["check", "result", "detail"], [[c["check"], c["status"].upper(), c["detail"].replace("|", "/")] for c in pc2.get("checks", [])]), ""]
        md.append("Recording: `pc2/pc2-record.jsonl` (replay with `neomaster debug replay`, cut vectors with `neomaster debug vectors`).")
        md.append("")
    print("\n".join(md))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    main(sys.argv[1])
