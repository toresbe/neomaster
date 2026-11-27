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

The home server's MA has these player providers enabled: Sendspin, DLNA, AirPlay,
Chromecast, Sonos (and HA players). There is no Squeezelite (slimproto) or Snapcast
provider. Sendspin is MA's own protocol: clock-synchronised, PCM over a WebSocket,
and a Rust client exists (`sendspin` 0.4, from the Sendspin project), so the player is
an actor in the neomaster binary instead of a separate process to supervise.

The existing "BeoMaster5" player in MA comes from MA's `dlna` provider, so it is most
likely the stock BeoSound 5 software's DLNA renderer, and would go away with it.

## Measured latency

`examples/latency_probe.rs` registers a Sendspin player whose output only records when
each buffer would be heard, then drives MA (2.10.13) with `play_media` and `next`. LAN
WAV source, home server, 5 trials each:

| Lead time offered | `play_media` → sound | `next` → sound |
|---|---|---|
| 500 ms | 590 ms | 1.56 s |
| 200 ms | 290 ms | 1.26 s |

The first buffer arrives about 75 ms after `play_media`; the rest is the lead time.
After `next`, MA takes about 1.05 s before the new stream starts, whatever the client
(the Python reference client measured the same). The UI should show the new track
at once and not wait for the audio. Spotify sources add the provider's own start time.

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
