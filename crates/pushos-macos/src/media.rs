//! Media control through AppleScript.
//!
//! `MediaRemote`, the framework that reports what is playing system-wide, has
//! been gated behind a private entitlement since macOS 15.4 and returns nothing
//! to unentitled callers. AppleScript against a named player is the supported
//! mechanism, so that is what PushOS uses.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

/// How long the volume PushOS just set is believed without asking again.
///
/// An encoder produces dozens of events for one turn of the wrist, and each
/// answer here would otherwise be a separate AppleScript process. Short enough
/// that a change made in the player itself is noticed almost at once, long
/// enough that a sweep of the encoder is one conversation rather than fifty.
const VOLUME_BELIEVED_FOR: Duration = Duration::from_millis(1_500);

/// Controls a scriptable media player.
#[derive(Debug, Clone)]
pub struct AppleScriptMedia {
    scripts: ScriptRunner,
    player: ApplicationName,
    /// The volume PushOS last saw or set, and when.
    ///
    /// Held because the alternative is a subprocess per encoder tick. Turning
    /// the master encoder produces events far faster than AppleScript can
    /// answer, and reading the volume before every step made a turn of the
    /// wrist into a hundred processes and a surface that would not keep up.
    volume: Arc<Mutex<Option<(u8, Instant)>>>,
    /// Where the volume is being taken, when a write is already on its way.
    ///
    /// Latest value wins, the way it does everywhere else in PushOS: an
    /// operator spinning an encoder is asking for the volume it ends on, not
    /// for every value it passed through on the way.
    writing: Arc<Mutex<Writing>>,
}

/// The state of the one write that may be in flight.
#[derive(Debug, Default)]
struct Writing {
    /// Whether a write is on its way.
    in_flight: bool,
    /// Where the volume should end up, if that is not where it is going.
    pending: Option<u8>,
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
            volume: Arc::new(Mutex::new(None)),
            writing: Arc::new(Mutex::new(Writing::default())),
        })
    }

    /// The player being controlled.
    pub fn player(&self) -> &str {
        self.player.as_str()
    }

    /// The volume PushOS believes the player is at, while that is still fresh.
    fn believed(&self) -> Option<u8> {
        let held = self.volume.lock().ok()?;
        let (percent, at) = (*held)?;
        (at.elapsed() < VOLUME_BELIEVED_FOR).then_some(percent)
    }

    /// Records the volume the player is at now.
    fn remember(&self, percent: u8) {
        if let Ok(mut held) = self.volume.lock() {
            *held = Some((percent, Instant::now()));
        }
    }

    /// Whether this caller is the one that does the writing.
    ///
    /// The first caller writes and keeps writing until nothing more is pending.
    /// Everyone else leaves their target behind and returns, because a second
    /// AppleScript process would not make the volume arrive any sooner.
    fn take_the_write(&self, percent: u8) -> bool {
        let Ok(mut writing) = self.writing.lock() else {
            // A poisoned lock means a previous writer panicked mid-flight.
            // Writing anyway is better than a volume control that stops.
            return true;
        };

        if writing.in_flight {
            writing.pending = Some(percent);
            false
        } else {
            writing.in_flight = true;
            writing.pending = None;
            true
        }
    }

    /// Where to go next, or nothing when the volume has arrived.
    ///
    /// Taking the target and standing down happen under one lock, so a request
    /// arriving at that moment cannot be left with nobody to carry it.
    fn next_target(&self, failed: bool) -> Option<u8> {
        let mut writing = self.writing.lock().ok()?;
        match writing.pending.take() {
            Some(next) if !failed => Some(next),
            _ => {
                writing.in_flight = false;
                None
            }
        }
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

    /// Takes the volume to a percentage, collapsing a flurry into one journey.
    ///
    /// An encoder produces events faster than AppleScript can answer them. Sent
    /// one at a time they queue, and the volume goes on moving after the
    /// operator has stopped turning. So one write is in flight at a time and
    /// later requests replace where it is going: what the operator asked for is
    /// where they stopped, not everywhere they passed through.
    async fn set_volume(&self, percent: u8) -> Result<(), ActionError> {
        let percent = percent.min(100);

        // Remembered before the write rather than after, so the next step of
        // the encoder computes from where the volume is going.
        self.remember(percent);

        if !self.take_the_write(percent) {
            return Ok(());
        }

        let mut target = percent;
        loop {
            let result = self
                .tell_if_running(&format!("set sound volume to {target}"))
                .await;

            match self.next_target(result.is_err()) {
                Some(next) => target = next,
                // Either it is where it was asked to go, or the write failed
                // and there is no reason to think the next one would not.
                None => return result.map(|_| ()),
            }
        }
    }

    async fn volume(&self) -> Result<Option<u8>, ActionError> {
        if let Some(known) = self.believed() {
            return Ok(Some(known));
        }

        let reply = self.tell_if_running("return sound volume as text").await?;
        if reply.is_empty() {
            return Ok(None);
        }
        reply
            .parse::<u16>()
            .map(|value| {
                let percent = u8::try_from(value.min(100)).unwrap_or(100);
                self.remember(percent);
                Some(percent)
            })
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

    #[tokio::test]
    async fn stepping_the_volume_does_not_ask_the_player_every_time() {
        // An encoder produces dozens of events for one turn of the wrist.
        // Reading the volume before each step made that a hundred AppleScript
        // processes and a surface that could not keep up.
        let processes = FakeProcesses::new();
        let media = controller(&processes);

        media.set_volume(40).await.expect("the fake succeeds");
        let after_setting = processes.spawned().len();

        for _ in 0..10 {
            assert_eq!(
                media.volume().await.expect("the fake succeeds"),
                Some(40),
                "the volume PushOS just set is the volume it is at"
            );
        }

        assert_eq!(
            processes.spawned().len(),
            after_setting,
            "reading back what PushOS just wrote should cost nothing"
        );
    }

    #[tokio::test]
    async fn the_volume_is_asked_for_when_nothing_is_known() {
        let processes = FakeProcesses::new();
        controller(&processes).volume().await.ok();
        assert_eq!(processes.spawned().len(), 1);
    }

    #[tokio::test]
    async fn a_flurry_of_volume_changes_becomes_one_journey() {
        // What the operator asked for is where they stopped turning, not
        // everywhere they passed through on the way.
        let processes = FakeProcesses::new();
        let media = Arc::new(controller(&processes));

        let mut turning = Vec::new();
        for percent in [10_u8, 20, 30, 40, 50] {
            let media = Arc::clone(&media);
            turning.push(tokio::spawn(async move { media.set_volume(percent).await }));
        }
        for turn in turning {
            turn.await
                .expect("the task finishes")
                .expect("the fake succeeds");
        }

        let written = processes.spawned().len();
        assert!(
            written <= 5,
            "five requests should not cost more than five writes, and usually fewer: {written}"
        );

        // Whatever the writes collapsed to, the volume ends where it was last
        // asked to be.
        assert_eq!(media.believed(), Some(50));
    }

    #[tokio::test]
    async fn a_volume_written_by_hand_is_still_believed_after() {
        let processes = FakeProcesses::new();
        let media = controller(&processes);
        media.set_volume(70).await.expect("the fake succeeds");
        assert_eq!(media.believed(), Some(70));
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
