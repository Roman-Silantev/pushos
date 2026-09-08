//! The wire protocol.
//!
//! Newline-delimited JSON over a Unix socket: one request per line, one
//! response per line. Small enough to drive by hand with `nc` when something is
//! wrong, and it needs no framework to serve.

use pushos_config::{BindingAddress, BindingSpec};
use serde::{Deserialize, Serialize};

/// What a running PushOS is driving.
///
/// Three states rather than a boolean. A stand-in surface is neither attached
/// nor unattached, and reporting it as either would tell an operator something
/// untrue about their own machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SurfaceReport {
    /// A real Push 2 is attached.
    Push2,
    /// A simulated surface is standing in for the hardware.
    Simulated,
    /// Nothing is attached.
    Absent,
}

impl SurfaceReport {
    /// Whether this is genuine hardware.
    pub const fn is_hardware(self) -> bool {
        matches!(self, Self::Push2)
    }

    /// A sentence an operator can read without further interpretation.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Push2 => "Push 2 attached",
            Self::Simulated => "simulated surface, no hardware",
            Self::Absent => "no surface attached",
        }
    }
}

/// The protocol version this build speaks.
///
/// Studio checks it against its own and refuses to run against a PushOS it does
/// not understand, rather than sending edits that would be misread.
pub const PROTOCOL_VERSION: u32 = 1;

/// What a client is asking for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Request {
    /// How the runtime is doing.
    Status,
    /// The vocabulary: every control, gesture, page and action available.
    Describe,
    /// Every binding currently configured.
    Bindings,
    /// Add a binding, or replace the one at the same address.
    Bind {
        /// The binding to write.
        spec: Box<BindingSpec>,
    },
    /// Remove the binding at an address.
    Unbind {
        /// Which binding to remove.
        address: BindingAddress,
    },
    /// Run a configured binding's action once, without touching the hardware.
    ///
    /// Deliberately narrower than "run an arbitrary action": a client can only
    /// trigger something the operator has already configured and permitted.
    Test {
        /// Which binding to run.
        address: BindingAddress,
    },
    /// Re-read the configuration from disk.
    Reload,
}

/// What the runtime answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Response {
    /// How the runtime is doing.
    Status(StatusReport),
    /// The vocabulary.
    Vocabulary(Box<Vocabulary>),
    /// The configured bindings.
    ///
    /// Wrapped in a struct rather than carrying the list directly: the tag is
    /// written into the same object as the payload, and a bare sequence has
    /// nowhere to put it.
    Bindings(BindingList),
    /// An edit was applied.
    Edited(EditReport),
    /// A binding was run.
    Tested(TestReport),
    /// The request could not be carried out.
    Failed(Failure),
}

/// The bindings currently configured.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BindingList {
    /// Every binding, in configuration order.
    pub bindings: Vec<BindingSpec>,
}

impl From<Vec<BindingSpec>> for BindingList {
    fn from(bindings: Vec<BindingSpec>) -> Self {
        Self { bindings }
    }
}

/// How the runtime is doing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusReport {
    /// The protocol this runtime speaks.
    pub protocol: u32,
    /// The PushOS version running.
    pub version: String,
    /// What the runtime is driving.
    pub surface: SurfaceReport,
    /// The page currently in effect.
    pub page: Option<String>,
    /// The workspace currently in effect.
    pub workspace: Option<String>,
    /// Where the configuration is read from.
    pub config_root: String,
    /// How many bindings are in force.
    pub binding_count: usize,
}

/// Everything a client needs to offer a choice without hard-coding it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vocabulary {
    /// Every addressable control.
    pub controls: Vec<ControlInfo>,
    /// Every gesture that can be bound.
    pub gestures: Vec<String>,
    /// Every configured page.
    pub pages: Vec<PageInfo>,
    /// Every installed action provider.
    pub providers: Vec<ProviderInfo>,
}

/// One control on the surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlInfo {
    /// The control's name, such as `pad.23`.
    pub id: String,
    /// Its broad category: `pad`, `button`, `encoder` or `touch_strip`.
    pub kind: String,
    /// Whether PushOS can light it.
    pub illuminated: bool,
    /// For pads, its position in the grid from the top left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<GridPosition>,
}

/// A pad's place in the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridPosition {
    /// Row, counted from the top.
    pub row: u8,
    /// Column, counted from the left.
    pub column: u8,
}

/// One configured page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageInfo {
    /// The page's identity.
    pub id: String,
    /// Its display name.
    pub name: String,
    /// Its description, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One installed action provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// The namespace it claims, such as `media`.
    pub name: String,
    /// The verbs it implements.
    pub verbs: Vec<String>,
    /// The capabilities it needs before any of its verbs may run.
    pub requires: Vec<String>,
    /// Whether those capabilities are currently granted.
    pub permitted: bool,
}

/// The outcome of an edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditReport {
    /// The file that changed.
    pub file: String,
    /// Whether an existing binding was replaced rather than one added.
    pub replaced: bool,
    /// How many bindings are in force now.
    pub binding_count: usize,
}

/// The outcome of running a binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestReport {
    /// The action that ran.
    pub action: String,
    /// How it finished.
    pub status: String,
    /// What it had to say, when anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Why a request could not be carried out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// A short machine-readable reason.
    pub kind: FailureKind,
    /// What went wrong, for a person.
    pub message: String,
    /// The individual problems, when a configuration was rejected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

