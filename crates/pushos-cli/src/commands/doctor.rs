//! Diagnosing an installation.
//!
//! Every check reports what it found and what to do about it. A check that
//! cannot run is reported as such rather than passing quietly, because a
//! diagnostic that only ever says "fine" is worse than none.

use std::path::Path;
use std::sync::Arc;

use pushos_config::RuntimeConfig;
use pushos_domain::ports::ProcessRunner;
use pushos_macos::{ScriptRunner, SystemProcessRunner};
use pushos_push2::{PortRole, Push2Device};

use super::paths;

/// How a check turned out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// Working.
    Good,
    /// Not working, but PushOS runs without it.
    Optional,
    /// Not working, and PushOS needs it.
    Blocking,
}

impl Verdict {
    const fn mark(self) -> &'static str {
        match self {
            Self::Good => "ok  ",
            Self::Optional => "note",
            Self::Blocking => "fail",
        }
    }
}

/// One line of the report.
struct Finding {
    verdict: Verdict,
    subject: &'static str,
    detail: String,
}

impl Finding {
    fn new(verdict: Verdict, subject: &'static str, detail: impl Into<String>) -> Self {
        Self {
            verdict,
            subject,
            detail: detail.into(),
        }
    }
}

/// Runs every check and prints the report.
pub(crate) async fn execute(requested: Option<&Path>) -> Result<(), String> {
    let root = paths::config_root(requested)?;
    let mut findings = vec![check_configuration(&root), check_database(), check_push()];
    findings.extend(check_voice(&root));
    findings.extend(check_memory(&root));
    findings.extend(check_attached(&root).await);
    findings.extend(check_hooks());
    findings.extend(check_host_tools().await);

    for finding in &findings {
        println!(
            "[{}] {:<16} {}",
            finding.verdict.mark(),
            finding.subject,
            finding.detail
        );
    }

    let blocking = findings
        .iter()
        .filter(|f| f.verdict == Verdict::Blocking)
        .count();
    if blocking == 0 {
        println!("\nPushOS can run.");
        return Ok(());
    }
    Err(format!(
        "\n{blocking} check(s) must be fixed before PushOS can run."
    ))
}

/// Whether anyone but the owner can read the configuration.
///
/// PushOS keeps what it writes to its owner, but a file the operator wrote
/// themselves is theirs, and quietly changing its permissions would be PushOS
/// deciding something about a file it did not make. Said rather than done.
fn readable_by_others(root: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        pushos_config::paths::files_for(root).iter().any(|file| {
            std::fs::metadata(file).is_ok_and(|held| held.permissions().mode() & 0o077 != 0)
        })
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        false
    }
}

fn check_configuration(root: &Path) -> Finding {
    // A directory with nothing in it reads as a valid configuration of no
    // pages and no bindings, which is true and useless: a mistyped `--config`
    // or a moved directory gives a surface that does nothing, and every other
    // check here would go on to call it healthy.
    if pushos_config::paths::files_for(root).is_empty() {
        return Finding::new(
            Verdict::Blocking,
            "configuration",
            format!(
                "{}: no configuration here. `pushos init` writes a starting one, \
                 or pass --config with the directory yours is in",
                root.display()
            ),
        );
    }

    match pushos_config::load(root).and_then(|file| RuntimeConfig::build(&file)) {
        Ok(config) => Finding::new(
            if readable_by_others(root) {
                Verdict::Optional
            } else {
                Verdict::Good
            },
            "configuration",
            format!(
                "{} ({} pages, {} bindings){}",
                root.display(),
                config.pages.len(),
                config.bindings.len(),
                if readable_by_others(root) {
                    "; readable by other accounts on this Mac. \
                     `chmod 600` it if a project's environment holds anything private"
                } else {
                    ""
                }
            ),
        ),
        Err(error) => Finding::new(
            Verdict::Blocking,
            "configuration",
            super::check::describe(&error).replace('\n', "\n                     "),
        ),
    }
}

fn check_database() -> Finding {
    let Ok(path) = paths::state_file() else {
        return Finding::new(
            Verdict::Blocking,
            "database",
            "nowhere to keep runtime state",
        );
    };
    // Read-only on purpose: opening it the way PushOS does would make this a
    // second writer against the database a running PushOS owns, and would trim
    // and vacuum the operator's own audit trail for the sake of looking at it.
    match pushos_storage::readable(&path) {
        Ok(true) => Finding::new(Verdict::Good, "database", path.display().to_string()),
        Ok(false) => Finding::new(
            Verdict::Good,
            "database",
            format!("{} (made when PushOS first runs)", path.display()),
        ),
        Err(error) => Finding::new(
            Verdict::Blocking,
            "database",
            format!("{}: {error}", path.display()),
        ),
    }
}

