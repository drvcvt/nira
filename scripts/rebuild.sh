#!/usr/bin/env bash
# Rebuild the nira desktop bundle so the app-launcher icon picks up the latest
# source. The launcher itself just execs the prebuilt binary (instant launch) —
# this is the deliberate "refresh the build" step. Run it after changing code.
#
# Debug build on purpose: the workspace dev profile already compiles every
# dependency (rodio/symphonia/dioxus/librespot) at opt-level 3, so audio is
# fine and incremental rebuilds stay fast — only nira's own crates are debug.
set -uo pipefail

PROJECT="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
export PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin:$PATH"
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

cd "$PROJECT" || exit 1
echo "Building nira desktop bundle (debug)…"
if dx build --desktop --package nira; then
    # dx's asset cache sometimes ships a stale css file into the bundle
    # (seen after a failed build in between). Verify and repair by hand —
    # one hashed bundle file per source file under nira/assets/css/.
    for src in nira/assets/css/*.css; do
        stem="$(basename "$src" .css)"
        for bundled in target/dx/nira/debug/linux/app/assets/"$stem"-*.css; do
            if [ -f "$bundled" ] && ! cmp -s "$src" "$bundled"; then
                cp "$src" "$bundled"
                echo "Repaired stale bundle CSS: $stem"
            fi
        done
    done
    notify-send -a nira "nira" "Rebuilt — launcher is up to date." 2>/dev/null || true
    echo "Done. Launch nira from your app menu (or rerun to refresh)."
else
    notify-send -u critical -a nira "nira: rebuild failed" "See terminal output." 2>/dev/null || true
    echo "Build failed." >&2
    exit 1
fi
