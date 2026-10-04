# Pipedeck for StreamController

A [StreamController](https://github.com/StreamController/StreamController)
plugin that puts Pipedeck on a Stream Deck: keys and dials that mute and
move the levels of channels, mixes, cells and the people of a call, choose
which mix is heard in the headphones, and the device it is heard on. Each
shows what it controls as Pipedeck draws it, kept up as the mixer changes,
whoever changed it.

| Action | A key | A dial (Stream Deck +) |
| --- | --- | --- |
| **Mute** | mutes or unmutes | press or touch mutes or unmutes |
| **Volume** | moves the level by its step (below zero lowers it) | turns the level up or down by the step; press or touch mutes |
| **Hear mix** | hears this mix alone, or turns it on or off beside the others | press, the same |
| **Listen on** | moves every mix heard to this device | press, the same |

Each action is pointed at one thing in its settings, picked from what
Pipedeck has at the time, and kept by id, so renaming a channel does not
lose the key. While Pipedeck is not running the keys say so, and come back
when it does.

## Install

```sh
integrations/streamcontroller/install.sh
```

copies the plugin and Pipedeck's icons into StreamController's plugins
folder (`~/.var/app/com.core447.StreamController/data/plugins`, or
`$STREAMCONTROLLER_DATA/plugins`). StreamController loads plugins when it
starts, so restart it; the actions are then under **Pipedeck**.

## How it talks to Pipedeck

Over Pipedeck's control socket, `$XDG_RUNTIME_DIR/pipedeck/control.sock`:
one connection, subscribed to the mixer's state and carrying the orders
back. The protocol is described in
[`control.rs`](../../crates/pipedeck-engine/src/control.rs); anything else
that can write a line of JSON to a Unix socket can use it too.
`python3 com_fabienmillet_Pipedeck/client.py` prints the state as it
changes.
