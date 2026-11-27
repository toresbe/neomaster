#!/bin/sh
# Read-only look at the BM5: what the bring-up run needs and what it will run on.
# Changes nothing. Prints "key: value" lines; exits 1 if something the run needs is missing.
#   sh preflight.sh            # as any user; some lines need root to be complete
PATH=$PATH:/usr/sbin:/sbin
missing=0
say() { printf '%s: %s\n' "$1" "$2"; }
need() { if [ -z "$2" ]; then say "$1" "MISSING"; missing=1; else say "$1" "$2"; fi; }

say host "$(hostname)"
say debian "$(cat /etc/debian_version 2>/dev/null) ($(. /etc/os-release 2>/dev/null; echo "$VERSION_CODENAME"))"
say kernel "$(uname -r) $(uname -m)"
say glibc "$(ldd --version 2>/dev/null | head -1 | awk '{print $NF}')"
say cpu "$(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ //') ($(grep -c ^processor /proc/cpuinfo) cpu)"
say cpu_mhz "$(grep -m1 'cpu MHz' /proc/cpuinfo | cut -d: -f2- | sed 's/^ //')"
say governor "$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo none)"
say mem_mb "$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo) total, $(awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo) available"
say load "$(cut -d' ' -f1-3 /proc/loadavg)"
say disk_free "$(df -h "${HOME:-/}" | awk 'NR==2 {print $4 " free on " $6}')"

# Is anything else busy? A half-finished upgrade makes every number wrong.
busy=$(ps -eo comm= | grep -E '^(apt|apt-get|dpkg|unattended-upgr)' | sort -u | tr '\n' ' ')
say package_manager_running "${busy:-no}"
[ -n "$busy" ] && missing=1
say xorg_running "$(pgrep -a Xorg 2>/dev/null | head -1 || echo no)"
say neomaster_running "$(pgrep -a neomaster 2>/dev/null | head -1 || echo no)"

# Graphics
say gpu "$(lspci -nn 2>/dev/null | grep -i vga | cut -d' ' -f2-)"
need xorg "$(command -v Xorg)"
say xserver "$(dpkg-query -W -f='${Version}' xserver-xorg-core 2>/dev/null)"
need openchrome "$(dpkg-query -W -f='${Version}' xserver-xorg-video-openchrome 2>/dev/null)"
say fbdev_driver "$(dpkg-query -W -f='${Version}' xserver-xorg-video-fbdev 2>/dev/null || echo none)"
say dev_fb "$(ls /dev/fb* 2>/dev/null | tr '\n' ' ' || true)"
say dev_dri "$(ls /dev/dri 2>/dev/null | tr '\n' ' ' || true)"
say viafb_module "$(modinfo -n viafb 2>/dev/null || echo none)"
log=$(ls -t /var/log/Xorg.*.log 2>/dev/null | head -1)
if [ -n "$log" ]; then
  say xorg_log "$log"
  say videoram "$(grep -m1 -o 'VideoRAM = [0-9]* kB' "$log")"
  say exa_offscreen "$(grep -m1 -o 'Offscreen pixmap area of [0-9]* bytes' "$log")"
  say accel "$(grep -m1 -o 'Using [A-Z]* acceleration architecture' "$log")"
fi

# USB: the PC2, the panel, the A.AUX codec
usb=$(lsusb 2>/dev/null)
need pc2 "$(echo "$usb" | grep -i '0cd4:0101')"
need panel "$(echo "$usb" | grep -i '0cd4:1112')"
say aux_codec "$(echo "$usb" | grep -i '08bb:2902' || echo none)"
say panel_driver "$(for d in /sys/bus/usb/devices/*; do [ "$(cat "$d/idProduct" 2>/dev/null)" = 1112 ] && ls "$d"/*:1.0/driver -l 2>/dev/null | sed 's/.*drivers\///'; done | head -1)"

# Audio
say alsa_cards "$(sed -n 's/^ *\([0-9]*\) \[\([^]]*\)\].*/\1:\2/p' /proc/asound/cards 2>/dev/null | tr -s ' ' | tr '\n' ' ')"
say alsa_default "$(aplay -L 2>/dev/null | grep -m1 -E '^default' || echo '(no aplay)')"

# Tools the run uses
need python3 "$(command -v python3)"
need sudo_or_root "$( [ "$(id -u)" = 0 ] && echo root || (sudo -n true 2>/dev/null && echo 'sudo (no password)') )"
say perf "$(command -v perf || echo none)"

# Network: Music Assistant
ma=${MA_URL:-http://192.168.2.238:8095}
host=$(echo "$ma" | sed -E 's#^[a-z]+://([^:/]+).*#\1#')
say ma_ping "$(ping -c1 -W2 "$host" >/dev/null 2>&1 && echo "$host reachable" || echo "$host NOT reachable")"

say ready "$( [ $missing = 0 ] && echo yes || echo NO)"
exit $missing
