//! Match desktop launchers to streams, whose client may hold their identity.

use libspa::utils::dict::DictRef;

use crate::types::{SourceConfig, SourceId};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Identity {
    pub key: String,
    pub aliases: Vec<String>,
}

impl Identity {
    pub fn read(node: &DictRef, client: Option<&DictRef>) -> Option<Self> {
        let client_prop = |key| client.and_then(|props| props.get(key));
        // PulseAudio streams may share a server client. Its executable is
        // then pipewire, not the application named by the stream.
        let same_application = node.get("application.name").is_none_or(|name| {
            client_prop("application.name").is_some_and(|client_name| client_name == name)
        });
        let app_client_prop = |key| same_application.then(|| client_prop(key)).flatten();
        // The portal's id agrees with Flatpak desktop entries. It lives on
        // the client, not necessarily on any of that client's audio nodes.
        let candidates = [
            client_prop("pipewire.access.portal.app_id"),
            node.get("pipewire.access.portal.app_id"),
            node.get("application.process.binary"),
            app_client_prop("application.process.binary"),
            node.get("application.id"),
            app_client_prop("application.id"),
            node.get("application.name"),
            app_client_prop("application.name"),
            node.get("node.name"),
        ];
        let mut aliases = Vec::new();
        for value in candidates.into_iter().flatten().filter(|s| !s.is_empty()) {
            if !aliases.iter().any(|alias| alias == value) {
                aliases.push(value.to_owned());
            }
        }
        Some(Self {
            key: aliases.first()?.clone(),
            aliases,
        })
    }

    pub fn matches(&self, key: &str) -> bool {
        self.aliases.iter().any(|alias| alias == key)
    }

    pub fn source(&self, sources: &[SourceConfig]) -> Option<SourceId> {
        self.source_except(sources, None)
    }

    pub fn source_except(
        &self,
        sources: &[SourceConfig],
        except: Option<SourceId>,
    ) -> Option<SourceId> {
        // An explicit assignment of the stable id beats an older assignment
        // of the same application's display name or binary on another row.
        self.aliases.iter().find_map(|key| {
            sources
                .iter()
                .find(|source| Some(source.id) != except && source.apps.contains(key))
                .map(|source| source.id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pipewire::properties::properties;

    #[test]
    fn spotify_uses_its_flatpak_id_from_the_client() {
        let node = properties! { "application.name" => "Spotify", "node.name" => "spotify" };
        let client = properties! { "pipewire.access.portal.app_id" => "com.spotify.Client" };
        let identity = Identity::read(node.as_ref(), Some(client.as_ref())).unwrap();
        assert_eq!(identity.key, "com.spotify.Client");
        assert!(identity.matches("Spotify"));
        assert!(identity.matches("spotify"));
    }

    #[test]
    fn flatpak_browsers_keep_their_binary_as_a_legacy_alias() {
        for (id, binary, name) in [
            ("com.microsoft.Edge", "msedge", "Microsoft Edge"),
            ("org.mozilla.firefox", "firefox", "Firefox"),
            ("com.brave.Browser", "brave", "Brave"),
        ] {
            let node =
                properties! { "application.process.binary" => binary, "application.name" => name };
            let client = properties! { "pipewire.access.portal.app_id" => id };
            let identity = Identity::read(node.as_ref(), Some(client.as_ref())).unwrap();
            assert_eq!(identity.key, id);
            assert!(identity.matches(binary));
            assert!(identity.matches(name));
        }
    }

    #[test]
    fn native_apps_use_the_binary_even_when_the_client_supplies_it() {
        let node = properties! { "application.name" => "Firefox" };
        let client = properties! { "application.name" => "Firefox", "application.process.binary" => "firefox", "application.id" => "org.mozilla.firefox" };
        let identity = Identity::read(node.as_ref(), Some(client.as_ref())).unwrap();
        assert_eq!(identity.key, "firefox");
        assert!(identity.matches("org.mozilla.firefox"));
    }

    #[test]
    fn a_shared_audio_server_client_does_not_become_the_application() {
        let node = properties! { "application.name" => "Firefox", "node.name" => "Firefox" };
        let client = properties! { "application.name" => "PipeWire", "application.process.binary" => "pipewire" };
        let identity = Identity::read(node.as_ref(), Some(client.as_ref())).unwrap();
        assert_eq!(identity.key, "Firefox");
        assert!(!identity.matches("pipewire"));
        assert!(!identity.matches("PipeWire"));
    }

    #[test]
    fn late_client_identity_preserves_assignments_and_prefers_the_stable_id() {
        let node = properties! { "application.name" => "Spotify" };
        let client = properties! { "pipewire.access.portal.app_id" => "com.spotify.Client" };
        let before = Identity::read(node.as_ref(), None).unwrap();
        let after = Identity::read(node.as_ref(), Some(client.as_ref())).unwrap();
        let mut config = crate::config::Config {
            sources: vec![
                SourceConfig::virtual_sink(SourceId(1), "System"),
                SourceConfig::virtual_sink(SourceId(2), "Music"),
            ],
            ..Default::default()
        };
        config.sources[0].apps = vec!["Spotify".into()];
        let old_source = config.sources[0].id;
        assert_eq!(before.source(&config.sources), Some(old_source));
        assert_eq!(after.source(&config.sources), Some(old_source));
        config.sources[1].apps = vec!["com.spotify.Client".into()];
        assert_eq!(after.source(&config.sources), Some(config.sources[1].id));
    }

    #[test]
    fn empty_ids_fall_back_without_matching_unrelated_apps() {
        let node = properties! { "application.name" => "", "node.name" => "player" };
        let client = properties! { "pipewire.access.portal.app_id" => "" };
        let identity = Identity::read(node.as_ref(), Some(client.as_ref())).unwrap();
        assert_eq!(identity.key, "player");
        assert!(!identity.matches("com.spotify.Client"));
        assert!(
            Identity::read(pipewire::properties::PropertiesBox::new().as_ref(), None).is_none()
        );
    }
}
