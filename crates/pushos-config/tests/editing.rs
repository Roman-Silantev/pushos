//! Editing configuration.
//!
//! The specification is emphatic that these files are the source of truth and
//! that Studio must not invent a format of its own. The tests that matter most
//! here are therefore about what an edit leaves behind, not only about what it
//! changes.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use pushos_config::{BindingAddress, BindingSpec, ConfigDocuments, PageSpec};
use pushos_domain::ids::ExecutionId;

/// A configuration directory that cleans up after itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("pushos-edit-{}", ExecutionId::generate()));
        std::fs::create_dir_all(&path).expect("the temporary directory is writable");
        Self(path)
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("writable");
        }
        std::fs::write(path, text).expect("writable");
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.0.join(name)).expect("readable")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// A file written by a person, with the things a person writes.
const HANDWRITTEN: &str = r#"# My PushOS setup.
#
# The play button is deliberately global: I want it everywhere.

[runtime]
home_page = "home"   # start here

[[pages]]
id = "home"
name = "Home"

[[pages]]
id = "music"
name = "Music"

# --- Transport ---------------------------------------------------------------

[[bindings]]
control = "button.play"
gesture = "press"
action  = "media.play_pause"
label   = "Play/Pause"

[[bindings]]
control = "pad.0"
gesture = "tap"
page = "music"
action = "media.next_track"
label = "Next"
"#;

fn documents(scratch: &Scratch) -> ConfigDocuments {
    ConfigDocuments::load(&scratch.0).expect("the configuration is readable")
}

#[test]
fn editing_one_binding_leaves_comments_and_layout_alone() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = documents(&scratch);
    documents.upsert_binding(
        &BindingSpec::new(
            BindingAddress::new("pad.0", "tap").on_page("music"),
            "media.previous_track",
        )
        .with_label("Previous"),
    );
    documents.save().expect("the edited configuration is valid");

    let saved = scratch.read("pushos.toml");
    assert!(
        saved.contains("# My PushOS setup."),
        "the header comment was lost"
    );
    assert!(
        saved.contains("# The play button is deliberately global: I want it everywhere."),
        "an explanatory comment was lost"
    );
    assert!(
        saved.contains("# --- Transport"),
        "a section comment was lost"
    );
    assert!(
        saved.contains("home_page = \"home\"   # start here"),
        "a trailing comment was lost"
    );
    assert!(
        saved.contains("action  = \"media.play_pause\""),
        "hand alignment was lost"
    );

    assert!(
        saved.contains("media.previous_track"),
        "the edit was not applied"
    );
    assert!(
        !saved.contains("media.next_track"),
        "the old action is still there"
    );
}

#[test]
fn a_new_binding_is_appended_rather_than_reordering_the_file() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = documents(&scratch);
    let edit = documents.upsert_binding(&BindingSpec::new(
        BindingAddress::new("pad.7", "double_tap"),
        "page.home",
    ));
    assert!(!edit.replaced);
    documents.save().expect("valid");

    let saved = scratch.read("pushos.toml");
    let play = saved
        .find("button.play")
        .expect("the original binding is still there");
    let added = saved.find("pad.7").expect("the new binding was written");
    assert!(play < added, "existing bindings should keep their position");
}

#[test]
fn replacing_reports_that_it_replaced() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = documents(&scratch);
    let edit = documents.upsert_binding(&BindingSpec::new(
        BindingAddress::new("button.play", "press"),
        "page.next",
    ));

    assert!(edit.replaced);
    assert!(edit.file.ends_with("pushos.toml"));
}

#[test]
fn bindings_are_edited_in_the_file_they_were_written_in() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);
    scratch.write(
        "conf.d/music.toml",
        "# Music-only bindings.\n\n[[bindings]]\ncontrol = \"pad.9\"\ngesture = \"tap\"\npage = \"music\"\naction = \"media.play_pause\"\n",
    );

    let mut documents = documents(&scratch);
    let edit = documents.upsert_binding(&BindingSpec::new(
        BindingAddress::new("pad.9", "tap").on_page("music"),
        "media.now_playing",
    ));
    documents.save().expect("valid");

    assert!(edit.replaced);
    assert!(
        edit.file.ends_with("music.toml"),
        "the edit should stay in its own file"
    );
    assert!(
        scratch
            .read("conf.d/music.toml")
            .contains("media.now_playing")
    );
    assert!(
        scratch
            .read("conf.d/music.toml")
            .contains("# Music-only bindings.")
    );
    assert_eq!(
        scratch.read("pushos.toml"),
        HANDWRITTEN,
        "the main file was not touched"
    );
}

