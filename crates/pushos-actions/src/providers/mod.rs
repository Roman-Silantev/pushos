//! The built-in action providers.
//!
//! Each provider claims one namespace and depends only on domain ports, never
//! on a concrete backend. That is what lets every one of them be tested with
//! fakes and swapped for a different implementation.

pub mod agent;
pub mod application;
pub mod media;
pub mod page;
pub mod shell;
pub mod shortcut;
pub mod terminal;
pub mod voice;
pub mod workflow;
pub mod workspace;
