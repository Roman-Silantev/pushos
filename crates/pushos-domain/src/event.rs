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
        /// The workspace now in effect.
        workspace: WorkspaceId,
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
    fn a_root_event_has_no_cause() {
        let root = EventEnvelope::root(
            EventSource::Config,
            CorrelationId::generate(),
            DomainEvent::ConfigUpdated { binding_count: 12 },
        );
        assert!(root.causation_id.is_none());
    }
}
