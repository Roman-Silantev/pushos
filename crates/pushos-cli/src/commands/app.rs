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
    bootstrap(&plist)?;

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

    // Before the binary goes. The hook in each agent's settings runs this very
    // executable on every permission question they ask; left behind, it points
    // at a path that no longer exists, fires on every prompt, and the command
    // that removes it is the one just deleted. A failure here is reported and
    // not fatal: the app should still come off.
    if let Err(error) = super::hook::uninstall() {
        eprintln!("could not take PushOS out of the agents' hooks: {error}");
        eprintln!("run `pushos hook uninstall` before removing the app, or edit them by hand");
    }

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

    println!("removed the PushOS app, its login item and its hooks in the agents");
    // Named rather than merely said to be kept, because an operator removing
    // PushOS for good has no way to guess where any of it lives.
    println!("what is left, should you want it:");
    if let Ok(config) = paths::config_root(None) {
        println!("  configuration  {}", config.display());
    }
    if let Ok(state) = paths::state_file()
        && let Some(directory) = state.parent()
    {
        println!("  notes and state {}", directory.display());
    }
    println!("  logs           {}/Library/Logs/PushOS", home.display());
    println!("working trees PushOS made in your projects are still registered;");
    println!("`git worktree prune` in a project clears any it left behind");
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

/// Signs the app with its entitlements, and the hardened runtime when it can.
///
/// With a named identity when there is one, so the signature is the same from
/// one build to the next. Without one, signed for this build only: macOS still
/// runs it, and asks for its permissions again after the next install.
///
/// The hardened runtime goes on only with a real identity. Asking for it on an
/// ad-hoc signature is what macOS kills the process for: the runtime promises
/// every executable page is covered by a signature it can keep checking, an
/// ad-hoc signature cannot make that promise, and the kernel finds out at the
/// moment a page of lazily-loaded code is first run. For PushOS that moment is
/// connecting to the Push 2 — so the app ran perfectly until the hardware was
/// plugged in, then died with `Code Signature Invalid` every ten seconds for
/// as long as launchd kept restarting it.
fn sign(layout: &AppLayout, identity: Option<&str>) -> Result<(), String> {
    let entitlements =
        std::env::temp_dir().join(format!("pushos-{}.entitlements", std::process::id()));
    std::fs::write(&entitlements, app::entitlements())
        .map_err(|error| format!("could not write the entitlements: {error}"))?;

    let arguments = signing_arguments(
        identity,
        &entitlements.to_string_lossy(),
        &layout.root().to_string_lossy(),
    );
    let borrowed: Vec<&str> = arguments.iter().map(String::as_str).collect();

    let signed = run("/usr/bin/codesign", &borrowed);
    std::fs::remove_file(&entitlements).ok();
    signed?;

    run(
        "/usr/bin/codesign",
        &["--verify", "--strict", &layout.root().to_string_lossy()],
    )
    .map(|_| ())
    .map_err(|error| format!("the app was signed but does not verify: {error}"))
}

/// What `codesign` is asked to do.
///
/// A function of its own so the one rule that matters can be tested without
/// signing anything: the hardened runtime goes on only with a real identity.
fn signing_arguments(identity: Option<&str>, entitlements: &str, app: &str) -> Vec<String> {
    let mut arguments = vec!["--force".to_owned()];
    if identity.is_some() {
        arguments.push("--options".to_owned());
        arguments.push("runtime".to_owned());
    }
    arguments.extend([
        "--timestamp=none".to_owned(),
        "--entitlements".to_owned(),
        entitlements.to_owned(),
        "--sign".to_owned(),
        identity.unwrap_or("-").to_owned(),
        app.to_owned(),
    ]);
    arguments
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
    launchctl(&["bootout", &format!("{}/{AGENT_LABEL}", domain()?)])?;

    // `bootout` returns before launchd has finished taking the service down,
    // and registering one that is still going fails saying only "Input/output
    // error". So this waits for it to be gone rather than leaving the next
    // step to fail for a reason nobody could act on.
    for _ in 0..GOING_LOOKS {
        if !is_loaded()? {
            return Ok(());
        }
        std::thread::sleep(GOING_PAUSE);
    }
    Ok(())
}

/// How many times to look for the old service to have gone.
const GOING_LOOKS: usize = 20;

/// How long between those looks: a second in all, where it normally goes at
/// once.
const GOING_PAUSE: Duration = Duration::from_millis(50);

/// Registers the login agent, waiting out a `launchd` that is still busy.
///
/// Taking the old one down and putting the new one up are not as separate as
/// they look: for a moment `launchd` knows about both, and says so with an
/// error that names no cause.
fn bootstrap(plist: &Path) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 0..BOOTSTRAP_TRIES {
        match launchctl(&["bootstrap", &domain()?, &plist.to_string_lossy()]) {
            Ok(()) => return Ok(()),
            Err(why) => last = why,
        }
        if attempt + 1 < BOOTSTRAP_TRIES {
            std::thread::sleep(GOING_PAUSE * 4);
        }
    }
    Err(last)
}

/// How many times registering is attempted before the error is the answer.
const BOOTSTRAP_TRIES: usize = 4;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hardened_runtime_is_only_asked_for_with_a_real_identity() {
        // Asking for it on an ad-hoc signature is what macOS kills the process
        // for, and it only finds out when a page of lazily-loaded code first
        // runs. For PushOS that page is the Push 2 adapter, so the app worked
        // perfectly until the hardware was plugged in and then died with
        // `Code Signature Invalid` every ten seconds for as long as launchd
        // kept restarting it.
        let adhoc = signing_arguments(None, "/tmp/e.entitlements", "/tmp/PushOS.app");
        assert!(
            !adhoc.iter().any(|argument| argument == "runtime"),
            "an ad-hoc signature cannot carry the hardened runtime: {adhoc:?}"
        );
        assert!(adhoc.iter().any(|argument| argument == "-"), "{adhoc:?}");

        let named = signing_arguments(
            Some("PushOS Local"),
            "/tmp/e.entitlements",
            "/tmp/PushOS.app",
        );
        assert!(
            named.iter().any(|argument| argument == "runtime"),
            "a real identity can, and should: {named:?}"
        );
        assert!(
            named.iter().any(|argument| argument == "PushOS Local"),
            "{named:?}"
        );
    }

    #[test]
    fn the_entitlements_and_the_app_are_always_passed() {
        for identity in [None, Some("PushOS Local")] {
            let arguments = signing_arguments(identity, "/tmp/e.entitlements", "/tmp/PushOS.app");
            assert!(arguments.iter().any(|held| held == "/tmp/e.entitlements"));
            assert!(arguments.iter().any(|held| held == "/tmp/PushOS.app"));
            assert!(arguments.iter().any(|held| held == "--force"));
        }
    }
}
