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

/// How many of the looks are offered as kinds when creating a channel. The
/// rest exist only in the icon picker.
pub const KINDS: usize = 8;

pub const PRESETS: &[Preset] = &[
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
        key: "aux2",
        label: "Aux 2",
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
    &PRESETS[..KINDS]
}

/// The preset a channel was created from, if it still exists.
pub fn find(key: Option<&str>) -> Option<&'static Preset> {
    let key = key?;
    PRESETS.iter().find(|preset| preset.key == key)
}

/// Icon of a channel: its preset's, or the neutral one for its kind.
pub fn icon_for(key: Option<&str>, is_input: bool) -> &'static str {
    match find(key) {
        Some(preset) => preset.icon,
        None if is_input => "pd-mic-symbolic",
        None => "pd-speaker-symbolic",
    }
}

/// CSS class colouring a badge, empty for a channel with no preset.
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
    fn a_channel_without_a_preset_keeps_a_neutral_icon() {
        assert_eq!(icon_for(Some("game"), false), "pd-game-symbolic");
        assert_eq!(icon_for(None, true), "pd-mic-symbolic");
        assert_eq!(icon_for(Some("gone"), false), "pd-speaker-symbolic");
        assert!(badge_class(Some("music")).is_some());
        assert!(badge_class(None).is_none());
    }
}