fn check_push() -> Finding {
    match Push2Device::connect(PortRole::User) {
        Ok(_) => Finding::new(Verdict::Good, "push 2", "connected on the user port"),
        Err(error) if error.is_absent() => Finding::new(
            Verdict::Optional,
            "push 2",
            "not attached; PushOS will wait for it and take it when it appears",
        ),
        Err(error) => Finding::new(
            Verdict::Optional,
            "push 2",
            format!("{error}; check nothing else is holding the device"),
        ),
    }
}

/// Reports whether each coding agent installed here asks the Push first.
///
/// Never blocking: an agent without the hook asks in its own window, as it
/// always has.
fn check_hooks() -> Vec<Finding> {
    super::hook::installed()
        .into_iter()
        .map(|(agent, answered)| {
            if answered {
                Finding::new(
                    Verdict::Good,
                    "questions",
                    format!("{}: its permission questions come to the Push", agent.name()),
                )
            } else {
                Finding::new(
                    Verdict::Optional,
                    "questions",
                    format!(
                        "{}: `pushos hook install` to answer its permission questions from the Push",
                        agent.name()
                    ),
                )
            }
        })
        .collect()
}

/// Reports whether PushOS can see the coding sessions it did not start.
///
/// Nothing at all unless the operator asked for it. Each source is reported on
/// its own, because each fails for its own reason: Terminal because macOS has
/// not allowed it, tmux or Claude Code because they are not installed. Only
/// Terminal refusing blocks anything; the others are ways in that are simply
/// not there yet.
async fn check_attached(root: &Path) -> Vec<Finding> {
    let Ok(config) = pushos_config::load(root).and_then(|file| RuntimeConfig::build(&file)) else {
        return Vec::new();
    };
    if !config.watch_sessions {
        return Vec::new();
    }

    let sessions = pushos_macos::MacSessions::new(Arc::new(SystemProcessRunner::new()));
    sessions
        .report()
        .await
        .into_iter()
        .map(|report| {
            let verdict = match (report.usable, report.source) {
                (true, _) => Verdict::Good,
                (false, "Terminal") => Verdict::Blocking,
                (false, _) => Verdict::Optional,
            };
            Finding::new(
                verdict,
                "sessions",
                format!("{}: {}", report.source, report.detail),
            )
        })
        .collect()
}

/// Reports where notes are kept, when the operator has configured any.
///
/// Nothing at all when memory is not configured. A source that does not exist
/// yet is worth saying and is not a fault: PushOS makes it when the first note
/// is written.
fn check_memory(root: &Path) -> Vec<Finding> {
    let Ok(config) = pushos_config::load(root).and_then(|file| RuntimeConfig::build(&file)) else {
        return Vec::new();
    };
    let Some(settings) = &config.memory else {
        return Vec::new();
    };

    let mut findings = vec![Finding::new(
        Verdict::Good,
        "notes",
        format!(
            "{} source(s); {}",
            settings.sources.len(),
            if settings.can_capture() {
                "new notes can be written"
            } else {
                "every source is read only, so nothing new can be written"
            }
        ),
    )];

    for source in &settings.sources {
        let (verdict, detail) = describe_source(source);
        findings.push(Finding::new(verdict, "note source", detail));
    }

    findings
}

/// What one note source is, and whether it can be used.
fn describe_source(source: &pushos_domain::ports::Source) -> (Verdict, String) {
    let where_it_is = format!("{}: {}", source.id, source.root.display());

    if !source.root.exists() {
        return (
            Verdict::Optional,
            format!("{where_it_is} (not there yet; made when the first note is written)"),
        );
    }
    if !source.root.is_dir() {
        return (
            Verdict::Blocking,
            format!("{where_it_is} (not a directory)"),
        );
    }
    if source.writable
        && std::fs::metadata(&source.root).is_ok_and(|it| it.permissions().readonly())
    {
        return (
            Verdict::Blocking,
            format!("{where_it_is} (marked writable, but the directory is not)"),
        );
    }

    (
        Verdict::Good,
        format!(
            "{where_it_is} ({})",
            if source.writable {
                "read and write"
            } else {
                "read only"
            }
        ),
    )
}

