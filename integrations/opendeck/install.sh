#!/bin/sh
# Build the Pipedeck plugin and install it into OpenDeck, then restart
# OpenDeck: it loads plug-ins when it starts.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
config=${OPENDECK_CONFIG:-"$HOME/.config/opendeck"}
dest="$config/plugins/dev.2c2t.pipedeck.sdPlugin"
target=$(rustc -vV | sed -n 's/^host: //p')

cargo build --release --manifest-path "$here/Cargo.toml"
binary="$(cargo metadata --format-version 1 --no-deps --manifest-path "$here/Cargo.toml" |
	sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release/pipedeck-opendeck"

rm -rf "$dest"
# The plugin under the name it had before.
rm -rf "$config/plugins/com.fabienmillet.pipedeck.sdPlugin"
mkdir -p "$dest/$target/bin" "$dest/icons"
cp -r "$here/plugin/." "$dest/"
cp "$binary" "$dest/$target/bin/"
"$binary" --icons "$dest/icons"
echo "installed in $dest"