impl Failure {
    /// Builds a failure with no itemised problems.
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            problems: Vec::new(),
        }
    }

    /// Attaches the individual problems.
    #[must_use]
    pub fn with_problems(mut self, problems: Vec<String>) -> Self {
        self.problems = problems;
        self
    }
}

/// The kind of failure, so a client can react without parsing prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FailureKind {
    /// The request was not understood.
    Malformed,
    /// The request named something that does not exist.
    NotFound,
    /// The edit would produce an unusable configuration.
    Rejected,
    /// The action needs a capability that is not granted.
    Denied,
    /// Something in the runtime went wrong.
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_as_tagged_json() {
        let requests = [
            Request::Status,
            Request::Describe,
            Request::Bindings,
            Request::Reload,
            Request::Unbind {
                address: BindingAddress::new("pad.0", "tap"),
            },
            Request::Test {
                address: BindingAddress::new("button.play", "press"),
            },
        ];

        for request in requests {
            let line = serde_json::to_string(&request).expect("serialisable");
            assert!(
                line.contains("\"request\""),
                "requests carry their tag: {line}"
            );
            assert_eq!(
                serde_json::from_str::<Request>(&line).expect("deserialisable"),
                request
            );
        }
    }

    #[test]
    fn a_request_fits_on_one_line() {
        let line = serde_json::to_string(&Request::Bind {
            spec: Box::new(BindingSpec::new(
                BindingAddress::new("pad.0", "tap"),
                "media.play_pause",
            )),
        })
        .expect("serialisable");

        assert!(!line.contains('\n'), "the framing is one request per line");
    }

    #[test]
    fn responses_round_trip_as_tagged_json() {
        let response = Response::Failed(
            Failure::new(FailureKind::Rejected, "the configuration is not usable")
                .with_problems(vec!["binding 1: unknown control `pad.99`".to_owned()]),
        );

        let line = serde_json::to_string(&response).expect("serialisable");
        assert_eq!(
            serde_json::from_str::<Response>(&line).expect("deserialisable"),
            response
        );
    }

    /// Every variant must actually encode. An internally tagged enum cannot
    /// carry a bare sequence, and the failure only shows up at runtime.
    #[test]
    fn every_response_variant_can_be_encoded_and_read_back() {
        let responses = [
            Response::Status(StatusReport {
                protocol: PROTOCOL_VERSION,
                version: "0.1.0".to_owned(),
                surface: SurfaceReport::Push2,
                page: Some("home".to_owned()),
                workspace: None,
                config_root: "/tmp".to_owned(),
                binding_count: 3,
            }),
            Response::Vocabulary(Box::new(Vocabulary {
                controls: vec![ControlInfo {
                    id: "pad.0".to_owned(),
                    kind: "pad".to_owned(),
                    illuminated: true,
                    grid: Some(GridPosition { row: 0, column: 0 }),
                }],
                gestures: vec!["tap".to_owned()],
                pages: vec![PageInfo {
                    id: "home".to_owned(),
                    name: "Home".to_owned(),
                    description: None,
                }],
                providers: vec![ProviderInfo {
                    name: "media".to_owned(),
                    verbs: vec!["play_pause".to_owned()],
                    requires: vec!["media.control".to_owned()],
                    permitted: true,
                }],
            })),
            Response::Bindings(
                vec![BindingSpec::new(
                    BindingAddress::new("pad.0", "tap"),
                    "page.next",
                )]
                .into(),
            ),
            Response::Edited(EditReport {
                file: "/tmp/pushos.toml".to_owned(),
                replaced: false,
                binding_count: 4,
            }),
            Response::Tested(TestReport {
                action: "media.play_pause".to_owned(),
                status: "completed".to_owned(),
                message: None,
            }),
            Response::Failed(Failure::new(FailureKind::NotFound, "nothing there")),
        ];

        for response in responses {
            let line = serde_json::to_string(&response)
                .unwrap_or_else(|error| panic!("{response:?} could not be encoded: {error}"));
            assert!(!line.contains('\n'), "the framing is one response per line");
            assert_eq!(
                serde_json::from_str::<Response>(&line).expect("deserialisable"),
                response
            );
        }
    }

    #[test]
    fn a_simulated_surface_is_never_described_as_attached_hardware() {
        assert!(SurfaceReport::Push2.is_hardware());
        assert!(!SurfaceReport::Simulated.is_hardware());
        assert!(!SurfaceReport::Absent.is_hardware());

        for report in [SurfaceReport::Simulated, SurfaceReport::Absent] {
            assert!(
                !report.describe().contains("Push 2 attached"),
                "`{}` reads as attached hardware",
                report.describe()
            );
        }
    }

    #[test]
    fn the_surface_round_trips_as_a_readable_name() {
        let encoded = serde_json::to_string(&SurfaceReport::Simulated).expect("serialisable");
        assert_eq!(encoded, "\"simulated\"");
        assert_eq!(
            serde_json::from_str::<SurfaceReport>(&encoded).expect("deserialisable"),
            SurfaceReport::Simulated
        );
    }

    #[test]
    fn an_unknown_request_is_rejected_rather_than_guessed_at() {
        assert!(serde_json::from_str::<Request>(r#"{"request":"self_destruct"}"#).is_err());
        assert!(serde_json::from_str::<Request>("not json at all").is_err());
    }

    #[test]
    fn the_protocol_version_is_stated_so_a_mismatch_can_be_refused() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
