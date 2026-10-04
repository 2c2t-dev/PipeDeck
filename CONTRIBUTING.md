# Contributing to Pipedeck

Thanks for helping. A bug report with logs, a fix, or an idea talked over in
[Discussions](https://github.com/2c2t-dev/PipeDeck/discussions) first are
all welcome.

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

`relink_check` and `reconnect_check` cover WirePlumber and PipeWire
restarts; see the README for those and the other examples.

## How the code is written

- Code, comments and commit messages are in English.
- A commit message says what changes and why, in the imperative:
  "Keep the plug-ins' output out of the desktop's volume controls", then a
  paragraph on the reason when it is not obvious.
- Comments say why, not what the next line does.

A few rules hold the engine together; a change that breaks one needs a
good reason, given in the pull request:

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

## Making a release

1. Set `version` in the root `Cargo.toml`, and move the changelog's
   *Unreleased* section under the new version.
2. Commit, and tag that commit: `git tag -a v0.2.0 -m "Pipedeck 0.2.0"`.
3. Push the commit, then the tag: `git push origin main v0.2.0`.

The tag builds the .deb, the .rpm and the AppImage, and puts them on a
release whose notes are that version's section of the changelog. The
workflow stops if the tag and `Cargo.toml` disagree.
