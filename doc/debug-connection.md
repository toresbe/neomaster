# The debug connection

For bringing neomaster up on the BM5: a live stream of what it sees and does, a way to drive it
from another machine, and recordings that turn into capture-replay tests. The code is in
`crates/neomaster/src/debug/`. It is off unless asked for, and costs one branch per record when
off.

## Turning it on

| Flag | Variable | |
|---|---|---|
| `--debug` | `NEOMASTER_DEBUG=1` | Unix socket at the default path |
| `--debug-socket PATH` | `NEOMASTER_DEBUG_SOCKET` | Unix socket at `PATH` (the client reads the same variable) |
| `--debug-tcp ADDR` | `NEOMASTER_DEBUG_TCP` | also TCP, e.g. `0.0.0.0:7155` or just `7155`; read-only |
| `--debug-tcp-inject` | `NEOMASTER_DEBUG_TCP_INJECT=1` | allow injection over TCP too |
| `--debug-record FILE` | `NEOMASTER_DEBUG_RECORD` | record everything from start-up to a JSON Lines file |

The default socket is `$XDG_RUNTIME_DIR/neomaster-debug.sock`, else `/tmp/neomaster-debug.sock`.
Injection is always allowed on the Unix socket (file permissions guard it) and refused over TCP
unless `--debug-tcp-inject` is given: a telegram injected from the LAN can switch off the house.

On the box, for a first hardware run:

```sh
neomaster --headless --role link-source --debug --debug-tcp 7155 --debug-record /var/tmp/nm-$(date +%s).jsonl
```

`--debug-record` catches the PC2 start-up and the first telegrams, which happen before any client
can connect. The file is flushed after every record, so it survives a freeze.

## The client

```sh
neomaster debug tail                          # everything but raw USB, readable
neomaster debug --tcp 192.168.2.75 tail --kinds ml_in,ml_out,ir
neomaster debug tail --all --json | jq .      # raw USB too, as JSON
neomaster debug tail --grep SOURCE_CONNECT
neomaster debug state                         # the core's state, once
neomaster debug record session.jsonl          # like --debug-record, from now on

neomaster debug pointer 40                    # move the pointer arc to 40 (0-127)
neomaster debug wheel 15                      # one list row down (15 counts per row)
neomaster debug volume -3
neomaster debug click go                      # press and release; `hold go 1500` for long GO
neomaster debug beo4 n.music                  # a Beo4 key from the IR eye; --held for held
neomaster debug telegram in  c2 c1 01 0a 7a 00 00 14 00 04   # as if received from the link
neomaster debug telegram out 80 c2 01 0a 00 00 00 11 00 01   # put it on the link
neomaster debug pc2 e4 01                     # a raw PC2 message
neomaster debug mark "list froze here"        # a note in the stream and the recording

neomaster debug replay session.jsonl          # feed its panel reports and IR keys back in
neomaster debug vectors session.jsonl kitchen_join --from 12 --to 40 > kitchen.rs
```

Telegrams can be typed with or without checksum and EOT; MLGW log lines paste as they are.
Injected input takes the same path as the real thing: a panel report goes through the panel
decoder, a Beo4 key through the role glue (so it is ignored in the link source role, as local IR
is), a telegram out through the PC2 actor's pacer.

## From a bug on the box to a test

1. Run with `--debug-record`, reproduce, and `neomaster debug mark` the moment it goes wrong.
2. `neomaster debug vectors FILE NAME --from S --to S` prints the window's telegrams as a
   `&[Row]` vector in the format of `crates/masterlink/tests/vectors` (MLGW form, `out` = sent by
   neomaster).
3. Add it to a vectors file and replay it in `crates/neomaster/src/link_tests.rs` with
   `replay(...)`, as the existing capture replays do: incoming rows are fed in, `out` rows are
   what neomaster must send.

For UI bugs, `neomaster debug replay FILE` feeds the recorded panel reports back in with their
timing, against `--sim` or the box.

## Wire protocol

JSON Lines both ways. Version 1.

Every record is an object with `t` (seconds since start-up) and `kind`, then fields:

| kind | fields |
|---|---|
| `hello` | `protocol`, `version`, `mode`, `role`, `pc2_device`, `inject`: first line on each connection and in recordings |
| `log` | `level`, `target`, `msg`: what goes to stderr |
| `panel_in` | `raw`, `menu`, `volume`, `pointer`, `buttons`: a panel IN report (real or injected) |
| `panel_input` | `input`: what the panel decoder made of it (`MenuWheel(15)`, `Pressed(Go)`, `LongPress(Go)`, ...) |
| `panel_out` | `raw`: an OUT report (click, backlight) |
| `ui_effect` | `effect`: what the UI asked for |
| `app_effect` | `effect`: what the role glue asked the player for |
| `ir` | `key_type`, `code`, `key`, `held`: a Beo4 key from the IR eye |
| `ml_in`, `ml_out` | `from`, `to`, `type`, `payload_type`, `subtype`, `src_src`, `dest_src`, `payload`, `mlgw`, `raw`, `checksum_ok` |
| `pc2_in` | `event`: any other PC2 report (mixer state, temperature, lock granted, ...) |
| `pc2_out` | `raw`: a PC2 message (mixer, lock) |
| `pc2_refused` | a telegram the PC2 would refuse, with `reason` |
| `pc2_session` | `state`: `open` with `mode`, then `Gone` or `Shutdown` |
| `usb_in`, `usb_out` | `raw`: every USB packet to and from the PC2 (left out unless asked for) |
| `core_event` | `event`: PC2 connected / gone, standby, quit |
| `inject` | `inject`: what a client injected |
| `state` | `changed` (`{field: [old, new]}`) and the full `state`: after each change, and on request |
| `timing` | `trigger` (`events` or `tick`), `events`, `handle_us`, `draw_us`, `redraw`: one per core loop turn |
| `mark` | `text` |
| `lagged` | `missed`: this connection fell behind and lost records |
| `ok`, `error` | the answer to a command (`error`, `line`) |

Commands are objects with `cmd`:

| cmd | fields |
|---|---|
| `filter` | `kinds`: list of kinds; `["all"]` everything; omitted or `["*"]` everything but `usb_*`; `[]` nothing |
| `panel` | `menu`, `volume` (counts), `pointer` (0-127): the core fills in the rest |
| `panel_raw` | `hex`: 4-6 report bytes |
| `button` | `button` (`go`, `left`, `right`), `down` |
| `beo4` | `code` (number, `"0x94"` or a name like `"n.music"`), `kind` or `held` |
| `telegram` | `hex`, `dir` (`in` or `out`) |
| `pc2` | `hex` |
| `standby`, `all_standby`, `state`, `ping` | |
| `mark` | `text` |

Without `--debug-tcp-inject`, a TCP connection may only send `filter`, `state`, `mark` and `ping`.
