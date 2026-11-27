#!/bin/bash
# The bring-up run, on the BM5. host/bringup.sh copies the kit here and starts this as root;
# it can also be started by hand from the kit directory:
#
#   sudo NM_INTERACTIVE=1 ./run-on-box.sh
#
# Settings (environment):
#   NM_TESTS        preflight,panel,pc2,xbench,player   (default: all)
#   NM_INTERACTIVE  1: prompt the person at the BM5 (panel walk-through, Beo4 key); 0: no prompts
#   NM_QUICK        1: short runs, to check the kit works (numbers not comparable)
#   NM_ROLE         neomaster's Masterlink role for the PC2 test (default link-source)
#   NM_XCONFIGS     X server set-ups to benchmark (default: exa exa16 noaccel)
#   NM_TRY_VIAFB    1: also load viafb for /dev/fb0 and benchmark the fbdev paths (unload after)
#   NM_SECONDS      length of the 60 fps loops and of steady playback (default 15 / 60)
#   NM_MEDIA, NM_MEDIA2  Music Assistant URIs to play (player test is skipped without NM_MEDIA)
#   MA_URL, SENDSPIN_URL  Music Assistant; the token is read from ./ma_token (deleted after)
#   NM_OUT          results directory (default ./results)
#   NM_FORCE        1: run even if preflight says the box is not ready
set -u
KIT=$(cd "$(dirname "$0")" && pwd)
BIN=$KIT/bin
OUT=${NM_OUT:-$KIT/results}
TESTS=${NM_TESTS:-preflight,panel,pc2,xbench,player}
INTERACTIVE=${NM_INTERACTIVE:-0}
QUICK=${NM_QUICK:-0}
ROLE=${NM_ROLE:-link-source}
XCONFIGS=${NM_XCONFIGS:-exa exa16 noaccel}
LOOP_S=${NM_SECONDS:-15}
PLAY_S=${NM_PLAY_SECONDS:-60}
DISPLAY_NUM=9
export PATH=$PATH:/usr/sbin:/sbin
mkdir -p "$OUT"
exec > >(tee -a "$OUT/run.log") 2>&1

has() { case ",$TESTS," in *",$1,"*) return 0 ;; esac; return 1; }
step() { printf '\n========== %s  (%s)\n' "$*" "$(date +%T)"; }
quick_flag=""
if [ "$QUICK" = 1 ]; then quick_flag="--quick"; LOOP_S=3; PLAY_S=15; fi

if [ "$(id -u)" != 0 ]; then echo "run-on-box.sh: run as root (USB devices, Xorg)"; exit 1; fi

MA_TOKEN=""
if [ -f "$KIT/ma_token" ]; then MA_TOKEN=$(cat "$KIT/ma_token"); rm -f "$KIT/ma_token"; fi
export MA_TOKEN

XPID=""
cleanup() {
  if [ -n "$XPID" ] && kill -0 "$XPID" 2>/dev/null; then kill "$XPID"; wait "$XPID" 2>/dev/null; fi
  pkill -f "$BIN/neomaster --headless" 2>/dev/null
  if [ -n "${SUDO_USER:-}" ]; then chown -R "$SUDO_USER" "$OUT" 2>/dev/null; fi
}
trap cleanup EXIT

step "kit $(cat "$KIT/VERSION" 2>/dev/null)"
uname -a
date -Is

if has preflight; then
  step preflight
  sh "$KIT/preflight.sh" | tee "$OUT/preflight.txt"
  if ! grep -q '^ready: yes' "$OUT/preflight.txt" && [ "${NM_FORCE:-0}" != 1 ]; then
    echo "Preflight says the box is not ready (see above); NM_FORCE=1 runs anyway."
    exit 1
  fi
fi

# ------------------------------------------------------------------ panel
if has panel; then
  step "panel probe (USB 0cd4:1112)"
  if [ "$INTERACTIVE" = 1 ]; then
    "$BIN/nm-panel-probe" --record "$OUT/panel-record.jsonl" --summary "$OUT/panel-summary.json" < /dev/tty
  else
    "$BIN/nm-panel-probe" --unattended 10 --record "$OUT/panel-record.jsonl" --summary "$OUT/panel-summary.json"
  fi
  echo "panel probe exit $?"
