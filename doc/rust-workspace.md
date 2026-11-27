# neomaster 2: the Rust workspace

The Rust rewrite lives next to the C++ neomaster in this repository until it replaces it. The
design is in the architecture sketch (Neomaster panel UI: architecture sketch); this file says
what exists and how to build it.

## Layout

| Crate | What it holds | State |
|---|---|---|
| `crates/masterlink` | Telegram codec, protocol constants, `AudioMaster` and `SourceCenter`, sans-IO | Ported from libpc2, capture replays pass |
| `crates/pc2` | PC2 framing, fragment assembler, wake-up packets, telegram pacer, mixer, fan loop, start-up and dispatch, sans-IO | Ported from libpc2; driven over USB by the binary's PC2 actor |
| `crates/bs5-panel` | Panel HID report decode, button edges, long press, click coalescing, backlight | Written from the 27 Nov captures; USB adapter not written yet (M4) |
| `crates/render` | Scene types, glyph rasterising (fontdue), images (PNG), XRender backend (x11rb) | Fills, text and images (scaled, cropped, tinted, additive, rotated) |
| `crates/ui` | Pointer zones, views, navigation, animation, layout and assets; state in, scene out | Stock layout measured from the 27 Nov screenshots: menu arc with beam, list and cover curves, now-playing, long-GO menu, volume arc, SETTINGS arc, view-switch fades, idle and blanking, on fake data. Uses the BM5's own font and textures from `$NEOMASTER_STOCK_ASSETS`, drawn look-alikes otherwise |
| `crates/ma-client` | Music Assistant WebSocket client: auth, reconnect, typed models and events, library, queue and transport commands | Works against MA 2.10.13; not wired into the binary yet |
| `crates/library` | SQLite mirror of the MA library (sort keys, letter index, playlist contents), event-driven sync, cover atlas (mmap, RGB565, browse order) | Mirrors the live library; not wired into the binary yet |
| `crates/a-aux` | A.AUX: finds the USB sound card ("USB Audio CODEC"), sans-IO source state, `alsaloop` adapter | Selecting A.AUX on the panel plays the card's line in; card detection checked on the BM5, playback not yet |
| `crates/radio` | N.RADIO presets: numbered list (TOML), Beo4 digit entry and ▲ ▼, channel -> preset, MA station lookup and playback | Wired into the binary; plays through MA (see doc/n-radio-presets.md) |
| `crates/airplay` | AirPlay: runs shairport-sync, parses its metadata pipe, sans-IO session (start, track, cover, pause, end), cover decode | Takes the source, shows track and cover, feeds link rooms; tested with Music Assistant as sender against bookworm's i386 shairport-sync, not on the BM5 yet (doc/airplay.md) |
| `crates/player` | Sendspin player: makes the BM5 a Music Assistant playback device through ALSA, reports stream start and end | Plays from the live MA on a desktop; not wired into the binary yet |
| `crates/settings` | The settings file (typed, versioned TOML, atomic save), the old environment variables as overrides, the Roles / source-name rules from MLConfig.dll, and the stock SETTINGS menu as data | Read at start, saved from the panel; the SETTINGS menu is drawn by `ui` |
| `crates/neomaster` | The binary: core task, PC2 actor (nusb), Masterlink role glue, simulated PC2, `--sim`, `--headless`, debug connection (`neomaster debug`) | Core loop, Masterlink loop and simulator run; panel actor not written yet |

"Sans-IO" means bytes and time in, effects out: no threads, no sockets, no clocks. Every state
machine takes `now` as a `Duration` since a fixed epoch, so tests drive time directly.

## Build and test

```sh
cargo test                       # all crates, on the host
cargo run -p neomaster -- --sim  # the UI in a 1024x768 window; `--keys` lists the bindings
cargo run -p neomaster -- --sim --role audio-master   # the same with the other Masterlink role
cargo run -p neomaster -- --sim --config /tmp/s.toml  # with a settings file of its own
neomaster --headless             # on the BM5: the Masterlink loop on the real PC2, no panel
neomaster --headless --debug     # the same, with the debug socket (doc/debug-connection.md)
```

The workspace needs Rust 1.85 or newer (nusb).

### For the BeoMaster 5 (i686)

The BM5's VIA C7-D is 32-bit only; `i686-unknown-linux-gnu` (Pentium 4 baseline, SSE2) fits it.

