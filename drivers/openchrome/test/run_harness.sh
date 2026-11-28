#!/bin/sh
# Usage: run_harness.sh <unpatched openchrome-0.6.0 tree> <patched tree>
# Needs xserver-xorg-dev and libpixman-1-dev. Both trees must have been
# configured (for config.h).
set -e
OLD=$1; NEW=$2; HERE=$(cd "$(dirname "$0")" && pwd); T=$(mktemp -d)
CF="$(pkg-config --cflags xorg-server pixman-1) -include stdlib.h"
gcc -O1 $CF -I"$OLD" -I"$OLD/src" "$HERE/emit_quad_harness.c" "$OLD/src/via_3d.c" -o "$T/old" -lm
gcc -O1 -DNEW $CF -I"$NEW" -I"$NEW/src" "$HERE/emit_quad_harness.c" "$NEW/src/via_3d.c" -o "$T/new" -lm
for n in 0 1 2; do
    "$T/old" $n > "$T/o$n"; "$T/new" $n > "$T/n$n"
    cmp -s "$T/o$n" "$T/n$n" && echo "untransformed, $n textures: identical" || { echo "untransformed, $n textures: DIFFERS"; exit 1; }
done
# Scale by 2 plus a 10 px offset, bilinear: corner (16,32) must map to
# (42,64), i.e. s=42/512=0.0820312, t=64/256=0.25, and HTXnTB must be 0x2490.
"$T/new" 1 1 1 0 2 > "$T/s"
grep -q '^78002490' "$T/s" && echo "bilinear filter bits: ok" || { echo "bilinear filter bits: WRONG"; exit 1; }
grep -q 'f=0.0820312' "$T/s" && grep -q 'f=0.25$' "$T/s" && echo "scaled texture coordinates: ok" || { echo "scaled texture coordinates: WRONG"; exit 1; }
# Clipping of transformed pictures without repeat.
( cd "$T" && python3 "$HERE/check_clip.py" )
rm -rf "$T"
