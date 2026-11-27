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
`player_queues/play_media` (option `replace`) on the queue in `NEOMASTER_MA_PLAYER`, and stops that
queue when N.RADIO stops or another source takes over. It reads `MA_URL`, `MA_TOKEN` and
`NEOMASTER_MA_PLAYER`, the names the settings layer uses; without them N.RADIO tunes and logs but
plays nothing. Several quick presses only start the newest station.

Tested on 27 Nov 2026 against MA 2.10.13 with a silent Sendspin player (`bm5_player` on the ALSA
`null` device) and `neomaster --headless` driven over the debug socket: the timer's SOURCE_CONNECT
found and played Deutschlandfunk (Radio Browser), a link room's digit 1 switched to preset 1, and
RELEASE stopped the queue.

## Importing the stock presets

The stock BeoSound 5 software keeps N.RADIO in SQLite, `E:\Cache\Data\nradio.db` on the BM5's
data partition (`/dev/sda3`, label "Data"). Copied from Tore's BM5 on 28 Nov 2026 (read-only
mount) and read:

- `favorite_list` rows 1-4 are the colour lists RED, GREEN, YELLOW and BLUE (99 items each, all
  empty here). Rows 5-103 are lists named `01` to `99` with room for one item each: **these are the
  numbered presets**, the numbers Beo4 digits and link channels select.
- `favorite_list_item` holds the stations (`entity_type` 12): name, stream URL, country, and the
  `station.id` in the 66,000-station directory B&O's service filled in.
- Tore's presets: 1 BBC World Service, 2 KUOW FM, 3 NRK Nyheter, 5 NRK Jazz, 6 WFMU, 7 Radio Nova,
  8 Deutschlandfunk. Preset 8 matches the wake-up timer's channel 8 on the link.

`radio::stock` (feature `stock`) reads this, and the example writes the preset file:

```sh
sudo mount -t ntfs-3g -o ro /dev/sda3 /mnt/e     # on the BM5; never read-write, Windows must boot
cp /mnt/e/Cache/Data/nradio.db /tmp/ && sudo umount /mnt/e
cargo run -p radio --features stock --example import_stock -- /tmp/nradio.db /etc/neomaster/radio-presets.toml
```

The stock stream URL becomes the preset's URI; Music Assistant plays a plain http(s) URL as a
radio station (`builtin://radio/<url>`), so no name matching is involved. All seven of Tore's
URLs resolved in MA 2.10.13. `crates/radio/radio-presets.example.toml` is that import.

## What BeoNetRadio.exe showed (older N.RADIO)

Before the SQLite database turned up, `BeoNetRadio.exe` (the older Delphi N.RADIO, Ghidra project
`work/ghidra-radio`, annotations `work/ghidra-annotations/beonetradio_presets.tsv`) was read:
the same four colour lists capped by `MaxItemsInColorPlaylist` = 99, the last item per colour in
`LastPlayedColoredFavItem.beo`, a DBISAM database (`RadioFiles`, `RadioFavoriteMaster`,
`RadioFavoriteDetails`), and Beo4 input as BeoIpc `Digit`, `ShiftDigit`, `StepUp`, `StepDown`
commands. BeoSound5.exe's database labels are annotated in `work/ghidra-bs5exe`
(`work/ghidra-annotations/bs5exe_nradio_presets.tsv`).

Still unknown: the stock digit timeout, and whether the stock software lets a channel number
reach into the colour lists.