```sh
# Debian/Ubuntu host (ALSA headers for the player crate, native and i386):
sudo dpkg --add-architecture i386
sudo apt install gcc-i686-linux-gnu libc6-dev-i386-cross libasound2-dev libasound2-dev:i386
export PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_PATH_i686_unknown_linux_gnu=/usr/lib/i386-linux-gnu/pkgconfig \
       CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_RUSTFLAGS="-L /usr/lib/i386-linux-gnu"
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
  Settings `[aux]` pick the playback device (`playback`), the latency (`latency_us`) and,
  optionally, the card by ALSA id (`card`; empty = the first "USB Audio CODEC").
  On the BM5 the A.AUX input is a built-in PCM2902 (USB 08bb:2902, ALSA card `CODEC`) and card 0
  is the VIA VT1708 onboard audio (`hw:VT82xx,0`), so `default` already plays to it.
- The DMR half is not replicated; Music Assistant covers network streams.

## The Masterlink loop

```text
 PC2 (USB 0cd4:0101)                 PC2 actor (pc2_actor.rs)             core task (core.rs)
 IN  ep 0x81, 9-byte packets ─────▶  pc2::Pc2: reassemble, fan loop ──▶  Event::Pc2(..) ──▶ Link (link.rs)
 OUT ep 0x01            ◀───────────  pace ML telegrams, frame       ◀──  Pc2Cmd         ◀── AudioMaster or SourceCenter,
                                      reopen every 2 s when gone                              PC2 Mixer, SourceMap
