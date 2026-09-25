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

## Build and load

```sh
integrations/vencord/build.sh
```

This clones Vencord into `~/.cache/pipedeck/vencord` (or
`$PIPEDECK_VENCORD_DIR`), copies the plugin into it and builds it. Then, in
Vesktop: Settings, Vesktop Settings, Vencord Location, and pick the `dist`
folder it prints; restart Vesktop and turn on **PipedeckVoices** under
Vencord's plugins.

Client mods are against Discord's terms, and an update of Discord can break
the plugin at any time. Taking other people's voices apart is for their ears
with their agreement.
