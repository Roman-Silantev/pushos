//! Installing and managing packs.
//!
//! Nothing is written until the operator has seen what installing would change.
//! That is the whole point of the review: a pack that can deploy should look
//! different on screen from one that draws a page, and the difference should be
//! visible without reading TOML.

use std::io::{IsTerminal, Write};
use std::path::Path;

use pushos_config::RuntimeConfig;
use pushos_domain::ids::PackId;
use pushos_domain::pack::{PackState, Requirements};
use pushos_domain::permissions::PermissionSet;
use pushos_packs::{Library, Review};

use super::paths;

/// Lists what is installed.
pub(crate) fn list(requested: Option<&Path>) -> Result<(), String> {
    let library = Library::at(paths::config_root(requested)?);
    let installed = library.installed();

    if installed.is_empty() {
        println!("no packs are installed");
        println!("install one with `pushos pack install <directory>`");
        return Ok(());
    }

    let widest = installed
        .iter()
        .map(|held| held.pack.id.as_str().len())
        .max()
        .unwrap_or(0);

    for held in installed {
        println!(
            "{:<widest$}  {:<8}  {}  ({})",
            held.pack.id.as_str(),
            held.state.describe(),
            held.pack.version,
            held.pack.adds,
            widest = widest
        );
    }
    Ok(())
}

/// Says what a pack is and what installing it would change.
pub(crate) fn show(requested: Option<&Path>, path: &Path) -> Result<(), String> {
    let root = paths::config_root(requested)?;
    let review = review_of(requested, path)?;
    print_review(&review);

    match pushos_packs::check(path, &root, &review.granting) {
        Ok(()) => println!("\nit would install here"),
        Err(problems) => {
            println!("\nit would not install here:");
            for problem in problems {
                println!("  {problem}");
            }
        }
    }
    Ok(())
}

/// Installs a pack, after showing what it would change.
pub(crate) fn install(
    requested: Option<&Path>,
    path: &Path,
    yes: bool,
    force: bool,
) -> Result<(), String> {
    let root = paths::config_root(requested)?;
    let review = review_of(requested, path)?;
    print_review(&review);

    // Reported before the question rather than after the answer: an operator
    // should not agree to something and then be told it was never going to work.
    if let Err(problems) = pushos_packs::check(path, &root, &review.granting) {
        println!("\nit would not install here:");
        for problem in &problems {
            println!("  {problem}");
        }
        return Err("nothing was installed".to_owned());
    }

    if !agreed(&review, yes)? {
        println!("nothing was installed");
        return Ok(());
    }

    let pack = Library::at(&root)
        .install(path, &review.granting, force)
        .map_err(|error| error.to_string())?;

    println!("\ninstalled {} {} ({})", pack.id, pack.version, pack.adds);
    if !review.works_fully() {
        let missing = &review.missing_providers;
        println!(
            "it expects {}, which {} not configured; add one under `[[providers]]`",
            missing.join(", "),
            if missing.len() == 1 { "is" } else { "are" }
        );
    }
    println!("run `pushos check` to see the surface it made");
    Ok(())
}

/// Removes an installed pack.
pub(crate) fn remove(requested: Option<&Path>, pack: &str) -> Result<(), String> {
    let library = Library::at(paths::config_root(requested)?);
    let id = PackId::new(pack);

    library.remove(&id).map_err(|error| error.to_string())?;
    println!("removed {id}");
    println!("anything you wrote yourself is untouched");
    Ok(())
}

/// Turns an installed pack on or off.
pub(crate) fn set_state(
    requested: Option<&Path>,
    pack: &str,
    state: PackState,
) -> Result<(), String> {
    let library = Library::at(paths::config_root(requested)?);
    let id = PackId::new(pack);

    library
        .set_state(&id, state)
        .map_err(|error| error.to_string())?;
    println!("{id} is {}", state.describe());
    Ok(())
}

/// Reads a pack and works out what installing it would mean here.
fn review_of(requested: Option<&Path>, path: &Path) -> Result<Review, String> {
    let root = paths::config_root(requested)?;
    let pack = pushos_packs::read(path).map_err(|error| error.to_string())?;

    // What is already configured, so the review can say what would change
    // rather than repeating the whole list.
    let (granted, providers) = match pushos_config::load(&root).and_then(|file| {
        let providers = file
            .providers
            .iter()
            .map(|provider| provider.id.clone())
            .collect::<Vec<_>>();
        RuntimeConfig::build(&file).map(|config| (config.permissions.clone(), providers))
    }) {
        Ok(both) => both,
        // A configuration that does not currently build is worth saying, but it
        // is not a reason to refuse to describe a pack.
        Err(error) => {
            eprintln!("note: the configuration here does not currently build: {error}");
            (PermissionSet::empty(), Vec::new())
        }
    };

    Ok(Review::of(pack, &granted, &providers))
}