/// Reports whether PushOS could listen, when the operator has asked it to.
///
/// Nothing at all when voice is not configured: a report full of lines about
/// something the operator did not ask for is a report nobody reads.
fn check_voice(root: &Path) -> Vec<Finding> {
    let Ok(config) = pushos_config::load(root).and_then(|file| RuntimeConfig::build(&file)) else {
        // Already reported as blocking by the configuration check; saying it
        // twice adds nothing.
        return Vec::new();
    };
    let Some(settings) = &config.voice else {
        return Vec::new();
    };

    let mut findings = vec![Finding::new(
        if settings.is_useful() {
            Verdict::Good
        } else {
            Verdict::Optional
        },
        "voice",
        format!(
            "{} engine, {} phrase(s){}",
            settings.engine,
            settings.commands.len(),
            if settings.is_useful() {
                String::new()
            } else {
                "; nothing is configured for it to do".to_owned()
            }
        ),
    )];

    findings.push(check_microphone());
    findings.extend(check_speech(settings));
    findings
}

/// Whether macOS will let PushOS hear anything.
///
/// Two questions, not one. Whether there is a microphone to open, and whether
/// this process is allowed to open it. Reporting only the first says everything
/// is fine right up until a control is held and nothing happens.
fn check_microphone() -> Finding {
    if let Err(error) = pushos_voice::microphone() {
        return Finding::new(Verdict::Blocking, "microphone", error.to_string());
    }

    // Opened, not merely listed. macOS offers a default input device on a Mac
    // that has no microphone in it, and only opening it says so.
    match pushos_voice::input_device_works() {
        Ok(named) => {
            if matches!(
                pushos_voice::RecordPermission::current(),
                pushos_voice::RecordPermission::Granted
            ) {
                return Finding::new(
                    Verdict::Good,
                    "microphone",
                    format!("{named}, and it opens"),
                );
            }
        }
        Err(error) => {
            return Finding::new(Verdict::Blocking, "microphone", error.to_string());
        }
    }

    let permission = pushos_voice::RecordPermission::current();
    let verdict = if permission.is_usable() {
        Verdict::Good
    } else {
        Verdict::Blocking
    };
    Finding::new(verdict, "microphone", permission.describe())
}

/// Whether the chosen engine can actually work out what was said.
fn check_speech(settings: &pushos_config::VoiceSettings) -> Vec<Finding> {
    #[cfg(target_os = "macos")]
    if settings.engine == pushos_domain::voice::VoiceEngine::Apple {
        let readiness = pushos_voice::AppleSpeech::readiness();
        return vec![Finding::new(
            if readiness.is_ready() {
                Verdict::Good
            } else if readiness.asked && !readiness.permitted {
                Verdict::Blocking
            } else {
                Verdict::Optional
            },
            "speech",
            readiness.explain(),
        )];
    }

    match pushos_voice::transcriber(settings.engine, settings.model.as_deref()) {
        Ok(engine) => vec![Finding::new(
            Verdict::Good,
            "speech",
            format!("{} is loaded and on this machine", engine.name()),
        )],
        Err(error) => vec![Finding::new(Verdict::Blocking, "speech", error.to_string())],
    }
}

/// Asks macOS whether this process may press keys in other applications.
///
/// A separate permission from the one that lets PushOS read a window, granted
/// in a separate pane, and an operator who has granted Automation has almost
/// certainly not granted this. Without it every key a pad presses fails, which
/// an operator would otherwise find out one pad at a time.
///
/// It answers for whatever is asking, and the grant is per application, so a
/// `doctor` run from a terminal answers for that terminal. The two are only
/// ever granted together in practice, and what it is really reporting is
/// whether this Mac has given anything here permission to type.
///
/// The question is asked rather than answered by trying: pressing a key to
/// find out whether a key can be pressed would type into whatever is in front.
async fn check_keystrokes(processes: &SystemProcessRunner) -> Finding {
    const ASK: &str = "tell application \"System Events\" to return UI elements enabled";
    let answer = ScriptRunner::new(Arc::new(*processes)).run(ASK, &[]).await;

    match answer.as_deref().map(str::trim) {
        Ok("true") => Finding::new(
            Verdict::Good,
            "keystrokes",
            "keys can be pressed in other applications",
        ),
        Ok(_) => Finding::new(
            // A note rather than blocking: everything else on the surface
            // works without it, and a pad that reads and focuses is still most
            // of a pad.
            Verdict::Optional,
            "keystrokes",
            "keys cannot be pressed in other applications, so a binding that \
             presses one will fail. Add PushOS in System Settings under \
             Privacy and Security, Accessibility",
        ),
        // macOS would not say. Reporting that as a refusal would send an
        // operator to fix something that may not be broken.
        Err(_) => Finding::new(
            Verdict::Optional,
            "keystrokes",
            "macOS would not say whether keys can be pressed in other \
             applications; if a pad that presses one fails, add PushOS in \
             System Settings under Privacy and Security, Accessibility",
        ),
    }
}

