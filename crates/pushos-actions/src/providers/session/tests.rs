//! What the session namespace does, against sessions that exist only in memory.

use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
use pushos_domain::context::SurfaceContext;
use pushos_domain::ids::CorrelationId;
use pushos_testkit::{FakeAttached, SessionCall};

use super::*;

fn rig() -> (SessionProvider, FakeAttached) {
    let fake = FakeAttached::with_sessions([
        ("/dev/ttys003", "✳ Sprint 2 setup"),
        ("/dev/ttys004", "◐ Review project tasks"),
    ]);
    (SessionProvider::new(Arc::new(fake.clone())), fake)
}

fn context(verb: &str, target: Option<&str>) -> ActionContext {
    let mut params = Params::new();
    if let Some(target) = target {
        params.set("target", ParamValue::Text(target.into()));
    }
    ActionContext::new(
        ActionDefinition::new(
            format!("session.{verb}")
                .parse::<ActionSelector>()
                .expect("valid"),
            params,
        ),
        CorrelationId::generate(),
        SurfaceContext::empty(),
    )
}

fn typing(target: &str, text: &str) -> ActionContext {
    let mut context = context("send", Some(target));
    context
        .definition
        .params
        .set("text", ParamValue::Text(text.into()));
    context
}

#[tokio::test]
async fn a_pad_can_name_a_session_by_part_of_its_title() {
    // Which is what an operator remembers: the sprint work, not ttys003.
    let (provider, _fake) = rig();
    let result = provider
        .execute(context("select", Some("sprint")))
        .await
        .expect("it is open");

    assert_eq!(result.message.as_deref(), Some("Sprint 2 setup"));
    assert_eq!(
        provider.selected().map(|id| id.to_string()),
        Some("/dev/ttys003".to_owned())
    );
}

#[tokio::test]
async fn a_pad_can_name_a_session_by_its_device() {
    let (provider, _fake) = rig();
    provider
        .execute(context("select", Some("tty:ttys004")))
        .await
        .expect("it is open");
    assert_eq!(
        provider.selected().map(|id| id.to_string()),
        Some("/dev/ttys004".to_owned())
    );
}

#[tokio::test]
async fn one_pad_with_no_target_serves_whichever_is_selected() {
    let (provider, fake) = rig();
    provider
        .execute(context("select", Some("review")))
        .await
        .expect("it is open");

    provider
        .execute(typing("selected", "y\n"))
        .await
        .expect("typing succeeds");

    assert!(
        fake.calls().contains(&SessionCall::Sent(
            "/dev/ttys004".to_owned(),
            "y\n".to_owned()
        )),
        "{:?}",
        fake.calls()
    );
}

#[tokio::test]
async fn typing_with_nothing_selected_says_so_rather_than_guessing() {
    // Typing into whichever session happened to be first would be the
    // worst possible answer to an ambiguous instruction.
    let (provider, fake) = rig();
    provider
        .execute(typing("selected", "y\n"))
        .await
        .expect_err("nothing is selected");
    assert!(
        !fake
            .calls()
            .iter()
            .any(|call| matches!(call, SessionCall::Sent(..)))
    );
}

#[tokio::test]
async fn interrupting_sends_the_control_character_not_the_word() {
    let (provider, fake) = rig();
    provider
        .execute(context("interrupt", Some("sprint")))
        .await
        .expect("it is open");

    assert!(
        fake.calls().contains(&SessionCall::Sent(
            "/dev/ttys003".to_owned(),
            "\u{3}".to_owned()
        )),
        "{:?}",
        fake.calls()
    );
}

#[tokio::test]
async fn showing_a_session_puts_the_last_of_it_on_the_display() {
    let (provider, fake) = rig();
    fake.showing(
        "an older line\n\n────────────\nthe tests are running\n❯ \n────────────\nall 40 passed\n",
    );

    let result = provider
        .execute(context("show", Some("sprint")))
        .await
        .expect("it is open");

    let Some(DisplayIntent::Focus { lines, title, .. }) = result.display else {
        panic!("a session should take the whole panel and stay there");
    };
    assert_eq!(title, "Sprint 2 setup");
    assert_eq!(
        lines.last().map(String::as_str),
        Some("all 40 passed"),
        "the last thing said is what matters: {lines:?}"
    );
    assert!(lines.len() <= SHOWN_LINES);
    assert!(
        !lines.iter().any(|line| line.trim() == "────────────"),
        "rules are not what was said: {lines:?}"
    );
}

