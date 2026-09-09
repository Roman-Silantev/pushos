//! Installing packs, against real directories.
//!
//! A pack is a directory that gets copied. Nothing here is faked, because the
//! claim being tested is exactly that: what is installed is what is in the
//! packs directory, and an operator can see it with `ls`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use pushos_config::{RuntimeConfig, paths};
use pushos_domain::ids::{ExecutionId, PackId};
use pushos_domain::pack::PackState;
use pushos_domain::permissions::{Permission, PermissionSet};
use pushos_packs::{GRANT_FILE, Library, PackError, Review, read};

/// A directory that cleans up after itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("pushos-pack-{}", ExecutionId::generate()));
        std::fs::create_dir_all(&path).expect("the temporary directory is writable");
        Self(path)
    }

    fn put(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("writable");
        }
        std::fs::write(&path, text).expect("writable");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

const MANIFEST: &str = r#"
id = "seo"
name = "SEO Growth Pack"
version = "1.0.0"
description = """
Audits and content work.
"""
author = "PushOS"
license = "Apache-2.0"

[requires]
permissions = ["shell.execute"]
optional_permissions = ["network"]
providers = ["claude"]
"#;

const PAGES: &str = r#"
[[pages]]
id = "seo"
name = "SEO"
"#;

const BINDINGS: &str = r#"
[[bindings]]
control = "pad.32"
gesture = "tap"
page = "seo"
action = "terminal.open"
params = { name = "audit" }
"#;

/// A pack directory with the usual shape.
fn a_pack(scratch: &Scratch) -> PathBuf {
    scratch.put("seo-pack/pack.toml", MANIFEST);
    scratch.put("seo-pack/pages/seo.toml", PAGES);
    scratch.put("seo-pack/bindings/default.toml", BINDINGS);
    scratch.0.join("seo-pack")
}

/// A configuration root with a home page already in it.
fn a_configuration(scratch: &Scratch) -> PathBuf {
    scratch.put(
        "config/pushos.toml",
        "[runtime]\nhome_page = \"home\"\n\n[[pages]]\nid = \"home\"\nname = \"Home\"\n",
    );
    scratch.0.join("config")
}

#[test]
fn a_pack_says_what_it_is_and_what_it_adds() {
    let scratch = Scratch::new();
    let pack = read(&a_pack(&scratch)).expect("a well-formed pack");

    assert_eq!(pack.id.as_str(), "seo");
    assert_eq!(pack.version, "1.0.0");
    assert_eq!(pack.summary(), "Audits and content work.");
    assert_eq!(pack.adds.pages, 1);
    assert_eq!(pack.adds.bindings, 1);
    assert_eq!(pack.requires.permissions, [Permission::ShellExecute]);
    assert_eq!(pack.requires.optional, [Permission::Network]);
}

#[test]
fn a_directory_that_is_not_a_pack_says_what_a_pack_is() {
    let scratch = Scratch::new();
    scratch.put("not-a-pack/pages/seo.toml", PAGES);

    let error = read(&scratch.0.join("not-a-pack")).expect_err("there is no manifest");
    assert!(error.to_string().contains("pack.toml"), "{error}");
}

#[test]
fn a_pack_cannot_grant_itself_anything() {
    // The rule that makes the review mean something. What a pack may do is what
    // the operator agreed to, and a pack able to write its own grant would have
    // made the review theatre.
    let scratch = Scratch::new();
    let pack = a_pack(&scratch);
    scratch.put(
        "seo-pack/sneaky.toml",
        "[permissions]\ngranted = [\"deploy.production\"]\n",
    );

    let error = read(&pack).expect_err("a pack may not grant itself");
    assert!(matches!(error, PackError::GrantsItself { .. }), "{error}");
    assert!(error.to_string().contains("sneaky.toml"), "{error}");
}

#[test]
fn installing_copies_the_pack_where_pushos_reads_from() {
    let scratch = Scratch::new();
    let library = Library::at(a_configuration(&scratch));

    library
        .install(&a_pack(&scratch), &[Permission::ShellExecute], false)
        .expect("installing succeeds");

    let installed = library.root().join(paths::PACK_DIRECTORY).join("seo");
    assert!(installed.join(paths::PACK_MANIFEST).is_file());
    assert!(installed.join("pages/seo.toml").is_file());
    assert!(
        installed.join("bindings/default.toml").is_file(),
        "the pack's shape should survive the copy"
    );
}

#[test]
fn an_installed_pack_is_part_of_the_surface() {
    let scratch = Scratch::new();
    let root = a_configuration(&scratch);
    let library = Library::at(&root);

    library
        .install(&a_pack(&scratch), &[Permission::ShellExecute], false)
        .expect("installing succeeds");

    let config = RuntimeConfig::build(&pushos_config::load(&root).expect("readable"))
        .expect("the surface still holds together");
    assert_eq!(config.pages.len(), 2, "the pack's page should be there");
    assert_eq!(config.bindings.len(), 1);
    assert!(config.permissions.allows(Permission::ShellExecute));
}

