//! An action runner that runs nothing.
//!
//! Stands in for the dispatcher wherever something asks for an action to be
//! carried out: a workflow step, or a phrase the operator spoke. It records
//! what it was asked for and can be told to refuse, so a test can assert on
//! both what was dispatched and what happened when it would not go.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pushos_domain::action::{ActionDefinition, ActionResult, ActionStatus};
use pushos_domain::context::SurfaceContext;
use pushos_domain::error::{ActionError, ErrorClass};
use pushos_domain::ports::ActionRunner;

/// A dispatcher that records rather than dispatches.
#[derive(Debug, Clone, Default)]
pub struct FakeActions {
    ran: Arc<Mutex<Vec<ActionDefinition>>>,
    refuse: Arc<Mutex<bool>>,
    /// Selectors to refuse outright, as an error.
    refused: Arc<Mutex<Vec<String>>>,
    /// Selectors to accept and then report as failed.
    ///
    /// A different thing from refusing, and worth being able to tell apart: an
    /// action that ran and did not work reports it in its result, and anything
    /// built on top has to notice that as well as an outright error.
    failing: Arc<Mutex<Vec<String>>>,
}

impl FakeActions {
    /// Builds a runner that accepts everything.
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything it was asked to run, in order.
    pub fn ran(&self) -> Vec<ActionDefinition> {
        self.ran.lock().map_or_default(|ran| ran.clone())
    }

    /// The selectors it was asked to run, as they are written in configuration.
    pub fn selectors(&self) -> Vec<String> {
        self.ran()
            .iter()
            .map(|definition| definition.selector.to_string())
            .collect()
    }

    /// Makes every subsequent request fail.
    pub fn refuse_everything(&self) {
        if let Ok(mut refuse) = self.refuse.lock() {
            *refuse = true;
        }
    }

    /// Makes one action fail with an error, leaving the rest working.
    pub fn refuse(&self, selector: &str) {
        if let Ok(mut refused) = self.refused.lock() {
            refused.push(selector.to_owned());
        }
    }

    /// Makes one action run and then report that it did not work.
    pub fn report_failure(&self, selector: &str) {
        if let Ok(mut failing) = self.failing.lock() {
            failing.push(selector.to_owned());
        }
    }

    fn listed(held: &Mutex<Vec<String>>, selector: &str) -> bool {
        held.lock()
            .is_ok_and(|names| names.iter().any(|name| name == selector))
    }
}

#[async_trait]
impl ActionRunner for FakeActions {
    async fn run(
        &self,
        definition: ActionDefinition,
        _surface: SurfaceContext,
    ) -> Result<ActionResult, ActionError> {
        let message = definition.selector.to_string();

        if self.refuse.lock().is_ok_and(|refuse| *refuse) || Self::listed(&self.refused, &message) {
            return Err(ActionError::backend(
                "the fake was told to refuse",
                ErrorClass::Permission,
                std::io::Error::other("refused"),
            ));
        }

        let failed = Self::listed(&self.failing, &message);
        if let Ok(mut ran) = self.ran.lock() {
            ran.push(definition);
        }
        Ok(ActionResult {
            status: if failed {
                ActionStatus::Failed
            } else {
                ActionStatus::Completed
            },
            message: Some(message),
            display: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use pushos_domain::action::{ActionSelector, Params};

    use super::*;

    fn play() -> ActionDefinition {
        ActionDefinition::new(
            "media.play_pause".parse::<ActionSelector>().expect("valid"),
            Params::new(),
        )
    }

    #[tokio::test]
    async fn it_records_what_it_was_asked_to_run() {
        let actions = FakeActions::new();
        actions
            .run(play(), SurfaceContext::default())
            .await
            .expect("the fake accepts by default");
        assert_eq!(actions.selectors(), ["media.play_pause"]);
    }

    #[tokio::test]
    async fn a_refused_action_is_not_recorded_as_run() {
        let actions = FakeActions::new();
        actions.refuse_everything();

        let error = actions
            .run(play(), SurfaceContext::default())
            .await
            .expect_err("it was told to refuse");
        assert_eq!(error.class(), ErrorClass::Permission);
        assert!(actions.ran().is_empty());
    }
}