#[tokio::test]
async fn the_terminals_own_furniture_is_not_shown_as_something_said() {
    // An agent draws a good deal of chrome. Filling the panel with its
    // rules and its status line would leave no room for the answer.
    let (provider, fake) = rig();
    fake.showing(
        "the actual answer\n\
         ────────────────\n\
         ❯ \n\
         ────────────────\n\
         new task? /clear to save 739k tokens\n\
         ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents\n",
    );

    let result = provider
        .execute(context("show", Some("sprint")))
        .await
        .expect("it is open");
    let Some(DisplayIntent::Focus { lines, .. }) = result.display else {
        panic!("focused");
    };
    assert_eq!(lines, ["the actual answer"], "{lines:?}");
}

#[tokio::test]
async fn a_session_that_was_closed_is_reported_rather_than_failing_oddly() {
    let (provider, fake) = rig();
    fake.close("/dev/ttys003");

    let error = provider
        .execute(context("select", Some("sprint")))
        .await
        .expect_err("that window is closed");
    assert_eq!(error.class(), ErrorClass::Validation);
    assert!(error.to_string().contains("sprint"), "{error}");
}

#[tokio::test]
async fn a_pad_can_press_a_key_rather_than_type_a_line() {
    // Accepting a suggestion is Tab and nothing else. Typing a line would
    // accept it and send it, which is a different instruction.
    let (provider, fake) = rig();
    let mut context = context("press", Some("sprint"));
    context
        .definition
        .params
        .set("key", ParamValue::Text("tab".into()));

    provider.execute(context).await.expect("it is open");
    assert!(
        fake.calls()
            .contains(&SessionCall::Pressed("/dev/ttys003".to_owned(), Key::Tab)),
        "{:?}",
        fake.calls()
    );
}

#[tokio::test]
async fn a_key_pushos_cannot_press_is_refused_by_name() {
    let (provider, _fake) = rig();
    let mut context = context("press", Some("sprint"));
    context
        .definition
        .params
        .set("key", ParamValue::Text("f13".into()));

    let error = provider
        .execute(context)
        .await
        .expect_err("there is no such key");
    assert!(error.to_string().contains("f13"), "{error}");
    assert_eq!(error.class(), ErrorClass::Validation);
}

/// The device a focused view is showing.
fn showing(result: &ActionResult) -> String {
    match &result.display {
        Some(DisplayIntent::Focus { title, .. }) => title.clone(),
        other => panic!("expected a focused session, got {other:?}"),
    }
}

#[tokio::test]
async fn turning_through_the_sessions_wraps_and_needs_no_first_choice() {
    // An operator reaching for an encoder has not chosen anything yet, and
    // a knob that did nothing until they had would be a knob nobody used.
    let (provider, _fake) = rig();

    assert_eq!(
        showing(&provider.execute(context("next", None)).await.unwrap()),
        "Sprint 2 setup"
    );
    assert_eq!(
        showing(&provider.execute(context("next", None)).await.unwrap()),
        "Review project tasks"
    );
    assert_eq!(
        showing(&provider.execute(context("next", None)).await.unwrap()),
        "Sprint 2 setup",
        "and round again"
    );
}

#[tokio::test]
async fn turning_the_other_way_goes_back() {
    let (provider, _fake) = rig();
    assert_eq!(
        showing(&provider.execute(context("previous", None)).await.unwrap()),
        "Review project tasks",
        "starting from the far end"
    );
    assert_eq!(
        showing(&provider.execute(context("previous", None)).await.unwrap()),
        "Sprint 2 setup"
    );
}

#[tokio::test]
async fn browsing_opens_what_it_arrives_at() {
    // Turning a knob to browse and seeing nothing change would be a knob
    // that does nothing.
    let (provider, _fake) = rig();
    let result = provider.execute(context("next", None)).await.expect("open");
    assert!(matches!(result.display, Some(DisplayIntent::Focus { .. })));
}

/// A history longer than the panel holds, numbered so a window is readable.
fn said(lines: usize) -> String {
    use std::fmt::Write as _;
    (1..=lines).fold(String::new(), |mut screen, line| {
        let _ = writeln!(screen, "line {line}");
        screen
    })
}

