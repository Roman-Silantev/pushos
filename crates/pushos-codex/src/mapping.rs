//! Codex's vocabulary, translated into the one PushOS uses.
//!
//! Everything above the agent port works in normalised states, so this is the
//! only place that knows what Codex calls things. A notification PushOS has no
//! use for produces nothing rather than a guess: the surface shows fewer, truer
//! things that way.

use pushos_domain::agent::AgentState;
use pushos_domain::ports::{AgentEvent, ApprovalId, ApprovalOption, StopReason};
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

/// Translates one message, or nothing if PushOS has no use for it.
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
        // What the agent is saying, as it says it.
        "item/agentMessage/delta" => text_of(params).map(|text| AgentEvent::Said { text }),
        // A tool starting and finishing are the same event with a different
        // answer to "is it done", which is what the port asks for.
        "item/started" => tool_of(params).map(|title| AgentEvent::UsedTool {
            title,
            finished: false,
        }),
        "item/completed" => tool_of(params).map(|title| AgentEvent::UsedTool {
            title,
            finished: true,
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
