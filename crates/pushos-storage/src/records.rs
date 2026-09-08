//! What PushOS stores.

use std::time::{SystemTime, UNIX_EPOCH};

use pushos_domain::event::EventEnvelope;

/// One row of the audit trail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRecord {
    /// The event's identity.
    pub id: String,
    /// Ties every row produced by one gesture together.
    pub correlation_id: String,
    /// What directly led to this, when anything did.
    pub causation_id: Option<String>,
    /// Seconds since the Unix epoch.
    pub recorded_at: i64,
    /// The component that emitted it.
    pub source: String,
    /// A stable name for what happened.
    pub kind: String,
    /// A human-readable summary, when the event carries one.
    pub detail: Option<String>,
}

impl EventRecord {
    /// Flattens an envelope into a storable row.
    ///
    /// The payload is reduced to a stable kind and a summary rather than
    /// serialised whole, so the audit trail stays readable and a change to an
    /// event's fields does not invalidate history.
    pub fn from_envelope(envelope: &EventEnvelope) -> Self {
        Self {
            id: envelope.id.to_string(),
            correlation_id: envelope.correlation_id.to_string(),
            causation_id: envelope.causation_id.map(|id| id.to_string()),
            recorded_at: seconds_since_epoch(envelope.timestamp),
            source: envelope.source.slug().to_owned(),
            kind: envelope.payload.kind().to_owned(),
            detail: envelope.payload.summary(),
        }
    }
}

/// What a workspace restores when the operator returns to it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceMemoryRow {
    /// The workspace.
    pub workspace_id: String,
    /// The page that was in effect when it was last left.
    pub last_page: Option<String>,
    /// The session that was selected.
    pub selected_session: Option<String>,
}

fn seconds_since_epoch(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use pushos_domain::event::{DomainEvent, EventSource};
    use pushos_domain::ids::CorrelationId;

    use super::*;

    #[test]
    fn an_envelope_flattens_into_a_readable_row() {
        let envelope = EventEnvelope::root(
            EventSource::Push,
            CorrelationId::generate(),
            DomainEvent::PushDisconnected {
                reason: "cable removed".to_owned(),
            },
        );
        let record = EventRecord::from_envelope(&envelope);

        assert_eq!(record.kind, "push.disconnected");
        assert_eq!(record.detail.as_deref(), Some("cable removed"));
        assert_eq!(record.source, "push");
        assert!(record.causation_id.is_none());
        assert!(
            record.recorded_at > 1_700_000_000,
            "the timestamp should be a real one"
        );
    }

    #[test]
    fn a_caused_event_records_what_led_to_it() {
        let root = EventEnvelope::root(
            EventSource::Push,
            CorrelationId::generate(),
            DomainEvent::PushConnected,
        );
        let child = root.caused(
            EventSource::Renderer,
            DomainEvent::ConfigUpdated { binding_count: 4 },
        );

        let record = EventRecord::from_envelope(&child);
        assert_eq!(
            record.causation_id.as_deref(),
            Some(root.id.to_string().as_str())
        );
        assert_eq!(record.correlation_id, root.correlation_id.to_string());
    }
}
