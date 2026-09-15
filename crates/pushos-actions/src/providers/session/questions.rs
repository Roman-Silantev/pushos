//! Questions sessions are waiting on the operator to answer from the Push.
//!
//! Each question is held by whoever asked, waiting, until a press answers it,
//! it runs out of patience, or it stops being a question: the operator
//! answered it in the session's own window, and the session has moved on.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use pushos_domain::attached::{Activity, Attached, Decision, SessionQuestion};
use tokio::sync::{oneshot, watch};

/// How long a question waits for a press.
///
/// Just under the ten minutes both agents give the command answering for
/// them, so PushOS stops waiting before the agent stops listening.
pub(crate) const PATIENCE: Duration = Duration::from_secs(570);

/// How long a question is left alone before the session's own state can
/// withdraw it.
///
/// A session starts showing that it is asking at about the moment it asks,
/// and a look at it taken just before would otherwise withdraw a question
/// that had only just arrived.
const SETTLING: Duration = Duration::from_secs(10);

/// Questions waiting for the operator, oldest first.
#[derive(Debug)]
pub(crate) struct Questions {
    next: AtomicU64,
    waiting: Mutex<Vec<Waiting>>,
    published: watch::Sender<Vec<SessionQuestion>>,
}

#[derive(Debug)]
struct Waiting {
    id: u64,
    question: SessionQuestion,
    asked: Instant,
    answer: oneshot::Sender<Decision>,
}

impl Default for Questions {
    fn default() -> Self {
        Self {
            next: AtomicU64::new(0),
            waiting: Mutex::new(Vec::new()),
            published: watch::channel(Vec::new()).0,
        }
    }
}

impl Questions {
    /// Follows the questions waiting, for the display.
    pub(crate) fn watch(&self) -> watch::Receiver<Vec<SessionQuestion>> {
        self.published.subscribe()
    }

    /// Asks, and waits for an answer from the Push.
    ///
    /// `None` when there was none: nobody pressed anything in time, or the
    /// question was answered where it was asked.
    pub(crate) async fn ask(
        &self,
        question: SessionQuestion,
        patience: Duration,
    ) -> Option<Decision> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        self.change(|waiting| {
            waiting.push(Waiting {
                id,
                question,
                asked: Instant::now(),
                answer,
            });
        });

        let decision = tokio::time::timeout(patience, answered)
            .await
            .ok()
            .and_then(Result::ok);

