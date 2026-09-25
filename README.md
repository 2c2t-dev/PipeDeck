# Pipedeck

A PipeWire mixer for Linux streamers, in the spirit of Elgato Wave Link.

The mixer is a **matrix**. Sources are rows, mixes are columns, and each cell
is an independent fader and mute. What you hear is not what goes to the
stream, because they are different columns.

- A **source**, called a channel in the interface, is either a virtual output
  any application can select in its audio settings, or a capture device such
  as a microphone. Creating one offers ready-made kinds, Music, Browser,
  System, Game, SFX, Voice chat and two Aux, each with its own icon and
  colour. A channel can also hold applications: their audio is moved
  onto it as they start playing, so you pick them once instead of every time.
  The picker offers what is playing right now and what the system knows how
  to launch, read from the desktop entries.
- A **mix** is created ready-made: the first is a Personal Mix, then a Chat,
  Stream, Record and Aux Mix, each with its own icon, and the window renames
  them. A mix collects the sources you send to it into a sink a capture client
  such as OBS can read, and plays to any number of output devices you attach
  to it, each with its own level. It has a master level of its own. Up to
  five mixes.
- Every channel and every mix carries a **meter**, and a cell draws what it
  passes on: its channel's level scaled by its own fader.
- A **cell** exists only when you press `+` on it. It is what links a source
  to a mix, and it carries that pair's fader and mute.

## Status

MVP: routing plumbing, the mixer UI, and effects on a channel.

## Building

Runtime and build dependencies: PipeWire >= 1.2 (headers), GTK >= 4.18,
libadwaita >= 1.7, clang (for bindgen), pkg-config, a Rust toolchain >= 1.80.

```sh
cargo build --release
./target/release/pipedeck
```

Logs go through `env_logger`: `RUST_LOG=pipedeck_engine=debug ./target/release/pipedeck`.

The level of a channel and of a mix is the volume of its sink, which is the
volume the system shows for it: move it from a volume applet or a media key
and the mixer follows, and the other way around.

State is persisted in `$XDG_CONFIG_HOME/pipedeck/config.toml`. A config from
the earlier two-bus layout is converted on first start into two mixes named
Stream Mix and Monitor, keeping every fader.

Set `PIPEDECK_APP_ID` to run a development build next to an installed one,
instead of handing over to the running instance.

Each card carries an icon from the bundled set, chosen from a small set of
looks: a channel picks
one when it is created, and both a channel and a mix can change it later from
the menu above their icon.

Every card can be dragged by its grip, which shows on hover with the pencil
in place of the card's icon: a channel along the rows, a mix along the
columns, never one among the other. A card dropped on another takes its place. The order is only how the
matrix is drawn; nothing on the graph moves, and it is saved with the rest.

Clicking a card opens the window of that object. A mix window holds its
name, its master level and the devices it plays to, each with a level of
its own. A channel window holds its name, its trim and the applications it carries.
A channel bound to a capture device shows that device instead, since
applications play into virtual outputs, not into a microphone. Both are where you rename or
remove the object.

## Discord calls

With Vesktop and Pipedeck's plugin for it, installed from Settings, Plug-ins,
Discord voices (see [integrations/vencord](integrations/vencord/README.md)),
the channel Vesktop
is assigned to unfolds into the people of the call you are in: each has a
sub-track of their own, with a level and a mute, on its way into the
channel, whose effects and cells they then go through. A person's level is
remembered for the next call. Discord's own mix of them is not played, so
each voice is heard once.

## Effects

A channel can run effects, which every mix then hears. The mixer runs four
of its own, written in Rust, so nothing has to be installed:

- **Noise suppression**, RNNoise through `nnnoiseless`: fans, keyboards and
  hiss behind a voice. It holds the sound back 10 ms, and its strength mixes
  the treated sound with the untouched one. Its window draws a voice over a
  room, the room as loud as it is left; dragging the noise's level down
  takes more out, and presets go from off to full.
