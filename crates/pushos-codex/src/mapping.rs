//! Codex's vocabulary, translated into the one PushOS uses.
//!
//! Everything above the agent port works in normalised states, so this is the
//! only place that knows what Codex calls things. A notification PushOS has no
//! use for produces nothing rather than a guess: the surface shows fewer, truer
//! things that way.

use pushos_domain::agent::AgentState;
use pushos_domain::ports::{AgentEvent, ApprovalId, ApprovalOption, StopReason};
use std::collections::HashMap;

use serde_json::Value;

use crate::server::Incoming;

/// Which thread a message was about, and what it means.
#[derive(Clone, Debug, PartialEq)]
pub struct Translated {
    /// The Codex thread id.
    pub thread: String,
    /// What PushOS should make of it.
    pub event: AgentEvent,
}

/// Reads the thread id out of a message.
///
/// Codex names it `threadId` almost everywhere and `conversationId` in the
/// older approval requests, which are the two spellings this accepts.
pub fn thread_of(params: &Value) -> Option<String> {
    for key in ["threadId", "conversationId"] {
        if let Some(id) = params.get(key).and_then(Value::as_str) {
            return Some(id.to_owned());
        }
    }
    None
}

/// How much of a streaming answer is kept while it arrives.
///
/// The display shows one line, so this only has to be long enough to fill it
/// and short enough that sixty-four of them are nothing. A long answer is
/// shown by its most recent part rather than its beginning, which is what an
/// operator watching an agent work wants to see.
const NARRATION_MOST: usize = 512;

/// The most threads whose part-finished sentences are kept at once.
///
/// One per live agent, with room to spare. A thread that ends without saying
/// so would otherwise leave its sentence behind for as long as PushOS ran.
const NARRATING_MOST: usize = 128;

/// Keeps track of what each thread is part-way through saying.
///
/// Codex streams an answer a few characters at a time. Reported as they
/// arrive, the display ends up showing the last fragment — which for most
/// English sentences is a full stop, and was exactly what the first live fleet
/// put on screen eight times over.
#[derive(Debug, Default)]
pub struct Narrator {
    saying: HashMap<String, String>,
}

impl Narrator {
    /// A narrator with nothing part-said.
    pub fn new() -> Self {
        Self::default()
    }

    /// Translates one message, keeping track of anything part-said.
    pub fn read(&mut self, incoming: &Incoming) -> Option<Translated> {
        if let Incoming::Notification { method, params } = incoming
            && method == "item/agentMessage/delta"
        {
            let thread = thread_of(params)?;
            let delta = text_of(params)?;
            return Some(Translated {
                event: AgentEvent::Said {
                    text: self.absorb(&thread, &delta),
                },
                thread,
            });
        }

        let translated = translate(incoming)?;
        // A finished message replaces whatever was being assembled for it; the
        // completed text is the whole answer and the fragments were a preview.
        if matches!(
            translated.event,
            AgentEvent::Said { .. } | AgentEvent::Ended { .. }
        ) {
            self.saying.remove(&translated.thread);
        }
        Some(translated)
    }

    /// Forgets what a thread was part-way through saying.
    pub fn forget(&mut self, thread: &str) {
        self.saying.remove(thread);
    }

    /// Adds a fragment to what a thread is saying, and returns the whole of it.
    fn absorb(&mut self, thread: &str, delta: &str) -> String {
        if !self.saying.contains_key(thread) && self.saying.len() >= NARRATING_MOST {
            // Something is not finishing its sentences. Keeping the ones
            // already being followed is better than following everything.
            return delta.to_owned();
        }

        let saying = self.saying.entry(thread.to_owned()).or_default();
        saying.push_str(delta);

        if saying.len() > NARRATION_MOST {
            // Keep the end, which is what is being said now, and cut on a
            // character boundary so the line is still text.
            let mut from = saying.len() - NARRATION_MOST;
            while !saying.is_char_boundary(from) {
                from += 1;
            }
            *saying = saying[from..].to_owned();
        }
        saying.clone()
    }
}

