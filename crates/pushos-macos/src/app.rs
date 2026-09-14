//! What PushOS looks like as a macOS app that starts at login.
//!
//! A command line tool borrows its permissions from whatever started it, so a
//! PushOS run from Terminal asks for the microphone as Terminal, and one closed
//! with its window is gone. As an app it has permissions of its own, keeps
//! running when no window is open, and starts when you log in.
//!
//! The app is the same executable in a bundle: no window, no Dock icon, and no
//! code that exists only for it. Everything here describes files; nothing here
//! writes them, starts anything or signs anything, so all of it can be checked
//! without touching the machine.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The app's identity to macOS: its permissions, its login item, its logs.
pub const BUNDLE_IDENTIFIER: &str = "dev.pushos.app";

/// The login agent's name to `launchd`.
pub const AGENT_LABEL: &str = "dev.pushos.agent";

/// What to call a certificate made on this Mac for signing PushOS.
///
/// Signing with the same certificate every time is what lets macOS recognise a
/// rebuilt PushOS as the same app and keep the permissions it was given.
/// Without one each build is signed afresh, and to macOS a new app.
pub const CERTIFICATE_NAME: &str = "PushOS Local";

/// The oldest macOS the app runs on.
///
/// Fourteen, because asking macOS whether the microphone may be used is only
/// possible from there.
const MINIMUM_MACOS: &str = "14.0";

/// The explanations macOS shows when PushOS asks for something, shared with the
/// command line tool so the two never say different things.
const SHARED_PLIST: &str = include_str!("../../../macos/Info.plist");

/// The identifier the shared property list carries for the command line tool.
const TOOL_IDENTIFIER: &str = "dev.pushos.cli";

/// What the app declares it needs under the hardened runtime.
const ENTITLEMENTS: &str = include_str!("../../../macos/PushOS.entitlements");

/// Where the parts of the app go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppLayout {
    root: PathBuf,
}

impl AppLayout {
    /// The app in the user's own Applications folder.
    ///
    /// Not `/Applications`, which needs an administrator to write to and is
    /// shared by everyone who uses the Mac.
    pub fn for_user(home: &Path) -> Self {
        Self {
            root: home.join("Applications").join("PushOS.app"),
        }
    }

    /// The app itself.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The executable inside it.
    pub fn executable(&self) -> PathBuf {
        self.root.join("Contents").join("MacOS").join("pushos")
    }

    /// Its property list.
    pub fn info_plist(&self) -> PathBuf {
        self.root.join("Contents").join("Info.plist")
    }
}

/// Where the login agent's definition goes.
pub fn agent_plist_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{AGENT_LABEL}.plist"))
}

/// Where the running app writes its log.
pub fn log_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Logs")
        .join("PushOS")
        .join("pushos.log")
}

/// The entitlements to sign the app with.
pub const fn entitlements() -> &'static str {
    ENTITLEMENTS
}

/// The app's property list.
///
/// The shared one, with the identifier changed to the app's and the keys an
/// app needs added: which file to run, that it is an app, its version, and
/// that it has no Dock icon or menu bar, because it has no window.
pub fn info_plist(version: &str) -> String {
    let identified = SHARED_PLIST.replace(
        &format!("<string>{TOOL_IDENTIFIER}</string>"),
        &format!("<string>{BUNDLE_IDENTIFIER}</string>"),
    );
    let version = escape(version);
    let added = format!(
        "\t<key>CFBundleExecutable</key>\n\t<string>pushos</string>\n\
         \t<key>CFBundlePackageType</key>\n\t<string>APPL</string>\n\
         \t<key>CFBundleInfoDictionaryVersion</key>\n\t<string>6.0</string>\n\
         \t<key>CFBundleShortVersionString</key>\n\t<string>{version}</string>\n\
         \t<key>CFBundleVersion</key>\n\t<string>{version}</string>\n\
         \t<key>LSMinimumSystemVersion</key>\n\t<string>{MINIMUM_MACOS}</string>\n\
         \t<key>LSUIElement</key>\n\t<true/>\n"
    );
    identified.replacen("</dict>", &format!("{added}</dict>"), 1)
}

/// How `launchd` should start PushOS at login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginAgent {
    /// The executable inside the app.
    pub executable: PathBuf,
    /// The configuration to run, resolved when it was installed.
    ///
    /// Written out rather than left to be worked out at login, because a login
    /// agent does not see the environment a shell sets up, and would otherwise
    /// look somewhere different from where `pushos` looked when it was asked.
    pub config: PathBuf,
    /// Where to write what it logs.
    pub log: PathBuf,
    /// Environment carried over from the shell it was installed from.
    ///
    /// A login agent starts with almost none. Without `PATH` it could not find
    /// the agent programs a role runs, which live wherever they were installed.
    pub environment: Vec<(String, String)>,
}

