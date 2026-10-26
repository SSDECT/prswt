#!/usr/bin/env bash
# DSK-08: builds and packages the PRSWT desktop app for the platform it runs on.
#
#   tools/package-desktop.sh [TARGET]
#
# Output in dist/ (names carry no version, so the README's
# releases/latest/download/<name> links always fetch the newest release):
#   Windows  PRSWT-windows-x64.zip                (prswt.exe)
#   Linux    PRSWT-linux-x64.tar.gz               (prswt + prswt.desktop)
#   macOS    PRSWT-macos-apple-silicon.dmg / PRSWT-macos-intel.dmg (PRSWT.app)
# With TARGET (a Rust target triple) it cross-builds that target instead of
# the host's, e.g. x86_64-apple-darwin on an Apple Silicon Mac.
#
# Signing (docs/signing.md):
#   PRSWT_PREBUILT=1        package the binary already in target/ (Windows:
#                           the release workflow signs it there first)
#   MACOS_SIGN_IDENTITY     "Developer ID Application: …" in the keychain;
#                           signs PRSWT.app (hardened runtime) and the .dmg
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD
#                           also notarize the .dmg and staple the ticket
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' engine/Cargo.toml | head -1)
TARGET=${1:-}
mkdir -p dist

build() {
  if [ "${PRSWT_PREBUILT:-}" = 1 ]; then
    echo "engine/target/${TARGET:+$TARGET/}release"
  elif [ -n "$TARGET" ]; then
    (cd engine && cargo build --release --locked -p prswt-desktop --target "$TARGET")
    echo "engine/target/$TARGET/release"
  else
    (cd engine && cargo build --release --locked -p prswt-desktop)
    echo "engine/target/release"
  fi
}

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    OUT=$(build | tail -1)
    NAME="PRSWT-windows-x64"
    rm -rf "dist/$NAME" && mkdir -p "dist/$NAME"
    cp "$OUT/prswt.exe" "dist/$NAME/"
    cp README.md "dist/$NAME/README.md" 2>/dev/null || true
    (cd dist && rm -f "$NAME.zip" && powershell -NoProfile -Command "Compress-Archive -Path '$NAME/*' -DestinationPath '$NAME.zip'")
    rm -rf "dist/$NAME"
    echo "dist/$NAME.zip"
    ;;
  Linux)
    OUT=$(build | tail -1)
    case "$(uname -m)" in x86_64) ARCH=x64 ;; *) ARCH=$(uname -m) ;; esac
    NAME="PRSWT-linux-$ARCH"
    rm -rf "dist/$NAME" && mkdir -p "dist/$NAME"
    cp "$OUT/prswt" "dist/$NAME/"
    cat > "dist/$NAME/prswt.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=PRSWT
Comment=Project scheduling and risk analysis
Exec=prswt %f
Terminal=false
Categories=Office;ProjectManagement;
MimeType=application/x-prswt;
EOF
    tar -C dist -czf "dist/$NAME.tar.gz" "$NAME"
    rm -rf "dist/$NAME"
    echo "dist/$NAME.tar.gz"
    ;;
  Darwin)
    OUT=$(build | tail -1)
    ARCH=${TARGET%%-*}
    ARCH=${ARCH:-$(uname -m)}
    APP="dist/PRSWT.app"
    rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
    cp "$OUT/prswt" "$APP/Contents/MacOS/prswt"
    cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>PRSWT</string>
  <key>CFBundleDisplayName</key><string>PRSWT</string>
  <key>CFBundleIdentifier</key><string>ai.juz.prswt</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleExecutable</key><string>prswt</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleDocumentTypes</key><array><dict>
    <key>CFBundleTypeName</key><string>PRSWT project</string>
    <key>CFBundleTypeExtensions</key><array><string>prswt</string></array>
    <key>CFBundleTypeRole</key><string>Editor</string>
  </dict></array>
</dict></plist>
EOF
    case "$ARCH" in arm64|aarch64) MAC=apple-silicon ;; *) MAC=intel ;; esac
    NAME="PRSWT-macos-$MAC"
    if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
      codesign --force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$APP"
      codesign --verify --strict --verbose=2 "$APP"
    fi
    rm -f "dist/$NAME.dmg"
    hdiutil create -volname PRSWT -srcfolder "$APP" -ov -format UDZO "dist/$NAME.dmg" >/dev/null
    rm -rf "$APP"
    if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
      codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "dist/$NAME.dmg"
      if [ -n "${APPLE_ID:-}" ]; then
        xcrun notarytool submit "dist/$NAME.dmg" --apple-id "$APPLE_ID"           --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD" --wait
        xcrun stapler staple "dist/$NAME.dmg"
      fi
    fi
    echo "dist/$NAME.dmg"
    ;;
  *)
    echo "unsupported OS $(uname -s)" >&2
    exit 1
    ;;
esac
