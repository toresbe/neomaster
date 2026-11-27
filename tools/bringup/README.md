# BM5 bring-up kit

Milestone M0 of the architecture sketch: hard numbers from the BeoMaster 5 itself, in one run,
to choose the render backend and confirm the player, plus smoke tests for the PC2 and the panel.

```sh
tools/bringup/bringup.sh --media library://album/131 --media2 library://track/1700 --ma-env ~/path/to/ma.env
tools/bringup/bringup.sh --interactive ...   # someone at the BM5: panel walk-through, Beo4 key
tools/bringup/bringup.sh --quick ...         # a few minutes, to check the kit itself works
tools/bringup/bringup.sh --dry-run           # build and stage only
```

It cross-builds the i686 binaries (in a `linux/386` `rust:1-bookworm` container, or with the
Debian cross toolchain of `doc/rust-workspace.md` via `--build native`), copies the kit to
`~/nm-bringup/<stamp>/` on the box, runs `box/run-on-box.sh` there as root, and pulls the results
into `~/projects/rea-local/work/bringup/<stamp>/` (`--out` to change) with a `summary.md` made by
`host/report.py`. It refuses to run unless the box is on bookworm and no package manager is
running (`--force` overrides). A full run takes about 15 minutes, most of it the render benchmark
under three X set-ups.

The token for Music Assistant comes from `MA_TOKEN` or a file holding `MA_TOKEN=...`
(`--ma-env`). It is copied to the box as a 0600 file that the run deletes when it starts.

## What runs, in order

| Step | Program | What it answers |
|---|---|---|
| preflight | `box/preflight.sh` | Debian, kernel, glibc, CPU, memory, openchrome and video RAM from the Xorg log, `/dev/fb0`, the PC2, panel and codec on USB, ALSA cards, Music Assistant reachable. Read-only; safe to run any time: `ssh bm5 sh -s < tools/bringup/box/preflight.sh` |
| panel | `nm-panel-probe` | Does the panel (USB 0cd4:1112) report every control, decoded by `bs5-panel` as neomaster decodes it? Do backlight and click reports work? What is its report rate? |
| pc2 | `box/smoke_pc2.py` + `neomaster --headless --debug` | Does neomaster open the real PC2 and run the start-up, do the debug commands answer, what does the link say in 30 s, how long does a core loop turn take, does a Beo4 key arrive, and does the recorded panel input replay through neomaster to the same decoded inputs |
| player | `nm-player-bench`, `latency_probe` | Sendspin player CPU, memory and warnings on the C7 while playing through ALSA; `play_media` → stream start; scheduled-sound latency (silent probe) |
| xbench | `nm-xbench` under Xorg `exa`, `exa16`, `noaccel` | The render backend decision, below |
| player under load | both at once | Does audio stay clean, and does the UI keep 60 fps, when both share the single C7 core |

### The render benchmark

`nm-xbench` draws what the panel UI will draw, from the sizes in the architecture sketch: covers
at the baked sizes (48 for lists, 64-96 for the arc) as XRGB8888, RGB565 and premultiplied
ARGB, the arc's fade (covers at partial alpha), translucent fills (a full-screen dim, highlight
bars), a list view's glyphs, a cover-strip redraw and a whole-screen redraw paced at 60 fps,
cover uploads, and neomaster's own UI (`crates/ui` on the fake library) through its own XRender
backend (`crates/render`).

It does so through two paths, under each X set-up:

- `xrender`: server-side pixmaps composited into a back buffer, glyphs from a server glyph set,
  one Composite to the window. Under `exa` / `exa16` that is openchrome's EXA acceleration at depth
  24 / 16; under `noaccel` the same requests are done by the X server in software (pixman).
- `sw-x11`: neomaster composites on the CPU into its own XRGB8888 buffer and copies finished
  regions to the window with PutImage: the "framebuffer blitter" fallback, still under X.
- `sw-fbdev` (with `--try-viafb`): the same blitter writing straight into `/dev/fb0`. The box has
  no framebuffer device by default (openchrome drives the CX700 without one); the run loads
  `viafb` for this and unloads it after. Opt-in because loading it under a running console is the
  least tested step.
- `sw-cpu`: the blitter without presenting, for the CPU cost alone.

Every timing ends with a one-pixel GetImage read-back, because X requests return before the
GPU has done them; per-cover figures are net of that round trip. The 60 fps loops report frame
time (to pixels done), missed deadlines, and how busy the whole box was, X server included,
which is what is left for audio and the rest of neomaster.

The video-memory test keeps allocating 96 px covers and compositing all of them once per pass;
the knee where the per-cover time triples is where EXA starts migrating pixmaps out of the
64 MB of video memory, i.e. how many covers can stay resident.

Run any of it by hand on the box (inside an X session, `DISPLAY` set):

```sh
nm-xbench --backend xrender --tests covers,strip60 --seconds 20
nm-xbench --backend sw-x11 --quick
nm-xbench --backend xrender --tests vram --vram-max-mb 120
```

### The panel probe

neomaster has no panel actor yet (M4), so the panel test reads the panel with its own small
nusb reader and `bs5-panel`'s decoder. Its recording is in the debug connection's record format,
so `neomaster debug replay panel-record.jsonl` feeds the real panel's reports into a running
neomaster (the PC2 smoke test does exactly that). It detaches the panel from `usbhid`; replug or
reboot to give it back.

## Reading the results

`summary.md` leads with the 60 fps table. A path is good enough when its p99 frame time stays
well under 16.7 ms with no missed frames and the box keeps CPU to spare; the player-under-load
run then says whether that spare CPU is enough. The raw JSON Lines (`xbench.jsonl`,
`player.jsonl`, `panel-summary.json`, `pc2/pc2-summary.json`), every program's console output,
the Xorg configs and logs, and neomaster's own debug recording (`pc2/pc2-record.jsonl`) are all
in the results directory.

## Known limits

- The PC2 test does not put anything on the link beyond neomaster's own start-up announcements:
  no telegram injection, no standby. Masterlink traffic is only seen if a room is in use.
- `neomaster` has no signal handler yet, so the smoke test's SIGINT ends it without the PC2's
  close sequence; its recording is flushed per record and survives that.
- On a desktop, `nm-player-bench --device null` spins a core: ALSA's null device does not pace
  playback. Use a real device (the BM5's default, VT82xx) for CPU figures.
- The software blitter is plain integer code without SIMD: an upper bound on the fallback's cost.
