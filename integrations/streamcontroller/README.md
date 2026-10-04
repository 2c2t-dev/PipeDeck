# Pipedeck for StreamController

A [StreamController](https://github.com/StreamController/StreamController)
plugin that puts Pipedeck on a Stream Deck the way Elgato's Wave Link
plugin puts Wave Link there. It has the same actions as the [OpenDeck
plugin](../opendeck/README.md) and draws its keys the same way, each
showing what it controls as Pipedeck draws it, kept up as the mixer
changes, whoever changed it.

| Action | Settings | A key | A dial (Stream Deck +) |
| --- | --- | --- | --- |
| **Channel Level** | a channel, or a person of its call; its main level or its level in one mix | mutes, sets the volume (with a fade if wanted), or adjusts it by a step | turns the level; press or touch mutes |
| **Mix Level** | a mix | the same | the same |
| **Monitor Mix** | one mix, or two to switch between | hears that mix in the headphones, alone | press, the same |
| **Main Output Device** | one device, or two to switch between | listens on that device | press, the same |
| **Call Voice** | a place in the Discord call: the first, the second… | as Channel Level, for whoever is there | the same |

A level shows its meter with it, as the mixer's do: inside the ring on a
key, in the bar on the touch strip with the level as a handle on it; set
**Display** to *Volume only* to leave it out. A channel's level in one mix
wears that mix's badge in its corner. Everything is kept by id, so renaming
a channel does not lose its key; while Pipedeck is not running the keys say
so, and come back when it does.

## Install

In Pipedeck: **Settings**, **Plug-ins**, **Stream Deck**, **Install** next
to StreamController, which puts the plugin and Pipedeck's icons in
StreamController's plugins folder
(`~/.var/app/com.core447.StreamController/data/plugins`). StreamController
loads plugins when it starts, so restart it; the actions are then under
**Pipedeck**. Drop one on a key or a dial, and pick what it controls in its
settings.

Or by hand, from a checkout: `integrations/streamcontroller/install.sh`
(or with `$STREAMCONTROLLER_DATA` for another data folder).

## How it talks to Pipedeck

Over Pipedeck's control socket, `$XDG_RUNTIME_DIR/pipedeck/control.sock`:
one connection, subscribed to the mixer's state and carrying the orders
back. The protocol is described in
[`control.rs`](../../crates/pipedeck-engine/src/control.rs); anything else
that can write a line of JSON to a Unix socket can use it too.
`python3 com_fabienmillet_Pipedeck/client.py` prints the state as it
changes.
