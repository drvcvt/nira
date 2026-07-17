#!/bin/sh
set -eu

profile=$1
shift

case "$profile" in
  debug) ;;
  release) set -- --release "$@" ;;
  *) echo "unknown desktop profile: $profile" >&2; exit 2 ;;
esac

dx build --desktop -p nira "$@"

xdotool_bin=$(readlink -f "$(command -v xdotool)")
xdotool_root=$(dirname "$(dirname "$xdotool_bin")")
lib_dir="target/dx/nira/$profile/linux/lib"
mkdir -p "$lib_dir"
cp "$xdotool_root/lib/libxdo.so.3" "$lib_dir/libxdo.so.3"