/// Prints the review an operator agrees to.
fn print_review(review: &Review) {
    let pack = &review.pack;
    println!("{} {} ({})", pack.name, pack.version, pack.id);
    if let Some(description) = &pack.description {
        for line in description.lines().map(str::trim).filter(|l| !l.is_empty()) {
            println!("  {line}");
        }
    }
    if let Some(author) = &pack.author {
        println!("  by {author}");
    }

    println!("\nadds:");
    if pack.adds.is_empty() {
        println!("  nothing");
    }
    for line in pack.adds.described() {
        println!("  {line}");
    }

    print_requests(&pack.requires, review);

    if !review.missing_providers.is_empty() {
        println!("\nexpects agent providers that are not configured:");
        for provider in &review.missing_providers {
            println!("  {provider}");
        }
        println!("  the pack still installs; roles name providers in preference order");
    }
}

/// Prints what a pack is asking for, and what that would change.
fn print_requests(requires: &Requirements, review: &Review) {
    if requires.is_empty() {
        println!("\nasks for nothing");
        return;
    }

    if review.granting.is_empty() {
        println!("\nasks for nothing you have not already allowed");
    } else {
        println!("\nwould grant:");
        for permission in &review.granting {
            let mark = if permission.is_irreversible() {
                "  !"
            } else {
                "  +"
            };
            println!("{mark} {permission}");
        }
    }

    for permission in &review.already {
        println!("  = {permission} (already allowed)");
    }
    for permission in &requires.optional {
        println!("  ? {permission} (optional; the pack works without it)");
    }

    if review.grants_something_irreversible() {
        println!("\n! marks something PushOS cannot undo. Read those twice.");
    }
}

/// Whether the operator agrees to install.
///
/// A pack that asks for nothing new is still confirmed, because installing
/// changes what the surface does. `--yes` is for scripts; with no terminal
/// attached there is nobody to ask, so the answer is no.
fn agreed(review: &Review, yes: bool) -> Result<bool, String> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Err(
            "nothing was installed: there is nobody to ask. Pass --yes to install without \
             confirming."
                .to_owned(),
        );
    }

    print!("\ninstall {}? [y/N] ", review.pack.id);
    std::io::stdout()
        .flush()
        .map_err(|error| format!("could not ask: {error}"))?;

    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|error| format!("could not read the answer: {error}"))?;

    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

#[cfg(test)]
mod tests {
    use pushos_config::{ConfigFile, RuntimeConfig};
    use pushos_domain::ids::ExecutionId;
    use pushos_domain::pack::{Contents, Pack};
    use pushos_domain::permissions::Permission;
    use pushos_packs::{Library, read};

    use super::*;

    fn review(permissions: Vec<Permission>, granted: &[Permission]) -> Review {
        let pack = Pack {
            id: PackId::new("seo"),
            name: "SEO".to_owned(),
            version: "1.0.0".to_owned(),
            description: None,
            author: None,
            license: None,
            requires: Requirements {
                permissions,
                ..Requirements::default()
            },
            adds: Contents::default(),
        };

        let mut set = PermissionSet::empty();
        for permission in granted {
            set.grant(*permission);
        }
        Review::of(pack, &set, &[])
    }

    #[test]
    fn asking_is_the_default_and_yes_is_deliberate() {
        // A pack changes what the surface does, so installing one is a decision
        // rather than a download.
        let review = review(vec![Permission::ShellExecute], &[]);
        assert!(agreed(&review, true).expect("--yes needs no terminal"));
    }

    #[test]
    fn with_nobody_to_ask_nothing_is_installed() {
        // Tests and scripts have no terminal. Taking silence for agreement is
        // how a pack gets installed by a pipeline nobody watched.
        let review = review(vec![Permission::DeployProduction], &[]);
        let error = agreed(&review, false).expect_err("there is nobody to ask");
        assert!(error.contains("--yes"), "{error}");
    }

