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
| **Channel Effect** | a channel and one of its effects | switches the effect off or on | press, the same |
| **Add to Channel** | an application, or the one in front, and a channel | puts the application on the channel, or takes it off | press, the same |
| **Call** | the page it goes to | goes to the call's page, saying how many are in it, or back | press, the same |

A person of a call wears their Discord picture, as Vesktop's plugin
fetched it. The application in front is the one playing in the window that has the
focus, which on KDE a KWin script of Pipedeck's says: switch on
**Application in front** in Pipedeck's Stream Deck settings.

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

## Ready-made pages

With the plugin installed, Pipedeck lays out **Pipedeck** pages from the
mixer as it is, and again when a channel, a mix or a device comes or goes:
StreamController keeps pages apart from decks, so there is a pair for each
kind, **Pipedeck** and **Pipedeck Call** for a Stream Deck, **Pipedeck +**
and **Pipedeck + Call**, **Pipedeck XL** and **Pipedeck XL Call**; give your
deck its own. They are laid out as OpenDeck's profiles are (see
[its README](../opendeck/README.md)), with a Call key between the two. With
**Follow calls** on, in a Call key's settings, a deck on a Pipedeck page
goes to the call's as a call starts, and back as it ends.

## How it talks to Pipedeck

Over Pipedeck's control socket, `$XDG_RUNTIME_DIR/pipedeck/control.sock`:
one connection, subscribed to the mixer's state and carrying the orders
back. The protocol is described in
[`control.rs`](../../crates/pipedeck-engine/src/control.rs); anything else
that can write a line of JSON to a Unix socket can use it too.
`python3 dev_2c2t_Pipedeck/client.py` prints the state as it
changes.
