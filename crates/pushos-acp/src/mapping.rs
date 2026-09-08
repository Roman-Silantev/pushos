//! Translating the protocol's vocabulary into PushOS's.
//!
//! This is the line the provider's own words stop at. Everything above it works
//! in normalised states and events, so an agent renaming a stop reason or
//! adding an update kind cannot ripple into bindings, lights or the display.
//!
//! Pure functions over protocol values, so the whole translation is tested
//! without a provider, a subprocess or a network.

use agent_client_protocol::schema::v1::{
    ContentBlock, SessionUpdate, StopReason as WireStopReason, ToolCall, ToolCallStatus,
    ToolCallUpdate,
};
use pushos_domain::ports::{AgentEvent, StopReason};

/// What PushOS makes of a stop reason.
///
/// Refusal and exhaustion are failures rather than clean finishes: the work the
/// operator asked for did not happen, and a green light would say it had.
pub fn stop_reason(reason: &WireStopReason) -> StopReason {
    match reason {
        WireStopReason::EndTurn => StopReason::Completed,
        WireStopReason::Cancelled => StopReason::Cancelled,
        WireStopReason::MaxTokens | WireStopReason::MaxTurnRequests => StopReason::Exhausted,
        WireStopReason::Refusal => StopReason::Refused,
        // The protocol may add reasons. An unrecognised one has still stopped
        // the work, and saying so is safer than reporting success.
        _ => StopReason::Failed,
    }
}

/// What PushOS makes of a session update.
///
/// Returns `None` for updates that say nothing an operator standing at a Push 2
/// can act on. A 960 by 160 panel is not a transcript.
pub fn session_update(update: &SessionUpdate) -> Option<AgentEvent> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            let text = content_text(&chunk.content)?;
            Some(AgentEvent::Said { text })
        }

        SessionUpdate::ToolCall(call) => Some(AgentEvent::UsedTool {
            title: tool_title(call),
            finished: matches!(
                call.status,
                ToolCallStatus::Completed | ToolCallStatus::Failed
            ),
        }),

        SessionUpdate::ToolCallUpdate(update) => Some(AgentEvent::UsedTool {
            title: tool_update_title(update),
            finished: matches!(
                update.fields.status,
                Some(ToolCallStatus::Completed | ToolCallStatus::Failed)
            ),
        }),

        // The operator's own words coming back, the agent's private reasoning,
        // plans, usage figures and configuration changes are all real, and none
        // of them belong on a hardware panel.
        _ => None,
    }
}

/// The readable text of a content block, if it has any.
pub fn content_text(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Text(text) => {
            let trimmed = text.text.trim();
            (!trimmed.is_empty()).then(|| collapse(trimmed))
        }
        // Images, audio and embedded resources have nothing to show here.
        _ => None,
    }
}

/// What a tool call is doing, in one line.
fn tool_title(call: &ToolCall) -> String {
    collapse(call.title.trim())
}

/// What a tool call update is doing, in one line.
fn tool_update_title(update: &ToolCallUpdate) -> String {
    update
        .fields
        .title
        .as_deref()
        .map_or_else(|| "working".to_owned(), |title| collapse(title.trim()))
}

/// How much of a line the display can use.
///
/// The panel shows one line of about this many characters; carrying more only
/// to truncate it later wastes the copy.
const LINE_LIMIT: usize = 120;

/// Reduces text to a single line the display can show.
fn collapse(text: &str) -> String {
    let single: String = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    if single.chars().count() <= LINE_LIMIT {
        return single;
    }

    let kept: String = single.chars().take(LINE_LIMIT).collect();
    format!("{kept}\u{2026}")
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::v1::{ContentChunk, TextContent};

    use super::*;

    fn said(text: &str) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text.to_owned(),
        ))))
    }

    #[test]
    fn a_clean_finish_is_the_only_completion() {
        assert_eq!(stop_reason(&WireStopReason::EndTurn), StopReason::Completed);
        assert_eq!(
            stop_reason(&WireStopReason::Cancelled),
            StopReason::Cancelled
        );
    }

    #[test]
    fn running_out_or_being_refused_is_a_failure_not_a_success() {
        // The work the operator asked for did not happen, and a green light
        // would say it had.
        for reason in [
            WireStopReason::MaxTokens,
            WireStopReason::MaxTurnRequests,
            WireStopReason::Refusal,
        ] {
            assert!(
                !matches!(stop_reason(&reason), StopReason::Completed),
                "{reason:?} was reported as a clean finish"
            );
            assert!(stop_reason(&reason).resulting_state().is_finished());
        }
    }

    #[test]
    fn what_the_agent_says_becomes_something_to_show() {
        let event = session_update(&said("rewriting the queue")).expect("worth showing");
        assert_eq!(
            event,
            AgentEvent::Said {
                text: "rewriting the queue".to_owned()
            }
        );
    }

    #[test]
    fn a_multi_line_message_becomes_one_line() {
        let event =
            session_update(&said("first line\n\n  second line  \nthird")).expect("worth showing");
        assert_eq!(
            event,
            AgentEvent::Said {
                text: "first line second line third".to_owned()
            }
        );
    }

    #[test]
    fn a_long_message_is_cut_to_something_the_panel_can_show() {
        let long = "x".repeat(500);
        let AgentEvent::Said { text } = session_update(&said(&long)).expect("worth showing") else {
            panic!("expected something said");
        };

        assert!(text.chars().count() <= LINE_LIMIT + 1);
        assert!(text.ends_with('\u{2026}'));
    }

    #[test]
    fn a_long_message_is_never_cut_through_a_character() {
        let long = "é".repeat(500);
        let AgentEvent::Said { text } = session_update(&said(&long)).expect("worth showing") else {
            panic!("expected something said");
        };
        assert!(text.ends_with('\u{2026}'));
        assert!(text.is_char_boundary(text.len() - 3));
    }

    #[test]
    fn an_empty_message_says_nothing() {
        assert!(session_update(&said("   \n  ")).is_none());
    }

    #[test]
    fn the_operators_own_words_are_not_echoed_back_to_them() {
        let echo = SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::Text(
            TextContent::new("do the thing".to_owned()),
        )));
        assert!(session_update(&echo).is_none());
    }

    #[test]
    fn private_reasoning_stays_off_the_panel() {
        let thought = SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(
            TextContent::new("hmm".to_owned()),
        )));
        assert!(
            session_update(&thought).is_none(),
            "a hardware panel is not a transcript"
        );
    }
}