/// What the panel is showing, as the numbers of the lines on it.
fn window(result: &ActionResult) -> Vec<usize> {
    let Some(DisplayIntent::Focus { lines, .. }) = &result.display else {
        panic!("focused");
    };
    lines
        .iter()
        .map(|line| {
            line.trim_start_matches("line ")
                .parse()
                .expect("the fixture numbers its lines")
        })
        .collect()
}

async fn moved(provider: &SessionProvider, verb: &str, key: &str, by: i64) -> ActionResult {
    let mut moving = context(verb, Some("sprint"));
    moving.definition.params.set(key, ParamValue::Integer(by));
    provider.execute(moving).await.expect("open")
}

#[tokio::test]
async fn scrolling_moves_back_through_what_was_said() {
    let (provider, fake) = rig();
    fake.showing(&said(12));
    provider
        .execute(context("select", Some("sprint")))
        .await
        .expect("open");

    let result = moved(&provider, "scroll", "by", 2).await;
    assert_eq!(window(&result), [3, 4, 5, 6, 7, 8, 9, 10], "two lines back");

    let Some(DisplayIntent::Focus { kind, .. }) = result.display else {
        panic!("focused");
    };
    assert!(kind.contains("back"), "and it says so: {kind}");
}

#[tokio::test]
async fn scrolling_stops_at_each_end_of_what_was_said() {
    let (provider, fake) = rig();
    fake.showing(&said(12));
    provider
        .execute(context("select", Some("sprint")))
        .await
        .expect("open");

    let far = moved(&provider, "scroll", "by", 500).await;
    assert_eq!(window(&far), [1, 2, 3, 4, 5, 6, 7, 8], "the beginning");

    let forward = moved(&provider, "scroll", "by", -500).await;
    assert_eq!(
        window(&forward),
        [5, 6, 7, 8, 9, 10, 11, 12],
        "and back to what it is saying now"
    );
}

#[tokio::test]
async fn a_history_that_fits_has_nowhere_to_scroll_to() {
    let (provider, fake) = rig();
    fake.showing(&said(4));
    provider
        .execute(context("select", Some("sprint")))
        .await
        .expect("open");

    let result = moved(&provider, "scroll", "by", 2).await;
    assert_eq!(window(&result), [1, 2, 3, 4], "all of it, and no moving");

    let Some(DisplayIntent::Focus { depth, .. }) = result.display else {
        panic!("focused");
    };
    assert_eq!(depth, None, "and no bar, because there is nowhere to go");
}

#[tokio::test]
async fn the_panel_is_told_where_in_the_history_it_is() {
    let (provider, fake) = rig();
    fake.showing(&said(108));
    provider
        .execute(context("select", Some("sprint")))
        .await
        .expect("open");

    let result = moved(&provider, "scroll", "by", 500).await;
    let Some(DisplayIntent::Focus { depth, .. }) = result.display else {
        panic!("focused");
    };
    assert_eq!(
        depth,
        Some(Depth {
            back: 100,
            total: 108
        }),
        "so the panel can draw where in it the operator is"
    );
}

#[tokio::test]
async fn a_knob_aimed_at_one_session_does_not_move_another() {
    // Eight knobs sit above eight columns, and each one moves the column
    // beneath it. A scroll position belongs to the thing it was scrolling
    // and there is one of them, so aiming a scroll has to move what it is
    // aimed at.
    let (provider, fake) = rig();
    fake.showing(&said(12));
    provider
        .execute(context("select", Some("sprint")))
        .await
        .expect("open");
    moved(&provider, "scroll", "by", 4).await;

    let mut aimed = context("scroll", Some("review"));
    aimed.definition.params.set("by", ParamValue::Integer(1));
    let result = provider.execute(aimed).await.expect("open");

    assert_eq!(
        window(&result),
        [4, 5, 6, 7, 8, 9, 10, 11],
        "one line back into the one the knob names, not five into it"
    );
    assert_eq!(
        provider.selected().map(|id| id.to_string()),
        Some("/dev/ttys004".to_owned()),
        "and the panel is now showing the one being read"
    );
}

