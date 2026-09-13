# Pipedeck

A PipeWire mixer for Linux streamers, in the spirit of Elgato Wave Link.

Each **source** (Game, Music, Chat…) is a virtual sink you can pick in any
application's audio settings. Every source feeds two mixes with fully
independent gain and mute:

- **Pipedeck Stream Mix**: a sink meant to be captured by OBS.
- **Monitor**: played on your default output device, what you hear.

What you hear in your headphones is not what goes to the stream.

## Status

MVP bootstrap: routing plumbing and the mixer UI. No plugins, no VST, no
per-application auto-routing.

## Building

Runtime and build dependencies: PipeWire ≥ 1.2 (headers), GTK ≥ 4.18,
libadwaita ≥ 1.7, clang (for bindgen), pkg-config, a Rust toolchain ≥ 1.80.

```sh
cargo build --release
./target/release/pipedeck
```

Logs go through `env_logger`: `RUST_LOG=pipedeck_engine=debug ./target/release/pipedeck`.

The state is persisted in `$XDG_CONFIG_HOME/pipedeck/config.toml`.

## Using it with OBS

Add an audio input capture in OBS and pick **Pipedeck Stream Mix** (or its
monitor, depending on the capture plugin). Route each application to the
Pipedeck source of your choice from the application or from pavucontrol.

## How the graph looks

```
apps ──▶ [pipedeck.N] null sink
            │
            ├─ monitor ──▶ loopback ──▶ [pipedeck.N.stream]  ──▶ [pipedeck.stream_mix]
            └─ monitor ──▶ loopback ──▶ [pipedeck.N.monitor] ──▶ default output device
```

One loopback per gain path, so one quantum of extra latency per path. The
faders are the `Props` volume of each loopback's playback node; the capture
sides are internal streams (`Stream/Input/Audio/Internal`) so they stay out
of pavucontrol's recording tab. Every node
belongs to the app's client connection: if the app dies, PipeWire drops
them all, nothing lingers.

## Workspace

- `crates/pipedeck-engine`: the PipeWire graph, config, command/event API.
  No UI dependency; meant to become a D-Bus daemon later.
- `crates/pipedeck`: the GTK4 + libadwaita application.

`cargo run -p pipedeck-engine --example smoke` exercises the engine against
the live PipeWire daemon (creates a source, checks volumes, removes it, and
verifies no node is left behind).
