//! The dials, and turning them without rewriting the file they live in.
//!
//! A binding says what a control does; these say how the surface behaves while
//! it does it — how many sessions may run, how bright the panel is, how long a
//! press has to last to be a hold. They are the settings an operator wants to
//! try, look at, and try again, which is exactly the thing a text file is bad
//! at and Studio is good at.
//!
//! Only dials live here. Permissions are not a dial: granting one is a
//! decision a person makes once, having read what it allows, and a panel of
//! toggles would turn that into something done absent-mindedly. Projects,
//! providers and agent roles are not dials either; they are the shape of the
//! configuration rather than settings within it.
//!
//! An unset dial is not the same as one set to its default. Unset means "you
//! have not said", and PushOS is free to change what it does by default;
//! written down means "I said". So writing `None` removes the key rather than
//! writing the default into the file.

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::model::{ConfigFile, SeatLimits};

/// The table each group of dials lives in.
const SESSIONS: &str = "sessions";
const SURFACE: &str = "surface";
const GESTURES: &str = "gestures";

/// Every dial an operator can turn, as the files have them.
///
/// `None` throughout means the dial is not written down at all, and PushOS
/// uses its default. That is a different thing from writing the default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// How many sessions may run and work at once.
    pub sessions: SessionDials,
    /// How the panel is lit.
    pub surface: SurfaceDials,
    /// How long a press has to last to mean something else.
    pub gestures: GestureDials,
}

/// What the fleet is allowed to cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionDials {
    /// Whether the terminals already open are watched at all.
    pub watch: Option<bool>,
    /// Sessions with a process each. `0` means no limit.
    pub most_live: Option<usize>,
    /// Codex threads loaded at once. `0` means no limit.
    pub most_threads: Option<usize>,
    /// Sessions working at once. `0` starts everything at once.
    pub working_at_once: Option<usize>,
    /// Minutes idle before a session is put away. `0` keeps them running.
    pub put_away_after_minutes: Option<u64>,
}

/// How the panel is lit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SurfaceDials {
    /// Brightness while in use, in percent.
    pub brightness: Option<i64>,
    /// Minutes untouched before dimming. `0` never dims.
    pub dim_after_minutes: Option<f64>,
    /// Minutes untouched before going dark. `0` never does.
    pub sleep_after_minutes: Option<f64>,
}

/// How long a press has to last to mean something else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GestureDials {
    /// How long a press must last to become a hold.
    pub hold_threshold_ms: Option<u64>,
    /// How close two taps must be to become a double tap.
    pub double_tap_window_ms: Option<u64>,
}

/// What PushOS does with a dial nobody has turned.
///
/// Sent alongside the settings so a panel can say "20 (default)" rather than
/// leaving a box empty and letting the operator guess.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Defaults {
    /// Sessions with a process each.
    pub most_live: Option<usize>,
    /// Codex threads loaded at once.
    pub most_threads: Option<usize>,
    /// Sessions working at once.
    pub working_at_once: Option<usize>,
    /// Minutes idle before a session is put away.
    pub put_away_after_minutes: Option<u64>,
    /// Brightness while in use, in percent.
    pub brightness: u8,
    /// Minutes untouched before dimming. `None` never dims.
    pub dim_after_minutes: Option<f64>,
    /// Minutes untouched before going dark. `None` never does.
    pub sleep_after_minutes: Option<f64>,
    /// How long a press must last to become a hold.
    pub hold_threshold_ms: u64,
    /// How close two taps must be to become a double tap.
    pub double_tap_window_ms: u64,
}

impl Defaults {
    /// What the runtime falls back to, taken from the runtime itself rather
    /// than written here twice.
    #[must_use]
    pub fn current() -> Self {
        let resting = pushos_domain::rest::RestPolicy::DEFAULT;
        let timing = pushos_bindings::GestureTiming::DEFAULT;
        Self {
            most_live: SeatLimits::DEFAULT.most_live,
            most_threads: SeatLimits::DEFAULT.most_threads,
            working_at_once: SeatLimits::DEFAULT.working_at_once,
            put_away_after_minutes: SeatLimits::DEFAULT
                .put_away_after
                .map(|after| after.as_secs() / 60),
            brightness: resting.brightness,
            dim_after_minutes: resting.dim_after.map(in_minutes),
            sleep_after_minutes: resting.sleep_after.map(in_minutes),
            hold_threshold_ms: as_millis(timing.hold_threshold),
            double_tap_window_ms: as_millis(timing.double_tap_window),
        }
    }
}

