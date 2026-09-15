//! The individual commands.

#[cfg(target_os = "macos")]
pub(crate) mod app;
pub(crate) mod check;
pub(crate) mod control;
pub(crate) mod doctor;
pub(crate) mod hook;
pub(crate) mod init;
pub(crate) mod pack;
pub(crate) mod paths;
pub(crate) mod run;