/// Translates one message, or nothing if PushOS has no use for it.
///
/// Pure, and therefore blind to anything spread across several messages. A
/// streamed answer is one of those; [`Narrator`] handles it.
pub fn translate(incoming: &Incoming) -> Option<Translated> {
    match incoming {
        Incoming::Notification { method, params } => {
            let thread = thread_of(params)?;
            notification(method, params).map(|event| Translated { thread, event })
        }
        Incoming::Request { id, method, params } => {
            let thread = thread_of(params)?;
            asked(id, method, params).map(|event| Translated { thread, event })
        }
    }
}

fn notification(method: &str, params: &Value) -> Option<AgentEvent> {
    match method {
        "turn/started" => Some(AgentEvent::StateChanged {
            state: AgentState::Working,
        }),
        "turn/completed" => Some(AgentEvent::StateChanged {
            state: AgentState::Completed,
        }),
        // What the agent is saying, as it says it. Handled by `Narrator` when
        // there is one, because a fragment on its own is not a sentence.
        "item/agentMessage/delta" => text_of(params).map(|text| AgentEvent::Said { text }),
        // A tool starting and finishing are the same event with a different
        // answer to "is it done", which is what the port asks for.
        "item/started" => tool_of(params).map(|title| AgentEvent::UsedTool {
            title,
            finished: false,
        }),
        // A finished message carries the whole of what was said, which is the
        // one place the complete answer appears.
        "item/completed" => said_of(params)
            .map(|text| AgentEvent::Said { text })
            .or_else(|| {
                tool_of(params).map(|title| AgentEvent::UsedTool {
                    title,
                    finished: true,
                })
            }),
        "thread/closed" => Some(AgentEvent::Ended {
            reason: StopReason::Completed,
        }),
        // What went wrong is said separately, because the reason a session
        // ended is a fixed vocabulary and the detail is not part of it.
        "error" => Some(AgentEvent::Ended {
            reason: StopReason::Failed,
        }),
        _ => None,
    }
}

/// The permission questions, which are the ones that must never be dropped.
///
/// A thread that has asked is stopped until it is answered, so an unrecognised
/// question is better reported as a question PushOS cannot phrase than swallowed.
fn asked(id: &Value, method: &str, params: &Value) -> Option<AgentEvent> {
    let question = match method {
        "execCommandApproval" | "item/commandExecution/requestApproval" => {
            let command = command_of(params).unwrap_or_else(|| "a command".to_owned());
            format!("run {command}")
        }
        "applyPatchApproval" | "item/fileChange/requestApproval" => "change files".to_owned(),
        "item/permissions/requestApproval" => params
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("do something it needs permission for")
            .to_owned(),
        _ => return None,
    };

    Some(AgentEvent::AskedPermission {
        request: ApprovalId(request_key(id)),
        question,
        options: vec![
            ApprovalOption {
                id: "approved".to_owned(),
                label: "Allow".to_owned(),
                allows: true,
            },
            ApprovalOption {
                id: "denied".to_owned(),
                label: "Deny".to_owned(),
                allows: false,
            },
        ],
    })
}

/// How a JSON-RPC id is written down so the answer can quote it exactly.
///
/// The protocol allows a number or a string, and answering with the wrong one
/// leaves the thread that asked stopped for good. Kept as the JSON itself so
/// it goes back exactly as it arrived.
pub fn request_key(id: &Value) -> String {
    id.to_string()
}

/// Reads back an id written by [`request_key`].
pub fn request_id(key: &str) -> Option<Value> {
    serde_json::from_str(key).ok()
}

