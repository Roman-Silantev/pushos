//! Installing PushOS as a macOS app that starts at login.
//!
//! Three steps, each done with the tool macOS provides for it: the app is
//! assembled in `~/Applications`, signed with `codesign`, and registered with
//! `launchd` as a login agent. Nothing here is PushOS-specific cleverness; it is
//! the ordinary way a background app is installed, written down once so nobody
//! has to remember it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use pushos_config::RuntimeConfig;
use pushos_macos::app::{
    self, AGENT_LABEL, AppLayout, BUNDLE_IDENTIFIER, CERTIFICATE_NAME, LoginAgent,
};

use super::{check, paths};

/// How long to wait for the installed app to start answering.
const STARTUP: Duration = Duration::from_secs(15);

/// Builds, signs and starts the app, replacing an installed one.
pub(crate) async fn install(
    requested: Option<&Path>,
    identity: Option<&str>,
) -> Result<(), String> {
    let home = home()?;
    let layout = AppLayout::for_user(&home);

    // Checked before anything is written. A login agent running a broken
    // configuration would fail, be restarted, and fail again every ten seconds.
    let config = std::path::absolute(paths::config_root(requested)?)
        .map_err(|error| format!("could not resolve the configuration directory: {error}"))?;
    if config.exists() {
        let file = pushos_config::load(&config).map_err(|error| check::describe(&error))?;
        RuntimeConfig::build(&file).map_err(|error| check::describe(&error))?;
    }

    // A PushOS started by hand holds the Push 2 and the control socket, and the
    // app would start, find it there, and exit, over and over.
    if !is_loaded()? && super::control::is_running().await {
        return Err(
            "PushOS is already running outside the app. Stop it first, then install again."
                .to_owned(),
        );
    }

    let identity = identity
        .map(ToOwned::to_owned)
        .or_else(|| find_certificate(CERTIFICATE_NAME));

    // Stopped before its executable is replaced, so nothing is running from a
    // file that is being written.
    stop()?;

    assemble(&layout)?;
    sign(&layout, identity.as_deref())?;

    let log = app::log_path(&home);
    if let Some(directory) = log.parent() {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    }

    let agent = LoginAgent {
        executable: layout.executable(),
        config: config.clone(),
        log: log.clone(),
        environment: LoginAgent::CARRIED
            .iter()
            .filter_map(|name| {
                std::env::var(name)
                    .ok()
                    .map(|value| ((*name).to_owned(), value))
            })
            .collect(),
    };
    let plist = app::agent_plist_path(&home);
    write_atomically(&plist, &agent.plist())?;
    launchctl(&["bootstrap", &domain()?, &plist.to_string_lossy()])?;

    let started = wait_until_answering().await;

    println!("installed {}", layout.root().display());
    println!(
        "  starts at login, and is running {}",
        if started {
            "now"
        } else {
            "(not answering yet)"
        }
    );
    println!("  configuration: {}", config.display());
    println!("  log:           {}", log.display());
    if let Some(name) = &identity {
        println!("  signed by:     {name}, so macOS keeps its permissions across rebuilds");
    } else {
        println!("  signed:        for this build only");
        println!();
        println!("macOS will forget the microphone and Terminal permissions each time");
        println!("PushOS is rebuilt and installed again. To stop that, create a signing");
        println!("certificate once, then run `pushos app install` again:");
        println!();
        println!("  1. Open Keychain Access.");
        println!("  2. Keychain Access menu > Certificate Assistant > Create a Certificate.");
        println!("  3. Name: {CERTIFICATE_NAME}");
        println!("     Identity Type: Self Signed Root");
        println!("     Certificate Type: Code Signing");
        println!("  4. Create, then Done.");
    }
    if !started {
        return Err(format!(
            "the app did not start answering within {} seconds; see {}",
            STARTUP.as_secs(),
            log.display()
        ));
    }
    Ok(())
}

/// Stops the app and removes it, leaving configuration, notes and logs alone.
pub(crate) fn uninstall() -> Result<(), String> {
    let home = home()?;
    let layout = AppLayout::for_user(&home);

    stop()?;
    let plist = app::agent_plist_path(&home);
    if plist.exists() {
        std::fs::remove_file(&plist)
            .map_err(|error| format!("could not remove {}: {error}", plist.display()))?;
    }

    // Only a directory that says it is PushOS is deleted. A path that happens to
    // have the same name is somebody else's.
    if layout.root().exists() {
        let ours = std::fs::read_to_string(layout.info_plist())
            .is_ok_and(|plist| plist.contains(BUNDLE_IDENTIFIER));
        if !ours {
            return Err(format!(
                "{} is not the PushOS app; leaving it alone",
                layout.root().display()
            ));
        }
        std::fs::remove_dir_all(layout.root())
            .map_err(|error| format!("could not remove {}: {error}", layout.root().display()))?;
    }

    println!("removed the PushOS app and its login item");
    println!("configuration, notes and logs are where they were");
    Ok(())
}

