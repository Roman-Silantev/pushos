//! Notes PushOS can capture and find.
//!
//! Deliberately small. PushOS is a control surface, not a knowledge base, and
//! this exists so a pad can capture a thought in one press and an agent can be
//! handed what it needs to know before it starts.
//!
//! Notes are Markdown files in directories the operator chose. That is the
//! whole storage design, and it is the point: their notes stay theirs, readable
//! and editable without PushOS, and by any other tool they already use. The
//! index is only what makes them findable and can be thrown away at any time.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used, clippy::panic))]

mod forgetful;
mod frontmatter;
mod library;
mod naming;

pub use forgetful::ForgetfulNotes;
pub use library::MarkdownLibrary;