impl Settings {
    /// The dials as a parsed configuration has them.
    pub fn read_from(file: &ConfigFile) -> Self {
        Self {
            sessions: SessionDials {
                watch: file.sessions.watch,
                most_live: file.sessions.most_live,
                most_threads: file.sessions.most_threads,
                working_at_once: file.sessions.working_at_once,
                put_away_after_minutes: file.sessions.put_away_after_minutes,
            },
            surface: SurfaceDials {
                brightness: file.surface.brightness,
                dim_after_minutes: file.surface.dim_after_minutes,
                sleep_after_minutes: file.surface.sleep_after_minutes,
            },
            gestures: GestureDials {
                hold_threshold_ms: file.gestures.hold_threshold_ms,
                double_tap_window_ms: file.gestures.double_tap_window_ms,
            },
        }
    }
}

/// Writes every dial into a document, leaving the rest of it alone.
///
/// A dial that is `None` has its key removed, so the file goes back to saying
/// nothing about it rather than freezing today's default in place.
pub(crate) fn write_into(document: &mut DocumentMut, settings: &Settings) {
    let sessions = &settings.sessions;
    dial(document, SESSIONS, "watch", sessions.watch.map(value));
    dial(
        document,
        SESSIONS,
        "most_live",
        sessions.most_live.map(whole),
    );
    dial(
        document,
        SESSIONS,
        "most_threads",
        sessions.most_threads.map(whole),
    );
    dial(
        document,
        SESSIONS,
        "working_at_once",
        sessions.working_at_once.map(whole),
    );
    dial(
        document,
        SESSIONS,
        "put_away_after_minutes",
        sessions.put_away_after_minutes.map(counted),
    );

    let surface = &settings.surface;
    dial(
        document,
        SURFACE,
        "brightness",
        surface.brightness.map(value),
    );
    dial(
        document,
        SURFACE,
        "dim_after_minutes",
        surface.dim_after_minutes.map(value),
    );
    dial(
        document,
        SURFACE,
        "sleep_after_minutes",
        surface.sleep_after_minutes.map(value),
    );

    let gestures = &settings.gestures;
    dial(
        document,
        GESTURES,
        "hold_threshold_ms",
        gestures.hold_threshold_ms.map(counted),
    );
    dial(
        document,
        GESTURES,
        "double_tap_window_ms",
        gestures.double_tap_window_ms.map(counted),
    );

    // A section emptied by turning everything in it back off goes too, rather
    // than leaving a bare heading behind. Only when it is actually empty: a
    // section may hold settings that are not dials, and those are the
    // operator's.
    for table in [SESSIONS, SURFACE, GESTURES] {
        let empty = document
            .get(table)
            .and_then(Item::as_table_like)
            .is_some_and(toml_edit::TableLike::is_empty);
        if empty {
            document.remove(table);
        }
    }
}

/// Whether a document says anything about any of these dials.
///
/// Which file to write them to is decided by this: the one that already has
/// them, rather than a second copy somewhere else for the loader to merge.
pub(crate) fn present_in(document: &DocumentMut) -> bool {
    [SESSIONS, SURFACE, GESTURES]
        .iter()
        .any(|table| document.get(table).is_some())
}

/// Sets one dial, or removes it when nobody has said what it should be.
fn dial(document: &mut DocumentMut, table: &str, key: &str, setting: Option<Item>) {
    let Some(setting) = setting else {
        if let Some(existing) = document.get_mut(table).and_then(Item::as_table_like_mut) {
            existing.remove(key);
        }
        return;
    };

    if document.get(table).is_none() {
        let mut fresh = Table::new();
        fresh.set_implicit(false);
        document.insert(table, Item::Table(fresh));
    }
    let Some(existing) = document.get_mut(table).and_then(Item::as_table_like_mut) else {
        return;
    };

    // The value is replaced in its place rather than the whole entry, so
    // whatever the author wrote around it — the comment above it saying why
    // twenty, the note after it — is still there afterwards.
    if let Some(slot) = existing.get_mut(key) {
        let decor = slot.as_value().map(|held| held.decor().clone());
        *slot = setting;
        if let (Some(decor), Some(written)) = (decor, slot.as_value_mut()) {
            *written.decor_mut() = decor;
        }
        return;
    }
    existing.insert(key, setting);
}

/// A duration as the files write it.
fn in_minutes(long: std::time::Duration) -> f64 {
    long.as_secs_f64() / 60.0
}

/// A duration in milliseconds, for the dials measured that way.
fn as_millis(long: std::time::Duration) -> u64 {
    u64::try_from(long.as_millis()).unwrap_or(u64::MAX)
}

/// A count as TOML holds it, which is a signed integer.
///
/// Saturating rather than wrapping: a number too large to write down is the
/// largest one that can be, which is still a limit nobody will reach.
fn whole(count: usize) -> Item {
    value(i64::try_from(count).unwrap_or(i64::MAX))
}

