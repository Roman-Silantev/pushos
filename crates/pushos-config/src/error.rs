//! What can be wrong with a configuration, said precisely.
//!
//! Every problem names the entry it came from, because a configuration file is
//! edited by a person and by Studio, and "invalid configuration" is not a
//! message either can act on.

use std::path::PathBuf;

use pushos_bindings::BindingConflict;

/// Why a configuration could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A file could not be written.
    #[error("could not write `{path}`")]
    Unwritable {
        /// The file or directory.
        path: PathBuf,
        /// What the file system reported.
        #[source]
        source: std::io::Error,
    },

    /// A file could not be read.
    #[error("could not read `{path}`")]
    Unreadable {
        /// The file.
        path: PathBuf,
        /// What the file system reported.
        #[source]
        source: std::io::Error,
    },

    /// A file was not valid TOML.
    #[error("`{path}` is not valid TOML")]
    Malformed {
        /// The file.
        path: PathBuf,
        /// What the parser reported.
        #[source]
        source: toml::de::Error,
    },

    /// The configuration parsed but does not describe a usable surface.
    #[error("the configuration is not usable")]
    Invalid {
        /// Every problem found, so one edit pass can fix them all.
        problems: Vec<Problem>,
    },
}

impl ConfigError {
    /// The individual problems, when the failure was a validation failure.
    pub fn problems(&self) -> &[Problem] {
        match self {
            Self::Invalid { problems } => problems,
            _ => &[],
        }
    }
}

/// One thing wrong with an otherwise well-formed configuration.
#[derive(Debug, thiserror::Error)]
pub enum Problem {
    /// A binding names a control that does not exist.
    #[error("binding {index}: {source}")]
    Control {
        /// Which binding, counting from one.
        index: usize,
        /// What the control parser reported.
        #[source]
        source: pushos_domain::controls::ParseControlError,
    },

    /// A binding names a gesture that does not exist.
    #[error("binding {index}: {source}")]
    Gesture {
        /// Which binding, counting from one.
        index: usize,
        /// What the gesture parser reported.
        #[source]
        source: pushos_domain::gesture::UnknownGesture,
    },

    /// A binding's action is not written as `provider.verb`.
    #[error("binding {index}: {source}")]
    Action {
        /// Which binding, counting from one.
        index: usize,
        /// What the selector parser reported.
        #[source]
        source: pushos_domain::action::MalformedSelector,
    },

    /// A binding declares a scope its other fields contradict.
    #[error("binding {index}: scope `{declared}` does not match its page and workspace fields")]
    ScopeMismatch {
        /// Which binding, counting from one.
        index: usize,
        /// The scope the entry declared.
        declared: String,
    },

    /// A binding refers to a page that is not declared.
    #[error("binding {index}: page `{page}` is not declared")]
    UnknownPage {
        /// Which binding, counting from one.
        index: usize,
        /// The page that was referenced.
        page: String,
    },

    /// The home page is not declared.
    #[error("the home page `{page}` is not declared")]
    UnknownHomePage {
        /// The page that was referenced.
        page: String,
    },

    /// Two pages share an identity.
    #[error("page `{id}` is declared more than once")]
    DuplicatePage {
        /// The contested identity.
        id: String,
    },

    /// Two providers share a name.
    #[error("provider `{id}` is declared more than once")]
    DuplicateProvider {
        /// The contested name.
        id: String,
    },

    /// A provider was declared with nothing to run.
    #[error("provider `{id}` has no program to run")]
    ProviderWithoutProgram {
        /// The provider that cannot start.
        id: String,
    },

    /// Two agent roles share an identity.
    #[error("agent `{id}` is declared more than once")]
    DuplicateAgent {
        /// The contested identity.
        id: String,
    },

    /// An agent role prefers a provider that is not declared.
    #[error("agent `{agent}` prefers undeclared provider(s): {}", providers.join(", "))]
    UnknownAgentProvider {
        /// The role that named them.
        agent: String,
        /// The providers that do not exist.
        providers: Vec<String>,
    },

    /// Two projects share an identity.
    #[error("workspace `{id}` is declared more than once")]
    DuplicateWorkspace {
        /// The contested identity.
        id: String,
    },

    /// A project's home page is not declared.
    #[error("workspace `{workspace}`: page `{page}` is not declared")]
    UnknownWorkspacePage {
        /// The project that named it.
        workspace: String,
        /// The page that was referenced.
        page: String,
    },

    /// A project wants a provider that is not declared.
    #[error("workspace `{workspace}`: role `{agent}` wants undeclared provider `{provider}`")]
    UnknownWorkspaceProvider {
        /// The project that named it.
        workspace: String,
        /// The role it was for.
        agent: String,
        /// The provider that does not exist.
        provider: String,
    },

    /// A binding is scoped to a project that is not declared.
    #[error("binding {index}: workspace `{workspace}` is not declared")]
    UnknownBindingWorkspace {
        /// Which binding, counting from one.
        index: usize,
        /// The project that was referenced.
        workspace: String,
    },

    /// Two bindings share an identity.
    #[error("binding id `{id}` is used more than once")]
    DuplicateBindingId {
        /// The contested identity.
        id: String,
    },

    /// Two bindings could both fire, and precedence cannot separate them.
    #[error(transparent)]
    Conflict(#[from] BindingConflict),

    /// The gesture timings would make gestures indistinguishable.
    #[error(transparent)]
    Timing(#[from] pushos_bindings::InvalidTiming),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_validation_failure_lists_every_problem_it_found() {
        let error = ConfigError::Invalid {
            problems: vec![
                Problem::UnknownPage {
                    index: 1,
                    page: "missing".to_owned(),
                },
                Problem::DuplicatePage {
                    id: "home".to_owned(),
                },
            ],
        };
        assert_eq!(error.problems().len(), 2);
    }

    #[test]
    fn a_read_failure_has_no_validation_problems_to_report() {
        let error = ConfigError::Unreadable {
            path: PathBuf::from("/nowhere/pushos.toml"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
        };
        assert!(error.problems().is_empty());
        assert!(error.to_string().contains("/nowhere/pushos.toml"));
    }

    #[test]
    fn a_problem_names_the_binding_it_came_from() {
        let problem = Problem::UnknownPage {
            index: 7,
            page: "music".to_owned(),
        };
        let message = problem.to_string();
        assert!(message.contains('7'));
        assert!(message.contains("music"));
    }
}
