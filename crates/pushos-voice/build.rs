//! Putting the microphone and speech explanations inside the examples.
//!
//! The `transcribe` example asks macOS to work out what was said, and macOS
//! will not answer a binary that carries no reason to show the operator. See
//! `macos/README.md` at the root of the repository.

fn main() {
    pushos_plist::link("examples");
}

#[path = "../../macos/plist.rs"]
mod pushos_plist;