/// The same, for the counts kept as `u64`.
fn counted(count: u64) -> Item {
    value(i64::try_from(count).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> DocumentMut {
        text.parse().expect("well-formed TOML")
    }

    #[test]
    fn a_dial_is_written_where_an_operator_would_look_for_it() {
        let mut doc = document("");
        write_into(
            &mut doc,
            &Settings {
                sessions: SessionDials {
                    most_live: Some(12),
                    ..SessionDials::default()
                },
                ..Settings::default()
            },
        );

        assert!(doc.to_string().contains("[sessions]"));
        assert!(doc.to_string().contains("most_live = 12"));
    }

    #[test]
    fn turning_a_dial_leaves_the_rest_of_the_file_as_its_author_wrote_it() {
        // The files are the source of truth and a person wrote them; an edit
        // that reformatted them would be Studio taking them over.
        let mut doc = document(
            "# The fleet\n[sessions]\n# twenty is plenty\nmost_live = 20\nwatch = true\n\n[[pages]]\nid = \"home\"\n",
        );

        write_into(
            &mut doc,
            &Settings {
                sessions: SessionDials {
                    most_live: Some(12),
                    watch: Some(true),
                    ..SessionDials::default()
                },
                ..Settings::default()
            },
        );

        let written = doc.to_string();
        assert!(written.contains("# The fleet"), "{written}");
        assert!(written.contains("# twenty is plenty"), "{written}");
        assert!(written.contains("most_live = 12"), "{written}");
        assert!(written.contains("id = \"home\""), "the rest is untouched");
    }

    #[test]
    fn a_dial_nobody_has_set_is_removed_rather_than_written_as_its_default() {
        // Silence means PushOS may change its mind about the default later.
        // Writing today's default in would quietly freeze it for ever.
        let mut doc = document("[sessions]\nmost_live = 20\nwatch = true\n");

        write_into(
            &mut doc,
            &Settings {
                sessions: SessionDials {
                    watch: Some(true),
                    ..SessionDials::default()
                },
                ..Settings::default()
            },
        );

        let written = doc.to_string();
        assert!(!written.contains("most_live"), "{written}");
        assert!(written.contains("watch = true"), "{written}");
    }

    #[test]
    fn every_dial_survives_being_written_and_read_again() {
        let settings = Settings {
            sessions: SessionDials {
                watch: Some(false),
                most_live: Some(12),
                most_threads: Some(48),
                working_at_once: Some(4),
                put_away_after_minutes: Some(15),
            },
            surface: SurfaceDials {
                brightness: Some(55),
                dim_after_minutes: Some(2.5),
                sleep_after_minutes: Some(20.0),
            },
            gestures: GestureDials {
                hold_threshold_ms: Some(400),
                double_tap_window_ms: Some(250),
            },
        };

        let mut doc = document("");
        write_into(&mut doc, &settings);
        let parsed: ConfigFile = toml::from_str(&doc.to_string()).expect("valid");

        assert_eq!(Settings::read_from(&parsed), settings);
    }

    #[test]
    fn the_defaults_are_the_runtime_s_own_rather_than_a_second_copy() {
        let defaults = Defaults::current();
        assert_eq!(defaults.most_live, SeatLimits::DEFAULT.most_live);
        assert_eq!(
            defaults.working_at_once,
            SeatLimits::DEFAULT.working_at_once
        );
        assert!(defaults.brightness > 0 && defaults.brightness <= 100);
        assert!(defaults.dim_after_minutes.is_some_and(|after| after > 0.0));
        assert!(defaults.hold_threshold_ms > 0);
    }

    #[test]
    fn a_section_emptied_of_every_dial_does_not_stay_behind() {
        let mut doc = document("[surface]\nbrightness = 50\n");
        write_into(&mut doc, &Settings::default());
        assert!(!doc.to_string().contains("[surface]"), "{doc}");
    }

    #[test]
    fn a_section_holding_something_that_is_not_a_dial_is_left_alone() {
        // `poll_ms` and `codex_features` are settings this panel does not
        // offer; unsetting the ones it does must not take them with it.
        let mut doc = document("[sessions]\nmost_live = 20\npoll_ms = 5000\n");
        write_into(&mut doc, &Settings::default());

        let written = doc.to_string();
        assert!(written.contains("poll_ms = 5000"), "{written}");
        assert!(!written.contains("most_live"), "{written}");
    }

    #[test]
    fn a_file_with_no_dials_in_it_says_so() {
        assert!(!present_in(&document("[[pages]]\nid = \"home\"\n")));
        assert!(present_in(&document("[surface]\nbrightness = 50\n")));
    }
}