/// The whole of a finished agent message.
fn said_of(params: &Value) -> Option<String> {
    let item = params.get("item")?;
    if item.get("type").and_then(Value::as_str)? != "agentMessage" {
        return None;
    }
    let text = item.get("text").and_then(Value::as_str)?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn text_of(params: &Value) -> Option<String> {
    for key in ["delta", "text"] {
        if let Some(text) = params.get(key).and_then(Value::as_str)
            && !text.is_empty()
        {
            return Some(text.to_owned());
        }
    }
    None
}

/// A short description of what a thread item is doing.
fn tool_of(params: &Value) -> Option<String> {
    let item = params.get("item")?;
    let kind = item.get("type").and_then(Value::as_str)?;
    // An assistant message is not a tool; it arrives as `Said` instead, and
    // reporting it twice would put it on the display twice.
    if kind == "agentMessage" || kind == "reasoning" {
        return None;
    }
    let detail = command_of(item)
        .or_else(|| item.get("title").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| kind.to_owned());
    Some(detail)
}

/// The command a message is about, as one readable line.
fn command_of(params: &Value) -> Option<String> {
    match params.get("command")? {
        Value::String(whole) => Some(whole.clone()),
        Value::Array(words) => {
            let joined: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
            (!joined.is_empty()).then(|| joined.join(" "))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn notified(method: &str, params: Value) -> Option<Translated> {
        translate(&Incoming::Notification {
            method: method.to_owned(),
            params,
        })
    }

    #[test]
    fn a_turn_starting_and_finishing_move_the_state() {
        let started = notified("turn/started", json!({"threadId": "t1"})).expect("translated");
        assert_eq!(started.thread, "t1");
        assert_eq!(
            started.event,
            AgentEvent::StateChanged {
                state: AgentState::Working
            }
        );

        let done = notified("turn/completed", json!({"threadId": "t1"})).expect("translated");
        assert_eq!(
            done.event,
            AgentEvent::StateChanged {
                state: AgentState::Completed
            }
        );
    }

    #[test]
    fn what_the_agent_says_arrives_as_it_is_said() {
        let said = notified(
            "item/agentMessage/delta",
            json!({"threadId": "t1", "delta": "running the tests"}),
        )
        .expect("translated");
        assert_eq!(
            said.event,
            AgentEvent::Said {
                text: "running the tests".to_owned()
            }
        );
    }

    #[test]
    fn an_empty_delta_says_nothing_rather_than_saying_nothing_loudly() {
        assert!(
            notified(
                "item/agentMessage/delta",
                json!({"threadId": "t1", "delta": ""})
            )
            .is_none()
        );
    }

    #[test]
    fn a_command_item_is_reported_as_the_command_it_runs() {
        let used = notified(
            "item/started",
            json!({
                "threadId": "t1",
                "item": {"type": "commandExecution", "command": ["cargo", "test"]},
            }),
        )
        .expect("translated");
        assert_eq!(
            used.event,
            AgentEvent::UsedTool {
                title: "cargo test".to_owned(),
                finished: false,
            }
        );
    }

    #[test]
    fn an_assistant_message_is_not_also_reported_as_a_tool() {
        // It already arrives as `Said`; reporting it here would put the same
        // line on the display twice.
        assert!(
            notified(
                "item/started",
                json!({"threadId": "t1", "item": {"type": "agentMessage"}})
            )
            .is_none()
        );
    }

    #[test]
    fn a_permission_question_keeps_the_id_its_answer_must_quote() {
        let asked = translate(&Incoming::Request {
            id: json!("req-9"),
            method: "execCommandApproval".to_owned(),
            params: json!({
                "conversationId": "t1",
                "command": ["rm", "-rf", "build"],
            }),
        })
        .expect("translated");

        assert_eq!(asked.thread, "t1");
        match asked.event {
            AgentEvent::AskedPermission {
                request,
                question,
                options,
            } => {
                assert!(question.contains("rm -rf build"));
                assert_eq!(options.len(), 2);
                assert!(options[0].allows);
                assert!(!options[1].allows);
                assert_eq!(
                    request_id(&request.0),
                    Some(json!("req-9")),
                    "the id must go back exactly as it arrived"
                );
            }
            other => panic!("expected a question, got {other:?}"),
        }
    }

    #[test]
    fn a_message_about_no_particular_thread_is_dropped() {
        assert!(notified("account/updated", json!({"plan": "pro"})).is_none());
    }

    #[test]
    fn an_unrecognised_notification_produces_nothing_rather_than_a_guess() {
        assert!(notified("thread/tokenUsage/updated", json!({"threadId": "t1"})).is_none());
    }
}

#[cfg(test)]
mod narration {
    use super::*;
    use serde_json::json;

    fn delta(thread: &str, text: &str) -> Incoming {
        Incoming::Notification {
            method: "item/agentMessage/delta".to_owned(),
            params: json!({"threadId": thread, "delta": text}),
        }
    }

    fn finished(thread: &str, text: &str) -> Incoming {
        Incoming::Notification {
            method: "item/completed".to_owned(),
            params: json!({
                "threadId": thread,
                "item": {"type": "agentMessage", "text": text},
            }),
        }
    }

    fn said(translated: Option<Translated>) -> String {
        match translated.map(|t| t.event) {
            Some(AgentEvent::Said { text }) => text,
            other => panic!("expected something said, got {other:?}"),
        }
    }

    #[test]
    fn a_streamed_answer_is_assembled_rather_than_shown_a_fragment_at_a_time() {
        // The bug this exists for: reported as they arrive, the last fragment
        // of most English sentences is a full stop, and the first live fleet
        // put "." on the display eight times over.
        let mut narrator = Narrator::new();
        assert_eq!(
            said(narrator.read(&delta("t1", "There are "))),
            "There are "
        );
        assert_eq!(
            said(narrator.read(&delta("t1", "twenty"))),
            "There are twenty"
        );
        assert_eq!(
            said(narrator.read(&delta("t1", " crates."))),
            "There are twenty crates."
        );
    }

    #[test]
    fn the_finished_message_replaces_what_was_being_assembled() {
        let mut narrator = Narrator::new();
        narrator.read(&delta("t1", "There are twe"));
        assert_eq!(
            said(narrator.read(&finished("t1", "There are twenty crates."))),
            "There are twenty crates."
        );

        // And the next answer starts clean rather than continuing the last.
        assert_eq!(said(narrator.read(&delta("t1", "Next"))), "Next");
    }

    #[test]
    fn two_agents_do_not_finish_each_others_sentences() {
        let mut narrator = Narrator::new();
        narrator.read(&delta("t1", "PushOS "));
        narrator.read(&delta("t2", "Ableton "));
        assert_eq!(
            said(narrator.read(&delta("t1", "is lean"))),
            "PushOS is lean"
        );
        assert_eq!(
            said(narrator.read(&delta("t2", "Push 2"))),
            "Ableton Push 2"
        );
    }

    #[test]
    fn a_long_answer_is_shown_by_its_most_recent_part() {
        let mut narrator = Narrator::new();
        for _ in 0..200 {
            narrator.read(&delta("t1", "words and more "));
        }
        let showing = said(narrator.read(&delta("t1", "END")));
        assert!(
            showing.len() <= NARRATION_MOST + "END".len(),
            "a sentence must not grow without bound: {} bytes",
            showing.len()
        );
        assert!(showing.ends_with("END"), "the newest part is what is shown");
    }

    #[test]
    fn a_cut_answer_is_still_text() {
        let mut narrator = Narrator::new();
        for _ in 0..200 {
            narrator.read(&delta("t1", "é"));
        }
        // Reaching this line at all means no cut landed mid-character.
        assert!(!said(narrator.read(&delta("t1", "é"))).is_empty());
    }

    #[test]
    fn a_thread_that_never_finishes_a_sentence_does_not_crowd_out_the_others() {
        let mut narrator = Narrator::new();
        for i in 0..NARRATING_MOST + 20 {
            narrator.read(&delta(&format!("t{i}"), "saying something"));
        }
        assert!(
            narrator.saying.len() <= NARRATING_MOST,
            "held {} part-said sentences",
            narrator.saying.len()
        );
    }

    #[test]
    fn a_thread_ending_forgets_what_it_was_saying() {
        let mut narrator = Narrator::new();
        narrator.read(&delta("t1", "half a sent"));
        narrator.forget("t1");
        assert_eq!(said(narrator.read(&delta("t1", "Fresh"))), "Fresh");
    }
}
