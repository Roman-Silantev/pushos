//! What is in the browser, built from what PushOS knows right now.
//!
//! Derived rather than stored. A tree kept between turns of the knob would
//! have to be invalidated every time a session ended, a project was opened or
//! an agent finished, and a browser showing a session that has gone is worse
//! than one that takes a moment to build. There are tens of rows, not
//! thousands; building them is a handful of clones.
//!
//! The rows are the things PushOS can already act on, which is what keeps this
//! honest: a row exists because there is an action behind it, not because a
//! tree looked neater with one.

use pushos_config::RuntimeConfig;
use pushos_domain::action::{ActionDefinition, ActionSelector, Params};
use pushos_domain::browse::Twig;
use pushos_ui::SessionLine;

/// The top level, in the order it is shown.
///
/// Projects first because choosing one changes what every other level means.
const TOPS: [&str; 3] = ["Projects", "Sessions", "Pages"];

/// Everything the browser can be built from.
///
/// Borrowed from what the surface already holds, so building a level waits on
/// nothing and locks nothing.
pub(crate) struct Rooted<'a> {
    /// The configuration in force.
    pub(crate) config: &'a RuntimeConfig,
    /// The sessions, as the display has them.
    pub(crate) sessions: &'a [SessionLine],
}

impl Rooted<'_> {
    /// The rows at a path, and what to say if there are none.
    pub(crate) fn level(&self, path: &[usize]) -> (Vec<Twig>, String) {
        match path {
            [] => (
                TOPS.iter().map(|top| Twig::branch(*top)).collect(),
                "nothing to browse".to_owned(),
            ),
            [top] => match TOPS.get(*top) {
                Some(&"Projects") => (self.projects(), "no projects are configured".to_owned()),
                Some(&"Sessions") => (self.open_sessions(), "nothing is running".to_owned()),
                Some(&"Pages") => (self.pages(), "no pages are configured".to_owned()),
                _ => (Vec::new(), "nothing here".to_owned()),
            },
            // Two levels is what there is. A deeper path is one the operator
            // cannot reach, so it answers as an empty level rather than
            // pretending.
            _ => (Vec::new(), "nothing further in".to_owned()),
        }
    }

    /// What choosing the row at a path should run, if anything.
    ///
    /// `None` for a row that leads further in, which is a move rather than an
    /// action.
    pub(crate) fn choice(&self, path: &[usize], at: usize) -> Option<ActionDefinition> {
        let [top] = path else {
            return None;
        };

        match TOPS.get(*top) {
            Some(&"Projects") => self
                .config
                .workspaces
                .get(at)
                .map(|workspace| targeted("workspace", "select", &workspace.id.to_string())),
            Some(&"Sessions") => self
                .sessions
                .get(at)
                .map(|session| targeted("session", "focus", &format!("title:{}", session.name))),
            Some(&"Pages") => self
                .config
                .pages
                .get(at)
                .map(|page| targeted("page", "show", &page.id.to_string())),
            _ => None,
        }
    }

    /// Where the cursor is, written across the top of the panel.
    pub(crate) fn trail(path: &[usize]) -> String {
        const ROOT: &str = "Browse";
        match path {
            [top] => TOPS
                .get(*top)
                .map_or_else(|| ROOT.to_owned(), |name| format!("{ROOT} › {name}")),
            _ => ROOT.to_owned(),
        }
    }

    fn projects(&self) -> Vec<Twig> {
        self.config
            .workspaces
            .iter()
            .map(|workspace| {
                Twig::leaf(workspace.name.clone())
                    .saying(shorten(&workspace.root.to_string_lossy()))
            })
            .collect()
    }

    fn open_sessions(&self) -> Vec<Twig> {
        self.sessions
            .iter()
            .map(|session| {
                let row = Twig::leaf(session.name.clone());
                match &session.detail {
                    Some(detail) if !detail.is_empty() => row.saying(detail.clone()),
                    _ => row,
                }
            })
            .collect()
    }

    fn pages(&self) -> Vec<Twig> {
        self.config
            .pages
            .iter()
            .map(|page| {
                let row = Twig::leaf(page.name.clone());
                match &page.description {
                    Some(said) => row.saying(said.clone()),
                    None => row,
                }
            })
            .collect()
    }
}

