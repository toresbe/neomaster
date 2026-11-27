# player

Makes the BeoMaster 5 a Music Assistant playback device, as a
[Sendspin](https://github.com/Sendspin/spec) player running inside neomaster.

```rust
let mut cfg = PlayerConfig::new("ws://192.168.2.238:8927/sendspin", "neomaster-bm5", "BeoMaster 5");
cfg.device = Some("default".into());          // ALSA device; PC2 output on the BM5
let (player, mut events) = Player::spawn(cfg);
// Event::StreamStarted → route the PC2 to this source, speakers on
// Event::StreamEnded   → release it
let queue = ma.player_for_output("neomaster-bm5").await?;   // e.g. "up6e0de696"
ma.play_media(&queue.unwrap(), &["library://album/131"], QueueOption::Play).await?;
```

## Why Sendspin

Compared on 27 Nov 2026 against the live MA 2.10.13, each client running as the Debian
bookworm **i386** build (the Sendspin one is this crate's `bm5_player`) in a container on
the home server, playing into a PulseAudio null sink. A recorder on the sink's monitor
timed when sound actually came out after `play_media` and after `next`. LAN source,
6 trials each, medians:

| Client (MA provider) | `play_media` → sound | `next` → sound | CPU while cycling* | RSS |
|---|---|---|---|---|
| Sendspin, this crate (`sendspin`, beta, built in) | 0.32 s | 1.29 s | 1.3 % | 19 MB |
| Squeezelite 1.9.9 (`squeezelite`, stable) | 0.29 s | 1.27 s | 0.8 % | 35 MB |
| Snapcast 0.26 client (`snapcast`, unmaintained) | 1.06 s | 2.78 s | 0.5 % | 10 MB |

\* Share of one Ryzen 9 5950X core, averaged over play/skip/stop cycles; the C7 is many
times slower, so expect roughly ten times these figures on the BM5.

- Sendspin and Squeezelite are equally fast: about 1 s of every skip is MA preparing
  the next stream, whichever client is used. Snapcast's fixed buffer adds about 1 s to
  everything, and MA no longer maintains its Snapcast provider.
- Sendspin runs inside the neomaster binary as an actor: no second process to supervise,
  and stream start and end arrive as events for PC2 routing. It is MA's own protocol.
- Squeezelite is the more mature client and uses a little less CPU (it decodes FLAC; the
  Sendspin client receives PCM but runs its own clock-sync and resampling in f32). It
  would run as a supervised process, with its power hook for PC2 routing.

Choice: Sendspin, with Squeezelite as the fallback if the `sendspin` crate (marked work in
progress upstream) misbehaves or its CPU load is too high on the C7. The deciding
measurement is CPU on the BM5 itself (milestone M0).

The existing "BeoMaster5" player in MA comes from MA's `dlna` provider, so it is most
likely the stock BeoSound 5 software's DLNA renderer, and would go away with it.

## Measured latency (schedule-based probe)

`examples/latency_probe.rs` registers a Sendspin player whose output only records when
each buffer is scheduled to be heard, then drives MA with `play_media` and `next`. LAN WAV
source, home server, 5 trials each:

| Lead time offered | `play_media` → sound | `next` → sound |
|---|---|---|
| 500 ms | 590 ms | 1.56 s |
| 200 ms | 290 ms | 1.26 s |

The first buffer arrives about 75 ms after `play_media`; the rest is the lead time. The UI
should show the new track at once and not wait for the audio. Spotify sources add the
provider's own start time.

## Notes

- Offers PCM 44.1 and 48 kHz, 16-bit: no decoding on the C7, about 1.4 Mbit/s.
- MA volume is accepted but only scales the stream; the real volume is the PC2's or the
  room master's, so leave MA at 100 %.
- `sendspin` pulls in `opus-rs`, which does not build for 32-bit x86; a patched copy is
  in `vendor/opus-rs` (see its `NEOMASTER-PATCH.md`).
- `sendspin` is marked work in progress upstream; it played correctly here, but watch it
  on the C7.

Examples: `latency_probe <url1> <url2> [trials] [lead_ms]` (silent) and
`bm5_player [client_id] [name] [alsa_device]` (plays real audio).
