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
| **Call Voice** | a place in the Discord call: the first, the second… | as Channel Level, for whoever is there | the same |
| **Channel Effect** | a channel and one of its effects | switches the effect off or on | press, the same |
| **Add to Channel** | an application, or the one in front, and a channel | puts the application on the channel, or takes it off | press, the same |
| **Call** | the call, or back to the mixer | goes to the Pipedeck Call profile, or back | press, the same |

A person of a call wears their Discord picture, as Vesktop's plugin
fetched it. The application in front is the one playing in the window that has the
focus, which on KDE a KWin script of Pipedeck's says: switch on
**Application in front** in Pipedeck's Stream Deck settings.

A level shows its meter with it, as the mixer's do: inside the ring on a
key, in the bar on the touch strip with the level as a handle on it; set
**Display** to *Volume only* to leave it out. A channel's level in one mix
is what the mixer calls a cell: the key wears
the mix's badge in the corner of the channel's. Everything is kept by id,
so renaming a channel does not lose its key; while Pipedeck is not running
the keys say so, and come back when it does.

## Install

In Pipedeck: **Settings**, **Plug-ins**, **Stream Deck**, **Install** next
to OpenDeck. It puts the plugin in OpenDeck's plugins folder
(`~/.config/opendeck/plugins`), lays out a Pipedeck profile for each deck,
and restarts OpenDeck if it was running, since it reads its plugins when it
starts; **Update** does the same when Pipedeck has a newer one.

Or by hand, from a checkout:

```sh
integrations/opendeck/install.sh
```

builds the plugin and copies it there; restart OpenDeck.

OpenDeck and StreamController cannot both hold a Stream Deck: use one.
StreamController reaches the decks through libusb and takes them from the
kernel, which OpenDeck reads them through; after stopping StreamController,
unplug and plug the decks back in, or hand each back to the kernel by its
USB path (`1-2.4.2` here, as `lsusb -t` or `/sys/bus/usb/devices` show it):

```sh
echo 1-2.4.2:1.0 | sudo tee /sys/bus/usb/drivers/usbhid/bind
```

## Ready-made profiles

Pipedeck lays out a profile called **Pipedeck** for each Stream Deck
OpenDeck knows, from the mixer as it is, next to the profiles already
there; pick it in OpenDeck's profile menu. With **Keep the Pipedeck
profiles laid out from the mixer** on, as it is unless turned off in the
same settings, it lays them out again a few seconds after a channel, a
mix, a cell or a device comes or goes, closing OpenDeck meanwhile: OpenDeck
holds a profile it has read and writes it back over the disk. A name or a
level changing does not call for it. What was changed by hand on those
profiles is replaced, so keep your own in another. **Lay out now** does it
at once; so does

```sh
~/.config/opendeck/plugins/dev.2c2t.pipedeck.sdPlugin/x86_64-unknown-linux-gnu/bin/pipedeck-opendeck --profiles
```

with OpenDeck closed.

- **Stream Deck** (15 keys): the channels on the top row, the mixes to hear
  in the headphones on the middle one with the output device at its end,
  the mixes at the bottom.
- **Stream Deck +**: a channel on each dial, the mixes on the top keys, the
  mixes to hear and the output device under them.
- **Stream Deck XL**: the mixer's grid, a column for each channel (up to
  six) with its own level on top and its level in each mix under it, then a
  column of mixes and a column of mixes to hear, the output device last.

A microphone's effects, noise suppression and the like, get a Channel
Effect key each in what is left: the keys first, then the dials.

Each has a **Call** key, which says how many are in the call and goes to a
second profile, **Pipedeck Call**: a person of the call on every dial and
key, by their place in it, so it follows the call as people come and go
without being laid out again, and a key back to the mixer. With **Follow
calls** ticked in a Call key's settings, every deck showing a Pipedeck
profile goes to the call's as a call starts, and back as it ends; a deck on
a profile of your own is left there.

Every key on them is an ordinary action: change it in its settings, move it
or replace it as any other, in a profile of your own.

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