#[test]
fn what_the_operator_granted_is_written_where_they_can_see_it() {
    // And withdraw it. The grant is PushOS's file, not the pack's, so which
    // permissions came from a decision is visible by reading the directory.
    let scratch = Scratch::new();
    let root = a_configuration(&scratch);
    let library = Library::at(&root);

    library
        .install(&a_pack(&scratch), &[Permission::ShellExecute], false)
        .expect("installing succeeds");

    let grant = root
        .join(paths::PACK_DIRECTORY)
        .join("seo")
        .join(GRANT_FILE);
    let text = std::fs::read_to_string(&grant).expect("the grant is written down");
    assert!(text.contains("shell.execute"), "{text}");

    std::fs::remove_file(&grant).expect("removable");
    let config =
        RuntimeConfig::build(&pushos_config::load(&root).expect("readable")).expect("still valid");
    assert!(
        !config.permissions.allows(Permission::ShellExecute),
        "deleting the grant should withdraw it, leaving the pack installed"
    );
}

#[test]
fn granting_nothing_writes_no_grant() {
    let scratch = Scratch::new();
    let root = a_configuration(&scratch);
    Library::at(&root)
        .install(&a_pack(&scratch), &[], false)
        .expect("installing succeeds");

    assert!(
        !root
            .join(paths::PACK_DIRECTORY)
            .join("seo")
            .join(GRANT_FILE)
            .exists(),
        "a file saying nothing was granted is noise"
    );
}

#[test]
fn a_pack_that_would_make_the_surface_ambiguous_is_refused_before_anything_is_copied() {
    // Two bindings on one control at the same scope and priority is an error,
    // not a coin toss. Finding that out at every startup afterwards would be
    // far worse than being told now.
    let scratch = Scratch::new();
    let root = a_configuration(&scratch);
    scratch.put(
        "config/conf.d/mine.toml",
        "[[pages]]\nid = \"seo\"\nname = \"Mine\"\n\n[[bindings]]\ncontrol = \"pad.32\"\ngesture = \"tap\"\npage = \"seo\"\naction = \"page.home\"\n",
    );

    let library = Library::at(&root);
    let error = library
        .install(&a_pack(&scratch), &[Permission::ShellExecute], false)
        .expect_err("the surface would be ambiguous");

    assert!(error.to_string().contains("pad.32"), "{error}");
    assert!(
        !root.join(paths::PACK_DIRECTORY).join("seo").exists(),
        "nothing should have been written"
    );
}

#[test]
fn a_pack_is_not_installed_twice_by_accident() {
    let scratch = Scratch::new();
    let library = Library::at(a_configuration(&scratch));
    let pack = a_pack(&scratch);

    library
        .install(&pack, &[Permission::ShellExecute], false)
        .expect("the first install succeeds");
    let error = library
        .install(&pack, &[Permission::ShellExecute], false)
        .expect_err("the second must not overwrite");
    assert!(matches!(error, PackError::AlreadyInstalled { .. }));

    library
        .install(&pack, &[Permission::ShellExecute], true)
        .expect("replacing is allowed when asked for");
}

#[test]
fn what_is_installed_can_be_listed() {
    let scratch = Scratch::new();
    let library = Library::at(a_configuration(&scratch));
    library
        .install(&a_pack(&scratch), &[Permission::ShellExecute], false)
        .expect("installing succeeds");

    let installed = library.installed();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].pack.id.as_str(), "seo");
    assert_eq!(installed[0].state, PackState::Enabled);
    assert!(library.find(&PackId::new("seo")).is_some());
    assert!(library.find(&PackId::new("absent")).is_none());
}

#[test]
fn a_disabled_pack_stays_on_disk_and_leaves_the_surface() {
    // Turning a pack off must not cost the operator whatever they changed
    // inside it.
    let scratch = Scratch::new();
    let root = a_configuration(&scratch);
    let library = Library::at(&root);
    library
        .install(&a_pack(&scratch), &[Permission::ShellExecute], false)
        .expect("installing succeeds");

    library
        .set_state(&PackId::new("seo"), PackState::Disabled)
        .expect("disabling succeeds");

    let config =
        RuntimeConfig::build(&pushos_config::load(&root).expect("readable")).expect("still valid");
    assert_eq!(config.pages.len(), 1, "the pack's page should be gone");
    assert!(
        root.join(paths::PACK_DIRECTORY).join("seo").is_dir(),
        "but the pack should still be there"
    );
    assert_eq!(
        library.find(&PackId::new("seo")).expect("listed").state,
        PackState::Disabled
    );

    library
        .set_state(&PackId::new("seo"), PackState::Enabled)
        .expect("enabling succeeds");
    let config =
        RuntimeConfig::build(&pushos_config::load(&root).expect("readable")).expect("still valid");
    assert_eq!(config.pages.len(), 2);
}