fi

# ------------------------------------------------------------------ PC2
if has pc2; then
  step "PC2 smoke test over the debug connection (role $ROLE)"
  args=(--neomaster "$BIN/neomaster" --out "$OUT/pc2" --role "$ROLE" --listen "$( [ "$QUICK" = 1 ] && echo 8 || echo 30)")
  [ "$INTERACTIVE" = 1 ] && args+=(--interactive)
  [ -s "$OUT/panel-record.jsonl" ] && args+=(--panel-recording "$OUT/panel-record.jsonl")
  if [ "$INTERACTIVE" = 1 ]; then python3 "$KIT/smoke_pc2.py" "${args[@]}" < /dev/tty; else python3 "$KIT/smoke_pc2.py" "${args[@]}"; fi
  echo "PC2 smoke exit $?"
fi

# ------------------------------------------------------------------ X
xconf() { # name driver depth extra-device-options
  cat <<EOF
# nm-bringup: $1
Section "ServerFlags"
    Option "AutoAddDevices" "false"
    Option "AutoEnableDevices" "false"
    Option "BlankTime" "0"
    Option "StandbyTime" "0"
    Option "SuspendTime" "0"
    Option "OffTime" "0"
EndSection
Section "Device"
    Identifier "card"
    Driver "$2"
$4
EndSection
Section "Screen"
    Identifier "screen"
    Device "card"
    DefaultDepth $3
    SubSection "Display"
        Depth $3
        Modes "1024x768"
    EndSubSection
EndSection
Section "ServerLayout"
    Identifier "layout"
    Screen "screen"
EndSection
EOF
}

start_x() { # name
  local name=$1 conf="$OUT/xorg-$1.conf"
  case $name in
    exa) xconf "$name" openchrome 24 '    Option "AccelMethod" "EXA"' > "$conf" ;;
    exa16) xconf "$name" openchrome 16 '    Option "AccelMethod" "EXA"' > "$conf" ;;
    noaccel) xconf "$name" openchrome 24 '    Option "NoAccel" "true"' > "$conf" ;;
    fbdev) xconf "$name" fbdev 24 '    Option "fbdev" "/dev/fb0"' > "$conf" ;;
    *) echo "unknown X config $name"; return 1 ;;
  esac
  mkdir -p "$KIT/xorg.conf.d.empty"
  rm -f /tmp/.X11-unix/X$DISPLAY_NUM /tmp/.X$DISPLAY_NUM-lock
  Xorg :$DISPLAY_NUM vt8 -config "$conf" -configdir "$KIT/xorg.conf.d.empty" -logfile "$OUT/Xorg-$name.log" \
    -nolisten tcp -noreset -s 0 -dpms > "$OUT/Xorg-$name.stderr" 2>&1 &
  XPID=$!
  for _ in $(seq 1 100); do
    [ -S /tmp/.X11-unix/X$DISPLAY_NUM ] && break
    kill -0 $XPID 2>/dev/null || break
    sleep 0.2
  done
  if ! kill -0 $XPID 2>/dev/null || [ ! -S /tmp/.X11-unix/X$DISPLAY_NUM ]; then
    echo "Xorg ($name) did not start; tail of its log:"; tail -20 "$OUT/Xorg-$name.log"; XPID=""; return 1
  fi
  sleep 1
  export DISPLAY=:$DISPLAY_NUM
  grep -E 'VideoRAM|acceleration architecture|Offscreen pixmap|NoAccel|Depth|EXA\(0\)' "$OUT/Xorg-$name.log" | head -12
}

stop_x() {
  if [ -n "$XPID" ]; then kill "$XPID" 2>/dev/null; wait "$XPID" 2>/dev/null; XPID=""; fi
  sleep 1
}