#[tokio::test]
async fn a_button_can_jump_straight_to_one_bank() {
    // Eight buttons down the right edge stand for eight banks. Walking to
    // the fifth from the first is four presses; naming it is one.
    let fake = FakeAttached::with_sessions(
        (0..20).map(|at| (format!("/dev/ttys{at:03}"), format!("session {at}"))),
    );
    let provider = SessionProvider::new(Arc::new(fake));

    let mut third = context("bank", None);
    third.definition.params.set("to", ParamValue::Integer(3));
    provider.execute(third).await.expect("open");
    assert_eq!(provider.showing_from(), 16);

    // Past the end lands on the last bank that has anything in it, rather
    // than on eight empty pads.
    let mut far = context("bank", None);
    far.definition.params.set("to", ParamValue::Integer(8));
    provider.execute(far).await.expect("open");
    assert_eq!(
        provider.showing_from(),
        16,
        "twenty sessions is three banks"
    );

    let mut first = context("bank", None);
    first.definition.params.set("to", ParamValue::Integer(1));
    provider.execute(first).await.expect("open");
    assert_eq!(provider.showing_from(), 0);
}

#[tokio::test]
async fn looking_at_something_else_starts_it_at_the_bottom() {
    // A scroll position belongs to the thing it was scrolling.
    let (provider, fake) = rig();
    fake.showing(&said(12));
    provider
        .execute(context("select", Some("sprint")))
        .await
        .expect("open");
    moved(&provider, "scroll", "by", 2).await;

    let result = provider
        .execute(context("show", Some("review")))
        .await
        .expect("open");
    assert_eq!(
        window(&result),
        [5, 6, 7, 8, 9, 10, 11, 12],
        "the new one starts where it is now"
    );
}

#[tokio::test]
async fn only_typing_needs_saying_yes_to() {
    // Looking at a session changes nothing. Typing into one runs whatever
    // it makes of the keystrokes.
    let (provider, _fake) = rig();
    let capabilities = provider.capabilities();

    for typing in ["send", "press", "interrupt", "open"] {
        assert_eq!(
            capabilities.required_for(&ActionVerb::new(typing)),
            [Permission::ShellExecute],
            "`{typing}` types into a live session"
        );
    }
    for looking in ["select", "show", "focus"] {
        assert!(
            capabilities
                .required_for(&ActionVerb::new(looking))
                .is_empty(),
            "`{looking}` only looks"
        );
    }
}

#[tokio::test]
async fn a_refused_automation_permission_is_reported_as_one() {
    let (provider, fake) = rig();
    fake.refuse();

    let error = provider
        .execute(context("select", Some("sprint")))
        .await
        .expect_err("permission was refused");
    assert_eq!(error.class(), ErrorClass::Permission);
}

#[tokio::test]
async fn an_unknown_verb_is_refused_by_name() {
    let (provider, _fake) = rig();
    let error = provider
        .execute(context("restart", None))
        .await
        .expect_err("there is no such verb");
    assert!(matches!(error, ActionError::UnknownVerb { .. }));
}

fn opening(name: &str) -> ActionContext {
    let mut context = context("open", None);
    let params = &mut context.definition.params;
    params.set("name", ParamValue::Text(name.into()));
    params.set("cwd", ParamValue::Text("/work/client".into()));
    params.set("command", ParamValue::Text("claude".into()));
    context
}

#[tokio::test]
async fn a_pad_that_keeps_a_worker_starts_it_shows_it_and_selects_it() {
    // Selected, so the next thing dictated goes to the worker just opened.
    let (provider, fake) = rig();
    let result = provider
        .execute(opening("client-1"))
        .await
        .expect("it starts");

    let calls = fake.calls();
    let Some(SessionCall::Opened(request)) = calls
        .iter()
        .find(|call| matches!(call, SessionCall::Opened(_)))
    else {
        panic!("nothing was opened: {calls:?}");
    };
    assert_eq!(request.name, "client-1");
    assert_eq!(
        request.directory,
        Some(std::path::PathBuf::from("/work/client"))
    );
    assert_eq!(request.command.as_deref(), Some("claude"));

    let selected = provider.selected().expect("the new session is selected");
    assert!(
        calls.contains(&SessionCall::Focused(selected.to_string())),
        "and a window is opened onto it: {calls:?}"
    );
    assert_eq!(result.message.as_deref(), Some("client-1 started"));
}

