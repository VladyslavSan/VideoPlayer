#!/usr/bin/env bash
# Builds "Video Player.app" (universal: Apple Silicon and Intel) into
# target/bundle, with libmpv in Contents/Frameworks, and zips it.
#   scripts/bundle-macos.sh [version]      version defaults to Cargo.toml's
# Needs scripts/fetch-libmpv-macos.sh first, and both Rust targets:
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
# The app is signed ad hoc (no Apple developer certificate), so the first
# launch of a downloaded copy needs right-click > Open, or:
#   xattr -dr com.apple.quarantine "/Applications/Video Player.app"
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
version="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
mpv="$root/third_party/mpv"
out="$root/target/bundle"
app="$out/Video Player.app"

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --target "$target"
done

rm -rf "$out"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks" "$app/Contents/Resources"
exe="$app/Contents/MacOS/video-player"
lipo -create -output "$exe" \
  target/aarch64-apple-darwin/release/video-player \
  target/x86_64-apple-darwin/release/video-player
# Development builds also look for libmpv in third_party/mpv; the app doesn't.
install_name_tool -delete_rpath "$mpv" "$exe"
cp "$mpv"/*.dylib "$app/Contents/Frameworks/"

# Icon: assets/app.png (1024 px) at every size macOS asks for.
iconset="$out/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" assets/app.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" assets/app.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
rm -rf "$iconset"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Video Player</string>
  <key>CFBundleDisplayName</key><string>Video Player</string>
  <key>CFBundleIdentifier</key><string>io.github.vladyslavsan.videoplayer</string>
  <key>CFBundleExecutable</key><string>video-player</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.entertainment</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSLocalNetworkUsageDescription</key>
  <string>Video Player looks for Jellyfin servers on your network.</string>
</dict>
</plist>
EOF

# Ad hoc signatures: required on Apple Silicon, and install_name_tool
# invalidated the executable's.
for lib in "$app/Contents/Frameworks"/*.dylib; do
  codesign --force --sign - "$lib"
done
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"

zip="$out/VideoPlayer-$version-macos-universal.zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
echo "Built $app"
echo "Zipped $zip"
