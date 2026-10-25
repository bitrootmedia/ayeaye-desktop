#!/bin/sh
# Builds dist/Ayeaye.app from a release build for this Mac's architecture.
# Ad-hoc signed —
# fine for this machine; another Mac needs one trip past Gatekeeper (see the
# menu bar app's README for the two ways).
set -eu
cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP=dist/Ayeaye.app

cargo build --release
BIN=target/release/ayeaye

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/ayeaye"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Ayeaye</string>
  <key>CFBundleDisplayName</key><string>Ayeaye</string>
  <key>CFBundleIdentifier</key><string>com.bitroot.ayeaye-desktop</string>
  <key>CFBundleExecutable</key><string>ayeaye</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <!-- Self-hosted instances on plain http://localhost are a normal case. -->
  <key>NSAppTransportSecurity</key><dict><key>NSAllowsArbitraryLoads</key><true/></dict>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$APP"
echo "built $APP"
