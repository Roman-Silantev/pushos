//! Putting the microphone and speech explanations inside the binary.
//!
//! See `macos/README.md` at the root of the repository for why this is done
//! with the linker rather than with a bundle.

fn main() {
    pushos_plist::link("bins");
}

#[path = "../../macos/plist.rs"]
mod pushos_plist;
