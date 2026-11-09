#!/usr/bin/env bash
# Downloads a pinned universal (arm64 + x86_64) libmpv build for macOS into
# third_party/mpv. It comes from media-kit's libmpv-darwin-build: mpv with
# OpenGL rendering and VideoToolbox decoding, all dylibs using @rpath.
set -euo pipefail

VERSION=v0.7.3
NAME=libmpv-libs_${VERSION}_macos-universal-video-full
SHA256=f570695d8ba8534835dd4cba0b6e275579c1c22c01a9da0442748ca37e2daac1

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/third_party/mpv"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/$NAME.tar.gz" \
  "https://github.com/media-kit/libmpv-darwin-build/releases/download/$VERSION/$NAME.tar.gz"
echo "$SHA256  $tmp/$NAME.tar.gz" | shasum -a 256 -c -

mkdir -p "$out"
tar -xzf "$tmp/$NAME.tar.gz" -C "$tmp"
cp "$tmp/$NAME"/*.dylib "$out/"
echo "libmpv ready in $out"
