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

Clicking a card opens the window of that object. A mix window holds its
name, its master level and the devices it plays to, each with a level of
its own. A channel window holds its name, its trim and the applications it carries.
A channel bound to a capture device shows that device instead, since
applications play into virtual outputs, not into a microphone. Both are where you rename or
remove the object.

## Effects

A channel can run effects, which every mix then hears: a low cut, shelves,
a band and a gain, each with its own controls. They are filters PipeWire
ships, so nothing has to be installed, and they run in one filter chain
between the channel and the cells that read it.

**A mix runs the same effects**, in its own tab, and only that mix hears
them. A column is treated the other way round from a channel: its cells play
into the chain and the chain into the mix sink, so a capture client such as
OBS reads the treated signal. That is where a processor belongs when it is
meant for one destination — and where the delay one adds stays out of the way
of the mix you monitor on.

A channel or a mix can also run **VST3 plug-ins**, which the mixer hosts
itself. It reads the bundles installed under the usual paths, plus
`VST3_PATH`, and offers their effects alongside the filters above. A plug-in
runs between two streams of its own, after whatever PipeWire runs for the
object it belongs to.

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

The library talks to stderr on its own account — it looks for a JACK server at
load and says so, and it keeps its state in `~/.libStereoTool_*.so.rc`. That is
the library, not the mixer.

The licence key is a field on the same page, passed to the library and to
nothing else. Without one Stereo Tool still runs and puts speech and beeps in
the audio, which is the vendor's doing; the settings say so rather than let it
be discovered on air. Settings come from a preset exported from Stereo Tool
itself, chosen on the effect. It adds 50 to 100 ms of delay depending on what
it runs, which is the reason to put it on the mix that goes out and not on a
channel every mix hears.

`cargo run -p pipedeck-engine --example stereotool_check [preset.sts]` runs a
tone through it with no PipeWire in the way, and says what the licence covers.

## Settings

The gear in the header bar opens them: the colour theme, whether Pipedeck
starts with the session, the quantum asked of its nodes, the VST3 plug-ins it
found and where Stereo Tool stands. Changing the quantum reloads every route,
so the audio stops for a moment.

## Using it with OBS

A mix is capturable whether or not it has an output device: add an audio
input capture in OBS and pick the mix by name. Attach your headphones to a
different mix to hear a different balance.

## How the graph looks

```
 apps ──▶ [pipedeck.src.N] ──monitor──┐
                                      ├─ loopback (cell fader) ──▶ [pipedeck.mix.M] ──┬─ loopback ──▶ device
 mic  ────────────────────────────────┘                            (captured by OBS)  └─ loopback ──▶ device
```

With effects, each side grows a stage. A channel is treated before the cells
read it, a mix after they have written into it, so the sink at the end of the
column is the treated one:

```
 [pipedeck.src.N] ─▶ [pipedeck.fx.N] ─▶ [pipedeck.vst.N] ─▶ cells
   channel            PipeWire filters    hosted plug-ins

 cells ─▶ [pipedeck.mixfx.M] ─▶ [pipedeck.mixvst.M] ─▶ [pipedeck.mix.M]
            PipeWire filters      hosted plug-ins        read by OBS
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
