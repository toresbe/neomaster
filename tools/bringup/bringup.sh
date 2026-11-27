#!/bin/bash
# BM5 hardware bring-up, one command: cross-build the i686 binaries, copy the kit to the box, run
# every test there, and pull the results back with a summary.
#
#   tools/bringup/bringup.sh [options]
#
#   --host USER@HOST     the BM5 (default toresbe@192.168.2.75, or $NM_HOST)
#   --out DIR            where results land (default ~/projects/rea-local/work/bringup, or $NM_RESULTS)
#   --tests LIST         preflight,panel,pc2,xbench,player (default: all)
#   --interactive        someone is at the BM5: walk them through the panel and a Beo4 key
#   --quick              short runs, to check the kit end to end (numbers not comparable)
#   --role ROLE          Masterlink role for the PC2 test (default link-source)
#   --xconfigs "A B"     X set-ups to benchmark (default "exa exa16 noaccel")
#   --try-viafb          also load viafb for /dev/fb0 and benchmark the fbdev paths
#   --media URI          Music Assistant item to play (player test skipped without it)
#   --media2 URI         a second item, for the skip and latency_probe measurements
#   --ma-env FILE        read MA_TOKEN=... from FILE (else $MA_TOKEN)
#   --build MODE         docker (default when docker exists), native, or skip
#   --force              run even if the box is not on bookworm or not idle
#   --dry-run            build and stage the kit, print what would run, touch nothing on the box
#
# Needs: ssh access to the box with passwordless sudo there; docker (linux/386 rust:1-bookworm)
# or a Debian cross toolchain (doc/rust-workspace.md) here.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
HOST=${NM_HOST:-toresbe@192.168.2.75}
OUT=${NM_RESULTS:-$HOME/projects/rea-local/work/bringup}
TESTS=preflight,panel,pc2,xbench,player
INTERACTIVE=0 QUICK=0 ROLE=link-source XCONFIGS="exa exa16 noaccel" VIAFB=0
MEDIA="" MEDIA2="" MA_ENV="" FORCE=0 DRY=0
BUILD=$(command -v docker >/dev/null && echo docker || echo native)

while [ $# -gt 0 ]; do
  case $1 in
    --host) HOST=$2; shift ;;
    --out) OUT=$2; shift ;;
    --tests) TESTS=$2; shift ;;
    --interactive) INTERACTIVE=1 ;;
    --quick) QUICK=1 ;;
    --role) ROLE=$2; shift ;;
    --xconfigs) XCONFIGS=$2; shift ;;
    --try-viafb) VIAFB=1 ;;
    --media) MEDIA=$2; shift ;;
    --media2) MEDIA2=$2; shift ;;
    --ma-env) MA_ENV=$2; shift ;;
    --build) BUILD=$2; shift ;;
    --force) FORCE=1 ;;
    --dry-run) DRY=1 ;;
    -h|--help) sed -n '2,/^set -e/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option $1 (--help)"; exit 2 ;;
  esac
  shift
done

STAMP=$(date +%Y%m%d-%H%M%S)
log() { printf '\033[1m== %s\033[0m\n' "$*"; }

# ------------------------------------------------------------------ build
case $BUILD in
  docker)
    log "building for i686 in docker (rust:1-bookworm, linux/386)"
    image=neomaster-i686-build
    if ! docker image inspect $image >/dev/null 2>&1; then
      printf 'FROM --platform=linux/386 rust:1-bookworm\nRUN apt-get update && apt-get install -y --no-install-recommends libasound2-dev pkg-config && rm -rf /var/lib/apt/lists/*\n' \
        | docker build --platform linux/386 -t $image -
    fi
    mkdir -p "$REPO/target/i686-docker"
    docker run --rm --platform linux/386 --user "$(id -u):$(id -g)" -v "$REPO:/src" -w /src \
      -e CARGO_HOME=/src/target/i686-docker/cargo-home -e CARGO_TARGET_DIR=/src/target/i686-docker \
      $image sh -c 'cargo build --release -p neomaster -p bringup && cargo build --release -p player --example latency_probe'
    BINDIR=$REPO/target/i686-docker/release
    ;;
  native)
    log "cross-building for i686-unknown-linux-gnu"
    (cd "$REPO" && PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_PATH_i686_unknown_linux_gnu=${PKG_CONFIG_PATH_i686_unknown_linux_gnu:-/usr/lib/i386-linux-gnu/pkgconfig} \
      CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_RUSTFLAGS=${CARGO_TARGET_I686_UNKNOWN_LINUX_GNU_RUSTFLAGS:--L /usr/lib/i386-linux-gnu} \
      cargo build --release --target i686-unknown-linux-gnu -p neomaster -p bringup \
      && cargo build --release --target i686-unknown-linux-gnu -p player --example latency_probe)
    BINDIR=$REPO/target/i686-unknown-linux-gnu/release
    ;;
  skip)
    BINDIR=${NM_BINDIR:-$REPO/target/i686-docker/release}
    ;;
  *) echo "--build docker|native|skip"; exit 2 ;;
