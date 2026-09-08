//! Media control through AppleScript.
//!
//! `MediaRemote`, the framework that reports what is playing system-wide, has
//! been gated behind a private entitlement since macOS 15.4 and returns nothing
//! to unentitled callers. AppleScript against a named player is the supported
//! mechanism, so that is what PushOS uses.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ports::{MediaController, MediaSnapshot, ProcessRunner};

use crate::script::{ApplicationName, ScriptRunner};

/// The player PushOS controls unless configured otherwise.
pub const DEFAULT_PLAYER: &str = "Music";

/// Separates the fields of the now-playing reply.
///
/// A tab cannot appear in a track title or artist name, so it is unambiguous.
const FIELD_SEPARATOR: char = '\t';

/// Controls a scriptable media player.
#[derive(Debug, Clone)]
pub struct AppleScriptMedia {
    scripts: ScriptRunner,
    player: ApplicationName,
}

impl AppleScriptMedia {
    /// Builds a controller for the default player.
    pub fn new(processes: Arc<dyn ProcessRunner>) -> Self {
        Self::for_player(processes, DEFAULT_PLAYER)
            .unwrap_or_else(|_| unreachable!("the default player name is a valid identifier"))
    }

    /// Builds a controller for a named player.
    pub fn for_player(
        processes: Arc<dyn ProcessRunner>,
        player: impl Into<String>,
    ) -> Result<Self, crate::script::UnsafeApplicationName> {
        Ok(Self {
            scripts: ScriptRunner::new(processes),
            player: ApplicationName::new(player)?,
        })
    }

    /// The player being controlled.
    pub fn player(&self) -> &str {
        self.player.as_str()
    }

    /// Runs a statement against the player, but only if it is already running.
    ///
    /// Guarding on `is running` matters: an unguarded `tell` launches the
    /// application, so a volume query would start a music player.
    async fn tell_if_running(&self, statement: &str) -> Result<String, ActionError> {
        let script = format!(
            "on run argv\n\
             \x20 if application \"{player}\" is running then\n\
             \x20   tell application \"{player}\"\n\
             \x20     {statement}\n\
             \x20   end tell\n\
             \x20 end if\n\
             end run",
            player = self.player.as_str(),
        );
        self.scripts.run(&script, &[]).await
    }

    /// Runs a statement, launching the player if it is not already running.
    async fn tell_launching(&self, statement: &str) -> Result<String, ActionError> {
        let script = format!(
            "on run argv\n\
             \x20 tell application \"{player}\"\n\
             \x20   {statement}\n\
             \x20 end tell\n\
             end run",
            player = self.player.as_str(),
        );
        self.scripts.run(&script, &[]).await
    }

    /// Parses the tab-separated now-playing reply.
    fn parse_now_playing(&self, reply: &str, playing: bool) -> Option<MediaSnapshot> {
        let mut fields = reply.split(FIELD_SEPARATOR);
        let title = fields.next().filter(|title| !title.is_empty())?.to_owned();
        let artist = fields
            .next()
            .filter(|artist| !artist.is_empty())
            .map(ToOwned::to_owned);
        let position = fields.next().and_then(parse_seconds);
        let duration = fields.next().and_then(parse_seconds);

        Some(MediaSnapshot {
            source: self.player.as_str().to_owned(),
            title,
            artist,
            position,
            duration,
            playing,
        })
    }
}

#[async_trait]
impl MediaController for AppleScriptMedia {
    async fn play_pause(&self) -> Result<(), ActionError> {
        // The one verb that may launch the player: pressing play with nothing
        // running is a request to start playing, not a query.
        self.tell_launching("playpause").await.map(|_| ())
    }

    async fn next_track(&self) -> Result<(), ActionError> {
        self.tell_if_running("next track").await.map(|_| ())
    }

    async fn previous_track(&self) -> Result<(), ActionError> {
        self.tell_if_running("previous track").await.map(|_| ())
    }

    async fn set_volume(&self, percent: u8) -> Result<(), ActionError> {
        let percent = percent.min(100);
        self.tell_if_running(&format!("set sound volume to {percent}"))
            .await
            .map(|_| ())
    }

    async fn volume(&self) -> Result<Option<u8>, ActionError> {
        let reply = self.tell_if_running("return sound volume as text").await?;
        if reply.is_empty() {
            return Ok(None);
        }
        reply
            .parse::<u16>()
            .map(|value| Some(u8::try_from(value.min(100)).unwrap_or(100)))
            .map_err(|error| {
                ActionError::backend(
                    format!("`{}` reported an unreadable volume", self.player.as_str()),
                    ErrorClass::ComponentFailure,
                    error,
                )
            })
    }

