//! Media transport control.
//!
//! The provider speaks only to the media port, so nothing about
//! Apple Music, AppleScript or any other player reaches this file.

use std::sync::Arc;

use async_trait::async_trait;
use pushos_domain::action::{ActionContext, ActionResult, DisplayIntent};
use pushos_domain::error::ActionError;
use pushos_domain::ids::{ActionVerb, ProviderName};
use pushos_domain::permissions::Permission;
use pushos_domain::ports::{ActionProvider, MediaController, MediaSnapshot, ProviderCapabilities};

/// The namespace this provider claims.
pub const NAMESPACE: &str = "media";

/// How far a single volume step moves, as a percentage.
const VOLUME_STEP: i16 = 5;

/// Controls playback through whatever media backend is installed.
#[derive(Debug)]
pub struct MediaProvider {
    controller: Arc<dyn MediaController>,
}

impl MediaProvider {
    /// Builds the provider over a media backend.
    pub fn new(controller: Arc<dyn MediaController>) -> Self {
        Self { controller }
    }

    /// Moves the volume by one step in the given direction.
    ///
    /// A backend that cannot report its current volume cannot be stepped
    /// relatively, so this says so rather than guessing at a starting point.
    async fn step_volume(&self, direction: i16) -> Result<ActionResult, ActionError> {
        let Some(current) = self.controller.volume().await? else {
            return Err(ActionError::NeedsConfirmation {
                reason: "the media backend does not report its volume; set an exact value"
                    .to_owned(),
            });
        };

        let next = (i16::from(current) + direction * VOLUME_STEP).clamp(0, 100);
        let percent = u8::try_from(next).unwrap_or(0);
        self.controller.set_volume(percent).await?;
        Ok(ActionResult::completed().with_message(format!("volume {percent}%")))
    }

    fn overlay(snapshot: Option<MediaSnapshot>) -> ActionResult {
        match snapshot {
            None => ActionResult::completed().with_message("nothing playing"),
            Some(playing) => {
                let detail = playing
                    .artist
                    .clone()
                    .unwrap_or_else(|| playing.source.clone());
                ActionResult::completed().with_display(DisplayIntent::Toast {
                    title: playing.title,
                    detail: Some(detail),
                })
            }
        }
    }
}

#[async_trait]
impl ActionProvider for MediaProvider {
    fn name(&self) -> ProviderName {
        ProviderName::new(NAMESPACE)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(
            [
                "play_pause",
                "next_track",
                "previous_track",
                "volume_up",
                "volume_down",
                "set_volume",
                "now_playing",
            ]
            .map(ActionVerb::new),
        )
        .requiring([Permission::MediaControl])
    }

