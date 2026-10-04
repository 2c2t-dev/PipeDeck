# Pipedeck for OpenDeck

An [OpenDeck](https://github.com/nekename/OpenDeck) plugin that puts
Pipedeck on a Stream Deck: keys and dials that mute and move the levels of
channels, mixes, cells and the people of a call, choose which mix is heard
in the headphones, and the device it is heard on. Each shows what it
controls as Pipedeck draws it, kept up as the mixer changes, whoever
changed it. It does what the [StreamController
plugin](../streamcontroller/README.md) does, for those who use OpenDeck.

| Action | A key | A dial (Stream Deck +) |
| --- | --- | --- |
| **Mute** | mutes or unmutes | press or touch mutes or unmutes |
| **Volume** | moves the level by its step (below zero lowers it) | each notch moves the level by the step; press or touch mutes |
| **Hear mix** | hears this mix alone, or turns it on or off beside the others | press, the same |
| **Listen on** | moves every mix heard to this device | press, the same |

Each action is pointed at one thing in its settings, picked from what
Pipedeck has at the time, and kept by id, so renaming a channel does not
lose the key. While Pipedeck is not running the keys say so, and come back
when it does.

## Install

```sh
integrations/opendeck/install.sh
```

builds the plugin and copies it into OpenDeck's plugins folder
(`~/.config/opendeck/plugins`, or `$OPENDECK_CONFIG/plugins`). OpenDeck
loads plugins when it starts, so restart it; the actions are then under
**Pipedeck**.

OpenDeck and StreamController cannot both hold a Stream Deck: use one.
StreamController reaches the decks through libusb and takes them from the
kernel, which OpenDeck reads them through; after stopping StreamController,
unplug and plug the decks back in, or hand each back to the kernel by its
USB path (`1-2.4.2` here, as `lsusb -t` or `/sys/bus/usb/devices` show it):

```sh
echo 1-2.4.2:1.0 | sudo tee /sys/bus/usb/drivers/usbhid/bind
```

## How it works

A Stream Deck SDK plugin, in Rust (`src/`), started by OpenDeck with the
port of its WebSocket. It follows Pipedeck over its control socket
(`$XDG_RUNTIME_DIR/pipedeck/control.sock`, the protocol is described in
[`control.rs`](../../crates/pipedeck-engine/src/control.rs)) and draws each
key as SVG, which OpenDeck renders: a key through its window, a dial's part
of the touch strip through `plugin/layouts/strip.json`, one picture the
size of the strip. The settings page is `plugin/propertyInspector`. The
badges are the mixer's own: the looks come from
`crates/pipedeck/src/presets.rs` and the icons from `crates/pipedeck/icons`.