xbench() { # label backend [extra args]
  local label=$1 backend=$2; shift 2
  echo "--- nm-xbench $backend under $label"
  "$BIN/nm-xbench" --backend "$backend" --label "$label" --seconds "$LOOP_S" --json "$OUT/xbench.jsonl" $quick_flag "$@" \
    | tee "$OUT/xbench-$label-$backend.txt"
}

player_args() {
  local a=(--media "$NM_MEDIA" --json "$OUT/player.jsonl")
  [ -n "${NM_MEDIA2:-}" ] && a+=(--media2 "$NM_MEDIA2")
  printf '%s\n' "${a[@]}"
}

if has player && [ -n "${NM_MEDIA:-}" ] && [ -n "$MA_TOKEN" ]; then
  step "Sendspin player alone (ALSA default)"
  mapfile -t pa < <(player_args)
  "$BIN/nm-player-bench" "${pa[@]}" --label alone --seconds "$PLAY_S" --trials "$( [ "$QUICK" = 1 ] && echo 1 || echo 3)" | tee "$OUT/player-alone.txt"
  if [ -x "$BIN/latency_probe" ] && [ -n "${NM_MEDIA2:-}" ]; then
    step "Sendspin scheduled-sound latency (latency_probe, silent)"
    "$BIN/latency_probe" "$NM_MEDIA" "$NM_MEDIA2" "$( [ "$QUICK" = 1 ] && echo 2 || echo 5)" 200 | tee "$OUT/latency-probe-200.txt"
  fi
elif has player; then
  step "Sendspin player: skipped (needs NM_MEDIA and a Music Assistant token)"
fi

if has xbench; then
  configs=$XCONFIGS
  if [ "${NM_TRY_VIAFB:-0}" = 1 ]; then configs="$configs viafb"; fi
  for cfg in $configs; do
    if [ "$cfg" = viafb ]; then
      step "fbdev paths (viafb)"
      modprobe viafb && sleep 2
      if [ -e /dev/fb0 ]; then
        cat /sys/class/graphics/fb0/name /sys/class/graphics/fb0/virtual_size /sys/class/graphics/fb0/bits_per_pixel 2>/dev/null
        xbench console sw-fbdev
        if start_x fbdev; then xbench fbdev xrender --tests sync,present,covers,fades,fills,glyphs,strip60,full60; xbench fbdev sw-x11; stop_x; fi
      else
        echo "viafb gave no /dev/fb0"
      fi
      modprobe -r viafb 2>/dev/null
      continue
    fi
    step "X server: $cfg"
    start_x "$cfg" || continue
    if [ "$cfg" = noaccel ]; then
      xbench "$cfg" xrender --tests sync,present,covers,fades,fills,glyphs,upload,strip60,full60,ui
    else
      xbench "$cfg" xrender
    fi
    xbench "$cfg" sw-x11 --tests sync,present,covers,fades,fills,glyphs,strip60,full60
    if [ "$cfg" = exa ] && has player && [ -n "${NM_MEDIA:-}" ] && [ -n "$MA_TOKEN" ]; then
      step "Sendspin player while the cover strip animates at 60 fps (XRender, exa)"
      mapfile -t pa < <(player_args)
      "$BIN/nm-player-bench" "${pa[@]}" --label "under-ui-load" --seconds "$PLAY_S" --trials 1 > "$OUT/player-under-load.txt" 2>&1 &
      ppid=$!
      "$BIN/nm-xbench" --backend xrender --label "exa+player" --tests strip60,full60 --seconds "$((PLAY_S / 2 + 15))" --json "$OUT/xbench.jsonl" \
        | tee "$OUT/xbench-exa+player.txt"
      wait $ppid; cat "$OUT/player-under-load.txt"
    fi
    stop_x
  done
fi

# CPU-only cost of the software blitter (no presenting), for reference
if has xbench; then
  step "software blitter, CPU only"
  xbench cpu sw-cpu --tests covers,fades,fills,glyphs,strip60,full60
fi

step done
ls -la "$OUT"