#[tokio::test]
async fn pressing_the_same_pad_again_comes_back_to_the_same_worker() {
    let (provider, _fake) = rig();
    provider
        .execute(opening("client-1"))
        .await
        .expect("it starts");
    let first = provider.selected();

    let again = provider
        .execute(opening("client-1"))
        .await
        .expect("it is found");
    assert_eq!(again.message.as_deref(), Some("client-1 open"));
    assert_eq!(provider.selected(), first);
}

#[tokio::test]
async fn a_worker_can_be_started_without_a_window() {
    let (provider, fake) = rig();
    let mut quiet = opening("client-1");
    quiet
        .definition
        .params
        .set("window", ParamValue::Flag(false));
    provider.execute(quiet).await.expect("it starts");

    assert!(
        !fake
            .calls()
            .iter()
            .any(|call| matches!(call, SessionCall::Focused(_))),
        "{:?}",
        fake.calls()
    );
}

#[tokio::test]
async fn a_name_that_cannot_keep_a_session_is_refused_before_anything_starts() {
    let (provider, fake) = rig();
    let error = provider
        .execute(opening("client 1"))
        .await
        .expect_err("a space cannot be in a name");
    assert_eq!(error.class(), ErrorClass::Validation);
    assert!(
        !fake
            .calls()
            .iter()
            .any(|call| matches!(call, SessionCall::Opened(_)))
    );
}

#[tokio::test]
async fn typing_into_a_session_pushos_can_only_watch_says_where_to_answer_it() {
    let fake = FakeAttached::holding([pushos_domain::attached::Attached::new(
        "claude:0d2c",
        "nemo-claw-0a",
        pushos_domain::attached::Activity::NeedsDecision,
        "Visual Studio Code",
    )
    .watched_only()]);
    let provider = SessionProvider::new(Arc::new(fake));

    let error = provider
        .execute(typing("nemo", "yes"))
        .await
        .expect_err("there is nothing to type into");
    assert!(error.to_string().contains("Visual Studio Code"), "{error}");
    assert_eq!(error.class(), ErrorClass::Validation);
}

fn asked_from(device: &str) -> pushos_domain::attached::SessionQuestion {
    pushos_domain::attached::SessionQuestion {
        agent: "Claude Code".to_owned(),
        session: None,
        device: Some(device.to_owned()),
        tool: "Bash".to_owned(),
        detail: "touch notes.md".to_owned(),
    }
}

async fn waiting_for(provider: &SessionProvider, count: usize) {
    let mut shown = provider.questions();
    shown
        .wait_for(|shown| shown.len() == count)
        .await
        .expect("the questions are published");
}

#[tokio::test]
async fn one_button_answers_whichever_session_asked_first() {
    // Allowing is the operator's decision about what was asked, and a single
    // pair of buttons serves every session.
    let (provider, _fake) = rig();
    let provider = Arc::new(provider);
    let asking = Arc::clone(&provider);
    let waiting = tokio::spawn(async move { asking.ask(asked_from("/dev/ttys004")).await });
    waiting_for(&provider, 1).await;

    let result = provider
        .execute(context("approve", None))
        .await
        .expect("something is asking");
    assert!(
        result
            .message
            .as_deref()
            .is_some_and(|said| said.starts_with("allowed")),
        "{result:?}"
    );
    assert_eq!(
        waiting.await.expect("joined"),
        Some(pushos_domain::attached::Decision::Allow)
    );
}

#[tokio::test]
async fn a_pad_aimed_at_one_session_answers_only_that_sessions_question() {
    let (provider, _fake) = rig();
    let provider = Arc::new(provider);
    for device in ["/dev/ttys003", "/dev/ttys004"] {
        let asking = Arc::clone(&provider);
        tokio::spawn(async move { asking.ask(asked_from(device)).await });
    }
    waiting_for(&provider, 2).await;

    provider
        .execute(context("deny", Some("review")))
        .await
        .expect("the review session is asking");
    let left = provider.questions().borrow().clone();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].device.as_deref(), Some("/dev/ttys003"));
}

#[tokio::test]
async fn answering_when_nothing_is_asking_says_so() {
    let (provider, _fake) = rig();
    let error = provider
        .execute(context("approve", None))
        .await
        .expect_err("nothing is asking");
    assert_eq!(error.class(), ErrorClass::Validation);
}

