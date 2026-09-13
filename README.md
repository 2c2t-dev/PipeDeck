# Pipedeck

A PipeWire mixer for Linux streamers, in the spirit of Elgato Wave Link.

The mixer is a **matrix**. Sources are rows, mixes are columns, and each cell
is an independent fader and mute. What you hear is not what goes to the
stream, because they are different columns.

- A **source**, called a channel in the interface, is either a virtual output
  any application can select in its audio settings, or a capture device such
  as a microphone. A channel can also hold applications: their audio is moved
  onto it as they start playing, so you pick them once instead of every time.
- A **mix** collects the sources you send to it into a sink a capture client
  such as OBS can read, and plays to any number of output devices you attach
  to it, each with its own level. It has a master level of its own. Up to
  five mixes.
- A **cell** exists only when you press `+` on it. It is what links a source
  to a mix, and it carries that pair's fader and mute.

## Status

MVP: routing plumbing and the mixer UI. No plugins, no VST, no per-application
auto-routing. Level meters are not implemented yet.

## Building

Runtime and build dependencies: PipeWire >= 1.2 (headers), GTK >= 4.18,
libadwaita >= 1.7, clang (for bindgen), pkg-config, a Rust toolchain >= 1.80.

```sh
cargo build --release
./target/release/pipedeck
```

Logs go through `env_logger`: `RUST_LOG=pipedeck_engine=debug ./target/release/pipedeck`.

State is persisted in `$XDG_CONFIG_HOME/pipedeck/config.toml`. A config from
the earlier two-bus layout is converted on first start into two mixes named
Stream Mix and Monitor, keeping every fader.

Set `PIPEDECK_APP_ID` to run a development build next to an installed one,
instead of handing over to the running instance.

Clicking a card opens the window of that object. A mix window holds its
name, its master level and the devices it plays to, each with a level of
its own. A channel window holds its name, its trim and the applications it carries.
A channel bound to a capture device shows that device instead, since
applications play into virtual outputs, not into a microphone. Both are where you rename or
remove the object.

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

Audio crosses two of our nodes on its way to a device, one for the cell and
one for the mix output. Both request the quantum set by `latency` in the
config, 512 frames by default, so the round trip stays in the same ballpark
as a single hop at PipeWire's usual 1024. Raise it if the machine reports
xruns.

The faders are the `Props` volume of each loopback's playback node. Capture
sides are internal streams (`Stream/Input/Audio/Internal`) so they stay out of
pavucontrol's recording tab, and every node belongs to the app's client
connection: if the app dies, PipeWire drops them all, nothing lingers.

## Workspace

- `crates/pipedeck-engine`: the PipeWire graph, config, command/event API.
  No UI dependency; meant to become a D-Bus daemon later.
- `crates/pipedeck`: the GTK4 + libadwaita application.

`cargo run -p pipedeck-engine --example smoke` exercises the engine against
the live PipeWire daemon: it builds a matrix, checks the nodes and volumes it
creates, attaches a real output device, then tears everything down and
verifies that nothing is left behind.