/// An action naming one thing.
fn targeted(provider: &str, verb: &str, target: &str) -> ActionDefinition {
    let mut params = Params::new();
    params.set(
        "target",
        pushos_domain::action::ParamValue::Text(target.into()),
    );
    ActionDefinition::new(ActionSelector::new(provider, verb), params)
}

/// A path as much of it as is worth reading on a 960 pixel display.
fn shorten(path: &str) -> String {
    const MOST: usize = 28;
    let home = std::env::var("HOME").unwrap_or_default();
    let written = match (home.is_empty(), path.strip_prefix(&home)) {
        (false, Some(rest)) => format!("~{rest}"),
        _ => path.to_owned(),
    };

    if written.chars().count() <= MOST {
        return written;
    }
    // The end of a path says which project; the start says where everything
    // lives, which is the same for all of them.
    let tail: String = written
        .chars()
        .skip(written.chars().count() - MOST + 1)
        .collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use pushos_config::ConfigFile;

    use super::*;

    fn config(text: &str) -> RuntimeConfig {
        let parsed: ConfigFile = toml::from_str(text).expect("well-formed");
        RuntimeConfig::build(&parsed).expect("valid")
    }

    const SOME: &str = r#"
        [[pages]]
        id = "home"
        name = "Home"
        description = "Everything at once"

        [[pages]]
        id = "music"
        name = "Music"

        [[workspaces]]
        id = "pushos"
        name = "PushOS"
        root = "/tmp/pushos"
    "#;

    #[test]
    fn the_top_level_is_the_three_kinds_of_thing() {
        let config = config(SOME);
        let tree = Rooted {
            config: &config,
            sessions: &[],
        };
        let (rows, _) = tree.level(&[]);
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter().all(|row| row.has_children),
            "all lead further in"
        );
        assert_eq!(rows[0].label, "Projects");
    }

    #[test]
    fn choosing_a_project_selects_it() {
        let config = config(SOME);
        let tree = Rooted {
            config: &config,
            sessions: &[],
        };
        let chosen = tree.choice(&[0], 0).expect("a project can be chosen");
        assert_eq!(chosen.selector.to_string(), "workspace.select");
        assert_eq!(chosen.params.text("target"), Some("pushos"));
    }

    #[test]
    fn choosing_a_page_shows_it() {
        let config = config(SOME);
        let tree = Rooted {
            config: &config,
            sessions: &[],
        };
        let (rows, _) = tree.level(&[2]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].detail.as_deref(), Some("Everything at once"));

        let chosen = tree.choice(&[2], 1).expect("a page can be chosen");
        assert_eq!(chosen.selector.to_string(), "page.show");
        assert_eq!(chosen.params.text("target"), Some("music"));
    }

    #[test]
    fn a_row_that_leads_further_in_is_a_move_rather_than_an_action() {
        let config = config(SOME);
        let tree = Rooted {
            config: &config,
            sessions: &[],
        };
        assert!(tree.choice(&[], 0).is_none(), "the top level only leads in");
    }

    #[test]
    fn a_level_with_nothing_in_it_says_what_is_missing() {
        let config = config(SOME);
        let tree = Rooted {
            config: &config,
            sessions: &[],
        };
        let (rows, empty) = tree.level(&[1]);
        assert!(rows.is_empty());
        assert_eq!(empty, "nothing is running");
    }

    #[test]
    fn a_path_deeper_than_the_tree_answers_empty_rather_than_pretending() {
        let config = config(SOME);
        let tree = Rooted {
            config: &config,
            sessions: &[],
        };
        let (rows, _) = tree.level(&[0, 0, 0]);
        assert!(rows.is_empty());
        assert!(tree.choice(&[0, 0], 0).is_none());
    }

    #[test]
    fn a_long_path_is_shortened_from_its_front() {
        // The end says which project; the front is the same for all of them.
        let long = shorten("/Users/somebody/Documents/Projects/a-long-project-name");
        assert!(long.chars().count() <= 28, "{long}");
        assert!(long.ends_with("a-long-project-name"), "{long}");
        assert!(long.starts_with('…'), "{long}");

        assert_eq!(shorten("/tmp/x"), "/tmp/x", "a short one is left alone");
    }

    #[test]
    fn the_trail_says_where_the_cursor_is() {
        assert_eq!(Rooted::trail(&[]), "Browse");
        assert_eq!(Rooted::trail(&[1]), "Browse › Sessions");
    }
}