#[test]
fn scope_makes_two_bindings_on_one_control_distinct() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = documents(&scratch);
    // Same control and gesture as an existing binding, but global rather than
    // scoped to the music page: a different binding, not an edit.
    let edit = documents.upsert_binding(&BindingSpec::new(
        BindingAddress::new("pad.0", "tap"),
        "page.next",
    ));

    assert!(!edit.replaced, "a different scope is a different binding");
    documents.save().expect("valid");

    let saved = scratch.read("pushos.toml");
    assert!(
        saved.contains("media.next_track"),
        "the scoped binding survived"
    );
    assert!(saved.contains("page.next"), "the global binding was added");
}

#[test]
fn removing_a_binding_takes_out_only_that_one() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = documents(&scratch);
    let removed = documents
        .remove_binding(&BindingAddress::new("pad.0", "tap").on_page("music"))
        .expect("the binding was there");
    assert!(removed.ends_with("pushos.toml"));
    documents.save().expect("valid");

    let saved = scratch.read("pushos.toml");
    assert!(!saved.contains("media.next_track"));
    assert!(
        saved.contains("media.play_pause"),
        "the other binding survived"
    );
    assert!(
        saved.contains("# --- Transport"),
        "comments survived a removal"
    );
}

#[test]
fn removing_something_that_is_not_there_changes_nothing() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = documents(&scratch);
    assert!(
        documents
            .remove_binding(&BindingAddress::new("pad.55", "hold"))
            .is_none()
    );
    assert!(!documents.is_dirty());
}

#[test]
fn an_edit_that_would_break_the_surface_writes_nothing() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = documents(&scratch);
    documents.upsert_binding(&BindingSpec::new(
        BindingAddress::new("pad.0", "tap").on_page("nowhere"),
        "page.next",
    ));

    let error = documents.save().expect_err("the page does not exist");
    assert!(
        !error.problems().is_empty(),
        "the reason should be reported"
    );
    assert_eq!(
        scratch.read("pushos.toml"),
        HANDWRITTEN,
        "the file must be untouched"
    );
}

#[test]
fn an_edit_creating_an_ambiguous_surface_is_refused() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = documents(&scratch);
    // A second global binding on the play button at the same priority: nothing
    // could decide between them.
    documents.upsert_binding(&BindingSpec::new(
        BindingAddress::new("button.play", "press"),
        "page.next",
    ));
    // That replaced rather than duplicated, so it is fine. Force a duplicate by
    // writing one directly.
    scratch.write(
        "conf.d/clash.toml",
        "[[bindings]]\ncontrol = \"button.play\"\ngesture = \"press\"\naction = \"page.home\"\n",
    );

    let mut documents = ConfigDocuments::load(&scratch.0).expect("readable");
    documents.upsert_binding(&BindingSpec::new(
        BindingAddress::new("pad.20", "tap"),
        "page.home",
    ));

    assert!(
        documents.save().is_err(),
        "an ambiguous surface must be refused"
    );
    assert_eq!(
        scratch.read("pushos.toml"),
        HANDWRITTEN,
        "nothing should have been written"
    );
}

#[test]
fn a_binding_can_be_created_in_an_empty_directory() {
    let scratch = Scratch::new();

    let mut documents = ConfigDocuments::load(&scratch.0).expect("an empty root is fine");
    documents.upsert_binding(
        &BindingSpec::new(
            BindingAddress::new("button.play", "press"),
            "media.play_pause",
        )
        .with_label("Play"),
    );
    documents.save().expect("valid");

    let saved = scratch.read("pushos.toml");
    assert!(saved.contains("media.play_pause"));
    assert!(saved.contains("label = \"Play\""));
}

#[test]
fn parameters_survive_a_round_trip() {
    use pushos_domain::action::ParamValue;

    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut spec = BindingSpec::new(BindingAddress::new("pad.3", "hold"), "shell.run");
    spec.params
        .insert("program".to_owned(), ParamValue::from("cargo"));
    spec.params.insert(
        "args".to_owned(),
        ParamValue::List(vec![ParamValue::from("test"), ParamValue::from("--all")]),
    );
    spec.params
        .insert("timeout_seconds".to_owned(), ParamValue::Integer(600));

    let mut documents = documents(&scratch);
    documents.upsert_binding(&spec);
    documents.save().expect("valid");

    let reloaded = pushos_config::load(&scratch.0).expect("readable");
    let written = reloaded
        .bindings
        .iter()
        .find(|entry| entry.control == "pad.3")
        .expect("the binding was written");

    assert_eq!(written.params["program"], ParamValue::from("cargo"));
    assert_eq!(written.params["timeout_seconds"], ParamValue::Integer(600));
    assert!(matches!(written.params["args"], ParamValue::List(ref items) if items.len() == 2));
}

