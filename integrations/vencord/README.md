# Pipedeck voices for Vesktop

A Vencord plugin that hands each person of a Discord call to Pipedeck as a
track of their own, so a Discord channel can unfold into one sub-track per
participant.

Vesktop plays Discord's voice through Chromium's WebRTC, where every person
arrives as a stream of their own, named after their user id, and is played
through an element of its own; Discord's own client mixes them in native
code, out of reach, which is why this is for Vesktop only.

The plugin tells Pipedeck who is in the voice channel you are in, over
Pipedeck's control socket (`$XDG_RUNTIME_DIR/pipedeck/control.sock`).
Pipedeck makes a sink for each of them on the channel Vesktop is assigned
to, and the plugin sends each person's element to theirs rather than to the
output Discord chose. Their volume and mute in Discord still apply; when
Pipedeck is not running, or has no sink for someone, their voice goes where
Discord sends it. What it does is written down in
`~/.cache/pipedeck/vencord-voices.log`.

## Install

In Pipedeck: Settings, Plug-ins, **Discord voices**, Install. Vencord only
runs the plugins built into it, so Pipedeck clones Vencord into
`~/.cache/pipedeck/vencord`, puts the plugin in, builds it (with git and
Node.js; pnpm is fetched through npx), points Vesktop at the result and
turns the plugin on. Vesktop writes its settings when it quits, so the last
step waits for it to be closed. Update does the same again; Remove points
Vesktop back at its own Vencord.

Pipedeck says when an update is due. The build is marked with the plugin it
holds and the Vesktop it was made for — when and how big Vesktop's archive
was written, since packages keep a file's build date rather than the day it
was installed — and when either has changed since, a notice says so as
Pipedeck starts, with the way to the settings.

The plugin's sources are built into Pipedeck, so it installs them from
wherever it runs. `build.sh` does the build alone, from this directory, for
working on the plugin.

Client mods are against Discord's terms, and an update of Discord can break
the plugin at any time. Taking other people's voices apart is for their ears
with their agreement.
