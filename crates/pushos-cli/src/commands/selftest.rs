//! Proving the hardware, on the hardware.
//!
//! Every automated test in PushOS runs against a stand-in surface, which is
//! the right way round: the core must be testable on a machine with no Push 2.
//! But a stand-in only ever does what somebody thought to make it do, and two
//! faults shipped for months behind exactly that gap — an app killed by the
//! kernel the moment Push code first ran, and a device plugged in after
//! startup that stayed invisible for the life of the process. Neither could
//! have been found without the device.
//!
//! So this is the part a person has to watch. It lights every light in turn so
//! a dead one is visible, draws a pattern that shows a dead row or column of
//! the screen, and then waits while the operator works every control, saying
//! at the end which of the hundred and forty-one arrived and which never did.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pushos_domain::color::{LedState, Rgb};
use pushos_domain::controls::{ControlId, PAD_COLUMNS};
use pushos_domain::ports::{DISPLAY_HEIGHT, DISPLAY_WIDTH, DisplayFrame, PushInput, PushOutput};
use pushos_domain::rest::Levels;
use pushos_push2::{PortRole, Push2Device};

/// How long each step of the light sweep is held.
///
/// Long enough to see, short enough that a hundred and forty-one of them is
/// not a coffee break.
const A_GLANCE: Duration = Duration::from_millis(45);

/// How long the display pattern is left up.
const A_LOOK: Duration = Duration::from_secs(4);

/// How long to wait for the operator to work the controls.
const PATIENCE: Duration = Duration::from_secs(90);

/// How long without a press, once they have started, before the input step
/// decides they have finished.
///
/// Only once they have started. Waiting this long for the *first* press means
/// giving up on anyone who was looking at the surface rather than the terminal
/// — which is everyone, since the surface is the thing being tested.
const FINISHED: Duration = Duration::from_secs(8);

/// Runs the whole check.
pub(crate) async fn execute() -> Result<(), String> {
    let (device, mut input) = Push2Device::connect(PortRole::User).map_err(|error| {
        format!("no Push 2 to test: {error}\nplug one in, and stop any PushOS holding it")
    })?;
    let surface: Arc<dyn PushOutput> = Arc::new(device);

    // Full brightness throughout: a light being dim is not the question.
    surface
        .set_brightness(Levels {
            lights: 100,
            screen: 100,
        })
        .await
        .map_err(|error| format!("could not set the brightness: {error}"))?;

    println!("Testing the Push 2. Watch the surface.\n");

    sweep_the_lights(surface.as_ref()).await?;
    draw_the_pattern(surface.as_ref()).await?;
    let seen = collect_presses(&mut input).await;

    surface
        .clear()
        .await
        .map_err(|error| format!("could not put the lights out: {error}"))?;

    report(&seen);
    Ok(())
}

/// Lights every light that has one, one at a time, then all of them together.
///
/// One at a time is what finds a single dead pad; all together is what finds a
/// power problem, because a Push running from bus power browns out before it
/// refuses.
async fn sweep_the_lights(surface: &dyn PushOutput) -> Result<(), String> {
    let lit: Vec<ControlId> = ControlId::all()
        .filter(|control| control.is_illuminated())
        .collect();
    println!("1. Lights: {} of them, one at a time.", lit.len());
    println!("   Anything that stays dark is worth knowing about.");

    for control in &lit {
        surface
            .set_led(*control, LedState::solid(Rgb::WHITE))
            .await
            .map_err(|error| format!("could not light {control}: {error}"))?;
        tokio::time::sleep(A_GLANCE).await;
        surface
            .set_led(*control, LedState::OFF)
            .await
            .map_err(|error| format!("could not put {control} out: {error}"))?;
    }

    let all_on: Vec<(ControlId, LedState)> = lit
        .iter()
        .map(|control| (*control, LedState::solid(Rgb::WHITE)))
        .collect();
    surface
        .set_leds(&all_on)
        .await
        .map_err(|error| format!("could not light everything: {error}"))?;
    tokio::time::sleep(A_LOOK).await;
    surface
        .clear()
        .await
        .map_err(|error| format!("could not put the lights out: {error}"))?;
    println!("   done.\n");
    Ok(())
}

