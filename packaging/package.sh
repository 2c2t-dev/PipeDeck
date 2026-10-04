#!/bin/bash
# Builds one of Pipedeck's packages into a folder, `dist` unless told:
#
#     packaging/package.sh deb|rpm|appimage [folder]
#
# Run on the system the package is for, its build dependencies installed:
# the release workflow runs it in Debian 13 for the .deb and the AppImage,
# and in Fedora 42 for the .rpm, the first releases with the GTK 4.18 and
# libadwaita 1.7 Pipedeck needs.
set -euo pipefail

kind=${1:?deb, rpm or appimage}
root=$(cd "$(dirname "$0")/.." && pwd)
out=$(mkdir -p "${2:-$root/dist}" && cd "${2:-$root/dist}" && pwd)
target=${CARGO_TARGET_DIR:-$root/target}
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)
data=$root/crates/pipedeck/data

cd "$root"
cargo build --release --locked -p pipedeck -p pipedeck-opendeck

case $kind in
deb)
    command -v cargo-deb >/dev/null || cargo install --locked cargo-deb
    cargo deb -p pipedeck --no-build --locked --output "$out"
    ;;

rpm)
    command -v cargo-generate-rpm >/dev/null || cargo install --locked cargo-generate-rpm
    strip "$target/release/pipedeck" "$target/release/pipedeck-opendeck"
    cargo generate-rpm -p crates/pipedeck --target-dir "$target" --output "$out"
    ;;

appimage)
    appdir=$target/AppDir
    tools=$target/appimage-tools
    rm -rf "$appdir"
    install -Dm755 "$target/release/pipedeck" "$appdir/usr/bin/pipedeck"
    install -Dm755 "$target/release/pipedeck-opendeck" "$appdir/usr/bin/pipedeck-opendeck"
    install -Dm644 "$data/dev._2c2t.Pipedeck.desktop" \
        "$appdir/usr/share/applications/dev._2c2t.Pipedeck.desktop"
    install -Dm644 "$data/dev._2c2t.Pipedeck.svg" \
        "$appdir/usr/share/icons/hicolor/scalable/apps/dev._2c2t.Pipedeck.svg"
    install -Dm644 "$data/dev._2c2t.Pipedeck-symbolic.svg" \
        "$appdir/usr/share/icons/hicolor/symbolic/apps/dev._2c2t.Pipedeck-symbolic.svg"
    for size in 16 24 32 48 64 96 128; do
        install -Dm644 "$data/icons/$size.png" \
            "$appdir/usr/share/icons/hicolor/${size}x${size}/apps/dev._2c2t.Pipedeck.png"
    done

    mkdir -p "$tools"
    [ -x "$tools/linuxdeploy" ] || curl -fsSL -o "$tools/linuxdeploy" \
        https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
    [ -x "$tools/linuxdeploy-plugin-gtk.sh" ] || curl -fsSL -o "$tools/linuxdeploy-plugin-gtk.sh" \
        https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh
    chmod +x "$tools/linuxdeploy" "$tools/linuxdeploy-plugin-gtk.sh"
    export PATH=$tools:$PATH DEPLOY_GTK_VERSION=4 APPIMAGE_EXTRACT_AND_RUN=1

    # PipeWire's client library stays the system's: it loads the system's
    # own plug-ins and speaks to the system's server.
    linuxdeploy --appdir "$appdir" --plugin gtk \
        --executable "$appdir/usr/bin/pipedeck" \
        --executable "$appdir/usr/bin/pipedeck-opendeck" \
        --desktop-file "$appdir/usr/share/applications/dev._2c2t.Pipedeck.desktop" \
        --icon-file "$appdir/usr/share/icons/hicolor/scalable/apps/dev._2c2t.Pipedeck.svg" \
        --exclude-library 'libpipewire-0.3.so*'
    # The GTK plug-in has GTK draw through X11, where GTK 4 is at home on
    # Wayland, and forces a theme, where libadwaita follows the desktop's
    # and Pipedeck's own setting.
    sed -i -e '/GDK_BACKEND/d' -e '/^export GTK_THEME=/d' \
        "$appdir/apprun-hooks/linuxdeploy-plugin-gtk.sh"
    LDAI_OUTPUT=$out/Pipedeck-$version-x86_64.AppImage \
        linuxdeploy --appdir "$appdir" --output appimage
    ;;

*)
    echo "deb, rpm or appimage, not $kind" >&2
    exit 2
    ;;
esac
ls -l "$out"
