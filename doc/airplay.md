# AirPlay

A phone, tablet or computer can stream to the BeoMaster 5 with AirPlay. neomaster takes the
stream as a source, the way it takes A.AUX: when a device starts streaming, the BM5 takes the
source on the Masterlink (how depends on its link role), shows the track and its cover on the
panel, sends the track to link rooms, and releases the source when the device stops.

The receiving is done by [shairport-sync](https://github.com/mikebrady/shairport-sync), which
neomaster runs itself. This is AirPlay 1 ("classic"); AirPlay 2 is assessed
[below](#airplay-2).

## How it fits together

```text
 phone ──AirPlay 1──▶ shairport-sync ──ALSA──▶ BM5 output ──▶ PC2 ──▶ speakers / Masterlink
                          │ metadata pipe (FIFO)
                          ▼
              crates/airplay  receiver: runs and restarts shairport-sync, reads the pipe
                              metadata: parses the pipe's items
                              Session:  items → Started / Track / Cover / Paused / Resumed / Ended
                              cover:    JPEG → 340×340 RGB (on the receiver's thread)
                          │ core::Event::AirPlay, AirPlayCover
                          ▼
              crates/neomaster core:  Link::select_source(AirPlay)   (take the source)
                                      Ui::set_external + show_now_playing (panel)
                                      Link::track_info               (DISPLAY_DATA + METADATA)
                                      Link::release_source(AirPlay)  (after Ended)
```

- **Start.** shairport-sync writes `ssnc pbeg` when a sender sets up a stream. The core then
  stops A.AUX's loop (the output must be free), shows now-playing on the panel (waking it from
  blank-screen standby) and selects `LocalSource::AirPlay` on the link, exactly as if it had
  been picked on the panel:
  - *Audio master*: speakers on, routing on, the AirPlay slot (see below) as the local source.
  - *Link source*: the slot's Beo4 key to the room's master (N.MUSIC, `0x94`); the master
    connects us with SOURCE_CONNECT, as for any of our sources.
- **Track and cover.** `core minm/asar/asal/asgn` between `ssnc mdst` and `mden` make one
  track; `ssnc PICT` is its cover. The panel gets both at once. Link rooms get the track as
  DISPLAY_DATA (`AIRPLAY`) and six METADATA telegrams (link source role, while we distribute;
  `masterlink::track_info`), once it has stood for 1 s: senders send several bundles in a burst
  (Music Assistant sends its queue neighbours' first), and each costs seven telegrams.
- **Pause.** `pfls` / `prsm`. The panel says PAUSED; the idle timers treat it as stopped.
- **End.** `pend`, then 3 s of grace (a sender that reconnects at once keeps the source), then
  `Link::release_source`: if the room still listens to AirPlay it goes to standby as if the
  user pressed it (audio master: speakers off; link source: STANDBY to the room's master). If
  the room has moved on to something else, nothing is sent.
- **Another source picked, or standby.** The core restarts shairport-sync, which drops the
  sender's connection. AirPlay 1 has no "stop" a receiver can send short of the DACP remote
  control back channel, which not every sender offers.
- **A second device** may take over from the first (`allow_session_interruption`), as on
  Apple's receivers. The source stays taken.

The volume stays where it is: shairport-sync plays at full scale (`ignore_volume_control`) and
the sender's slider does nothing. The real volume is the PC2's (audio master) or the room
master's (link source), set with the panel wheel or a Beo4. Mapping the sender's volume
(`ssnc pvol`) onto those is a possible follow-up; it needs a sensible cap, since a phone at
full volume would otherwise mean volume 90.

### Which Masterlink source AirPlay is

AirPlay is not a B&O source, so it has no ML source number or Beo4 key of its own. neomaster
announces it as A.AUX (`masterlink::AIRPLAY_SLOT`):

| Role | ML source | Beo4 key | A room asking for that source while AirPlay plays |
|---|---|---|---|
| Audio master | A.AUX `0x97` | A.AUX `0x83` | gets AirPlay |
| Link source | N.MUSIC slot `0x7a` (A.AUX rides on it in this role) | N.MUSIC `0x94` | gets AirPlay |

Why A.AUX: the stock BeoSound5.exe played a pushed network stream (a DLNA controller pushing to
the BM5's renderer, the "DMR" half of A.AUX) as A.AUX. AirPlay is the same thing from a
different sender, so it takes the same place, and a room that selects A.AUX while a phone is
streaming hears the phone. DISPLAY_DATA still says `AIRPLAY`, so rooms with a display show
what plays.

The alternative is N.MUSIC (`0x7a`, Beo4 `0x94`) in both roles. That puts AirPlay on the key
most rooms already use for the BM5, but in the audio master role it would then compete with
Music Assistant for N.MUSIC. Changing it is one constant (`AIRPLAY_SLOT` in
`crates/masterlink/src/sources.rs`).

## Configuration

AirPlay is on by default. neomaster writes `shairport-sync.conf` and the metadata FIFO into
`$XDG_RUNTIME_DIR/neomaster` and runs `shairport-sync -c` on it.

| Variable | Default | |
|---|---|---|
| `NEOMASTER_AIRPLAY` | on | `0`, `off`, `no` or `false` turns AirPlay off |
| `NEOMASTER_AIRPLAY_NAME` | `BeoMaster 5` | the name senders show |
| `NEOMASTER_AIRPLAY_DEVICE` | A.AUX's playback device (`aux.playback` in the settings file, `NEOMASTER_AUX_PLAYBACK`) | ALSA output: the one the PC2 hears |
| `NEOMASTER_AIRPLAY_BACKEND` | `alsa` | `dummy` plays nothing (tests) |
| `NEOMASTER_AIRPLAY_PORT` | 5000 | RTSP port |
| `NEOMASTER_AIRPLAY_COMMAND` | `shairport-sync` | anything that takes `-c FILE`, e.g. a wrapper that runs it in a container |
| `NEOMASTER_AIRPLAY_DIR` | `$XDG_RUNTIME_DIR/neomaster` | configuration and pipe |

If shairport-sync is missing, neomaster logs it once and runs without AirPlay. If it exits, it
is restarted with a back-off (2 s doubling to 60 s). It dies with neomaster (`PR_SET_PDEATHSIG`).

### On the BM5 (Debian bookworm, i386)

```sh
sudo apt install shairport-sync avahi-daemon
sudo systemctl disable --now shairport-sync   # neomaster runs its own
```

bookworm's package is shairport-sync 3.3.8, built for i386 with ALSA, Avahi, soxr, the
metadata pipe, D-Bus, MPRIS and MQTT (`shairport-sync -V`:
`3.3.8-libdaemon-OpenSSL-Avahi-ALSA-jack-pa-dummy-stdout-pipe-soxr-convolution-metadata-mqtt-dbus-mpris-sysconfdir:/etc`).
Checked in a `linux/386` bookworm container: it runs, registers `_raop._tcp` through the
host's Avahi, and Music Assistant's AirPlay sender streams to it with title, artist, album and
cover art. Two warnings at start-up (it cannot claim its D-Bus and MPRIS names on the system
bus as an ordinary user) are harmless; neomaster uses the pipe.

The user neomaster runs as needs the `audio` group. The output device must be free when AirPlay
starts (neomaster stops A.AUX's loop first); with `dmix` in front of the onboard card it does
not matter.

Not checked on the BM5 itself yet (it is mid-upgrade): the CPU cost on the C7, and the right
ALSA device. shairport-sync measures the CPU at start-up and picks soxr interpolation only if
the CPU is fast enough, else its basic one; either is fine for the C7.

### Testing without a BM5

`neomaster --sim` runs AirPlay too. With shairport-sync in a bookworm i386 container
(`--network host`, the host's D-Bus socket for Avahi, `--init` so SIGTERM reaches it) and
`NEOMASTER_AIRPLAY_COMMAND` pointing at a wrapper that runs it, any AirPlay sender on the LAN
sees the simulator. Music Assistant's AirPlay provider is a convenient sender: it finds the
receiver by itself, and `player_queues/play_media`, `next`, `pause`, `stop` drive it.

## AirPlay 2

AirPlay 2 adds multi-room sync with other AirPlay 2 speakers, buffered streams (faster start,
AAC or ALAC up to 48 kHz/24-bit), and the Home app. iOS still sends AirPlay 1 to receivers that
only offer it, so AirPlay 1 is not a dead end; what is lost is grouping the BM5 with HomePods or
other AirPlay 2 speakers.

What it would take:

- **shairport-sync 4.x or later built `--with-airplay-2`.** bookworm only has 3.3.8 (AirPlay 1).
  Debian's newer packages (forky has 5.6.2 and nqptp 1.2.9) come from releases that no longer
  support i386 as a system architecture and need their own libraries, so on the BM5 it has to
  be built: on bookworm i386 with `libplist-dev libsodium-dev
  libgcrypt20-dev uuid-dev libavcodec-dev libavformat-dev libavutil-dev` (FFmpeg for AAC) on
  top of the AirPlay 1 dependencies. See "Build check" below.
- **nqptp**, a small daemon that follows the PTP clock AirPlay 2 synchronises to. It needs UDP
  ports 319 and 320 to itself (systemd `AmbientCapabilities=CAP_NET_BIND_SERVICE`) and talks to
  shairport-sync through POSIX shared memory. Not in bookworm either; it builds with autotools
  and no extra libraries.
- **CPU.** Upstream recommends "a system with the power of a Raspberry Pi B, or better" for
  AirPlay 2 (a single 700 MHz ARM11). The C7-D (1.5 GHz-class x86 with SSE2/SSE3) is above
  that. The extra work over AirPlay 1 is AAC decoding in FFmpeg for buffered streams (my estimate,
  not measured: well under a tenth of a core at 44.1/48 kHz stereo), the PTP follower and
  pairing crypto at session set-up. Measure on the BM5 before relying on it; FFmpeg's memory
  footprint on a 1 GB machine that also runs X and neomaster is the other thing to watch.
- **neomaster changes: almost none.** The metadata pipe is the same in 4.x. AirPlay 2 adds items
  neomaster can use: `ssnc snam` (the sender's name, shown on the panel: already handled),
  `ssnc conn`/`disc` (connection, before and after streams) and `ssnc abeg`/`aend` (active
  state). AirPlay 2 senders keep the connection between tracks and pauses, so `pend` comes later
  and less often; the grace period covers that. The config needs `airplay_device_id_offset` if
  it ever runs next to another instance. Release by restarting shairport-sync still works;
  AirPlay 2 also offers a proper remote-control channel (D-Bus `RemoteControl`, MPRIS) for
  pause and next.
- **Pairing.** AirPlay 2 receivers can be added to the Home app; not needed for streaming.

Recommendation: ship AirPlay 1 now (packaged, tested, enough for "play from my phone"), and
build an AirPlay 2 package for bookworm i386 when multi-room with other AirPlay speakers is
wanted, measuring CPU and memory on the BM5 first.

### Build check

Done on 27 Nov 2026 in a `linux/386` `debian:bookworm` container (the BM5's userland), from
the upstream tags nqptp 1.2.4 and shairport-sync 4.3.7:

```sh
apt install build-essential git autoconf automake libtool pkg-config libpopt-dev libconfig-dev \
  libasound2-dev libavahi-client-dev libssl-dev libsoxr-dev libplist-dev libsodium-dev \
  libavutil-dev libavcodec-dev libavformat-dev uuid-dev libgcrypt20-dev xxd
(cd nqptp && autoreconf -fi && ./configure && make)
(cd shairport-sync && autoreconf -fi && ./configure --sysconfdir=/etc --with-alsa --with-soxr \
  --with-avahi --with-ssl=openssl --with-airplay-2 --with-metadata --with-pipe --with-stdout \
  --with-dummy && make)
```

Both build without warnings. `shairport-sync -V` says
`4.3.7-AirPlay2-smi10-OpenSSL-Avahi-ALSA-dummy-stdout-pipe-soxr-metadata-sysconfdir:/etc`.
With nqptp running, it starts and idles at 22 MB resident (nqptp: 2 MB). Not tested: streaming
from an AirPlay 2 sender (none here), and the CPU on the C7.

Debian's FFmpeg drags in over 100 shared libraries (X11, cairo, video codecs) that
shairport-sync does not need: it only uses the AAC decoder. A package for the BM5 would be
better off against a minimal FFmpeg (`--disable-everything --enable-decoder=aac`) or a static
one.
