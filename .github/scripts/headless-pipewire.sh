#!/bin/bash
# Starts a PipeWire with no sound card, for the smoke test to run against in
# a container: a session bus, the server, WirePlumber, and three devices
# made of null nodes, two speakers and a microphone, the whole graph timed
# by PipeWire's own dummy driver.
#
#     source .github/scripts/headless-pipewire.sh
#
# Sourced, it leaves XDG_RUNTIME_DIR and DBUS_SESSION_BUS_ADDRESS set for
# what runs next; in a GitHub workflow, it also hands them to later steps.
set -euo pipefail

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/runtime}"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
logs="$XDG_RUNTIME_DIR/logs"
mkdir -p "$logs"

dbus-daemon --session --fork --address="$DBUS_SESSION_BUS_ADDRESS"
pipewire >"$logs/pipewire.log" 2>&1 &
for _ in $(seq 100); do
    [ -S "$XDG_RUNTIME_DIR/pipewire-0" ] && break
    sleep 0.1
done
wireplumber >"$logs/wireplumber.log" 2>&1 &

device() {
    pw-cli create-node adapter "{
        factory.name = support.null-audio-sink
        node.name = $1
        node.description = \"$2\"
        media.class = $3
        audio.position = [ FL FR ]
        object.linger = true
    }" >/dev/null
}
device ci.headphones "Headphones" Audio/Sink
device ci.speakers "Speakers" Audio/Sink
device ci.microphone "Microphone" Audio/Source

# WirePlumber settles, and picks a default sink, before anything is asked.
for _ in $(seq 100); do
    wpctl status 2>/dev/null | grep -q "Headphones" && break
    sleep 0.1
done

if [ -n "${GITHUB_ENV:-}" ]; then
    echo "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR" >>"$GITHUB_ENV"
    echo "DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS" >>"$GITHUB_ENV"
fi
