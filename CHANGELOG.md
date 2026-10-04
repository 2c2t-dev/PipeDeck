# Changelog

What changes from one release of Pipedeck to the next. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0]

The first release.

### Mixer

- A matrix of channels and mixes, with a fader and a mute in every cell.
- Channels for applications or capture devices, and applications attached
  to a channel moved onto it as they start playing.
- Up to five mixes, each an input device for OBS or a call, playing to any
  number of output devices.
- Meters on every channel, mix and cell.
- Choosing the headphones you listen on, and which mixes you hear there.
- Levels kept in step with the desktop's volume controls.
- Surviving PipeWire and WirePlumber restarts.

### Effects

- Noise suppression, an equaliser, a de-esser and a compressor, built in.
- VST3 plug-ins, and Thimeo's Stereo Tool, on a channel.
- Switching an effect off without removing it.

### Integrations

- OpenDeck and StreamController plugins, with ready-made layouts for the
  Stream Deck, Stream Deck + and XL.
- With Vesktop, a Discord call split into one sub-track per person.
- On KDE, sending the application in front to a channel from a Stream Deck
  key.

### Desktop

- Keeping the mixer running in the notification area with its window
  closed, and starting with the session.
- .deb, .rpm and AppImage packages.

[Unreleased]: https://github.com/2c2t-dev/PipeDeck/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/2c2t-dev/PipeDeck/releases/tag/v0.1.0
