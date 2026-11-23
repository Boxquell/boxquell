#!/usr/bin/env bash
# Build the boxquell base image: Debian rootfs as a raw ext4 file.
# Also writes <output>.vmlinuz and <output>.initrd for the QEMU backend.
# usage: image/build.sh <output.img> [size, default 4G]
set -euo pipefail
out=${1:?usage: build.sh <output.img> [size]}
size=${2:-4G}
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
cid=
trap 'rm -rf "$tmp"; [ -z "$cid" ] || docker rm -f "$cid" >/dev/null 2>&1 || true' EXIT

docker build -q -t boxquell-base "$here" >/dev/null
cid=$(docker create boxquell-base)
docker export "$cid" -o "$tmp/rootfs.tar"
# mke2fs >= 1.47.1 reads a tarball directly, which keeps file ownership without root.
mkdir -p "$(dirname "$out")"
rm -f "$out"
mke2fs -q -t ext4 -L boxquell-root -d "$tmp/rootfs.tar" "$out" "$size"
tar -xf "$tmp/rootfs.tar" -C "$tmp" --wildcards 'boot/vmlinuz-*' 'boot/initrd.img-*'
cp "$tmp"/boot/vmlinuz-* "${out%.img}.vmlinuz"
cp "$tmp"/boot/initrd.img-* "${out%.img}.initrd"
echo "wrote $out"
