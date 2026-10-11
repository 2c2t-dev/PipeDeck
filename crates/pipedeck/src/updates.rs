//! Whether a newer Pipedeck is out, asked of GitHub as Pipedeck starts and
//! once a day after, for as long as the settings say so.
//!
//! Only for the builds nothing else updates: a Flatpak's updates come with
//! the others the desktop installs, so it is not asked there. The question
//! goes through `curl`, which every desktop has, rather than through a web
//! client built into Pipedeck for this one request.

use std::time::Duration;

use adw::gtk::glib;
use libadwaita as adw;
use serde::Deserialize;

use crate::settings::Settings;
use crate::vesktop::version_in;

/// The newest release that is not a test one.
const LATEST: &str = "https://api.github.com/repos/2c2t-dev/PipeDeck/releases/latest";
/// Where a release's page is: anything else the answer names is not opened.
const PAGES: &str = "https://github.com/2c2t-dev/PipeDeck/releases/";
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// A release newer than this Pipedeck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub page: String,
}

/// Ask now, then every day, while the setting is on; `newer` is told of a
/// newer release once, however many times it is found again.
pub fn watch(newer: impl Fn(Release) + 'static) {
    if crate::launcher::sandboxed() {
        return;
    }
    glib::spawn_future_local(async move {
        let mut told: Option<String> = None;
        loop {
            if Settings::load().check_updates {
                let (tx, rx) = async_channel::bounded(1);
                std::thread::spawn(move || {
                    let _ = tx.send_blocking(latest());
                });
                if let Ok(Some(release)) = rx.recv().await {
                    if told.as_deref() != Some(release.version.as_str()) {
                        told = Some(release.version.clone());
                        newer(release);
                    }
                }
            }
            glib::timeout_future(DAY).await;
        }
    });
}

/// The latest release, when it is newer than this Pipedeck.
fn latest() -> Option<Release> {
    let output = std::process::Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "20",
            "--header",
            "Accept: application/vnd.github+json",
            "--user-agent",
            concat!("Pipedeck/", env!("CARGO_PKG_VERSION")),
            LATEST,
        ])
        .output();
    let output = match output {
        Ok(output) => output,
        Err(e) => {
            log::info!("cannot look for a newer Pipedeck, no curl: {e}");
            return None;
        }
    };
    if !output.status.success() {
        log::info!(
            "cannot look for a newer Pipedeck: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }
    newer_than(
        env!("CARGO_PKG_VERSION"),
        &String::from_utf8_lossy(&output.stdout),
    )
}

/// The release GitHub's answer is about, when it is newer than `current`.
fn newer_than(current: &str, answer: &str) -> Option<Release> {
    #[derive(Deserialize)]
    struct Answer {
        tag_name: String,
        html_url: String,
    }
    let answer: Answer = serde_json::from_str(answer).ok()?;
    let version = answer.tag_name.trim_start_matches('v');
    (version_in(version)? > version_in(current)? && answer.html_url.starts_with(PAGES)).then(|| {
        Release {
            version: version.to_owned(),
            page: answer.html_url,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(tag: &str) -> String {
        format!(
            r#"{{"tag_name": "{tag}", "name": "Pipedeck {tag}", "draft": false,
                "html_url": "https://github.com/2c2t-dev/PipeDeck/releases/tag/{tag}"}}"#
        )
    }

    #[test]
    fn a_newer_release_is_found() {
        assert_eq!(
            newer_than("0.1.3", &answer("v0.1.4")),
            Some(Release {
                version: "0.1.4".into(),
                page: "https://github.com/2c2t-dev/PipeDeck/releases/tag/v0.1.4".into(),
            })
        );
        assert!(newer_than("0.1.3", &answer("v0.2.0")).is_some());
        assert!(newer_than("0.9.3", &answer("v1.0.0")).is_some());
    }

    #[test]
    fn the_same_or_an_older_release_is_not() {
        assert_eq!(newer_than("0.1.3", &answer("v0.1.3")), None);
        assert_eq!(newer_than("0.1.3", &answer("v0.1.2")), None);
        assert_eq!(newer_than("0.10.0", &answer("v0.9.0")), None);
    }

    #[test]
    fn a_strange_answer_is_left_alone() {
        assert_eq!(newer_than("0.1.3", "Not Found"), None);
        assert_eq!(
            newer_than("0.1.3", r#"{"message": "API rate limit exceeded"}"#),
            None
        );
        assert_eq!(newer_than("0.1.3", &answer("nightly")), None);
        assert_eq!(
            newer_than(
                "0.1.3",
                r#"{"tag_name": "v0.1.4", "html_url": "https://example.com/v0.1.4"}"#
            ),
            None
        );
    }
}
