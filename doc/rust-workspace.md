# neomaster 2: the Rust workspace

The Rust rewrite lives next to the C++ neomaster in this repository until it replaces it. The
design is in the architecture sketch (Neomaster panel UI: architecture sketch); this file says
what exists and how to build it.

## Layout

| Crate | What it holds | State |
|---|---|---|
| `crates/masterlink` | Telegram codec, protocol constants, `AudioMaster` and `SourceCenter`, sans-IO | Ported from libpc2, capture replays pass |
| `crates/pc2` | PC2 framing, fragment assembler, wake-up packets, telegram pacer, mixer, fan loop, start-up and dispatch, sans-IO | Ported from libpc2; USB adapter not written yet (M4) |
| `crates/bs5-panel` | Panel HID report decode, button edges, long press, click coalescing, backlight | Written from the 27 Nov captures; USB adapter not written yet (M4) |
| `crates/render` | Scene types, glyph rasterising (fontdue), XRender backend (x11rb) | Fills and text; covers come with M2 |
| `crates/ui` | Pointer zones, views, navigation, animation; state in, scene out | List, cover arc (placeholder tiles), now-playing, context menu, volume, idle and blanking, on fake data |
| `crates/ma-client` | Music Assistant WebSocket client: auth, reconnect, typed models and events, library, queue and transport commands | Works against MA 2.10.13; not wired into the binary yet |
| `crates/library` | SQLite mirror of the MA library (sort keys, letter index, playlist contents), event-driven sync, cover atlas (mmap, RGB565, browse order) | Mirrors the live library; not wired into the binary yet |
| `crates/a-aux` | A.AUX: finds the USB sound card ("USB Audio CODEC"), sans-IO source state, `alsaloop` adapter | Selecting A.AUX on the panel plays the card's line in; card detection checked on the BM5, playback not yet |
| `crates/neomaster` | The binary: core task, actors, `--sim` | Core loop and simulator run |

"Sans-IO" means bytes and time in, effects out: no threads, no sockets, no clocks. Every state
machine takes `now` as a `Duration` since a fixed epoch, so tests drive time directly.

## Build and test

```sh
cargo test                       # all crates, on the host
cargo run -p neomaster -- --sim  # the UI in a 1024x768 window; `--keys` lists the bindings
```

### For the BeoMaster 5 (i686)

The BM5's VIA C7-D is 32-bit only; `i686-unknown-linux-gnu` (Pentium 4 baseline, SSE2) fits it.

```sh
# Debian/Ubuntu host:
sudo apt install gcc-i686-linux-gnu libc6-dev-i386-cross
rustup target add i686-unknown-linux-gnu
cargo build --release --target i686-unknown-linux-gnu
```

`.cargo/config.toml` names Debian's cross linker. On a multilib host (for example Arch with
`lib32-glibc` and `lib32-gcc-libs`), override it instead:

```sh
CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_LINKER=gcc RUSTFLAGS="-C link-arg=-m32" \
  cargo build --release --target i686-unknown-linux-gnu
```

The release binary is about 1.5 MB and needs glibc 2.34 or newer (Debian bookworm has 2.36).
Without an i386 loader installed, the 32-bit tests still run on an x86-64 host through the cross
toolchain's loader:

```sh
CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_RUNNER="/usr/i686-linux-gnu/lib/ld-linux.so.2 --library-path /usr/i686-linux-gnu/lib" \
  cargo test --target i686-unknown-linux-gnu
```

CI (`.github/workflows/rust.yml`) runs the tests on the host and as i686, and builds the i686
release binary.

## The libpc2 port

