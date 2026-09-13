//! The kinds of empty channel offered when creating one.
//!
//! A preset is only a starting point: it fills in a name, an icon and a
//! colour. The engine stores its key on the channel and knows nothing else
//! about it, which keeps the look of the mixer out of the audio graph.

/// Icons are bundled with the application rather than taken from the
/// desktop's icon theme, so a row of them looks like one set wherever it
/// runs. See `crates/pipedeck/icons`.
///
/// A ready-made channel identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    /// Stored on the channel, so its look survives a restart.
    pub key: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    /// Background of the badge, with a dark glyph on top.
    pub color: &'static str,
}

/// The catalogue is laid out as numbers, then the kinds a channel can be
/// created as, then looks that exist only in the picker.
const NUMBERS: usize = 5;
pub const KINDS: usize = 8;

pub const PRESETS: &[Preset] = &[
    // Numbers come first: they are what a mix is most often given.
    Preset {
        key: "one",
        label: "1",
        icon: "pd-1-symbolic",
        color: "#8a93a6",
    },
    Preset {
        key: "two",
        label: "2",
        icon: "pd-2-symbolic",
        color: "#8a93a6",
    },
    Preset {
        key: "three",
        label: "3",
        icon: "pd-3-symbolic",
        color: "#8a93a6",
    },
    Preset {
        key: "four",
        label: "4",
        icon: "pd-4-symbolic",
        color: "#8a93a6",
    },
    Preset {
        key: "five",
        label: "5",
        icon: "pd-5-symbolic",
        color: "#8a93a6",
    },
    Preset {
        key: "music",
        label: "Music",
        icon: "pd-music-symbolic",
        color: "#e35db5",
    },
    Preset {
        key: "browser",
        label: "Browser",
        icon: "pd-browser-symbolic",
        color: "#9b6ef3",
    },
    Preset {
        key: "system",
        label: "System",
        icon: "pd-system-symbolic",
        color: "#3a8ee6",
    },
    Preset {
        key: "game",
        label: "Game",
        icon: "pd-game-symbolic",
        color: "#e6584f",
    },
    Preset {
        key: "sfx",
        label: "SFX",
        icon: "pd-sfx-symbolic",
        color: "#f08a24",
    },
    Preset {
        key: "voice",
        label: "Voice chat",
        icon: "pd-voice-symbolic",
        color: "#e8c33a",
    },
    Preset {
        key: "aux1",
        label: "Aux 1",
        icon: "pd-aux-symbolic",
        color: "#2fb9ad",
    },
    Preset {
        key: "speaker",
        label: "Speaker",
        icon: "pd-speaker-symbolic",
        color: "#4cc26a",
    },
    // Past this point the looks are only offered by the icon picker.
    Preset {
        key: "headset",
        label: "Headset",
        icon: "pd-headset-symbolic",
        color: "#3ba7c9",
    },
    Preset {
        key: "mic",
        label: "Microphone",
        icon: "pd-mic-symbolic",
        color: "#d8594f",
    },
    Preset {
        key: "star",
        label: "Star",
        icon: "pd-star-symbolic",
        color: "#e0b13a",
    },
    Preset {
        key: "people",
        label: "People",
        icon: "pd-people-symbolic",
        color: "#5b8cf5",
    },
    Preset {
        key: "video",
        label: "Video",
        icon: "pd-video-symbolic",
        color: "#a563e8",
    },
    Preset {
        key: "stream",
        label: "Stream",
        icon: "pd-stream-symbolic",
        color: "#e35db5",
    },
    Preset {
        key: "record",
        label: "Record",
        icon: "pd-record-symbolic",
        color: "#e0514b",
    },
    Preset {
        key: "fun",
        label: "Fun",
        icon: "pd-fun-symbolic",
        color: "#7ec44f",
    },
];

/// The looks offered as kinds when a channel is created.
pub fn kinds() -> &'static [Preset] {
    &PRESETS[NUMBERS..NUMBERS + KINDS]
}

/// What a channel can wear: everything but the numbers, which say which mix
/// is which and mean nothing on a row.
pub fn channel_looks() -> &'static [Preset] {
    &PRESETS[NUMBERS..]
}

/// The look a key names, if it still exists.
pub fn find(key: Option<&str>) -> Option<&'static Preset> {
    let key = key?;
    PRESETS.iter().find(|preset| preset.key == key)
}

/// The key an object falls back to when it has chosen no look.
///
/// A default that is one of the looks rather than a colourless icon of its
/// own means the picker always has something to point at, and a card never
/// stands out for having made no choice.
pub fn default_key(is_input: bool) -> &'static str {
    if is_input {
        "mic"
    } else {
        "speaker"
    }
}

/// The look to draw for an object, whether or not it chose one.
pub fn look(key: Option<&str>, is_input: bool) -> &'static Preset {
    find(key)
        .or_else(|| find(Some(default_key(is_input))))
        .expect("the default look is in the catalogue")
}

/// CSS class colouring a badge.
pub fn badge_class(key: Option<&str>) -> Option<String> {
    find(key).map(|preset| format!("pd-badge-{}", preset.key))
}

/// The colour rules the presets need, appended to the stylesheet.
pub fn css() -> String {
    let mut css = String::new();
    for preset in PRESETS {
        css.push_str(&format!(
            ".pd-badge-{key}, .pd-badge-large.pd-badge-{key} {{ background-color: {color}; color: #12141a; }}\n",
            key = preset.key,
            color = preset.color,
        ));
    }
    css
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_the_head_of_the_catalogue() {
        assert_eq!(kinds().len(), KINDS);
        assert!(
            PRESETS.len() > KINDS,
            "the picker needs more than the kinds"
        );
        let mut keys: Vec<&str> = PRESETS.iter().map(|p| p.key).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "two looks share a key");
    }

    #[test]
    fn numbers_are_for_mixes_only() {
        let numbered: Vec<&str> = PRESETS[..NUMBERS].iter().map(|p| p.key).collect();
        assert_eq!(numbered.len(), NUMBERS);
        for look in channel_looks() {
            assert!(
                !numbered.contains(&look.key),
                "a channel was offered the number {}",
                look.label
            );
        }
        assert_eq!(channel_looks().len(), PRESETS.len() - NUMBERS);
    }

    #[test]
    fn every_preset_has_a_colour_rule() {
        let css = css();
        for preset in PRESETS {
            assert!(
                css.contains(&format!(".pd-badge-{}", preset.key)),
                "{}",
                preset.key
            );
            assert!(css.contains(preset.color), "{}", preset.color);
        }
    }

    #[test]
    fn an_object_without_a_look_falls_back_into_the_catalogue() {
        assert_eq!(look(Some("game"), false).icon, "pd-game-symbolic");
        assert_eq!(look(None, true).icon, "pd-mic-symbolic");
        assert_eq!(look(Some("gone"), false).icon, "pd-speaker-symbolic");
        // Whatever an object carries, the look it is drawn with is one of
        // the catalogue, so the picker can always point at it.
        for key in [None, Some("gone"), Some("music")] {
            assert!(PRESETS.iter().any(|p| p.key == look(key, false).key));
        }
        assert!(badge_class(Some("music")).is_some());
    }
}
