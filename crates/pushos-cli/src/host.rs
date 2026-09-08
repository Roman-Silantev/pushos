//! Assembling the host-specific half of PushOS.
//!
//! One place decides which concrete adapters back the domain's ports. Swapping
//! a host means changing this file and nothing else.

use std::sync::Arc;

use pushos_actions::providers::{application, media, page, shell, shortcut};
use pushos_config::RuntimeConfig;
use pushos_domain::ports::{ActionProvider, ProcessRunner};
use pushos_macos::{AppleScriptMedia, OpenLauncher, ShortcutsCli, SystemProcessRunner};
use tracing::warn;

/// Builds every action provider PushOS ships with.
///
/// A provider whose backend cannot be configured is left out rather than
/// installed in a broken state, so a binding that references it fails with
/// "no provider" instead of failing halfway through doing something.
pub(crate) fn providers(config: &RuntimeConfig) -> Vec<Arc<dyn ActionProvider>> {
    let processes: Arc<dyn ProcessRunner> = Arc::new(SystemProcessRunner::new());

    let mut providers: Vec<Arc<dyn ActionProvider>> = vec![
        Arc::new(page::PageProvider::new()),
        Arc::new(application::ApplicationProvider::new(Arc::new(
            OpenLauncher::new(Arc::clone(&processes)),
        ))),
        Arc::new(shortcut::ShortcutProvider::new(Arc::new(
            ShortcutsCli::new(Arc::clone(&processes)),
        ))),
        Arc::new(shell::ShellProvider::new(Arc::clone(&processes))),
    ];

    if let Some(controller) = media_controller(config, &processes) {
        providers.push(Arc::new(media::MediaProvider::new(controller)));
    } else {
        warn!("media actions are unavailable; check the configured media player");
    }

    providers
}

fn media_controller(
    config: &RuntimeConfig,
    processes: &Arc<dyn ProcessRunner>,
) -> Option<Arc<dyn pushos_domain::ports::MediaController>> {
    let controller = match &config.media_player {
        Some(player) => AppleScriptMedia::for_player(Arc::clone(processes), player.as_str())
            .inspect_err(|error| warn!(%error, "the configured media player is not usable"))
            .ok()?,
        None => AppleScriptMedia::new(Arc::clone(processes)),
    };
    Some(Arc::new(controller))
}

#[cfg(test)]
mod tests {
    use pushos_config::ConfigFile;

    use super::*;

    fn config(text: &str) -> RuntimeConfig {
        let parsed: ConfigFile = toml::from_str(text).expect("well-formed");
        RuntimeConfig::build(&parsed).expect("valid")
    }

    #[test]
    fn every_shipped_namespace_is_present_by_default() {
        let namespaces: Vec<_> = providers(&config(""))
            .iter()
            .map(|provider| provider.name().to_string())
            .collect();

        for expected in ["page", "app", "shortcut", "shell", "media"] {
            assert!(
                namespaces.contains(&expected.to_owned()),
                "`{expected}` is missing"
            );
        }
    }

    #[test]
    fn namespaces_are_unique_so_the_registry_will_accept_them_all() {
        let mut namespaces: Vec<_> = providers(&config(""))
            .iter()
            .map(|provider| provider.name().to_string())
            .collect();
        namespaces.sort();
        let total = namespaces.len();
        namespaces.dedup();
        assert_eq!(total, namespaces.len());
    }

    #[test]
    fn an_unusable_media_player_leaves_the_namespace_unclaimed() {
        let unusable = config("[runtime]\nmedia_player = \"Music\\\" to quit\"\n");
        let namespaces: Vec<_> = providers(&unusable)
            .iter()
            .map(|provider| provider.name().to_string())
            .collect();

        assert!(
            !namespaces.contains(&"media".to_owned()),
            "a provider must not be installed in a broken state"
        );
        assert!(
            namespaces.contains(&"shell".to_owned()),
            "the rest still work"
        );
    }
}