/// Draws a pattern that shows what a blank screen cannot.
///
/// A grid finds a dead row or column, and a full-width sweep of colour finds a
/// panel that has lost one of them.
async fn draw_the_pattern(surface: &dyn PushOutput) -> Result<(), String> {
    println!(
        "2. Display: a grid and a colour sweep for {}s.",
        A_LOOK.as_secs()
    );
    println!("   Look for a missing line, a missing colour, or a blank band.");

    let mut frame = DisplayFrame::blank();
    for y in 0..DISPLAY_HEIGHT {
        for x in 0..DISPLAY_WIDTH {
            // Red across, green down, so a channel that has gone is obvious,
            // with a white grid over it to show a row or column that has.
            let on_a_line = x % 64 == 0 || y % 32 == 0;
            let colour = if on_a_line {
                Rgb::WHITE
            } else {
                #[allow(clippy::cast_possible_truncation)]
                let across = (x * 255 / DISPLAY_WIDTH.max(1)) as u8;
                #[allow(clippy::cast_possible_truncation)]
                let down = (y * 255 / DISPLAY_HEIGHT.max(1)) as u8;
                Rgb::new(across, down, 128)
            };
            if let Some(pixel) = frame.pixels_mut().get_mut(y * DISPLAY_WIDTH + x) {
                *pixel = colour.to_bgr565();
            }
        }
    }

    // Presented repeatedly: the hardware blanks itself if no frame arrives for
    // two seconds, so one frame would be a flash rather than a pattern.
    let until = Instant::now() + A_LOOK;
    while Instant::now() < until {
        surface
            .present(&frame)
            .await
            .map_err(|error| format!("could not draw on the display: {error}"))?;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    println!("   done.\n");
    Ok(())
}

/// Waits while the operator works every control, and remembers what arrived.
async fn collect_presses(input: &mut impl PushInput) -> BTreeSet<String> {
    let total = ControlId::all().count();
    println!("3. Controls: press every pad and button, turn every encoder,");
    println!("   and run a finger along the touch strip.");
    println!(
        "   {total} to find. Stops {}s after you finish, or {}s from now.",
        FINISHED.as_secs(),
        PATIENCE.as_secs()
    );

    let mut seen = BTreeSet::new();
    let give_up = Instant::now() + PATIENCE;

    loop {
        // The whole budget for the first press, because they are watching the
        // surface and not this. A short quiet only ends it once something has
        // arrived and then stopped arriving.
        let waiting = if seen.is_empty() {
            give_up.saturating_duration_since(Instant::now())
        } else {
            FINISHED
        };
        if waiting.is_zero() {
            break;
        }

        let Ok(Some(event)) = tokio::time::timeout(waiting, input.next_event()).await else {
            break;
        };
        if seen.insert(event.control.to_string()) {
            println!("   {} ({} of {total})", event.control, seen.len());
        }
    }
    println!();
    seen
}

/// Says what was proven and what was not.
fn report(seen: &BTreeSet<String>) {
    let every: Vec<String> = ControlId::all()
        .map(|control| control.to_string())
        .collect();
    let missing: Vec<&String> = every
        .iter()
        .filter(|control| !seen.contains(*control))
        .collect();

    println!(
        "{} of {} controls reported something.",
        seen.len(),
        every.len()
    );

    if missing.is_empty() {
        println!("\nEvery control on this Push 2 works.");
        return;
    }

    // Named, not counted: an operator who skipped the touch strip needs to see
    // that it was the touch strip, so they can decide whether they care.
    println!("\nNothing arrived from these — either untouched, or not working:");
    for control in missing.chunks(PAD_COLUMNS as usize) {
        let row: Vec<&str> = control.iter().map(|held| held.as_str()).collect();
        println!("  {}", row.join("  "));
    }
    println!("\nRun it again and work only those, to tell a skip from a fault.");
}