```

- **PC2 actor.** `open_usb` claims interface 0 with nusb and runs a reader (four IN transfers in
  flight) and a writer task. `run_session` owns a fresh `pc2::Pc2` per connection: it sends the
  start-up messages (F1, `80 <node> 00`, the F6 option for the role), feeds every IN packet to the
  state machine, forwards the events to the core, and writes what the core and the pacer release.
  `supervise` reopens the device every 2 s after it goes away and tells the core
  (`Pc2Connected` / `Pc2Disconnected`), which restarts the role's announcements.
- **Link** (sans-IO, like the protocol crates) owns the role's state machine and the mixer state,
  and maps intents and PC2 events to `Pc2Cmd`s and `AppEffect`s (play, stop, source key, channel,
  standby, volume). `Core::link_out` carries them out: `Play(AAux)` starts the A.AUX loop and any
  other `Play` or `Stop` stops it; Music Assistant (N.MUSIC, N.RADIO) is not wired in yet and is
  only logged. In the link source role A.AUX starts when the room's master connects us, since its
  audio reaches the room over the link.
- **Role.** Settings › Roles › AUDIO MASTER (`roles.audio_master`), overridden for one run by
  `NEOMASTER_ML_ROLE` (the C++ neomaster's variable; `source_center` works) or `--role
  audio-master|link-source`. The default is off (link source): it never arbitrates for the bus,
  so a wrong default cannot disturb a link that has another audio master. Changing it on the
  panel rebuilds the link glue and restarts the PC2 in the new mode (`Pc2Cmd::Restart`) without
  reopening the device.

| Intent | Audio master | Link source |
|---|---|---|
| Volume wheel | PC2 `eb` absolute volume | Beo4 VOL UP `0x60` / DOWN `0x64` to the room's master, one per 4 counts (the UI volume is re-centred after each turn) |
| Pick a source | play it, `e3` tone + speakers on in B&O's order, `set_local_source` | its Beo4 key to the room's master (`0x94` N.MUSIC, `0x93` N.RADIO; A.AUX rides on N.MUSIC); play when the master connects us |
| Short standby (S) | speakers off, then routing; `local_standby`; the player keeps going for link rooms still listening | Beo4 STANDBY `0x0c` to the room's master |
| Long standby (Shift+S, held STANDBY on the remote) | `all_standby`: bus lock (`24` / IN 44 / `e4 01`), STANDBY_1 to ALL | the same from PC_1: `SourceCenter::all_standby` |
| PC2 IR key | acted on: source keys, VOL, MUTE, STANDBY (held = long), TV (listen to the video master), anything else to the playing source | logged only; the room's master has its own eye |
| Link room asks for our source | `AppEffect::Play` (`SourceMap::local_source`), SOURCE_CONTROL keys and channels to the player | the same, plus DISPLAY_DATA / METADATA after the connect replies (PR #10) |

Inferred, not captured: the link source's long standby. No capture shows a BM5 in that role
sending "all standby"; a link room does it as a lock round plus `80 <node> 01 0a 00 00 00 11 00 01`,
so we send that from PC_1 (C2), our address for everything else.

Tests (`crates/neomaster/src/link_tests.rs`, `pc2_actor.rs`): the PC2 state machine, `Link` and a
simulated PC2 (`sim_pc2.rs`: answers ML telegrams with ML status, grants the bus lock, reports
mixer changes, sends IR and telegrams split into 9-byte packets) replay the capture vectors of
`crates/masterlink/tests/vectors` end to end: every window of both the 2018 audio master USB
capture and the 2026 live source center captures, including the panel's own key telegrams at
their captured moments. The actor test runs the async plumbing against the simulated PC2
(start-up, pacing, reassembly, unplug, reopen, shutdown). `--sim` uses the simulated PC2 too, so
the whole loop runs on a desktop; `--sim --usb` drives a real PC2 instead.

## Settings

Everything that used to be an environment variable or a constant is in one file, by default
`/etc/neomaster/settings.toml` when running as root (the BM5) and
`~/.config/neomaster/settings.toml` otherwise; `--config FILE` or `$NEOMASTER_CONFIG` pick another.

- **Format.** TOML with a `version` key (now 1). Every key has the stock default, so a missing
  file or a file with a few lines is valid. Out-of-range values are pulled back into range and
  logged. A file that does not parse is moved to `settings.toml.bad` and the defaults are used.
  A file from a newer neomaster is read but never overwritten.
- **Saving.** Choices made in the SETTINGS menu are saved on a background thread: write
  `settings.toml.tmp`, fsync, rename over the old file, fsync the directory.
- **Environment.** `NEOMASTER_ML_ROLE`, `NEOMASTER_AUX_CARD`, `NEOMASTER_AUX_PLAYBACK`,
  `NEOMASTER_AUX_LATENCY_US`, `MA_URL` and `NEOMASTER_MA_PLAYER` still work and win over the file
  for that run, but are never saved into it (unless the same setting is then changed on the
  panel). `MA_TOKEN`, or `music_assistant.token_file`, gives the Music Assistant token; the token
  itself never goes in the settings file.

| Section | Keys | Applied |
|---|---|---|
| `[roles]` | `audio_master`, `am_option` (0, 1, 2, 5, 6), `delay`, `names` / `saved` (the four slot names), `primary` | live: the link glue and the PC2 restart |
| `[system]` | `language`, `time_zone`, `ir_timeout` (2-10), `power_saving`, `debug_logging` | `debug_logging` live (when `RUST_LOG` is unset); the others are stored and shown |
| `[idle]` | `return_after_s` (180), `standby_tick_s` (1800), `active_playback_timeout_h` (12) | live, into `ui::idle::IdleConfig` |
| `[service]` | `n_radio_buffer` (2-6), `disable_mots` | stored and shown (no N.RADIO player yet) |
| `[network]` | `host_name`, `location` | shown; the host is not changed |
| `[music_assistant]` | `url`, `player_id`, `token_file` | the N.RADIO actor, at start |
| `[aux]` | `card`, `playback`, `latency_us` | live: card choice at the next poll, the loop settings at the next start |
| `[link]` | `room_master`, `start_volume`, `bass`, `treble`, `loudness` | live, as for roles |

**Source names.** The Roles rules are MLConfig.dll's (rules doc "BM5 source names and Roles:
rules for neomaster"; `settings::names`, `settings::roles`). A slot's name gives its ML source
and Beo4 key (A.MEM 0x79/0x91, CD 0x8d/0x92, N.MUSIC 0x7a/0x94, RADIO 0x6f/0x81, N.RADIO
0xa1/0x93), so only names are stored and `Roles::source_map` derives the `SourceMap`. Switching
AUDIO MASTER off saves the names and forces the link defaults (SystemConfiguration 7); switching
it on restores them (SystemConfiguration from the AM option). The three music slots stay a
permutation. The MODE list follows the names (for example A.MEM instead of N.MUSIC).

**The menu.** GO on SETTINGS in the MODE list opens 7 sections; RIGHT LEFT RIGHT LEFT, then GO on
SETTINGS, opens the hidden menu with 9 (as on the stock BM5). `settings::menu` builds the
sections from the settings and the machine facts (uptime, storage, addresses, versions, CD drive);
`ui::settings_zones` puts them on the arc (the hidden menu's zone boundaries fitted to the
captured pointer positions) and `ui::settings_menu` draws each section on the list curve and
handles the grammar: the pointer picks a
section, the wheel moves, LEFT opens a choice's picker or a submenu, GO (or LEFT) keeps a choice,
RIGHT leaves a picker or submenu. Conditional items follow the rules: PLAY TIMER or DELAY by AM
option, the naming items only as audio master, CD naming only with the B&O CD drive. Stock items
neomaster does not do (software update, recovery, Spotify accounts, client password) are listed
and show NOT AVAILABLE.

Open points: the stock commit gesture in a picker was not captured (GO keeps, RIGHT cancels here);
DELAY's unit and range are not decoded, so it is shown but not offered; no speaker-type item
exists in the 7.04 software's menus.
