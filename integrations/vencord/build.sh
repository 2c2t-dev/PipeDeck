#!/bin/sh
# Build Vencord with Pipedeck's plugin in it, for Vesktop to load.
#
# Vencord is cloned once into a cache of its own and updated after; the
# plugin is copied into its userplugins, since Vencord's build resolves a
# plugin's imports from where it sits. Point Vesktop at the dist folder
# printed at the end: Vesktop Settings, Vencord Location.
set -eu

here=$(dirname "$(realpath "$0")")
dir=${PIPEDECK_VENCORD_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/pipedeck/vencord}

if [ -d "$dir/.git" ]; then
    git -C "$dir" pull --ff-only --quiet
else
    git clone --quiet --depth 1 https://github.com/Vendicated/Vencord "$dir"
fi

mkdir -p "$dir/src/userplugins"
rm -f "$dir/dist/pipedeck-voices"
rm -rf "$dir/src/userplugins/pipedeckVoices.vesktop"
cp -r "$here/pipedeckVoices.vesktop" "$dir/src/userplugins/"

cd "$dir"
# Vencord is built with pnpm, which this machine may not have.
pnpm() { npx --yes "pnpm@$(sed -n 's/.*"packageManager": "pnpm@\([^"]*\)".*/\1/p' package.json)" "$@"; }
pnpm install --frozen-lockfile --silent
pnpm build

# Vesktop loads the vencordDesktop files: the plugin must be in them.
if ! grep -q PipedeckVoices dist/vencordDesktopRenderer.js; then
    echo "The build left the plugin out of Vesktop's files." >&2
    exit 1
fi
# The mark Pipedeck reads to know this build holds the plugin.
printf PipedeckVoices > dist/pipedeck-voices

echo
echo "Built. In Vesktop: Settings, Vesktop Settings, Vencord Location: $dir/dist"