    async fn now_playing(&self) -> Result<Option<MediaSnapshot>, ActionError> {
        let reply = self
            .tell_if_running(
                "if player state is stopped then return \"\"\n\
                 \x20     set t to current track\n\
                 \x20     return (name of t) & tab & (artist of t) & tab & \
                 (player position as text) & tab & ((duration of t) as text)",
            )
            .await?;

        if reply.is_empty() {
            return Ok(None);
        }

        let playing = self.tell_if_running("return player state as text").await? == "playing";
        Ok(self.parse_now_playing(&reply, playing))
    }
}

/// Reads a fractional seconds value as a duration, ignoring anything unreadable.
fn parse_seconds(value: &str) -> Option<Duration> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|seconds| *seconds >= 0.0)
        .map(Duration::from_secs_f64)
}

#[cfg(test)]
mod tests {
    use pushos_testkit::FakeProcesses;

    use super::*;

    fn controller(processes: &FakeProcesses) -> AppleScriptMedia {
        AppleScriptMedia::new(Arc::new(processes.clone()))
    }

    #[test]
    fn the_default_player_is_apple_music() {
        assert_eq!(controller(&FakeProcesses::new()).player(), "Music");
    }

    #[test]
    fn an_unsafe_player_name_is_refused_at_construction() {
        let result =
            AppleScriptMedia::for_player(Arc::new(FakeProcesses::new()), "Music\" to quit\ntell");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn queries_run_through_osascript_with_the_script_as_an_argument() {
        let processes = FakeProcesses::new();
        controller(&processes)
            .next_track()
            .await
            .expect("the fake succeeds");

        let spawned = processes.spawned();
        assert_eq!(spawned[0].program, "/usr/bin/osascript");
        assert_eq!(spawned[0].args[0], "-e");
        assert!(spawned[0].args[1].contains("next track"));
    }

    #[tokio::test]
    async fn every_verb_but_play_pause_refuses_to_launch_the_player() {
        let processes = FakeProcesses::new();
        let media = controller(&processes);

        media.next_track().await.expect("succeeds");
        media.previous_track().await.expect("succeeds");
        media.set_volume(30).await.expect("succeeds");
        let _ = media.volume().await;

        for spec in processes.spawned() {
            assert!(
                spec.args[1].contains("is running"),
                "a query must not start a music player: {}",
                spec.args[1]
            );
        }
    }

    #[tokio::test]
    async fn play_pause_may_start_the_player() {
        let processes = FakeProcesses::new();
        controller(&processes).play_pause().await.expect("succeeds");

        let script = &processes.spawned()[0].args[1];
        assert!(script.contains("playpause"));
        assert!(
            !script.contains("is running"),
            "pressing play is a request to start playing"
        );
    }

    #[tokio::test]
    async fn volume_is_clamped_before_it_reaches_the_player() {
        let processes = FakeProcesses::new();
        controller(&processes)
            .set_volume(200)
            .await
            .expect("succeeds");
        assert!(processes.spawned()[0].args[1].contains("set sound volume to 100"));
    }

    #[tokio::test]
    async fn a_silent_player_reports_no_volume_rather_than_zero() {
        // The fake returns empty output, which is what a stopped player gives.
        let processes = FakeProcesses::new();
        assert_eq!(
            controller(&processes).volume().await.expect("succeeds"),
            None
        );
    }

    #[test]
    fn a_now_playing_reply_is_parsed_into_its_fields() {
        let media = controller(&FakeProcesses::new());
        let snapshot = media
            .parse_now_playing("Windowlicker\tAphex Twin\t61.5\t366.0", true)
            .expect("a complete reply");

        assert_eq!(snapshot.title, "Windowlicker");
        assert_eq!(snapshot.artist.as_deref(), Some("Aphex Twin"));
        assert_eq!(snapshot.position, Some(Duration::from_secs_f64(61.5)));
        assert_eq!(snapshot.duration, Some(Duration::from_secs_f64(366.0)));
        assert_eq!(snapshot.source, "Music");
        assert!(snapshot.playing);
    }

    #[test]
    fn a_track_with_no_artist_still_parses() {
        let media = controller(&FakeProcesses::new());
        let snapshot = media
            .parse_now_playing("Untitled\t\t0\t120", false)
            .expect("a reply");
        assert_eq!(snapshot.title, "Untitled");
        assert!(snapshot.artist.is_none());
        assert!(!snapshot.playing);
    }

    #[test]
    fn an_empty_reply_is_not_a_track() {
        let media = controller(&FakeProcesses::new());
        assert!(media.parse_now_playing("", true).is_none());
    }

    #[test]
    fn unreadable_timings_are_dropped_rather_than_guessed() {
        assert_eq!(parse_seconds("12.5"), Some(Duration::from_secs_f64(12.5)));
        assert_eq!(parse_seconds("missing value"), None);
        assert_eq!(parse_seconds("-1"), None);
    }
}