#[test]
fn clearing_an_optional_field_removes_it_rather_than_leaving_it_empty() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = documents(&scratch);
    // The play binding has a label; save it without one.
    documents.upsert_binding(&BindingSpec::new(
        BindingAddress::new("button.play", "press"),
        "media.play_pause",
    ));
    documents.save().expect("valid");

    let saved = scratch.read("pushos.toml");
    assert!(!saved.contains("label   = \"Play/Pause\""));
    assert!(
        saved.contains("label = \"Next\""),
        "the other binding kept its caption"
    );
}

#[test]
fn saving_leaves_no_temporary_files_behind() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = documents(&scratch);
    documents.upsert_binding(&BindingSpec::new(
        BindingAddress::new("pad.30", "tap"),
        "page.home",
    ));
    documents.save().expect("valid");

    let leftovers: Vec<_> = std::fs::read_dir(&scratch.0)
        .expect("readable")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("tmp"))
        .collect();

    assert!(leftovers.is_empty(), "found {leftovers:?}");
}

#[test]
fn saving_twice_writes_only_what_changed_the_second_time() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = documents(&scratch);
    documents.upsert_binding(&BindingSpec::new(
        BindingAddress::new("pad.30", "tap"),
        "page.home",
    ));
    documents.save().expect("valid");
    assert!(!documents.is_dirty());

    documents.save().expect("nothing to do");
    assert!(!documents.is_dirty());
}

// --- Pages ------------------------------------------------------------------

#[test]
fn a_new_page_is_added_and_the_file_is_still_the_one_its_author_wrote() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = ConfigDocuments::load(&scratch.0).expect("it loads");
    let edit = documents.upsert_page(&PageSpec::new("develop", "Development"));
    assert!(!edit.replaced);
    documents.save().expect("the configuration is still valid");

    let text = scratch.read("pushos.toml");
    assert!(text.contains("# My PushOS setup."), "the comments survive");
    assert!(text.contains("# start here"), "so do the inline ones");
    assert!(text.contains(r#"id = "develop""#), "{text}");
    assert!(text.contains(r#"name = "Development""#), "{text}");
}

#[test]
fn adding_a_page_that_already_exists_edits_it_rather_than_repeating_it() {
    // Two pages with one identity is the ambiguity the validator refuses, so
    // this must never produce one.
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = ConfigDocuments::load(&scratch.0).expect("it loads");
    let mut renamed = PageSpec::new("music", "Sound");
    renamed.description = Some("Playback and volume".to_owned());

    let edit = documents.upsert_page(&renamed);
    assert!(edit.replaced);
    documents.save().expect("the configuration is still valid");

    let text = scratch.read("pushos.toml");
    assert_eq!(text.matches(r#"id = "music""#).count(), 1, "{text}");
    assert!(text.contains(r#"name = "Sound""#), "{text}");
    assert!(text.contains("Playback and volume"), "{text}");
}

#[test]
fn removing_a_page_takes_what_was_on_it() {
    // Leaving the bindings behind would produce a configuration that refuses
    // to load, and the operator asked to remove a page rather than to be told
    // afterwards that they cannot.
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = ConfigDocuments::load(&scratch.0).expect("it loads");
    let on_music = documents.bindings_on_page("music");
    assert!(on_music > 0, "the fixture should have some");

    let removal = documents.remove_page("music").expect("the page is there");
    assert_eq!(removal.bindings_removed, on_music);
    documents.save().expect("the configuration is still valid");

    let text = scratch.read("pushos.toml");
    assert!(!text.contains(r#"id = "music""#), "{text}");
    assert!(!text.contains(r#"page = "music""#), "{text}");
    assert!(text.contains(r#"id = "home""#), "the other pages stay");
}

#[test]
fn removing_a_page_leaves_bindings_that_apply_everywhere_alone() {
    // A global binding is not on any page, so removing a page is not a reason
    // to lose it.
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = ConfigDocuments::load(&scratch.0).expect("it loads");
    documents.remove_page("music").expect("the page is there");
    documents.save().expect("valid");

    assert!(scratch.read("pushos.toml").contains("button.play"));
}

#[test]
fn removing_a_page_that_is_not_there_changes_nothing() {
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = ConfigDocuments::load(&scratch.0).expect("it loads");
    assert!(documents.remove_page("nowhere").is_none());
    assert!(!documents.is_dirty());
}

#[test]
fn removing_the_home_page_is_refused_before_anything_is_written() {
    // The runtime setting would then name a page that does not exist, which is
    // exactly what validation is for.
    let scratch = Scratch::new();
    scratch.write("pushos.toml", HANDWRITTEN);

    let mut documents = ConfigDocuments::load(&scratch.0).expect("it loads");
    documents.remove_page("home").expect("the page is there");

    assert!(documents.save().is_err(), "it should refuse");
    let text = scratch.read("pushos.toml");
    assert!(
        text.contains(r#"id = "home""#),
        "nothing was written: {text}"
    );
}
