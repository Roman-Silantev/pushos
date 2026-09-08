//! The PushOS domain model.
//!
//! This crate holds the vocabulary of the system and the interfaces it depends
//! on. It has no knowledge of MIDI, USB, SQLite, macOS or any agent vendor, and
//! it must stay that way: every dependency arrow points inward, towards here.
//!
//! The central abstraction is a four-step pipeline:
//!
//! ```text
//! Control -> Gesture -> Binding -> Action
//! ```
//!
//! A pad is not wired to an agent. A pad produces a [`controls::ControlId`];
//! the recogniser turns input into a [`gesture::Gesture`]; the resolver picks a
//! [`binding::Binding`]; the dispatcher runs an [`action::ActionDefinition`].
//! Agents are one provider of actions among many.

#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]
#![doc(html_no_source)]

pub mod action;
pub mod agent;
pub mod binding;
pub mod color;
pub mod context;
pub mod controls;
pub mod error;
pub mod event;
pub mod gesture;
pub mod ids;
pub mod input;
pub mod page;
pub mod permissions;
pub mod ports;
pub mod terminal;
pub mod workspace;

/// The types most callers need.
pub mod prelude {
    pub use crate::action::{
        ActionContext, ActionDefinition, ActionResult, ActionSelector, ActionStatus, DisplayIntent,
        ParamValue, Params,
    };
    pub use crate::agent::{AgentDefinition, AgentState, AgentTarget};
    pub use crate::binding::{Binding, BindingKey, BindingScope};
    pub use crate::color::{LedAnimation, LedState, Rgb, StatusColor};
    pub use crate::context::SurfaceContext;
    pub use crate::controls::{ButtonId, ControlId, ControlKind, EncoderId, PadIndex};
    pub use crate::error::{ActionError, ErrorClass};
    pub use crate::event::{DomainEvent, EventEnvelope, EventSource};
    pub use crate::gesture::Gesture;
    pub use crate::ids::{
        ActionVerb, BindingId, CorrelationId, PageId, ProviderName, SessionId, WorkspaceId,
    };
    pub use crate::input::{ControlEvent, InputPhase};
    pub use crate::page::{Page, PageTarget};
    pub use crate::permissions::{Permission, PermissionSet};
    pub use crate::ports::{ActionProvider, Clock, PushInput, PushOutput, SurfaceKind};
}
