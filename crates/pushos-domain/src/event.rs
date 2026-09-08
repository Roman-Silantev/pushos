//! The typed event vocabulary and its envelope.

use std::time::SystemTime;

use crate::action::{ActionSelector, ActionStatus};
use crate::binding::BindingKey;
use crate::controls::ControlId;
use crate::gesture::Gesture;
use crate::ids::{BindingId, CorrelationId, EventId, PageId, WorkspaceId};

/// Something that happened inside PushOS.
///
/// Every variant is typed. Nothing carries a free-form event name, so a
/// subscriber cannot silently miss a rename.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum DomainEvent {
    /// A gesture was recognised on a control.
    GestureRecognised {
        /// What was touched, and how.
        key: BindingKey,
    },
    /// A gesture matched a binding.
    BindingResolved {
        /// The binding that won.
        binding: BindingId,
        /// The action it will run.
        selector: ActionSelector,
    },
    /// A gesture matched no binding on the current surface.
    BindingUnmatched {
        /// The control that was touched.
        control: ControlId,
        /// The gesture that was recognised.
        gesture: Gesture,
    },
    /// An action reached a new status.
    ActionProgressed {
        /// Which action.
        selector: ActionSelector,
        /// Its new status.
        status: ActionStatus,
        /// A short note, when the provider supplied one.
        message: Option<String>,
    },
    /// The active page changed.
    PageChanged {
        /// The page now in effect.
        page: PageId,
    },
    /// The active workspace changed.
    WorkspaceChanged {
        /// The workspace now in effect, or none if the operator left one.
        workspace: Option<WorkspaceId>,
    },
    /// The Push 2 became available.
    PushConnected,
    /// The Push 2 went away.
    PushDisconnected {
        /// Why, in operator-readable terms.
        reason: String,
    },
    /// Configuration was replaced atomically.
    ConfigUpdated {
        /// How many bindings the new configuration contains.
        binding_count: usize,
    },
    /// Configuration was rejected and the running configuration was kept.
    ConfigRejected {
        /// Why it was rejected.
        reason: String,
    },
}

impl DomainEvent {
    /// A stable, dotted name for what happened.
    ///
    /// Lives here rather than in the persistence layer so that adding a variant
    /// forces naming it, instead of quietly recording it as "unknown".
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::GestureRecognised { .. } => "gesture.recognised",
            Self::BindingResolved { .. } => "binding.resolved",
            Self::BindingUnmatched { .. } => "binding.unmatched",
            Self::ActionProgressed { .. } => "action.progressed",
            Self::PageChanged { .. } => "page.changed",
            Self::WorkspaceChanged { .. } => "workspace.changed",
            Self::PushConnected => "push.connected",
            Self::PushDisconnected { .. } => "push.disconnected",
            Self::ConfigUpdated { .. } => "config.updated",
            Self::ConfigRejected { .. } => "config.rejected",
        }
    }

    /// A short human-readable summary, when the event carries detail worth
    /// keeping in the audit trail.
    pub fn summary(&self) -> Option<String> {
        match self {
            Self::GestureRecognised { key } => Some(format!("{} {}", key.control, key.gesture)),
            Self::BindingResolved { binding, selector } => Some(format!("{binding} -> {selector}")),
            Self::BindingUnmatched { control, gesture } => Some(format!("{control} {gesture}")),
            Self::ActionProgressed {
                selector,
                status,
                message,
            } => Some(match message {
                Some(message) => format!("{selector} {status:?}: {message}"),
                None => format!("{selector} {status:?}"),
            }),
            Self::PageChanged { page } => Some(page.to_string()),
            Self::WorkspaceChanged { workspace } => Some(
                workspace
                    .as_ref()
                    .map_or_else(|| "none".to_owned(), ToString::to_string),
            ),
            Self::PushDisconnected { reason } | Self::ConfigRejected { reason } => {
                Some(reason.clone())
            }
            Self::ConfigUpdated { binding_count } => Some(format!("{binding_count} bindings")),
            Self::PushConnected => None,
        }
    }
}