async fn check_host_tools() -> Vec<Finding> {
    let processes = SystemProcessRunner::new();
    let mut findings = Vec::new();

    for (subject, program, note) in [
        (
            "shortcuts",
            "/usr/bin/shortcuts",
            "Shortcut actions will not run",
        ),
        (
            "applescript",
            "/usr/bin/osascript",
            "media actions will not run",
        ),
        ("open", "/usr/bin/open", "application actions will not run"),
    ] {
        let present = tokio::fs::metadata(program).await.is_ok();
        findings.push(if present {
            Finding::new(Verdict::Good, subject, program)
        } else {
            Finding::new(
                Verdict::Optional,
                subject,
                format!("{program} is missing; {note}"),
            )
        });
    }

    findings.push(check_keystrokes(&processes).await);

    // Running something harmless proves the runner works, not merely that the
    // binary exists.
    let outcome = processes
        .run(&pushos_domain::ports::ProcessSpec::new(
            "/bin/echo",
            ["pushos".to_owned()],
        ))
        .await;
    findings.push(match outcome {
        Ok(outcome) if outcome.succeeded() => Finding::new(
            Verdict::Good,
            "subprocesses",
            "can start and reap processes",
        ),
        Ok(outcome) => Finding::new(
            Verdict::Blocking,
            "subprocesses",
            format!("a trivial process exited {:?}", outcome.exit_code),
        ),
        Err(error) => Finding::new(Verdict::Blocking, "subprocesses", error.to_string()),
    });

    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_verdict_has_a_distinct_fixed_width_mark() {
        let marks = [
            Verdict::Good.mark(),
            Verdict::Optional.mark(),
            Verdict::Blocking.mark(),
        ];
        assert!(marks.iter().all(|mark| mark.len() == marks[0].len()));

        let mut sorted = marks;
        sorted.sort_unstable();
        sorted.iter().reduce(|a, b| {
            assert_ne!(a, b, "verdict marks must be distinguishable");
            b
        });
    }

    #[tokio::test]
    async fn the_host_checks_prove_the_runner_works_rather_than_only_that_files_exist() {
        let findings = check_host_tools().await;
        let subprocesses = findings
            .iter()
            .find(|f| f.subject == "subprocesses")
            .expect("the check runs");
        assert_eq!(subprocesses.verdict, Verdict::Good);
    }

    #[test]
    fn a_missing_configuration_is_reported_as_blocking() {
        // It loads as empty and is therefore valid, which is true and useless.
        // A mistyped `--config` or a directory that has been moved gives a
        // surface that does nothing, and a diagnostic that called that healthy
        // would leave nowhere to look. Starting is unaffected: `run` and
        // `app install` read the configuration themselves, and a machine with
        // none still starts with nothing bound.
        let finding = check_configuration(Path::new("/nowhere/at/all"));
        assert_eq!(finding.verdict, Verdict::Blocking);
        assert!(
            finding.detail.contains("pushos init"),
            "and says what to do about it: {}",
            finding.detail
        );
    }

    #[test]
    fn an_invalid_configuration_is_reported_as_blocking() {
        let directory = std::env::temp_dir().join(format!(
            "pushos-doctor-{}",
            pushos_domain::ids::ExecutionId::generate()
        ));
        std::fs::create_dir_all(&directory).expect("writable");
        std::fs::write(
            directory.join("pushos.toml"),
            "[[bindings]]\ncontrol = \"pad.400\"\ngesture = \"tap\"\naction = \"page.next\"\n",
        )
        .expect("writable");

        assert_eq!(check_configuration(&directory).verdict, Verdict::Blocking);
        std::fs::remove_dir_all(&directory).ok();
    }
}