#[test]
fn removing_a_pack_deletes_what_was_copied_and_nothing_else() {
    let scratch = Scratch::new();
    let root = a_configuration(&scratch);
    let library = Library::at(&root);
    library
        .install(&a_pack(&scratch), &[Permission::ShellExecute], false)
        .expect("installing succeeds");

    library
        .remove(&PackId::new("seo"))
        .expect("removing succeeds");

    assert!(library.installed().is_empty());
    assert!(
        root.join(paths::MAIN_FILE).is_file(),
        "what the operator wrote is theirs and is untouched"
    );
    let config =
        RuntimeConfig::build(&pushos_config::load(&root).expect("readable")).expect("still valid");
    assert_eq!(config.pages.len(), 1);
    assert!(!config.permissions.allows(Permission::ShellExecute));
}

#[test]
fn removing_something_that_is_not_installed_says_so() {
    let scratch = Scratch::new();
    let error = Library::at(a_configuration(&scratch))
        .remove(&PackId::new("absent"))
        .expect_err("nothing is installed");
    assert!(matches!(error, PackError::NotInstalled { .. }));
}

#[test]
fn what_the_operator_already_allows_is_not_asked_for_again() {
    let scratch = Scratch::new();
    let pack = read(&a_pack(&scratch)).expect("a well-formed pack");

    let mut granted = PermissionSet::empty();
    granted.grant(Permission::ShellExecute);

    let review = Review::of(pack, &granted, &["claude".to_owned()]);
    assert!(!review.widens_permissions());
    assert_eq!(review.already, [Permission::ShellExecute]);
    assert!(review.works_fully());
}

#[test]
fn a_review_says_what_installing_would_change() {
    let scratch = Scratch::new();
    let pack = read(&a_pack(&scratch)).expect("a well-formed pack");

    let review = Review::of(pack, &PermissionSet::empty(), &[]);
    assert_eq!(review.granting, [Permission::ShellExecute]);
    assert!(!review.grants_something_irreversible());
    assert_eq!(review.missing_providers, ["claude"]);
}

#[test]
fn a_pack_developed_in_git_does_not_bring_git_with_it() {
    let scratch = Scratch::new();
    let source = a_pack(&scratch);
    scratch.put("seo-pack/.git/config", "[core]\n");
    scratch.put("seo-pack/.DS_Store", "junk");

    let library = Library::at(a_configuration(&scratch));
    library
        .install(&source, &[Permission::ShellExecute], false)
        .expect("installing succeeds");

    let installed = library.root().join(paths::PACK_DIRECTORY).join("seo");
    assert!(!installed.join(".git").exists());
    assert!(!installed.join(".DS_Store").exists());
}

#[test]
fn a_pack_whose_files_are_broken_is_refused_rather_than_half_read() {
    let scratch = Scratch::new();
    let pack = a_pack(&scratch);
    scratch.put("seo-pack/pages/broken.toml", "this is not = = toml");

    assert!(read(&pack).is_err(), "a pack has to be readable as a whole");
}

#[test]
fn installing_over_a_broken_pack_is_still_checked_first() {
    // Replacing must not be a way to skip the checks, or `--force` would become
    // the way anything gets installed.
    let scratch = Scratch::new();
    let root = a_configuration(&scratch);
    let library = Library::at(&root);
    library
        .install(&a_pack(&scratch), &[Permission::ShellExecute], false)
        .expect("the first install succeeds");

    scratch.put(
        "config/conf.d/mine.toml",
        "[[bindings]]\ncontrol = \"pad.32\"\ngesture = \"tap\"\npage = \"seo\"\naction = \"page.home\"\n",
    );

    let error = library
        .install(&a_pack(&scratch), &[Permission::ShellExecute], true)
        .expect_err("the surface would be ambiguous");
    assert!(error.to_string().contains("pad.32"), "{error}");
}

#[test]
fn a_pack_arriving_with_its_own_grant_file_is_refused() {
    // PushOS writes that file, and the reader is told to skip it. A pack
    // shipping one would be writing its own permissions under a name nobody
    // checks, which is the same hole by another route.
    let scratch = Scratch::new();
    let source = a_pack(&scratch);
    scratch.put(
        &format!("seo-pack/{GRANT_FILE}"),
        "[permissions]\ngranted = [\"deploy.production\"]\n",
    );

    let error = Library::at(a_configuration(&scratch))
        .install(&source, &[], false)
        .expect_err("that name is PushOS's");
    assert!(matches!(error, PackError::GrantsItself { .. }), "{error}");
}
