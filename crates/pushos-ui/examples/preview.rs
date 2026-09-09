//! Renders a display frame to a file, so the layout can be looked at without
//! a Push 2 attached.
//!
//! ```text
//! cargo run -p pushos-ui --example preview -- /tmp/pushos.ppm [page|sessions|focus|overlay|notes|splash|offline]
//! ```

use std::io::Write as _;

use pushos_domain::ports::{DISPLAY_HEIGHT, DISPLAY_WIDTH, DisplayFrame};
use pushos_domain::voice::Listening;
use pushos_ui::{
    Focus, Notice, Overlay, PageView, PushRenderer, SessionLine, Slot, Splash, SurfacePresence,
    Tone, UiSnapshot,
};

fn main() -> std::io::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "pushos-preview.ppm".to_owned());
    let mode = std::env::args().nth(2).unwrap_or_else(|| "page".to_owned());

    let snapshot = match mode.as_str() {
        "sessions" => sessions(),
        "overlay" => overlay(),
        "notes" => notes(),
        "focus" => focus(),
        "focus-mid" => UiSnapshot {
            frame: 4,
            ..focus()
        },
        "focus-rest" => UiSnapshot {
            frame: 30,
            ..focus()
        },
        "splash" => splash(),
        "offline" => UiSnapshot::disconnected(),
        _ => page(),
    };

    let mut renderer = PushRenderer::new().map_err(std::io::Error::other)?;
    let mut frame = DisplayFrame::blank();
    renderer.render(&snapshot, &mut frame);

    let mut file = std::fs::File::create(&path)?;
    write!(file, "P6\n{DISPLAY_WIDTH} {DISPLAY_HEIGHT}\n255\n")?;
    for pixel in frame.pixels() {
        // Undo the display's packing so the file is ordinary eight-bit colour.
        let blue = ((pixel >> 11) & 0x1F) as u8;
        let green = ((pixel >> 5) & 0x3F) as u8;
        let red = (pixel & 0x1F) as u8;
        file.write_all(&[
            (red << 3) | (red >> 2),
            (green << 2) | (green >> 4),
            (blue << 3) | (blue >> 2),
        ])?;
    }

    println!("wrote {path}");
    Ok(())
}

/// A page with its eight columns in use.
fn page() -> UiSnapshot {
    UiSnapshot {
        page: PageView::new("development", "Development").at(2, 3),
        workspace: Some("sydclaw".to_owned()),
        surface: SurfacePresence::Hardware,
        slots: [
            Some(Slot::new("Home").with_value("Play/Pause")),
            Some(Slot::new("Develop").with_value("Terminal")),
            Some(Slot::new("Music").with_value("Now playing")),
            Some(
                Slot::new("Builder")
                    .with_value("working")
                    .with_tone(Tone::Active),
            ),
            Some(
                Slot::new("Reviewer")
                    .with_value("waiting")
                    .with_tone(Tone::Attention)
                    .selected(),
            ),
            Some(Slot::new("Tests").with_value("passing")),
            None,
            Some(
                Slot::new("Ship")
                    .with_value("held")
                    .with_tone(Tone::Failure),
            ),
        ],
        footer: Some("Projects, editors and test runs".to_owned()),
        notice: Some(Notice::new(
            "cargo test finished: 464 passing",
            Tone::Normal,
        )),
        overlay: None,
        focus: None,
        splash: None,
        sessions: Vec::new(),
        listening: Listening::Recording,
        frame: 12,
    }
}

/// Agents and terminals working at the same time.
///
/// The columns belong to whatever is running, whichever kind of work it is,
/// with the ones wanting attention first.
fn sessions() -> UiSnapshot {
    UiSnapshot {
        notice: None,
        sessions: vec![
            SessionLine::new("reviewer", "waiting", Tone::Attention)
                .with_detail("May I push to origin/main?")
                .selected(),
            SessionLine::new("tests", "failed", Tone::Failure).with_detail("3 failed, 461 passed"),
            SessionLine::new("builder", "working", Tone::Active)
                .with_detail("editing crates/pushos-terminal/src/pty.rs"),
            SessionLine::new("server", "running", Tone::Active)
                .with_detail("listening on 127.0.0.1:4000"),
            SessionLine::new("architect", "sleeping", Tone::Muted),
        ],
        ..page()
    }
}

/// The waiting screen, part way through its animation.
fn splash() -> UiSnapshot {
    UiSnapshot {
        splash: Some(Splash::new("PushOS", 34).with_detail("waiting for Push 2")),
        ..page()
    }
}

/// One session, looked at closely.
fn focus() -> UiSnapshot {
    UiSnapshot {
        focus: Some(
            Focus::new("ttys005", "Sprint 2 setup")
                .doing("working", Tone::Active)
                .saying([
                    "Read the migration and the two callers it has.".to_owned(),
                    "The second one passes the old column name, so it would fail at run time."
                        .to_owned(),
                    "Renaming it now and running the tests.".to_owned(),
                    "cargo test --workspace".to_owned(),
                ]),
        ),
        ..page()
    }
}

/// Notes found, which is a list to read rather than a question to answer.
fn notes() -> UiSnapshot {
    UiSnapshot {
        overlay: Some(Overlay::new("notes \u{2014} deploy", "3 found").listing([
            "Use one write owner \u{2014} the storage task owns the connection".to_owned(),
            "Deploy needs the new token \u{2014} it is in the vault".to_owned(),
            "Ship on Thursday \u{2014} after the review".to_owned(),
        ])),
        ..page()
    }
}

/// An overlay asking the operator something.
fn overlay() -> UiSnapshot {
    UiSnapshot {
        overlay: Some(
            Overlay::new("Deployment", "Preview build failed")
                .with_detail("3 of 4 checks passed \u{2014} tests timed out after 12m")
                .with_progress(0.75)
                .asking([
                    "OPEN INCIDENT".to_owned(),
                    "RETRY".to_owned(),
                    "IGNORE".to_owned(),
                ]),
        ),
        ..page()
    }
}
