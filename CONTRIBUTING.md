# Contributing to Pipedeck

Thanks for helping. A bug report with logs, a fix, or an idea talked over in
[Discussions](https://github.com/2c2t-dev/PipeDeck/discussions) first are
all welcome.

Everyone taking part follows the [code of conduct](CODE_OF_CONDUCT.md).

## Building and running

You need PipeWire ≥ 1.2 (with headers), GTK ≥ 4.18, libadwaita ≥ 1.7,
clang, pkg-config and Rust ≥ 1.92.

```sh
cargo build --release
./target/release/pipedeck
```

To try a build next to the Pipedeck you use every day, give it an id, node
names, a control socket and a configuration of its own, so it neither hands
over to the running one nor claims the applications it moves:

```sh
PIPEDECK_APP_ID=dev._2c2t.PipedeckDev PIPEDECK_NODE_PREFIX=pipedeck-dev \
    PIPEDECK_CONTROL_SOCKET=$XDG_RUNTIME_DIR/pipedeck-dev.sock \
    XDG_CONFIG_HOME=$(mktemp -d) \
    cargo run -p pipedeck
```

Run this way, it does not lay out your Stream Deck pages either.

| Variable | Use |
| --- | --- |
| `PIPEDECK_APP_ID` | An application id of its own, so it does not hand over to the running Pipedeck. |
| `PIPEDECK_NODE_PREFIX` | Node names of its own, so two mixers can run side by side. |
| `PIPEDECK_CONTROL_SOCKET` | Another path for the control socket. |
| `PIPEDECK_STEREOTOOL_NOISE` | Show what Stereo Tool writes to the terminal, which is hidden otherwise. |

## Where things are

| Folder | Contents |
| --- | --- |
| `crates/pipedeck-engine` | The PipeWire graph, the config and the control socket. No GTK. |
| `crates/pipedeck` | The GTK 4 and libadwaita application. |
| `integrations/opendeck` | The OpenDeck plugin, in Rust. |
| `integrations/streamcontroller` | The StreamController plugin, in Python. |
| `integrations/vencord` | The Vesktop plugin. |

## Before a pull request

The CI runs these, so running them first saves a round trip:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For a change to the engine, also run the checks against the live PipeWire
server. They run beside your own Pipedeck without touching it:

```sh
cargo run -p pipedeck-engine --example smoke
```

The smoke test also runs after every push to `main`, in a container,
against a PipeWire with no sound card that
`.github/scripts/headless-pipewire.sh` starts; sourced in a container of
your own, it gives you the same.

The other checks, each run with `cargo run -p pipedeck-engine --example
<name>`:

| Example | Checks |
| --- | --- |
| `relink_check` | The mixer repairs itself after WirePlumber restarts. |
| `reconnect_check` | The mixer comes back after PipeWire restarts. Point `PIPEWIRE_REMOTE` at a private server, since restarting your own cuts all audio. |
| `plugins`, `plugin_check [name]` | Lists the VST3 plug-ins, runs a tone through one. |
| `stereotool_check [preset.sts]` | Runs a tone through Stereo Tool; `--window` also shows its window. |

## How the code is written

- Code, comments and commit messages are in English.
- A commit message says what changes and why, in the imperative:
  "Keep the plug-ins' output out of the desktop's volume controls", then a
  paragraph on the reason when it is not obvious.
- Comments say why, not what the next line does.

## The graph

What Pipedeck puts on the graph, as `pw-dump`, `pw-top` or qpwgraph show it.
`N` is a channel's id, `M` a mix's.

| Node | What it is |
| --- | --- |
| `pipedeck.src.N` | A channel: the sink applications play into. |
| `pipedeck.voice.N.<user>` | One person of a Discord call, playing into their channel. |
| `pipedeck.fx.N`, `pipedeck.vst.N` | The channel once its effects have run, which the cells then read. |
| `pipedeck.link.N.M` | A cell: a loopback carrying the cell's fader and mute. |
| `pipedeck.mix.M` | A mix: the source OBS records. |
| `pipedeck.out.M.<index>` | A mix playing to an output device. |
| `pipedeck.meter.*` | What the meters listen to. |

A channel is a sink, and a mix is a source, which is why each shows up in
only one list. A session manager routes nothing into a source, so Pipedeck
links the cells into their mix itself, port by port.

Every node belongs to Pipedeck's own PipeWire connection, so if Pipedeck
dies, nothing is left behind. On quitting, it hands the applications it
moved back to the session manager.

## Rules the engine holds to

A change that breaks one needs a good reason, given in the pull request:

- A node's `node.name` comes from its id (`pipedeck.src.3`), never from the
  name the user gave it, which goes in `node.description`. Renaming must
  never touch the graph.
- The streams Pipedeck makes carry `state.restore-props = false`, and
  `state.restore-target = false` where they are routed, so WirePlumber never
  overrides Pipedeck's volumes or links.
- Capture sides are `Stream/Input/Audio/Internal`, which keeps them out of
  the desktop's volume controls.
- Never destroy a loaded module from inside a registry or proxy callback;
  do it at the top of a loop turn.
- Bind proxies from the real global object inside `on_global`; never build
  one to call `registry.bind`.
