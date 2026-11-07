#!/usr/bin/env bash
# Downloads a pinned libmpv build into third_party/mpv and generates the MSVC
# import library, which the upstream package doesn't include. Run from Git Bash.
set -euo pipefail

TAG=20261106
NAME=mpv-dev-x86_64-20261106-git-9028187538.7z
SHA256=b5a3cc91b5dabf88ad5bb21d000834da4fd0bbda30f4584d39c082127bd62414

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/third_party/mpv"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/$NAME" \
  "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/$TAG/$NAME"
echo "$SHA256 *$tmp/$NAME" | sha256sum -c -

mkdir -p "$out" "$tmp/x"
/c/Windows/System32/tar.exe -xf "$tmp/$NAME" -C "$tmp/x"
cp "$tmp/x/libmpv-2.dll" "$out/"
cp -r "$tmp/x/include" "$out/"

vswhere="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
vs="$("$vswhere" -latest -property installationPath | tr -d '\r')"
ver="$(tr -d '\r\n ' < "$vs/VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt")"
bin="$vs/VC/Tools/MSVC/$ver/bin/HostX64/x64"

{
  echo "LIBRARY libmpv-2.dll"
  echo "EXPORTS"
  "$bin/dumpbin.exe" -nologo -exports "$out/libmpv-2.dll" \
    | tr -d '\r' | awk '$4 ~ /^mpv_/ { print $4 }'
} > "$tmp/mpv.def"

MSYS2_ARG_CONV_EXCL='*' "$bin/lib.exe" -nologo -machine:x64 \
  -def:"$(cygpath -w "$tmp/mpv.def")" -out:"$(cygpath -w "$out/mpv.lib")"
rm -f "$out/mpv.exp"
echo "libmpv ready in $out"
