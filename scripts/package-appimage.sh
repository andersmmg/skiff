#!/usr/bin/env bash
# Package Skiff as an AppImage.
#
# Produces target/appimage/Skiff-<version>-<arch>.AppImage and leaves the
# AppDir around for inspection. When an aria2-next engine is available it is
# bundled next to the binary — exactly where the engine's discovery looks
# (settings binary > SKIFF_ARIA2_NEXT > PATH > beside the executable), so a
# fresh user gets a self-contained download manager.
#
# Requires: cargo, appimagetool (https://github.com/AppImage/appimagetool).
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if ! command -v appimagetool >/dev/null; then
    echo "package-appimage: appimagetool not found on PATH" >&2
    echo "  install it from https://github.com/AppImage/appimagetool/releases" >&2
    exit 1
fi

# The engine: bin/ is git-ignored, so prefer a checked-out copy, then PATH.
if [ -f bin/aria2-next ]; then
    engine=bin/aria2-next
elif command -v aria2-next >/dev/null; then
    engine=$(command -v aria2-next)
else
    engine=
fi
if [ -z "$engine" ]; then
    echo "WARNING: aria2-next not found (bin/aria2-next or PATH);" >&2
    echo "         the image will not be self-contained — it needs aria2-next" >&2
    echo "         at runtime, or an engine binary path in Skiff's settings." >&2
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
arch=$(uname -m)
app=target/appimage
appdir="$app/Skiff.AppDir"
out="$app/Skiff-$version-$arch.AppImage"

echo "==> cargo build --release --package skiff"
cargo build --release --package skiff

echo "==> assembling $appdir"
rm -rf "$appdir"
mkdir -p \
    "$appdir/usr/bin" \
    "$appdir/usr/share/applications" \
    "$appdir/usr/share/icons/hicolor/scalable/apps" \
    "$appdir/usr/share/icons/hicolor/256x256/apps"

cp target/release/skiff "$appdir/usr/bin/skiff"
if [ -n "$engine" ]; then
    echo "==> bundling engine: $engine"
    cp "$engine" "$appdir/usr/bin/aria2-next"
fi
# Relative symlink: the AppImage runtime mounts the AppDir and runs AppRun,
# which then resolves inside the mount.
ln -s usr/bin/skiff "$appdir/AppRun"

cp packaging/skiff.desktop "$appdir/skiff.desktop"
cp packaging/skiff.desktop "$appdir/usr/share/applications/skiff.desktop"
cp packaging/skiff.svg "$appdir/skiff.svg"
cp packaging/skiff.svg "$appdir/usr/share/icons/hicolor/scalable/apps/skiff.svg"
cp packaging/skiff.png "$appdir/skiff.png"
cp packaging/skiff.png "$appdir/usr/share/icons/hicolor/256x256/apps/skiff.png"

echo "==> appimagetool $appdir $out"
rm -f "$out"
ARCH="$arch" appimagetool "$appdir" "$out"

echo "==> done"
ls -lh "$out"
