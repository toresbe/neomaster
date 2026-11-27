# N.RADIO presets

A B&O system picks a radio station by number. neomaster keeps a numbered preset list and plays
each preset as a Music Assistant (MA) radio station on the BM5's MA player.

## Where numbers come from

| Input | Path | Handled in |
|---|---|---|
| A link room or timer asks us (link source) for N.RADIO at a channel | SOURCE_CONNECT short form, byte 2 (`masterlink::protocol::source_connect`) | `SourceCenter` -> `Effect::SourceChannel` |
| A room asks us (audio master) for N.RADIO at a channel | GOTO_SOURCE / LINK_CONNECT_COMMAND_1, byte 2 (`link_connect::requested_channel`) | `AudioMaster` -> `Effect::SourceChannel` |
| Beo4 digits, ▲ ▼ in a link room | SOURCE_CONTROL `<source> <key>` | `Effect::SourceKey` |
| Beo4 digits, ▲ ▼ at the BM5 (audio master) | the PC2's IR eye | `Link::ir_key` -> `AppEffect::SourceKey` |
| The panel | `Core::radio_preset` (debug: `neomaster debug radio N`) | the panel's N.RADIO list is not drawn yet |

`radio::Tuner` turns these into a preset: a channel plays that preset; a digit plays at once
unless a longer preset could follow (1 then 2 within 1.5 s = 12); ▲ ▼ step to the next defined
preset, wrapping. N.RADIO picked without a number resumes the last preset (saved), else the first.
When a room connects N.RADIO with a channel, the core starts that preset directly, not the last
one first. A repeat request for the preset already playing (the timer's follow-up GOTO_SOURCE)
changes nothing.

The wake-up timer of 27 Nov 2026, 06:59 (`work/ml_wake_timer_0700.txt` in rea-local) is a test:
`core::tests::wake_up_timer_plays_preset_8` replays its two SOURCE_CONNECTs and the RELEASE.

## The preset file

Until the settings file lands, the presets have their own file: `$NEOMASTER_RADIO_PRESETS`, else
`/etc/neomaster/radio-presets.toml` as root (the BM5), else
`$XDG_CONFIG_HOME/neomaster/radio-presets.toml`. `radio::Presets` is plain serde data, so it can
move into the settings file as a `[n_radio]` table unchanged.

```toml
last = 8                       # the preset played last; neomaster rewrites this

[[preset]]
number = 8                     # 1-99
name = "Deutschlandfunk"
uri = "library://radio/3"      # any MA URI; leave it out to look the name up
```

A preset with a name and no `uri` is looked up the first time it plays: the MA library first, then
every radio provider (`music/search`, media type radio; exact name, then prefix, then the first
hit). A station found by search is added to the MA library, and its `library://radio/<id>` URI is
written back to the file. Name matching is a convenience: check what it picked (the log says) and
put the URI you want in the file. In a test, "NRK P1" matched "NRK P1 Buskerud".

Searching needs a radio provider in MA. Radio Browser (`radiobrowser`) has no account and covers
the stations tested; TuneIn works too.

## Music Assistant

The N.RADIO actor (`crates/neomaster/src/radio_actor.rs`) plays with
`player_queues/play_media` (option `replace`) on the BM5's queue, and stops that queue when N.RADIO
stops or another source takes over. It takes the server, the queue and the token from the
settings' `[music_assistant]` (`url`, `player_id`, and `MA_TOKEN` or `token_file`; `MA_URL` and
`NEOMASTER_MA_PLAYER` override them for a run); without them N.RADIO tunes and logs but plays
nothing. Several quick presses only start the newest station.

Tested on 27 Nov 2026 against MA 2.10.13 with a silent Sendspin player (`bm5_player` on the ALSA
`null` device) and `neomaster --headless` driven over the debug socket: the timer's SOURCE_CONNECT
found and played Deutschlandfunk (Radio Browser), a link room's digit 1 switched to preset 1, and
RELEASE stopped the queue.

## How the stock BM5 numbered its stations

From `BeoNetRadio.exe` (Ghidra project `work/ghidra-radio`, annotations
`work/ghidra-annotations/beonetradio_presets.tsv`) and the C: image's registry:

Observed:
- N.RADIO keeps its favourites in four colour lists, CP_RED, CP_GREEN, CP_YELLOW and CP_BLUE
  (colour 1-4), each capped by `HKCU\Software\Bang & Olufsen\BeoPlayer\N.Radio\MaxItemsInColorPlaylist`
  (99 on this BM5). The last item played in each colour is kept in
  `<N.RADIO data dir>\LastPlayedColoredFavItem.beo` as `CP_<colour>=<item>`.
- Stations and favourites live in a DBISAM database: tables `RadioFiles` (with `ColorFavCode`,
  `ColorFavDependency`), `RadioFavoriteMaster`, `RadioFavoriteDetails` (`Sequence`, `ItemType`).
- BeoNetRadio takes Beo4 input as BeoIpc source commands: `Digit`, `ShiftDigit`, `StepUp`,
  `StepDown`, `ControlRed/Green/Yellow/Blue`, `Track` (dispatch at 0x0422cd18).
- On the link, the stock BM5 answered "N.RADIO 8" with Deutschlandfunk and sent its name, genre
  ("News") and country ("Germany") in EXTENDED_SOURCE_INFORMATION.

Inferred:
- A channel number is a position in a colour list (which list the link's numbers use is not
  decoded), so numbers run 1-99. neomaster's flat 1-99 list keeps the numbers rooms and timers
  already use.

Unknown:
- Which colour list (or the whole favourites list) Beo4 digits and SOURCE_CONNECT channels index;
  the event handlers are assigned in code, not in the form resources, and were not traced.
- BeoNetRadio's digit timeout.
- The database itself: `BeoLinkMedia\AbsoluteSharePath` is `E:\BM-SHARE` (system files under
  `\System`), on the BM5's data partition, which is not in the C: image. Reading it (DBISAM
  `.dat` / `.idx` / `.blb`) would give the real list to migrate. Until then, presets are entered
  by hand; preset 8 = Deutschlandfunk is confirmed from the link.