impl LoginAgent {
    /// The variables worth carrying over, of the ones PushOS or the programs it
    /// starts actually read.
    pub const CARRIED: [&'static str; 4] = ["PATH", "SHELL", "LANG", "XDG_DATA_HOME"];

    /// The agent's property list.
    pub fn plist(&self) -> String {
        let mut environment = String::new();
        for (name, value) in &self.environment {
            let _ = write!(
                environment,
                "\t\t<key>{}</key>\n\t\t<string>{}</string>\n",
                escape(name),
                escape(value)
            );
        }

        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>AssociatedBundleIdentifiers</key>
	<array>
		<string>{bundle}</string>
	</array>
	<key>ProgramArguments</key>
	<array>
		<string>{executable}</string>
		<string>--config</string>
		<string>{config}</string>
		<string>run</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key>
		<false/>
	</dict>
	<key>ThrottleInterval</key>
	<integer>10</integer>
	<key>ProcessType</key>
	<string>Interactive</string>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
	<key>EnvironmentVariables</key>
	<dict>
{environment}	</dict>
</dict>
</plist>
"#,
            label = AGENT_LABEL,
            bundle = BUNDLE_IDENTIFIER,
            executable = escape(&self.executable.to_string_lossy()),
            config = escape(&self.config.to_string_lossy()),
            log = escape(&self.log.to_string_lossy()),
        )
    }
}

/// Escapes text for a property list's character data.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asks macOS's own tool whether a property list is well formed.
    fn lints(text: &str) -> bool {
        // A file of its own for every check: the tests run at once, and two
        // sharing a name would each lint whatever the other last wrote.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "pushos-plist-{}-{}.plist",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&path, text).expect("writable");
        let ok = std::process::Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&path)
            .output()
            .is_ok_and(|output| output.status.success());
        std::fs::remove_file(&path).ok();
        ok
    }

    fn agent() -> LoginAgent {
        LoginAgent {
            executable: PathBuf::from(
                "/Users/someone/Applications/PushOS.app/Contents/MacOS/pushos",
            ),
            config: PathBuf::from("/Users/someone/.config/pushos"),
            log: PathBuf::from("/Users/someone/Library/Logs/PushOS/pushos.log"),
            environment: vec![("PATH".to_owned(), "/opt/homebrew/bin:/usr/bin".to_owned())],
        }
    }

    #[test]
    fn the_app_lives_in_the_users_own_applications_folder() {
        let layout = AppLayout::for_user(Path::new("/Users/someone"));
        assert_eq!(
            layout.root(),
            Path::new("/Users/someone/Applications/PushOS.app")
        );
        assert!(layout.executable().ends_with("Contents/MacOS/pushos"));
        assert!(layout.info_plist().ends_with("Contents/Info.plist"));
    }

    #[test]
    fn the_apps_property_list_is_well_formed_and_says_what_an_app_must() {
        let plist = info_plist("0.1.0");
        assert!(lints(&plist), "plutil rejects it:\n{plist}");
        assert!(plist.contains(BUNDLE_IDENTIFIER));
        assert!(
            !plist.contains(TOOL_IDENTIFIER),
            "the app has its own identity"
        );
        for key in [
            "CFBundleExecutable",
            "CFBundlePackageType",
            "LSUIElement",
            "LSMinimumSystemVersion",
        ] {
            assert!(plist.contains(key), "missing {key}");
        }
    }

    #[test]
    fn the_app_explains_everything_it_will_ask_for() {
        // macOS refuses a request that comes without its explanation, and the
        // app asks for all three.
        let plist = info_plist("0.1.0");
        for key in [
            "NSMicrophoneUsageDescription",
            "NSSpeechRecognitionUsageDescription",
            "NSAppleEventsUsageDescription",
        ] {
            assert!(plist.contains(key), "missing {key}");
        }
    }

    #[test]
    fn the_entitlements_cover_the_microphone_and_apple_events() {
        // Under the hardened runtime, missing either one is a silent refusal
        // with no prompt at all.
        assert!(lints(entitlements()));
        assert!(entitlements().contains("com.apple.security.device.audio-input"));
        assert!(entitlements().contains("com.apple.security.automation.apple-events"));
    }

    #[test]
    fn the_login_agent_is_well_formed_and_runs_the_configuration_it_was_given() {
        let plist = agent().plist();
        assert!(lints(&plist), "plutil rejects it:\n{plist}");
        assert!(plist.contains("<string>--config</string>"));
        assert!(plist.contains("<string>/Users/someone/.config/pushos</string>"));
        assert!(plist.contains("<string>run</string>"));
        assert!(plist.contains("<key>PATH</key>"));
    }

    #[test]
    fn the_login_agent_restarts_after_a_crash_but_not_after_being_stopped() {
        let plist = agent().plist();
        assert!(plist.contains("<key>SuccessfulExit</key>\n\t\t<false/>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n\t<true/>"));
    }

    #[test]
    fn text_that_would_break_the_property_list_is_escaped() {
        let mut odd = agent();
        odd.config = PathBuf::from("/Users/a&b/<config>");
        let plist = odd.plist();
        assert!(lints(&plist), "plutil rejects it:\n{plist}");
        assert!(plist.contains("/Users/a&amp;b/&lt;config&gt;"));
    }
}
