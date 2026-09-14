//! How hard the hardware works, and when it rests: the `[surface]` section.

use std::time::Duration;

use crate::error::Problem;
use crate::model::ConfigFile;

/// Builds the workflows, refusing anything that could not run.
///
/// A step missing what its kind needs is reported by name rather than skipped,
/// because a graph with a hole in it is a run that stops halfway.
/// Builds when the surface dims and sleeps.
///
/// Anything left out keeps its default, so a file that only says how bright
/// it wants the surface still rests.
pub(crate) fn build(
    file: &ConfigFile,
    problems: &mut Vec<Problem>,
) -> pushos_domain::rest::RestPolicy {
    use pushos_domain::rest::RestPolicy;

    let section = &file.surface;
    let mut policy = RestPolicy::DEFAULT;

    if let Some(value) = section.brightness {
        match u8::try_from(value) {
            Ok(percent @ 1..=100) => policy.brightness = percent,
            _ => problems.push(Problem::SurfaceBrightness { value }),
        }
    }

    let minutes = |field: &'static str, written: Option<f64>, default: Option<Duration>| {
        match written {
            None => Ok(default),
            Some(value) if !value.is_finite() || value < 0.0 => {
                Err(Problem::SurfaceMinutes { field, value })
            }
            // Nought switches the stage off rather than making it immediate: a
            // surface that slept the moment it was left would never be awake.
            Some(0.0) => Ok(None),
            Some(value) => Ok(Some(Duration::from_secs_f64(value * 60.0))),
        }
    };

    match minutes(
        "dim_after_minutes",
        section.dim_after_minutes,
        policy.dim_after,
    ) {
        Ok(after) => policy.dim_after = after,
        Err(problem) => problems.push(problem),
    }
    match minutes(
        "sleep_after_minutes",
        section.sleep_after_minutes,
        policy.sleep_after,
    ) {
        Ok(after) => policy.sleep_after = after,
        Err(problem) => problems.push(problem),
    }

    if let (Some(dim), Some(sleep)) = (policy.dim_after, policy.sleep_after)
        && sleep <= dim
    {
        problems.push(Problem::SurfaceSleepsBeforeDimming {
            dim: dim.as_secs_f64() / 60.0,
            sleep: sleep.as_secs_f64() / 60.0,
        });
    }

    policy
}
