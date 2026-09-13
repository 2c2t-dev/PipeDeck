//! The kinds of empty channel offered when creating one.
//!
//! A preset is only a starting point: it fills in a name, an icon and a
//! colour. The engine stores its key on the channel and knows nothing else
//! about it, which keeps the look of the mixer out of the audio graph.

/// Icon names are picked to exist in the two icon themes a Linux desktop is
/// most likely to be running, since a missing one shows as a broken image.
/// [`crate::widgets::badge`] falls back at runtime for the rest.
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

pub const PRESETS: &[Preset] = &[
    Preset {
        key: "music",
        label: "Music",
        icon: "multimedia-player-symbolic",
        color: "#e35db5",
    },
    Preset {
        key: "browser",
        label: "Browser",
        icon: "internet-web-browser-symbolic",
        color: "#9b6ef3",
    },
    Preset {
        key: "system",
        label: "System",
        icon: "computer-symbolic",
        color: "#3a8ee6",
    },
    Preset {
        key: "game",
        label: "Game",
        icon: "applications-games-symbolic",
        color: "#e6584f",
    },
    Preset {
        key: "sfx",
        label: "SFX",
        icon: "media-playback-start-symbolic",
        color: "#f08a24",
    },
    Preset {
        key: "voice",
        label: "Voice chat",
        icon: "call-start-symbolic",
        color: "#e8c33a",
    },
    Preset {
        key: "aux1",
        label: "Aux 1",
        icon: "audio-card-symbolic",
        color: "#2fb9ad",
    },
    Preset {
        key: "aux2",
        label: "Aux 2",
        icon: "audio-speakers-symbolic",
        color: "#4cc26a",
    },
];

/// The preset a channel was created from, if it still exists.
pub fn find(key: Option<&str>) -> Option<&'static Preset> {
    let key = key?;
    PRESETS.iter().find(|preset| preset.key == key)
}

/// Icon of a channel: its preset's, or the neutral one for its kind.
pub fn icon_for(key: Option<&str>, is_input: bool) -> &'static str {
    match find(key) {
        Some(preset) => preset.icon,
        None if is_input => "audio-input-microphone-symbolic",
        None => "audio-speakers-symbolic",
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
        assert_eq!(icon_for(Some("game"), false), "applications-games-symbolic");
        assert_eq!(icon_for(None, true), "audio-input-microphone-symbolic");
        assert_eq!(icon_for(Some("gone"), false), "audio-speakers-symbolic");
        assert!(badge_class(Some("music")).is_some());
        assert!(badge_class(None).is_none());
    }
}
