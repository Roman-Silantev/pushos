//! Keeping the downloads that start an agent from piling up.
//!
//! Claude's and Codex's adapters are started with `npx`, which fetches the
//! latest release when one has come out. npm keeps every package it has ever
//! fetched in its cache and never throws any of them away, so each release of
//! an adapter would add its whole download to the disk for good.
//!
//! The adapters PushOS starts are therefore given a cache of their own, and
//! before one starts that cache is kept under a size. Only the store of
//! downloads is cleared: what `npx` installed stays, so an agent still starts
//! with the network down, and the next release is fetched once when it comes.

use std::path::Path;

use tracing::info;

/// The variable npm reads its cache directory from.
pub const NPM_CACHE: &str = "npm_config_cache";

/// The most npm's store of downloads may hold before it is cleared.
///
/// A little over the two adapters' current downloads together, so a single
/// release does not clear it and a handful of releases always does.
pub const CAP: u64 = 768 * 1024 * 1024;

/// Where, inside an npm cache, the downloads are stored.
const DOWNLOADS: &str = "_cacache";

/// Clears the downloads in an npm cache when they have grown past `cap`.
///
/// Returns how much was freed. Nothing but npm's own store of downloads is
/// touched, and only inside the cache PushOS gave the adapters.
pub fn keep_under(cache: &Path, cap: u64) -> u64 {
    let downloads = cache.join(DOWNLOADS);
    let held = size_of(&downloads);
    if held <= cap {
        return 0;
    }
    if std::fs::remove_dir_all(&downloads).is_err() {
        return 0;
    }
    info!(
        freed_bytes = held,
        cache = %cache.display(),
        "cleared the agent adapters' old downloads"
    );
    held
}

/// The size of everything under a directory, links not followed.
fn size_of(path: &Path) -> u64 {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    std::fs::read_dir(path).map_or(0, |entries| {
        entries.flatten().map(|entry| size_of(&entry.path())).sum()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(label: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("pushos-npm-cache-{label}-{}", std::process::id()));
        std::fs::create_dir_all(directory.join(DOWNLOADS).join("content")).expect("writable");
        std::fs::create_dir_all(directory.join("_npx/abc/node_modules")).expect("writable");
        std::fs::write(directory.join("_npx/abc/node_modules/adapter.js"), "x").expect("writable");
        directory
    }

    #[test]
    fn downloads_past_the_cap_are_cleared_and_the_installed_adapter_stays() {
        let directory = cache("over");
        std::fs::write(
            directory.join(DOWNLOADS).join("content/one"),
            vec![0_u8; 4_096],
        )
        .expect("writable");

        let freed = keep_under(&directory, 1_024);

        assert_eq!(freed, 4_096);
        assert!(!directory.join(DOWNLOADS).exists());
        assert!(
            directory.join("_npx/abc/node_modules/adapter.js").exists(),
            "the adapter still starts without the network"
        );
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn downloads_under_the_cap_are_left_alone() {
        let directory = cache("under");
        std::fs::write(
            directory.join(DOWNLOADS).join("content/one"),
            vec![0_u8; 512],
        )
        .expect("writable");

        assert_eq!(keep_under(&directory, 1_024), 0);
        assert!(directory.join(DOWNLOADS).join("content/one").exists());
        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn a_cache_that_does_not_exist_yet_is_nothing_to_clear() {
        assert_eq!(keep_under(Path::new("/definitely/not/a/cache"), 0), 0);
    }
}
