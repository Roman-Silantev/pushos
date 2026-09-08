//! The event bus.
//!
//! Typed events, bounded channels, and one rule about loss: high-frequency
//! progress may be coalesced, but a state transition, an approval, a failure or
//! anything security-relevant is never dropped quietly.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use pushos_domain::event::{DomainEvent, EventEnvelope};
use tokio::sync::broadcast;
use tracing::error;

/// How many events a subscriber may fall behind before it starts missing them.
const CAPACITY: usize = 512;

/// Publishes events to every interested component.
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Arc<EventEnvelope>>,
    dropped: Arc<AtomicU64>,
}

impl EventBus {
    /// Builds a bus.
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(CAPACITY);
        Self {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Publishes an event.
    ///
    /// With no subscribers this is a no-op rather than an error: PushOS runs
    /// perfectly well with nothing listening.
    pub fn publish(&self, envelope: EventEnvelope) {
        let important = is_important(&envelope.payload);
        if self.sender.send(Arc::new(envelope)).is_err() && important {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Subscribes to everything published from now on.
    pub fn subscribe(&self) -> EventSubscription {
        EventSubscription {
            receiver: self.sender.subscribe(),
            dropped: Arc::clone(&self.dropped),
        }
    }

    /// How many subscribers are listening.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// How many important events have been missed.
    ///
    /// Non-zero means a subscriber fell behind, which is a fault worth
    /// surfacing rather than a number to watch quietly.
    pub fn dropped_important(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// One subscriber's view of the bus.
#[derive(Debug)]
pub struct EventSubscription {
    receiver: broadcast::Receiver<Arc<EventEnvelope>>,
    dropped: Arc<AtomicU64>,
}

impl EventSubscription {
    /// Waits for the next event.
    ///
    /// A subscriber that falls too far behind skips ahead rather than stalling
    /// the publisher, and the skip is counted and logged so it cannot pass
    /// unnoticed.
    pub async fn recv(&mut self) -> Option<Arc<EventEnvelope>> {
        loop {
            match self.receiver.recv().await {
                Ok(envelope) => return Some(envelope),
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    self.dropped.fetch_add(count, Ordering::Relaxed);
                    error!(count, "a subscriber fell behind and missed events");
                }
            }
        }
    }
}

/// Whether losing this event would leave PushOS or its operator misinformed.
const fn is_important(event: &DomainEvent) -> bool {
    !matches!(
        event,
        // Recognising a gesture is reported again the moment it resolves, so a
        // coalesced one costs nothing.
        DomainEvent::GestureRecognised { .. }
    )
}

#[cfg(test)]
mod tests {
    use pushos_domain::event::EventSource;
    use pushos_domain::ids::CorrelationId;

    use super::*;

    fn envelope(payload: DomainEvent) -> EventEnvelope {
        EventEnvelope::root(EventSource::Supervisor, CorrelationId::generate(), payload)
    }

    #[tokio::test]
    async fn a_subscriber_receives_what_is_published() {
        let bus = EventBus::new();
        let mut subscription = bus.subscribe();

        bus.publish(envelope(DomainEvent::PushConnected));
        let received = subscription.recv().await.expect("an event was published");
        assert_eq!(received.payload, DomainEvent::PushConnected);
    }

    #[tokio::test]
    async fn publishing_with_nobody_listening_is_not_an_error() {
        let bus = EventBus::new();
        bus.publish(envelope(DomainEvent::PushConnected));
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn every_subscriber_sees_every_event() {
        let bus = EventBus::new();
        let mut first = bus.subscribe();
        let mut second = bus.subscribe();

        bus.publish(envelope(DomainEvent::PushConnected));

        assert_eq!(
            first.recv().await.expect("published").payload,
            DomainEvent::PushConnected
        );
        assert_eq!(
            second.recv().await.expect("published").payload,
            DomainEvent::PushConnected
        );
    }

    #[tokio::test]
    async fn a_subscriber_that_falls_behind_skips_ahead_and_the_skip_is_counted() {
        let bus = EventBus::new();
        let mut subscription = bus.subscribe();

        for index in 0..CAPACITY + 50 {
            bus.publish(envelope(DomainEvent::ConfigUpdated {
                binding_count: index,
            }));
        }

        // The subscriber keeps working rather than stalling the publisher.
        let received = subscription.recv().await.expect("events are still coming");
        assert!(matches!(
            received.payload,
            DomainEvent::ConfigUpdated { .. }
        ));
        assert!(
            bus.dropped_important() > 0,
            "the skip must be recorded, not hidden"
        );
    }

    #[tokio::test]
    async fn a_subscription_ends_when_the_bus_is_gone() {
        let mut subscription = EventBus::new().subscribe();
        assert!(subscription.recv().await.is_none());
    }

    #[test]
    fn state_transitions_are_important_and_gesture_noise_is_not() {
        assert!(is_important(&DomainEvent::PushConnected));
        assert!(is_important(&DomainEvent::ConfigRejected {
            reason: String::new()
        }));
        assert!(is_important(&DomainEvent::PageChanged {
            page: "home".into()
        }));
        assert!(!is_important(&DomainEvent::GestureRecognised {
            key: pushos_domain::binding::BindingKey::new(
                pushos_domain::controls::ControlId::TouchStrip,
                pushos_domain::gesture::Gesture::Touch,
            ),
        }));
    }
}