Ported from toresbe/libpc2 master at 4b61da8 (including libpc2 PR #5: KEY_TRANSFER-only lock grants and the renamed protocol constants).

| libpc2 | Rust |
|---|---|
| `masterlink/protocol.hpp` | `masterlink::protocol` (same names, same provenance notes) |
| `masterlink/telegram.*` (`MasterlinkTelegram`) | `masterlink::Telegram` (`decode`, `encode`, `encode_mlgw`) |
| `masterlink/audio_master.*` | `masterlink::AudioMaster` |
| `masterlink/source_center.*` | `masterlink::SourceCenter` |
| `AudioMasterHost` | `masterlink::Host` trait, plus `Effects`, a host that records `Effect` values |
| `*_capture_vectors.inc`, `*_live_vectors.inc` | `crates/masterlink/tests/vectors/*.rs`, converted mechanically |
| `audio_master_tests.cpp`, `source_center_tests.cpp` | `crates/masterlink/tests/*.rs`, every test ported |
| `pc2/fragment_assembler.*`, `pc2/wakeup.hpp` | `pc2::frame` |
| `pc2/telegram_pacer.*` | `pc2::TelegramPacer` (time-driven, no worker thread) |
| `pc2/fan_control.*` | `pc2::FanControl` |
| `pc2/mixer.*` | `pc2::Mixer` (returns the PC2 messages instead of sending them) |
| `pc2/pc2device.*`, the dispatch in `pc2.cpp`, `PC2Beolink::send_telegram` | `pc2::Pc2` |
| `pc2/beo4.hpp`, `masterlink/masterlink.hpp` | `pc2::beo4`, `masterlink::protocol::source` |
| pc2 tests (assembler, wake-up, fan, pacer) | `crates/pc2/tests/libpc2_ports.rs` |

Differences on purpose:

- No threads or mutexes. `PC2Beolink`'s ticker, the pacer's worker, the mailbox and the libusb
  callbacks become the actors' loops plus `tick(now)` calls. `mailbox_tests.cpp` has no
  counterpart.
- `AudioMaster`'s lock waiters were closures; they are now a small enum of actions.
- `Telegram::decode` cuts a payload shorter than its SIZE byte instead of reading past the end.
- `Mixer::set_parameters` takes (volume, bass, treble, ...), the order on the wire; libpc2's
  took treble before bass.
- A malformed USB packet is logged and dropped, as libpc2's event loop already did.

Not ported, because neomaster does not use it: the legacy request/response machinery in
`masterlink.cpp` (`send_request`, `interrogate`, the hard-coded replies for Beoport and
promiscuous mode), `DecodedTelegram` and its subclasses, the ANSI pretty-printer
(`analysis.cpp`, `pprint.cpp`) and `utils/pcap_reader`. The USB I/O (`pc2_usb_io.*`) comes back
as a thin nusb adapter in M4.

## The simulator

`neomaster --sim` opens the screen as a window and turns keys into the same 6-byte HID reports
the panel sends, so the core, the panel decoder and the UI run exactly as on the BM5. The bindings
follow BeoSound5.exe's own keyboard emulation (arrows, G, A/Z, +/-, S), plus number keys that jump
the pointer to a zone. Holding GO opens the context menu (the simulator turns on XKB detectable
autorepeat so a held key does not repeat as press/release pairs).

## A.AUX

Decompiled from BeoSound5.exe and BeoIpcML.exe (names and comments are in the Ghidra projects;
the TSVs are `bs5exe_aaux.tsv` and `beoipcml_aaux.tsv` in the RE workspace).

What the original software does:

- A.AUX (BeoSound5.exe source 8) has two halves. Selecting it starts USB playback
  (`AuxSource::OpenUsbPlayback`); a DLNA controller pushing a stream to the BM5's renderer
  switches it to DMR playback instead. Beo4 N.MUSIC followed by 4 forces USB, by 2 forces DMR.
- USB playback (`DirectShowEngine::PlayFromUsb`) is a DirectShow graph from the capture device
  "USB Audio CODEC" (TI/Burr-Brown PCM2900-family USB sound cards) to "DirectSound: HD Audio
  rear output", the output every BM5 source plays through. The device is matched with the
  case-insensitive regex `USB\s+AUDIO\s+CODEC`. Plugging the card in only logs; unplugging it
  tells the source. There is no track info.
- On the link (BeoIpcML `TPcAuxProxy`): with AUDIO MASTER on, A.AUX is its own device, ML
  source 0x97 with Beo4 key 0x83. With AUDIO MASTER off (SystemConfiguration 7, the BM5 as a
  source center) there is no A.AUX on the link: turning A.AUX on switches the N.MUSIC device on
  (or leaves it on), so link rooms see N.MUSIC (0x7a, or wherever MLConfig moved that slot).

What neomaster does:

- `masterlink::SourceMap` gives each local source its ML source and Beo4 key per role and maps
  a link request back. `crates/masterlink/tests/a_aux.rs` replays the source center captures
  with A.AUX playing: the link sees the original N.MUSIC answers byte for byte.
- `a_aux::AuxSource` plays the card while A.AUX is selected and the card is present. Unlike
  BeoSound5.exe it waits for a missing card and resumes after replugging.
- `a_aux::loopback::Loopback` runs `alsaloop -C plughw:CARD=<id>,DEV=0 -P <playback> -t 50000`.
  `NEOMASTER_AUX_PLAYBACK` picks the playback device, `NEOMASTER_AUX_LATENCY_US` the latency.
  On the BM5 the A.AUX input is a built-in PCM2902 (USB 08bb:2902, ALSA card `CODEC`) and card 0
  is the VIA VT1708 onboard audio (`hw:VT82xx,0`), so `default` already plays to it.
- The DMR half is not replicated; Music Assistant covers network streams.