- **Equaliser**, five bands shaped for a voice — a low cut, a low shelf, two
  bells and a high shelf — set by dragging them on a curve. Each band has
  its colour and draws its own shape under the curve; scroll over a bell to
  widen it, double-click a band to put it back, or pick one under the graph
  to type its exact values. Behind the curve, the range is cut into the
  zones a voice is talked about in — rumble, body, mud, honk, presence,
  sibilance, air — and pointing at one says what to do there. Presets give
  a starting point: clear, warm, podcast, less boom, less mud, and more.
- **De-esser**: splits the sound at a frequency, listens for s and sh there,
  and turns only the part above down when they are too loud. Its window
  draws what it does to a loud s across the top of the range; one handle
  sets both controls, sideways for where the s start and down to take them
  further, and presets suit a light touch, a deep voice or a high one. *Learn from my
  voice* listens for five seconds of speech with s in it, finds where they
  are loudest, splits just under, and sets the strength so the s go over
  its threshold and the rest of the voice does not.
- **Compressor**, with only a threshold, a ratio and a makeup gain, set on
  its curve: the threshold is the bend, dragged sideways; the ratio the top
  of the curve, dragged down for more; the makeup its foot, dragged up. A
  line under it says what it does to a shout, and presets go from gentle to
  broadcast. Its timing is fixed at what suits a voice. *Learn from my voice* listens for
  five seconds while you speak and sets all three from what it heard: the
  threshold where the voice usually is, a ratio as firm as the voice is
  uneven, and the makeup that brings its loud moments to about -10 dB.

Each one's controls open in a window of their own, from the gear on its card,
so the ones being set can stay open beside the mixer; they close with the
channel's window.

They run with the plug-ins, on the audio thread. Adding or taking one off
makes the chain again, which stops the audio for a moment; a control turned
is only a number written, so it is heard at once and the audio never stops
for it.

A channel bound to a microphone runs them too, in a tab of its own: it has no
sink to read, so the chain captures the device itself and every mix hears the
microphone through it.

A mix runs none: a mix is what comes out of the channels, not another place to
treat them.

A channel can also run **VST3 plug-ins**, which the mixer hosts itself. It
reads the bundles installed under the usual paths, plus `VST3_PATH`, and
offers their effects alongside the ones above. A plug-in runs between two
streams of its own, after whatever PipeWire runs for the channel, and every
mix hears the result.

Their windows are not implemented: a plug-in's own editor is an X11 surface
to embed, and GTK4 has no socket for one. Parameters are at their defaults.

A VST3 on Linux is a directory named `something.vst3`, not a file, and it
holds the shared object under `Contents/x86_64-linux/`. The **Plug-ins** page
of the settings installs one into `~/.vst3`: point it at the directory and it
is copied whole, or at a bare `.so` and the directory is built around it. It
then reads the paths again, so the effect is offered right away. Plug-ins are
read once per run, because opening one runs its own code.

## Stereo Tool

Thimeo's broadcast processor runs as an effect like any other, on a channel
or on a mix. Pipedeck hosts `libStereoTool`, the shared library the vendor
ships for exactly this and the one Liquidsoap calls, rather than the VST3
build: the library takes its settings from a preset file through the API,
where the plug-in would need an editor window Pipedeck has no way to embed.

