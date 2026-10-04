# Pipedeck for OpenDeck

An [OpenDeck](https://github.com/nekename/OpenDeck) plugin that puts
Pipedeck on a Stream Deck the way Elgato's Wave Link plugin puts Wave Link
there: the same actions, set up the same way, each showing what it
controls as Pipedeck draws it, kept up as the mixer changes, whoever
changed it.

| Action | Settings | A key | A dial (Stream Deck +) |
| --- | --- | --- | --- |
| **Channel Level** | a channel, or a person of its call; its main level or its level in one mix | mutes, sets the volume (with a fade if wanted), or adjusts it by a step | turns the level; press or touch mutes |
| **Mix Level** | a mix | the same | the same |
| **Monitor Mix** | one mix, or two to switch between | hears that mix in the headphones, alone | press, the same |
| **Main Output Device** | one device, or two to switch between | listens on that device | press, the same |

A channel's level in one mix is what the mixer calls a cell: the key wears
the mix's badge in the corner of the channel's. Everything is kept by id,
so renaming a channel does not lose its key; while Pipedeck is not running
the keys say so, and come back when it does.

## Install

```sh
integrations/opendeck/install.sh
```

builds the plugin and copies it into OpenDeck's plugins folder
(`~/.config/opendeck/plugins`, or `$OPENDECK_CONFIG/plugins`). OpenDeck
loads plugins when it starts, so restart it; the actions are then under
**Pipedeck**. Drop one on a key or a dial, and pick what it controls in the
settings under it.

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