/// Says whether the app is installed, starting at login, signed and running.
pub(crate) async fn status() -> Result<(), String> {
    let home = home()?;
    let layout = AppLayout::for_user(&home);

    let installed = layout.executable().is_file();
    println!(
        "app:        {}",
        if installed {
            layout.root().display().to_string()
        } else {
            "not installed".to_owned()
        }
    );
    println!("at login:   {}", if is_loaded()? { "yes" } else { "no" });
    if installed {
        println!(
            "signed by:  {}",
            signer(&layout).unwrap_or_else(|| "unknown".to_owned())
        );
    }
    println!(
        "running:    {}",
        if super::control::is_running().await {
            "yes"
        } else {
            "no"
        }
    );
    Ok(())
}

/// Puts the executable and its property list in place.
fn assemble(layout: &AppLayout) -> Result<(), String> {
    let executable = layout.executable();
    let directory = executable
        .parent()
        .ok_or_else(|| "the app has nowhere to put its executable".to_owned())?;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()))?;

    let current = std::env::current_exe()
        .map_err(|error| format!("could not find the running pushos: {error}"))?;
    // Installing from the installed app itself is a reinstall, and copying a
    // file onto itself would empty it.
    if !same_file(&current, &executable) {
        let staging = executable.with_extension("installing");
        std::fs::copy(&current, &staging)
            .map_err(|error| format!("could not copy pushos into the app: {error}"))?;
        std::fs::rename(&staging, &executable)
            .map_err(|error| format!("could not put pushos in place: {error}"))?;
    }

    write_atomically(
        &layout.info_plist(),
        &app::info_plist(env!("CARGO_PKG_VERSION")),
    )
}

/// Signs the app with the hardened runtime and its entitlements.
///
/// With a named identity when there is one, so the signature is the same from
/// one build to the next. Without one, signed for this build only: macOS still
/// runs it, and asks for its permissions again after the next install.
fn sign(layout: &AppLayout, identity: Option<&str>) -> Result<(), String> {
    let entitlements =
        std::env::temp_dir().join(format!("pushos-{}.entitlements", std::process::id()));
    std::fs::write(&entitlements, app::entitlements())
        .map_err(|error| format!("could not write the entitlements: {error}"))?;

    let signed = run(
        "/usr/bin/codesign",
        &[
            "--force",
            "--options",
            "runtime",
            "--timestamp=none",
            "--entitlements",
            &entitlements.to_string_lossy(),
            "--sign",
            identity.unwrap_or("-"),
            &layout.root().to_string_lossy(),
        ],
    );
    std::fs::remove_file(&entitlements).ok();
    signed?;

    run(
        "/usr/bin/codesign",
        &["--verify", "--strict", &layout.root().to_string_lossy()],
    )
    .map(|_| ())
    .map_err(|error| format!("the app was signed but does not verify: {error}"))
}

/// The name of a code signing certificate on this Mac, if one has it.
///
/// Every certificate, not only trusted ones: one made in Keychain Access is not
/// trusted by anyone else, and does not need to be to keep permissions steady on
/// the Mac it was made on.
fn find_certificate(name: &str) -> Option<String> {
    let listed = run("/usr/bin/security", &["find-identity", "-p", "codesigning"]).ok()?;
    listed
        .contains(&format!("\"{name}\""))
        .then(|| name.to_owned())
}

/// Who signed the installed app, as `codesign` reports it.
fn signer(layout: &AppLayout) -> Option<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2"])
        .arg(layout.root())
        .output()
        .ok()?;
    // codesign describes a signature on standard error.
    let report = String::from_utf8_lossy(&output.stderr);
    if report.contains("Signature=adhoc") {
        return Some("this build only".to_owned());
    }
    report
        .lines()
        .find_map(|line| line.strip_prefix("Authority="))
        .map(ToOwned::to_owned)
}

/// Stops the login agent if it is running. Not being loaded is not a failure.
fn stop() -> Result<(), String> {
    if !is_loaded()? {
        return Ok(());
    }
    launchctl(&["bootout", &format!("{}/{AGENT_LABEL}", domain()?)])
}

/// Whether the login agent is registered with `launchd`.
fn is_loaded() -> Result<bool, String> {
    let status = Command::new("/bin/launchctl")
        .args(["print", &format!("{}/{AGENT_LABEL}", domain()?)])
        .output()
        .map_err(|error| format!("could not run launchctl: {error}"))?;
    Ok(status.status.success())
}

/// The logged-in user's `launchd` domain.
fn domain() -> Result<String, String> {
    let id = run("/usr/bin/id", &["-u"])?;
    Ok(format!("gui/{}", id.trim()))
}

fn launchctl(arguments: &[&str]) -> Result<(), String> {
    run("/bin/launchctl", arguments).map(|_| ())
}

/// Waits for the app to answer on the control socket.
async fn wait_until_answering() -> bool {
    let deadline = Instant::now() + STARTUP;
    while Instant::now() < deadline {
        if super::control::is_running().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

/// Runs a system tool, returning what it printed or why it failed.
fn run(program: &str, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("could not run {program}: {error}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    Err(format!("{program} failed: {}", said.trim()))
}

/// Writes a file so a reader never sees half of it.
fn write_atomically(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    }
    let staging = path.with_extension("installing");
    std::fs::write(&staging, contents)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    std::fs::rename(&staging, path)
        .map_err(|error| format!("could not put {} in place: {error}", path.display()))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn home() -> Result<PathBuf, String> {
    pushos_config::paths::home_directory()
        .ok_or_else(|| "could not work out your home directory; set HOME".to_owned())
}
