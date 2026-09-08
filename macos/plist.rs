//! Linking the property list into a binary, shared by the crates that need it.
//!
//! Included with `#[path]` by each build script rather than made a crate of its
//! own: a build dependency exists to be built before everything else, and one
//! that consists of a single function is not worth that.

/// Tells the linker to put `macos/Info.plist` inside what it is building.
///
/// `kind` is the Cargo target class to apply it to: `bins` for the `pushos`
/// executable, `examples` for the development tools. Anything else is linked
/// separately and wants no part of it.
pub(crate) fn link(kind: &str) {
    println!("cargo:rerun-if-changed=../../macos/Info.plist");
    println!("cargo:rerun-if-changed=../../macos/plist.rs");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let Ok(crate_root) = std::env::var("CARGO_MANIFEST_DIR") else {
        // Building without the explanations beats not building: everything
        // except voice works exactly the same way.
        println!("cargo:warning=PushOS is being built without its microphone explanation");
        return;
    };

    let plist = std::path::Path::new(&crate_root).join("../../macos/Info.plist");
    if !plist.is_file() {
        println!(
            "cargo:warning=`{}` is missing; PushOS will not be able to ask for the microphone",
            plist.display()
        );
        return;
    }

    println!(
        "cargo:rustc-link-arg-{kind}=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
}