    async fn execute(&self, context: ActionContext) -> Result<ActionResult, ActionError> {
        match context.definition.selector.verb.as_str() {
            "play_pause" => {
                self.controller.play_pause().await?;
                Ok(ActionResult::completed())
            }
            "next_track" => {
                self.controller.next_track().await?;
                Ok(ActionResult::completed())
            }
            "previous_track" => {
                self.controller.previous_track().await?;
                Ok(ActionResult::completed())
            }
            "volume_up" => self.step_volume(1).await,
            "volume_down" => self.step_volume(-1).await,
            "set_volume" => {
                let percent = context
                    .params()
                    .get("percent")
                    .and_then(pushos_domain::action::ParamValue::as_integer)
                    .ok_or_else(|| pushos_domain::action::MissingParam {
                        key: "percent".to_owned(),
                        expected: "integer",
                    })?;
                let percent = u8::try_from(percent.clamp(0, 100)).unwrap_or(0);
                self.controller.set_volume(percent).await?;
                Ok(ActionResult::completed().with_message(format!("volume {percent}%")))
            }
            "now_playing" => Ok(Self::overlay(self.controller.now_playing().await?)),
            verb => Err(ActionError::UnknownVerb {
                provider: self.name(),
                verb: verb.to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionDefinition, ActionSelector, ParamValue, Params};
    use pushos_domain::context::SurfaceContext;
    use pushos_domain::error::ErrorClass;
    use pushos_domain::ids::CorrelationId;
    use pushos_testkit::FakeMedia;

    use super::*;

    fn provider(media: &FakeMedia) -> MediaProvider {
        MediaProvider::new(Arc::new(media.clone()))
    }

    async fn run(
        media: &FakeMedia,
        verb: &str,
        params: Params,
    ) -> Result<ActionResult, ActionError> {
        let definition = ActionDefinition::new(ActionSelector::new(NAMESPACE, verb), params);
        provider(media)
            .execute(ActionContext::new(
                definition,
                CorrelationId::generate(),
                SurfaceContext::empty(),
            ))
            .await
    }

    #[tokio::test]
    async fn transport_verbs_reach_the_backend_once_each() {
        let media = FakeMedia::new();
        run(&media, "play_pause", Params::new())
            .await
            .expect("succeeds");
        run(&media, "next_track", Params::new())
            .await
            .expect("succeeds");
        run(&media, "previous_track", Params::new())
            .await
            .expect("succeeds");

        assert_eq!(media.calls().len(), 3);
    }

    #[tokio::test]
    async fn setting_volume_clamps_rather_than_wrapping() {
        let media = FakeMedia::new();
        let mut params = Params::new();
        params.set("percent", ParamValue::Integer(400));
        run(&media, "set_volume", params).await.expect("succeeds");

        let mut params = Params::new();
        params.set("percent", ParamValue::Integer(-30));
        run(&media, "set_volume", params).await.expect("succeeds");

        assert_eq!(
            media.calls(),
            [
                pushos_testkit::MediaCall::Volume(100),
                pushos_testkit::MediaCall::Volume(0)
            ]
        );
    }

    #[tokio::test]
    async fn setting_volume_without_a_percentage_is_a_validation_error() {
        let media = FakeMedia::new();
        let error = run(&media, "set_volume", Params::new())
            .await
            .expect_err("no percentage");
        assert_eq!(error.class(), ErrorClass::Validation);
        assert!(media.calls().is_empty());
    }

    #[tokio::test]
    async fn stepping_volume_moves_from_the_reported_level() {
        let media = FakeMedia::new();
        media.set_reported_volume(Some(40));

        run(&media, "volume_up", Params::new())
            .await
            .expect("succeeds");
        run(&media, "volume_up", Params::new())
            .await
            .expect("succeeds");
        run(&media, "volume_down", Params::new())
            .await
            .expect("succeeds");

        assert_eq!(
            media.calls(),
            [
                pushos_testkit::MediaCall::Volume(45),
                pushos_testkit::MediaCall::Volume(50),
                pushos_testkit::MediaCall::Volume(45)
            ]
        );
    }

    #[tokio::test]
    async fn stepping_volume_clamps_at_both_ends() {
        let media = FakeMedia::new();
        media.set_reported_volume(Some(98));
        run(&media, "volume_up", Params::new())
            .await
            .expect("succeeds");
        assert_eq!(media.calls(), [pushos_testkit::MediaCall::Volume(100)]);
    }

    #[tokio::test]
    async fn a_backend_that_cannot_report_volume_asks_for_an_exact_value() {
        let media = FakeMedia::new();
        media.set_reported_volume(None);

        let error = run(&media, "volume_up", Params::new())
            .await
            .expect_err("no level to step from");
        assert_eq!(error.class(), ErrorClass::UserActionRequired);
        assert!(media.calls().is_empty(), "volume must not move on a guess");
    }

    #[tokio::test]
    async fn now_playing_becomes_a_transient_overlay() {
        let media = FakeMedia::new();
        media.set_now_playing(MediaSnapshot {
            source: "Music".to_owned(),
            title: "Windowlicker".to_owned(),
            artist: Some("Aphex Twin".to_owned()),
            playing: true,
            ..MediaSnapshot::default()
        });

        let result = run(&media, "now_playing", Params::new())
            .await
            .expect("succeeds");
        assert_eq!(
            result.display,
            Some(DisplayIntent::Toast {
                title: "Windowlicker".to_owned(),
                detail: Some("Aphex Twin".to_owned()),
            })
        );
    }

    #[tokio::test]
    async fn nothing_playing_reports_that_rather_than_showing_an_empty_overlay() {
        let media = FakeMedia::new();
        let result = run(&media, "now_playing", Params::new())
            .await
            .expect("succeeds");
        assert!(result.display.is_none());
        assert_eq!(result.message.as_deref(), Some("nothing playing"));
    }

    #[tokio::test]
    async fn a_backend_failure_keeps_its_classification() {
        let media = FakeMedia::new();
        media.fail_with(ErrorClass::Retryable);
        let error = run(&media, "play_pause", Params::new())
            .await
            .expect_err("told to fail");
        assert_eq!(error.class(), ErrorClass::Retryable);
    }

    #[test]
    fn media_control_is_a_declared_capability() {
        let media = FakeMedia::new();
        assert_eq!(
            provider(&media).capabilities().required_permissions(),
            [Permission::MediaControl]
        );
    }
}