It is proprietary and nothing of it is bundled. Download it from
[thimeo.com](https://www.thimeo.com/stereo-tool/download/) and import the
archive from the **Plug-ins** page. The archive is built on Windows and holds
every machine's build — half a gigabyte of them, the Kantar edition under the
same names in a directory of its own — so only the builds for this machine are
kept, flat, in `~/.local/share/pipedeck/stereotool/`. The one the vendor
documents is loaded first, and the `noX11` build after it, for a machine whose
X11 libraries a normal build would ask for and not find.
`PIPEDECK_STEREOTOOL` points at a copy kept elsewhere.

The library is talkative on its own account: creating a processor walks every
ALSA device on the machine and looks for a JACK server, some fifty lines of
complaint each time, written in C straight to the standard error. Pipedeck
points that descriptor at `/dev/null` for the length of those calls and puts
it back after; `PIPEDECK_STEREOTOOL_NOISE=1` leaves it alone when the
library's own words are what is wanted. It is asked where it stands once per
run — at startup, after an import, and when a licence key is given — because
a library already loaded cannot be swapped inside one run anyway. It also
keeps its state in `~/.libStereoTool_*.so.rc`.

**Window** on the effect brings up Stereo Tool's own interface — every band
and every curve it has — on the processor the mixer is running, so what you
change is heard at once and stays in its own settings file. It is a window of
its own next to Pipedeck's, drawn by the library in X11, which is why the X11
build is the one loaded first. The builds for a machine without X11 carry no
window, and then the button is not offered; a preset exported from Stereo Tool
can be loaded on the effect instead, which is what a machine with no display
has.

Its interface has to be asked for the way a plug-in's is. `GUI_Show` takes the
X11 id of a host window, as a plug-in is handed the window its host drew for
it, and given none it does nothing at all — no error, no window. Told about
one, it opens a toplevel of its own beside it, which declares the usual close
request and then ignores it, the way a plug-in leaves its editor to whoever
opened it: a window whose close button does nothing.

So Pipedeck adopts it. It makes an X11 window of its own, hands its id to the
library, finds the window the library opens a moment later and reparents it
under its own. The window manager then decorates Pipedeck's window, and the
close button reaches a client that listens: the engine reads the request on
its next tick and takes the interface down. The window keeps the size the
library gave it — whether the library lays itself out again at another size
is not something to find out on the user — and **Window** on the effect
opens it, or brings it to the front if it is already up. Removing the effect
or changing the chain closes it as well, since the processor it draws goes
with them.

The licence key is a field on the settings page, passed to the library and to
nothing else. Without one Stereo Tool still runs and puts speech and beeps in
the audio, which is the vendor's doing; the settings say so rather than let it
be discovered on air. It adds 50 to 100 ms of delay depending on what it runs,
and a channel is heard by every mix, so what it delays includes the mix you
monitor on.

`cargo run -p pipedeck-engine --example stereotool_check [preset.sts]` runs a
tone through it with no PipeWire in the way, and says what the licence covers;
`--window` puts its interface on the screen for ten seconds, which is the one
thing the symbols alone cannot tell you. `PIPEDECK_SMOKE_WINDOW=1` adds the
same check to the smoke test, where the window is opened the way the interface
opens it: from the engine thread.

## When PipeWire restarts

The mixer lives through it. Losing the server ends a session and nothing
more: the graph is taken apart with the loop already stopped — taking one
apart while its loop runs double-frees what the broken connection has
already freed — and what the mixer *is* outlives it, since the config is
handed from one session to the next. A loop with nothing on it but the
command channel then knocks once a second until a server answers, and the
whole mixer goes back on the graph.

WirePlumber restarting on its own is quieter and does more damage: PipeWire
and every node stay, but the metadata applications are routed through is made
again, empty, and the streams at either end of a cell are renegotiated, taking
with them the links the mixer made itself. The mixer notices both. It binds
the new metadata and sends every assigned application back to its channel,
and it watches the links it made, so one that goes without being asked to is
made again on the next tick. `examples/relink_check` shows it: a cell and an
application, a WirePlumber restarted under them, and the two whole again two
seconds later.

`cargo run -p pipedeck-engine --example reconnect_check` watches that happen.
Point `PIPEWIRE_REMOTE` at a server you are willing to kill — a private one,
started with `PIPEWIRE_RUNTIME_DIR=/tmp/pdw PIPEWIRE_CORE=pdtest pipewire`,
since restarting the session's own cuts everybody's audio.

## Settings

The gear in the header bar opens them: the colour theme, whether Pipedeck
starts with the session, the quantum asked of its nodes, the VST3 plug-ins it
found and where Stereo Tool stands. Changing the quantum reloads every route,
so the audio stops for a moment.

## Switching sound cards

You listen on one device — your headphones — and the ear on each mix card
says whether that mix is heard there: lit, it is; dark, it is not. Several can
be lit, to hear the chat over the game. The button in the header bar says
which device that is, sets how loud, and lists every output device; picking
another moves every mix you hear onto it and off the old one, which is what
changing sound cards means when you are wearing one of them. Until one is
picked, the first device a mix already plays to stands in, so nothing changes
for someone who never touches it.

Underneath, it is all outputs. A mix is heard on your headphones when it has
an output to them that is switched on; the ear switches it, adding one if
there is none. Each output in a mix's window has that switch of its own: off,
the mix stops playing there and lets go of the device, and the output stays
in the list with its level for when it comes back on.

## Using it with OBS

Every mix *is* an input device, named after it, so OBS, Discord or a browser
list it where they list microphones and nowhere else: add an audio input
capture and pick the mix by name. It is there whether or not the mix plays to
a device. Renaming one makes the node again, since a node carries the
description it was born with and that description is the name in someone's
list; a recorder has to pick it again. Attach your headphones to a different
mix to hear a different balance.

A channel is the opposite: a sink, which the system lists among the outputs
like a pair of headphones, so an application can be pointed at it. Rows are
what you play into, columns are what you record — and each is only ever in the
one list.

## How the graph looks

```
 apps ──▶ [pipedeck.src.N] ──monitor──┐
           an output device           ├─ loopback (cell fader) ═▶ [pipedeck.mix.M] ──┬─ loopback ──▶ device
 mic  ────────────────────────────────┘                            an input device   └─ loopback ──▶ device
                                                                   (recorded by OBS)
```

A row is a sink and a column is a source, which is what each of them is to the
rest of the system, and the reason each appears in one list only. Both are the
same null node underneath — a column has input ports like any sink, and what
plays into them comes out of its capture ports — but its class says source,
and a session manager routes nothing into a source. So the cells (`═▶` above)
are linked into their column by hand, port to port, by the mixer itself. That
is the one place here where PipeWire's own routing is not asked to do the
work; everything else still is.

Volume comes with it: `monitor.channel-volumes` makes a node apply its volume
to what it hands on, which is a pre-fader trim on a row and the master on a
column, and it works on a source made this way as well as on a sink.

With effects, a channel grows a stage or two before the cells read it. A row
bound to a microphone starts on that microphone instead of on a sink:

```
 [pipedeck.src.N] ─▶ [pipedeck.vst.N] ─▶ cells
  or the microphone   the mixer's own effects and hosted plug-ins
```

Audio crosses two of our nodes on its way to a device, one for the cell and
one for the mix output. Both request the quantum set in the settings, 512 frames by default, so the
round trip stays in the same ballpark as a single hop at PipeWire's usual
1024. Raise it if the machine reports xruns.

The faders are the `Props` volume of each loopback's playback node. Capture
sides are internal streams (`Stream/Input/Audio/Internal`) so they stay out of
pavucontrol's recording tab, and every node belongs to the app's client
connection: if the app dies, PipeWire drops them all, nothing lingers.

## Icons

The icons are Material Symbols, bundled under `crates/pipedeck/icons` and
compiled into the binary, rather than taken from the desktop's icon theme:
a name a theme lacks is drawn as a broken image, and the names that do exist
come from different families, which shows when a row of them sits in a grid.
Apache License 2.0, see that directory.

## Workspace

- `crates/pipedeck-engine`: the PipeWire graph, config, command/event API.
  No UI dependency; meant to become a D-Bus daemon later.
- `crates/pipedeck`: the GTK4 + libadwaita application.

`cargo run -p pipedeck-engine --example smoke` exercises the engine against
the live PipeWire daemon: it builds a matrix, checks the nodes and volumes it
creates, attaches a real output device, then tears everything down and
verifies that nothing is left behind.
