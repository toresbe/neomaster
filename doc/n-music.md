# N.MUSIC: Music Assistant on the panel

The panel browses the Music Assistant (MA) library from a local mirror and plays on the BM5's own
MA player, a Sendspin client built into the binary. Everything that waits on the network lives in
one actor (`crates/neomaster/src/music.rs`); the core stays sans-IO and talks to it with
`MusicCmd` (out) and `Event::Music(MusicEvent)` (in).

```text
 Music Assistant ──ws──▶ ma-client ──▶ library::sync ──▶ SQLite mirror + cover atlas ──▶ ui::Library ──┐
        │  ▲                   │                                                                         │
        │  └── play / queue ◀──┤◀── MusicCmd ◀──────────────────────────────── core (Effect, AppEffect)  │
        │                      └──▶ now playing, queue, 512 px cover ──▶ MusicEvent ──▶ core ──▶ Ui ◀───┘
        └──Sendspin──▶ player (ALSA) ── stream started / ended ──▶ MusicEvent::Stream
```

## What the panel does with it

- **Lists and covers.** ARTISTS, ALBUMS, TITLES and COVERS show the mirror, in the store's browse
  order (MA's sort names). Covers come from the atlas baked at 108 px (the largest cover on the
  panel; the renderer scales it for the others). Until the atlas is there, the cover's average
  colour stands in. When the mirror changes, every list keeps its place where its item still exists
  (`Ui::set_library`).
- **GO** replaces MA's queue with the artist, album or track and plays it; on a track inside an
  album it plays the album from that track. The now-playing view shows the guess at once and MA's
  report replaces it. **ADD TO QUEUE** (long GO) appends.
- **On the now-playing view** GO plays and pauses, LEFT plays the previous track and RIGHT the
  next (through MA). On N.RADIO, LEFT and RIGHT step presets like the Beo4's arrows; with nothing
  playing, GO starts N.MUSIC. A long GO there does nothing.
- **QUEUE** lists MA's queue from the current item on (MA keeps what has played before it); GO
  jumps there.
- **Now playing** follows MA's queue: title, artist / album, the 512 px cover from MA's image proxy.
  While N.MUSIC is distributed, link rooms get the track as DISPLAY_DATA / METADATA.

## Sources and roles

The panel's GO selects N.MUSIC the way picking it in MODE does, and the play request waits until
the source actually starts:

| | Audio master | Link source |
|---|---|---|
| GO on the panel | N.MUSIC on the speakers and the request sent at once | N.MUSIC's Beo4 key to the room's master; the request is sent when the master connects us |
| N.MUSIC started otherwise (Beo4, a link room, MODE) | MA resumes the queue | the same |
| Another source, standby, blank-screen standby, the room releasing us | MA pauses, so N.MUSIC resumes where it was | the same |
| N.RADIO next | no pause: the station replaces the queue | the same |
| N.MUSIC after N.RADIO | N.RADIO sends no stop (from its own connection it could land after N.MUSIC started); N.MUSIC plays its last album again from the track it was on, since resuming would resume the station | the same |
| MA starts playing on the BM5 by itself (its app) with nothing local playing | N.MUSIC is selected (speakers on), without a second play | logged: the room's master picks the source |

Remote keys for N.MUSIC, from our own IR eye (audio master) or from a link room's SOURCE_CONTROL
(both roles):

| Key | Does |
|---|---|
| PLAY (`0x35`, also GO) | resume |
| STOP (`0x36`) | pause |
| ▲ / ▼ (`0x1e` / `0x1f`) | next / previous track |
| ▶ / ◀ (`0x34` / `0x32`) | next / previous album, in ALBUMS order, from the album playing |

Inferred: the manual's Beo4 page says "step through tracks" (▲▼) and "step in albums" (◀▶) without
directions; the mapping above is the usual B&O one, not checked against BeoSound5.exe. Digits on
N.MUSIC (a MOTS seed in the stock software) and coloured lists are not done.

## Configuration

The settings' `[music_assistant]` section: `url` (`MA_URL` overrides it), the token (`MA_TOKEN`, or
the first line of `token_file`) and `player_id` (`NEOMASTER_MA_PLAYER`). Without a URL and a token
the panel browses a made-up library and nothing plays. With `player_id` empty, N.MUSIC and N.RADIO
play on the built-in player's queue; set it to play on another MA player instead. The built-in
player has no settings yet, only environment variables:

| Variable | Meaning |
|---|---|
| `NEOMASTER_PLAYER` | `sendspin` (default): the built-in player; `none`: an external player such as Squeezelite (the fallback), named by `player_id` |
| `NEOMASTER_SENDSPIN_URL` | MA's Sendspin endpoint (default `ws://<MA host>:8927/sendspin`) |
| `NEOMASTER_PLAYER_ID`, `NEOMASTER_PLAYER_NAME` | the built-in player's id and name in MA (default `neomaster-<hostname>`, "BeoMaster 5"; with `--sim`, `neomaster-sim-<hostname>`, so a desktop never takes over the BM5's player) |
| `NEOMASTER_PLAYER_DEVICE` | its ALSA device. With `--sim` the default is `null` (silent); set it to `default` to hear it. On the BM5, `default` is card 0, the A.AUX USB adapter: use `plughw:CARD=VT82xx,DEV=0`, the analog out to the PC2 (also for `NEOMASTER_AUX_PLAYBACK`) |
| `NEOMASTER_DATA_DIR` | the mirror, covers and atlas (default `/var/lib/neomaster` as root, else `~/.local/share/neomaster`) |

MA wraps the Sendspin client in a "universal" player (`up…`) a few seconds after it first
connects; the actor looks it up with `players/all` and uses its id as the queue. A first run pulls
the whole library (about 1.3 s for 3,200 items here), fetches the covers (5 s for 825) and bakes
the atlas (16 s on a desktop debug build; to be measured on the C7).

## Trying it

```sh
MA_URL=http://192.168.2.238:8095 MA_TOKEN=... NEOMASTER_STOCK_ASSETS=/mnt/A \
  neomaster --sim --role audio-master --debug
neomaster debug pointer 70; neomaster debug click go      # play the first album (silently)
neomaster debug beo4 0x1e                                 # next track
neomaster debug state                                     # "music": what plays
```

The `--sim` player stays in MA's player list after the run (MA keeps every Sendspin client it has
seen); remove it in MA's settings if it is in the way.