esac

# ------------------------------------------------------------------ stage
KIT=$(mktemp -d "${TMPDIR:-/tmp}/nm-bringup.XXXXXX")
trap 'rm -rf "$KIT"' EXIT
mkdir -p "$KIT/bin"
for b in neomaster nm-xbench nm-panel-probe nm-player-bench; do
  cp "$BINDIR/$b" "$KIT/bin/"
  file -b "$KIT/bin/$b" | grep -Eq 'ELF 32-bit.*(80386|i386)' || { echo "$BINDIR/$b is not an i386 binary"; exit 1; }
done
[ -f "$BINDIR/examples/latency_probe" ] && cp "$BINDIR/examples/latency_probe" "$KIT/bin/"
cp "$HERE/box/run-on-box.sh" "$HERE/box/preflight.sh" "$HERE/box/smoke_pc2.py" "$KIT/"
(cd "$REPO" && echo "$(git describe --always --dirty) $(git rev-parse --abbrev-ref HEAD) built $(date -Is)") > "$KIT/VERSION"
log "kit: $(cat "$KIT/VERSION"), $(du -sh "$KIT" | cut -f1)"

TOKEN=${MA_TOKEN:-}
if [ -n "$MA_ENV" ]; then TOKEN=$(sed -n 's/^\(export \)\{0,1\}MA_TOKEN=["'"'"']\{0,1\}\([^"'"'"']*\).*/\2/p' "$MA_ENV" | head -1); fi
if [[ ",$TESTS," == *",player,"* ]] && { [ -z "$MEDIA" ] || [ -z "$TOKEN" ]; }; then
  echo "note: the player test needs --media and a token (MA_TOKEN or --ma-env); it will be skipped"
fi

REMOTE=nm-bringup/$STAMP
ENVS=(NM_TESTS="$TESTS" NM_INTERACTIVE=$INTERACTIVE NM_QUICK=$QUICK NM_ROLE="$ROLE" NM_XCONFIGS="$XCONFIGS"
      NM_TRY_VIAFB=$VIAFB NM_MEDIA="$MEDIA" NM_MEDIA2="$MEDIA2" NM_FORCE=$FORCE)
[ -n "${MA_URL:-}" ] && ENVS+=(MA_URL="$MA_URL")
[ -n "${SENDSPIN_URL:-}" ] && ENVS+=(SENDSPIN_URL="$SENDSPIN_URL")
remote_cmd="cd $REMOTE && sudo env $(printf '%q ' "${ENVS[@]}")./run-on-box.sh"

if [ $DRY = 1 ]; then
  log "dry run: would copy $KIT to $HOST:$REMOTE and run:"
  echo "  ssh -t $HOST $remote_cmd"
  ls -la "$KIT" "$KIT/bin"
  exit 0
fi

# ------------------------------------------------------------------ gate
log "checking $HOST"
pre=$(ssh -o BatchMode=yes "$HOST" 'sh -s' < "$HERE/box/preflight.sh" 2>/dev/null || true)
echo "$pre" | grep -E '^(debian|kernel|package_manager_running|pc2|panel|ready):'
if [ $FORCE != 1 ]; then
  echo "$pre" | grep -q '^debian: 12' || { echo "The box is not on Debian 12 (bookworm) yet; --force to run anyway."; exit 1; }
  echo "$pre" | grep -q '^ready: yes' || { echo "Preflight says the box is not ready; --force to run anyway."; exit 1; }
fi

# ------------------------------------------------------------------ deploy and run
log "copying the kit to $HOST:$REMOTE"
ssh "$HOST" "mkdir -p $REMOTE"
scp -q -r "$KIT"/* "$HOST:$REMOTE/"
if [ -n "$TOKEN" ]; then printf '%s' "$TOKEN" | ssh "$HOST" "umask 077; cat > $REMOTE/ma_token"; fi

log "running on the box (results in $REMOTE/results)"
set +e
if [ $INTERACTIVE = 1 ]; then ssh -t "$HOST" "$remote_cmd"; else ssh "$HOST" "$remote_cmd"; fi
rc=$?
set -e
ssh "$HOST" "rm -f $REMOTE/ma_token"

# ------------------------------------------------------------------ fetch
DEST=$OUT/$STAMP
mkdir -p "$DEST"
log "fetching results into $DEST"
scp -q -r "$HOST:$REMOTE/results/." "$DEST/"
cp "$KIT/VERSION" "$DEST/"
python3 "$HERE/host/report.py" "$DEST" > "$DEST/summary.md" || echo "report.py failed; raw results are in $DEST"
ln -sfn "$STAMP" "$OUT/latest"
log "done (box exit $rc): $DEST/summary.md"
exit $rc