#[tokio::test]
async fn only_allowing_needs_saying_yes_to() {
    let (provider, _fake) = rig();
    let capabilities = provider.capabilities();
    assert_eq!(
        capabilities.required_for(&ActionVerb::new("approve")),
        [Permission::ShellExecute]
    );
    assert!(
        capabilities
            .required_for(&ActionVerb::new("deny"))
            .is_empty()
    );
}

// --- Taking turns -----------------------------------------------------------
// Sixty-four pads can hold sessions, but the model will not answer sixty-four
// streams at once. Work given while the fleet is busy waits rather than failing.

/// Two sessions a coding agent keeps: one working, one ready for more.
fn a_busy_fleet() -> (SessionProvider, FakeAttached) {
    let working = pushos_domain::attached::Attached::new(
        "thread:busy",
        "already working",
        Activity::Working,
        "Codex",
    )
    .dispatched(pushos_domain::ports::Keeper::CodexThreads)
    .named("builder");
    let ready = pushos_domain::attached::Attached::new(
        "thread:free",
        "waiting for work",
        Activity::Ready,
        "Codex",
    )
    .dispatched(pushos_domain::ports::Keeper::CodexThreads)
    .named("reviewer");

    let fake = FakeAttached::holding([working, ready]);
    (SessionProvider::new(Arc::new(fake.clone())), fake)
}

fn sent(fake: &FakeAttached) -> Vec<(String, String)> {
    fake.calls()
        .into_iter()
        .filter_map(|call| match call {
            SessionCall::Sent(session, text) => Some((session, text)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn work_given_while_the_fleet_is_busy_waits_its_turn() {
    let (provider, fake) = a_busy_fleet();
    provider.allow_working(Some(1));

    let result = provider
        .execute(typing("id:thread:free", "review the invoices"))
        .await
        .expect("accepted");

    assert_eq!(result.status, ActionStatus::Started);
    assert!(
        result
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("waits its turn"),
        "{result:?}"
    );
    assert!(
        sent(&fake).is_empty(),
        "nothing was sent while one was working"
    );
    assert_eq!(provider.waiting_turns(), 1);
}

#[tokio::test]
async fn waiting_work_goes_as_soon_as_there_is_room() {
    let (provider, fake) = a_busy_fleet();
    provider.allow_working(Some(1));
    provider
        .execute(typing("id:thread:free", "review the invoices"))
        .await
        .expect("accepted");

    let started = provider.start_waiting_work(1).await;

    assert_eq!(started, 1);
    assert_eq!(
        sent(&fake),
        [("thread:free".to_owned(), "review the invoices".to_owned())]
    );
    assert_eq!(provider.waiting_turns(), 0);
}

#[tokio::test]
async fn work_goes_straight_out_while_the_fleet_has_room() {
    let (provider, fake) = a_busy_fleet();
    provider.allow_working(Some(4));

    let result = provider
        .execute(typing("id:thread:free", "review the invoices"))
        .await
        .expect("accepted");

    assert_eq!(result.status, ActionStatus::Completed);
    assert_eq!(sent(&fake).len(), 1, "one working, four allowed");
    assert_eq!(provider.waiting_turns(), 0);
}

#[tokio::test]
async fn a_session_in_a_terminal_is_typed_into_whatever_the_fleet_is_doing() {
    // It is a program the operator is looking at; its timing is theirs.
    let (provider, fake) = rig();
    provider.allow_working(Some(1));

    provider
        .execute(typing("tty:/dev/ttys003", "hello"))
        .await
        .expect("accepted");

    assert_eq!(sent(&fake).len(), 1);
    assert_eq!(provider.waiting_turns(), 0);
}

#[tokio::test]
async fn stopping_something_never_waits_for_a_turn() {
    let (provider, fake) = a_busy_fleet();
    provider.allow_working(Some(1));

    provider
        .execute(context("interrupt", Some("id:thread:free")))
        .await
        .expect("accepted");

    assert_eq!(sent(&fake).len(), 1, "an interrupt goes at once");
}

#[tokio::test]
async fn work_for_a_session_that_has_gone_stops_waiting() {
    let (provider, _fake) = a_busy_fleet();
    provider.allow_working(Some(1));
    provider
        .execute(typing("id:thread:free", "review the invoices"))
        .await
        .expect("accepted");

    provider.keep_work_for(&[]);

    assert_eq!(provider.waiting_turns(), 0);
    assert_eq!(provider.start_waiting_work(4).await, 0);
}
