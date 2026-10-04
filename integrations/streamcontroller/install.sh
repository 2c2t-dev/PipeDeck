#!/bin/sh
# Install the Pipedeck plugin into StreamController, with the mixer's icons,
# then restart StreamController: it loads plug-ins when it starts.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
data=${STREAMCONTROLLER_DATA:-"$HOME/.var/app/com.core447.StreamController/data"}
name=dev_2c2t_Pipedeck
dest="$data/plugins/$name"

mkdir -p "$data/plugins"
rm -rf "$dest"
mkdir -p "$dest/assets/icons"
cp "$here/$name"/*.py "$here/$name"/manifest.json "$here/$name"/locales.csv "$dest/"
cp "$repo/crates/pipedeck/icons"/*.svg "$dest/assets/icons/"
echo "installed in $dest"