/// Which component emitted an event, as a stable lowercase name.
impl EventSource {
    /// The name used in logs and in the audit trail.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Push => "push",
            Self::Gestures => "gestures",
            Self::Bindings => "bindings",
            Self::Actions => "actions",
            Self::Renderer => "renderer",
            Self::Config => "config",
            Self::Supervisor => "supervisor",
        }
    }
}

/// An event plus the metadata that makes it traceable.
#[derive(Clone, Debug)]
pub struct EventEnvelope {
    /// Unique identity of this event.
    pub id: EventId,
    /// Ties every event caused by one gesture together.
    pub correlation_id: CorrelationId,
    /// The event that directly caused this one, when there was one.
    pub causation_id: Option<EventId>,
    /// Wall-clock time of emission, for logs and persistence.
    pub timestamp: SystemTime,
    /// The component that emitted it.
    pub source: EventSource,
    /// What happened.
    pub payload: DomainEvent,
}

impl EventEnvelope {
    /// Emits a new event at the head of a causal chain.
    pub fn root(source: EventSource, correlation_id: CorrelationId, payload: DomainEvent) -> Self {
        Self {
            id: EventId::generate(),
            correlation_id,
            causation_id: None,
            timestamp: SystemTime::now(),
            source,
            payload,
        }
    }

    /// Emits an event caused by this one, inheriting its correlation.
    #[must_use]
    pub fn caused(&self, source: EventSource, payload: DomainEvent) -> Self {
        Self {
            id: EventId::generate(),
            correlation_id: self.correlation_id,
            causation_id: Some(self.id),
            timestamp: SystemTime::now(),
            source,
            payload,
        }
    }
}

/// Which component emitted an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EventSource {
    /// The Push 2 adapter.
    Push,
    /// The gesture recogniser.
    Gestures,
    /// The binding resolver.
    Bindings,
    /// The action dispatcher.
    Actions,
    /// The renderer.
    Renderer,
    /// The configuration loader.
    Config,
    /// The runtime supervisor.
    Supervisor,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caused_event_inherits_correlation_and_records_its_cause() {
        let correlation = CorrelationId::generate();
        let root = EventEnvelope::root(EventSource::Push, correlation, DomainEvent::PushConnected);
        let child = root.caused(
            EventSource::Bindings,
            DomainEvent::BindingUnmatched {
                control: ControlId::TouchStrip,
                gesture: Gesture::Touch,
            },
        );

        assert_eq!(child.correlation_id, correlation);
        assert_eq!(child.causation_id, Some(root.id));
        assert_ne!(child.id, root.id);
    }

    #[test]
    fn every_event_has_a_dotted_lowercase_name() {
        let events = [
            DomainEvent::PushConnected,
            DomainEvent::PushDisconnected {
                reason: String::new(),
            },
            DomainEvent::PageChanged {
                page: "home".into(),
            },
            DomainEvent::WorkspaceChanged {
                workspace: Some("syd".into()),
            },
            // Leaving a project is a thing that happened, and is recorded.
            DomainEvent::WorkspaceChanged { workspace: None },
            DomainEvent::ConfigUpdated { binding_count: 0 },
            DomainEvent::ConfigRejected {
                reason: String::new(),
            },
        ];

        for event in events {
            let kind = event.kind();
            assert!(kind.contains('.'), "`{kind}` should be a dotted name");
            assert_eq!(kind, kind.to_lowercase());
        }
    }

    #[test]
    fn only_events_with_detail_carry_a_summary() {
        assert!(DomainEvent::PushConnected.summary().is_none());
        assert_eq!(
            DomainEvent::PageChanged {
                page: "music".into()
            }
            .summary()
            .as_deref(),
            Some("music")
        );
    }

    #[test]
    fn a_root_event_has_no_cause() {
        let root = EventEnvelope::root(
            EventSource::Config,
            CorrelationId::generate(),
            DomainEvent::ConfigUpdated { binding_count: 12 },
        );
        assert!(root.causation_id.is_none());
    }
}