        // Gone already if it was answered or withdrawn; still here if it ran
        // out of patience.
        self.change(|waiting| waiting.retain(|held| held.id != id));
        decision
    }

    /// Answers the oldest question that `from` picks out.
    ///
    /// Returns the question that was answered, or `None` when nothing matched.
    pub(crate) fn answer(
        &self,
        from: impl Fn(&SessionQuestion) -> bool,
        decision: Decision,
    ) -> Option<SessionQuestion> {
        let mut answered = None;
        self.change(|waiting| {
            if let Some(at) = waiting.iter().position(|held| from(&held.question)) {
                let held = waiting.remove(at);
                // The asker may have stopped waiting a moment ago; the answer
                // is still the operator's and is reported as given.
                let _ = held.answer.send(decision);
                answered = Some(held.question);
            }
        });
        answered
    }

    /// Withdraws questions their sessions have stopped asking.
    ///
    /// A session still open and no longer asking was answered in its own
    /// window, and one no longer open has nobody left to answer. Either way
    /// the Push should stop offering to answer it.
    pub(crate) fn settle(&self, open: &[Attached]) {
        let now = Instant::now();
        self.change(|waiting| {
            waiting.retain(|held| {
                if now.duration_since(held.asked) < SETTLING {
                    return true;
                }
                open.iter()
                    .find(|session| held.question.is_from(session))
                    .is_some_and(|session| session.activity == Activity::NeedsDecision)
            });
        });
    }

    /// Whether anything is waiting.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.waiting
            .lock()
            .map_or(true, |waiting| waiting.is_empty())
    }

    /// Changes what is waiting, and publishes the result if it differs.
    fn change(&self, edit: impl FnOnce(&mut Vec<Waiting>)) {
        let Ok(mut waiting) = self.waiting.lock() else {
            return;
        };
        edit(&mut waiting);
        let now: Vec<SessionQuestion> = waiting.iter().map(|held| held.question.clone()).collect();
        self.published.send_if_modified(|shown| {
            if *shown == now {
                false
            } else {
                *shown = now;
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn question(device: &str) -> SessionQuestion {
        SessionQuestion {
            agent: "Claude Code".to_owned(),
            session: None,
            device: Some(device.to_owned()),
            tool: "Bash".to_owned(),
            detail: "touch notes.md".to_owned(),
        }
    }

    #[tokio::test]
    async fn a_press_answers_the_question_that_is_waiting() {
        let questions = Arc::new(Questions::default());
        let asking = Arc::clone(&questions);
        let waiting =
            tokio::spawn(async move { asking.ask(question("/dev/ttys003"), PATIENCE).await });

        let mut shown = questions.watch();
        shown
            .wait_for(|shown| !shown.is_empty())
            .await
            .expect("published");

        let answered = questions.answer(|_| true, Decision::Allow);
        assert_eq!(answered.map(|q| q.tool).as_deref(), Some("Bash"));
        assert_eq!(waiting.await.expect("joined"), Some(Decision::Allow));
        assert!(questions.is_empty());
        assert!(
            questions.watch().borrow().is_empty(),
            "and the display is told"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_question_nobody_answers_gives_up_with_no_decision() {
        let questions = Questions::default();
        assert_eq!(
            questions
                .ask(question("/dev/ttys003"), Duration::from_secs(1))
                .await,
            None
        );
        assert!(questions.is_empty());
    }

    #[tokio::test]
    async fn only_the_session_named_is_answered() {
        let questions = Arc::new(Questions::default());
        for device in ["/dev/ttys003", "/dev/ttys004"] {
            let asking = Arc::clone(&questions);
            tokio::spawn(async move { asking.ask(question(device), PATIENCE).await });
        }
        let mut shown = questions.watch();
        shown
            .wait_for(|shown| shown.len() == 2)
            .await
            .expect("both published");

        let answered = questions.answer(
            |q| q.device.as_deref() == Some("/dev/ttys004"),
            Decision::Deny,
        );
        assert_eq!(
            answered.and_then(|q| q.device).as_deref(),
            Some("/dev/ttys004")
        );
        assert_eq!(questions.watch().borrow().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_question_answered_in_its_own_window_is_withdrawn_once_it_has_settled() {
        let questions = Arc::new(Questions::default());
        let asking = Arc::clone(&questions);
        let waiting =
            tokio::spawn(async move { asking.ask(question("/dev/ttys003"), PATIENCE).await });
        let mut shown = questions.watch();
        shown
            .wait_for(|shown| !shown.is_empty())
            .await
            .expect("published");

        let moved_on = [Attached::new(
            "/dev/ttys003",
            "work",
            Activity::Working,
            "Terminal",
        )];
        questions.settle(&moved_on);
        assert!(!questions.is_empty(), "too soon to tell");

        // The clock the store reads is the real one, so the settling period is
        // stepped past by rewinding when the question was asked.
        if let Ok(mut waiting) = questions.waiting.lock() {
            for held in waiting.iter_mut() {
                held.asked -= SETTLING;
            }
        }
        questions.settle(&moved_on);
        assert!(questions.is_empty());
        assert_eq!(
            waiting.await.expect("joined"),
            None,
            "and the asker is let go"
        );
    }

    #[tokio::test]
    async fn a_question_still_being_asked_is_kept() {
        let questions = Arc::new(Questions::default());
        let asking = Arc::clone(&questions);
        tokio::spawn(async move { asking.ask(question("/dev/ttys003"), PATIENCE).await });
        let mut shown = questions.watch();
        shown
            .wait_for(|shown| !shown.is_empty())
            .await
            .expect("published");
        if let Ok(mut waiting) = questions.waiting.lock() {
            for held in waiting.iter_mut() {
                held.asked -= SETTLING;
            }
        }

        let asking_still = [Attached::new(
            "/dev/ttys003",
            "work",
            Activity::NeedsDecision,
            "Terminal",
        )];
        questions.settle(&asking_still);
        assert!(!questions.is_empty());
    }
}
