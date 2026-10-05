<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/banner-dark.svg">
    <img src=".github/banner-light.svg" alt="Pipedeck" width="420">
  </picture>
</h1>

A PipeWire mixer for Linux streamers, in the spirit of Elgato Wave Link.
Send each application to a channel, then decide how loud every channel is
in each mix: what you hear, what goes to the stream and what goes to the
call are separate balances.

- **A matrix mixer**: channels are rows, mixes are columns, and each cell has
  its own fader and mute.
- **Effects per channel**: noise suppression, equaliser, de-esser and
  compressor built in, plus VST3 plug-ins and Thimeo's Stereo Tool.
- **Meters** on every channel, mix and cell.
- **Stream Deck** support through OpenDeck or StreamController, with keys
  and dials that look like the mixer.
- **Discord calls** split into one track per person, with Vesktop.
- Lives through PipeWire and WirePlumber restarts.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/mixer-dark.png">
    <img src=".github/mixer-light.png" alt="The mixer: five channels in rows, three mixes in columns, a fader in each cell that links them" width="800">
  </picture>
</p>

## Installing

Each [release](https://github.com/2c2t-dev/PipeDeck/releases) has:

| Package | For |
| --- | --- |
| `.deb` | Debian 13, Ubuntu 25.04 and later |
| `.rpm` | Fedora 42 and later |
| `.AppImage` | Other distributions with glibc 2.41 or later. It carries GTK and libadwaita, and uses the system's PipeWire. |

All three need a GTK at least 4.18, a libadwaita at least 1.7 and PipeWire
1.2 or later, which is why older releases are not covered.

On any distribution, Pipedeck also comes as a Flatpak, from
[2c2t's repository](https://flatpak.2c2t.dev/), and is kept up to date with
your other Flatpaks:

```sh
flatpak remote-add --if-not-exists 2c2t https://flatpak.2c2t.dev/2c2t.flatpakrepo
flatpak install 2c2t dev._2c2t.Pipedeck
```

From its sandbox, Pipedeck cannot install StreamController's and Vesktop's
plugins, which are installed from outside (see their READMEs), nor the KWin
script that says which application is in front.

## Building

Requirements: PipeWire ≥ 1.2 (with headers), GTK ≥ 4.18, libadwaita ≥ 1.7,
clang (for bindgen), pkg-config and Rust ≥ 1.92.

```sh
cargo build --release
./target/release/pipedeck
```

Run this way, or from an AppImage, Pipedeck adds itself to the launcher,
with its icon, under `~/.local/share` on its first start.

## How it works

**Channels** (rows) are where sound comes from. A channel is either a
virtual output, which any application can pick in its audio settings, or a
capture device such as a microphone. You can attach applications to a
channel: Pipedeck moves their audio there whenever they start playing.

**Mixes** (columns) are where sound goes. Each mix is an input device named
after it: OBS, Discord or a browser list it with the microphones. A mix can
also play to one or more output devices, each with its own level. There can
be up to five mixes: Personal, Chat, Stream, Record and Aux.

**Cells** connect a channel to a mix. A cell exists only once you press `+`
on it, and it carries that pair's fader and mute.

**The ear** on a mix card says whether you hear that mix in your
headphones. The button in the header bar chooses which device your
headphones are, and moves every mix you listen to when you change it.

A channel's or a mix's level is the volume of its PipeWire node, so a volume
applet or a media key moves the fader too. Cards can be reordered by
dragging them by their grip, and a click on a card opens its settings.

## Effects

Effects run on a channel, and every mix hears the result. A channel bound to
a microphone runs them too. Mixes have no effects.

| Effect | What it does |
| --- | --- |
| Noise suppression | Removes fans, keyboards and hiss behind a voice (RNNoise, 10 ms of delay). |
| Equaliser | Five bands shaped for a voice, set by dragging them on a curve, with presets. |
| De-esser | Turns down harsh s and sh sounds. *Learn from my voice* sets it for you. |
| Compressor | Evens out loud and quiet moments. *Learn from my voice* sets it for you. |

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/equaliser-dark.png">
    <img src=".github/equaliser-light.png" alt="The equaliser's window: five bands on a curve, over the zones a voice is talked about in" width="560">
  </picture>
</p>

Each effect can be switched off without removing it. Changing a setting is
heard at once; adding or removing an effect stops the audio for a moment.

**VST3 plug-ins** are read from the usual folders and from `VST3_PATH`. The
**Plug-ins** page of the settings installs a `.vst3` bundle, or a bare `.so`,
into `~/.vst3`. Plug-in windows are not supported yet, so their parameters
stay at their defaults.

**Stereo Tool** is proprietary and not bundled: download it from
[thimeo.com](https://www.thimeo.com/stereo-tool/download/) and import the
archive from the **Plug-ins** page. Only the builds for your machine are
kept, in `~/.local/share/pipedeck/stereotool/`. Its own window opens from the
effect (X11 builds only), or load a preset exported from it. Without a
licence key it inserts speech and beeps into the audio. It adds 50 to 100 ms
of delay.

## Stream Deck and Discord

| Integration | What it does |
| --- | --- |
| [OpenDeck plugin](integrations/opendeck/README.md) | Mutes, levels, the mix you hear, effects and more on keys and dials, with meters. Ready-made profiles for the Stream Deck, Stream Deck + and XL. |
| [StreamController plugin](integrations/streamcontroller/README.md) | The same actions and the same look, for StreamController. |
| [Vesktop plugin](integrations/vencord/README.md) | Splits the Discord call into one sub-track per person, each with its own level and mute, remembered for the next call. |

All three are installed from **Settings → Plug-ins**. They talk to the
mixer through its control socket, `$XDG_RUNTIME_DIR/pipedeck/control.sock`.

On KDE, **Application in front** lets a Stream Deck key send the application
you are looking at to a channel.

## Settings

- **General**: colour theme, start at login, keep running in the
  notification area when the window is closed (`pipedeck --background`
  starts it that way, Ctrl+Q quits), and drawing without the graphics card
  if text shows up damaged.
- **Audio**: the quantum Pipedeck's nodes ask for, 512 frames by default.
  Raise it if you get xruns. **Export** writes the mixer to a file, to keep
  or to bring to another computer, without the Stereo Tool key; **Import**
  puts a mixer from such a file in place of yours. Devices the other
  computer lacks are picked again there.
- **Plug-ins**: VST3, Stereo Tool, Stream Deck and Discord.

The mixer is saved in `$XDG_CONFIG_HOME/pipedeck/config.toml`, the window's
own settings in `interface.toml` next to it.

## Environment variables

| Variable | Use |
| --- | --- |
| `RUST_LOG` | How much Pipedeck logs, e.g. `RUST_LOG=pipedeck=debug,pipedeck_engine=debug` for a bug report. |
| `PIPEDECK_STEREOTOOL` | Load Stereo Tool from somewhere else. |
| `VST3_PATH` | More folders to look for VST3 plug-ins in. |

## Contributing

Fixes and ideas are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) says how to
build, test and send a change, and [CHANGELOG.md](CHANGELOG.md) lists what
each release brings.

## License

Pipedeck is under the [MIT License](LICENSE).

The interface icons are [Material Symbols](https://fonts.google.com/icons),
under the Apache License 2.0, in `crates/pipedeck/icons`.
