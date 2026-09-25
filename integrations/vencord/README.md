# Pipedeck voices for Vesktop

A Vencord plugin that hands each person of a Discord call to Pipedeck as a
track of their own, so a Discord channel can unfold into one sub-track per
participant.

Vesktop plays Discord's voice through Chromium's WebRTC, where every person
arrives as a separate audio track; Discord's own client mixes them in native
code, out of reach, which is why this is for Vesktop only.

**For now it only looks.** It writes down what Discord's web voice does
with each person's audio to `~/.cache/pipedeck/vencord-voices.log`, and
changes nothing in what is heard.

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