    /// Where the packs that ship with PushOS live.
    fn shipped() -> Vec<std::path::PathBuf> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
        let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
            .unwrap_or_else(|error| panic!("`{}` should be readable: {error}", root.display()))
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.join(pushos_config::paths::PACK_MANIFEST).is_file())
            .collect();
        found.sort();

        assert!(!found.is_empty(), "PushOS should ship at least one pack");
        found
    }

    /// A configuration root with a home page and nothing else.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("pushos-shipped-{}", ExecutionId::generate()));
            std::fs::create_dir_all(&path).expect("the temporary directory is writable");
            std::fs::write(
                path.join(pushos_config::paths::MAIN_FILE),
                "[runtime]\nhome_page = \"home\"\n\n[[pages]]\nid = \"home\"\nname = \"Home\"\n",
            )
            .expect("writable");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn every_shipped_pack_says_what_it_is_and_adds_something() {
        for path in shipped() {
            let pack = read(&path)
                .unwrap_or_else(|error| panic!("`{}` should be a pack: {error}", path.display()));
            assert!(
                !pack.adds.is_empty(),
                "`{}` contributes nothing to the surface",
                pack.id
            );
            assert!(
                pack.description.is_some(),
                "`{}` should say what it is for",
                pack.id
            );
        }
    }

    #[test]
    fn every_shipped_pack_installs_onto_a_plain_configuration() {
        // Which also proves it makes no ambiguous binding, names no page that
        // does not exist and uses no control PushOS cannot address.
        for path in shipped() {
            let scratch = Scratch::new();
            let pack = read(&path).expect("a shipped pack");

            Library::at(&scratch.0)
                .install(&path, &pack.requires.permissions, false)
                .unwrap_or_else(|error| panic!("`{}` should install: {error}", pack.id));

            RuntimeConfig::build(&pushos_config::load(&scratch.0).expect("readable"))
                .unwrap_or_else(|error| {
                    let problems: Vec<String> =
                        error.problems().iter().map(ToString::to_string).collect();
                    panic!("`{}` made an invalid surface: {problems:?}", pack.id)
                });
        }
    }

    #[test]
    fn every_shipped_pack_asks_for_exactly_what_its_bindings_need() {
        // Both halves of one promise. Asking for less means a pad that fails
        // under a finger; asking for more teaches the operator to skim.
        for path in shipped() {
            let scratch = Scratch::new();
            let pack = read(&path).expect("a shipped pack");

            Library::at(&scratch.0)
                .install(&path, &pack.requires.permissions, false)
                .expect("installs");

            let parsed = pushos_config::load(&scratch.0).expect("readable");
            let config = RuntimeConfig::build(&parsed).expect("valid");
            let required = crate::host::shipped_permissions(&config);

            let mut needed: Vec<Permission> = Vec::new();
            for binding in &parsed.bindings {
                for permission in required.get(&binding.action).into_iter().flatten() {
                    if !needed.contains(permission) {
                        needed.push(*permission);
                    }
                }
            }

            for permission in &needed {
                assert!(
                    pack.requires.permissions.contains(permission),
                    "`{}` binds something needing `{permission}` and never asks for it",
                    pack.id
                );
            }
            for permission in &pack.requires.permissions {
                assert!(
                    needed.contains(permission),
                    "`{}` asks for `{permission}` and never uses it",
                    pack.id
                );
            }
        }
    }

    #[test]
    fn every_shipped_pack_stays_on_its_own_pages() {
        // A pack binding a control globally would change what a surface the
        // operator already set up does, which is not what installing means.
        for path in shipped() {
            let pack = read(&path).expect("a shipped pack");
            let mut merged = ConfigFile::default();
            for file in pushos_config::paths::files_in_pack(&path) {
                let text = std::fs::read_to_string(&file).expect("readable");
                merged.merge(toml::from_str(&text).expect("valid TOML"));
            }

            let pages: Vec<&str> = merged.pages.iter().map(|page| page.id.as_str()).collect();
            for binding in &merged.bindings {
                let page = binding.page.as_deref().unwrap_or("");
                assert!(
                    pages.contains(&page),
                    "`{}` binds `{}` outside its own pages",
                    pack.id,
                    binding.control
                );
            }
        }
    }

    #[test]
    fn every_shipped_pack_can_be_removed_without_taking_anything_else() {
        for path in shipped() {
            let scratch = Scratch::new();
            let pack = read(&path).expect("a shipped pack");
            let library = Library::at(&scratch.0);

            library
                .install(&path, &pack.requires.permissions, false)
                .expect("installs");
            library.remove(&pack.id).expect("removes");

            let config = RuntimeConfig::build(&pushos_config::load(&scratch.0).expect("readable"))
                .expect("still valid");
            assert_eq!(
                config.pages.len(),
                1,
                "`{}` should have taken only its own pages",
                pack.id
            );
            assert_eq!(
                config.permissions.iter().count(),
                0,
                "`{}` should have taken its grant with it",
                pack.id
            );
        }
    }

    #[test]
    fn every_capability_a_pack_can_ask_for_can_be_written_down() {
        // A manifest names capabilities as text, so every one of them has to
        // survive being written and read back.
        for permission in Permission::ALL {
            let written = permission.to_string();
            assert_eq!(
                written.parse::<Permission>().ok(),
                Some(permission),
                "`{written}` does not read back"
            );
        }
    }
}
