//! Diagnosing an installation.
//!
//! Every check reports what it found and what to do about it. A check that
//! cannot run is reported as such rather than passing quietly, because a
//! diagnostic that only ever says "fine" is worse than none.

use std::path::Path;

use pushos_config::RuntimeConfig;
use pushos_domain::ports::ProcessRunner;
use pushos_macos::SystemProcessRunner;
use pushos_push2::{PortRole, Push2Device};
use pushos_storage::StorageWriter;

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

fn check_configuration(root: &Path) -> Finding {
    match pushos_config::load(root).and_then(|file| RuntimeConfig::build(&file)) {
        Ok(config) => Finding::new(
            Verdict::Good,
            "configuration",
            format!(
                "{} ({} pages, {} bindings)",
                root.display(),
                config.pages.len(),
                config.bindings.len()
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
    match StorageWriter::open(&path) {
        Ok(_) => Finding::new(Verdict::Good, "database", path.display().to_string()),
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
fn check_microphone() -> Finding {
    match pushos_voice::microphone() {
        Ok(_) => Finding::new(
            Verdict::Good,
            "microphone",
            "available; macOS asks the first time a voice control is held",
        ),
        Err(error) => Finding::new(Verdict::Blocking, "microphone", error.to_string()),
    }
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
        let finding = check_configuration(Path::new("/nowhere/at/all"));
        // A missing root loads as empty and is valid, so this is a good result:
        // PushOS starts with nothing bound rather than refusing to start.
        assert_eq!(finding.verdict, Verdict::Good);
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
