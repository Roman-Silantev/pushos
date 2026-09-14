//! Hearing the Mac go to sleep.
//!
//! A Push 2 has its own power supply and keeps whatever lights it was last
//! given for as long as that supply is on. When the Mac sleeps, PushOS is
//! frozen mid-state and cannot tell the Push anything, so whatever was lit at
//! that moment stays lit all night. The only way to avoid that is to hear the
//! sleep coming and put the lights out first.
//!
//! macOS offers exactly that through IOKit's system power notifications, which
//! are delivered before any hardware is powered off and wait for the listener
//! to acknowledge. They are C functions and a C callback, so this module is
//! allowed `unsafe`, and nothing else in this crate is. Everything it touches is
//! made, used and released on its own thread, and the rest of PushOS sees a
//! `watch` channel of [`MachinePower`] and nothing more.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use pushos_domain::rest::MachinePower;
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// How long to hold the Mac's sleep while the lights go out.
///
/// Putting a Push 2 dark is a handful of MIDI messages and one black frame,
/// done in milliseconds. A quarter of a second is ample for that and unnoticed
/// in a sleep that takes seconds. macOS would wait up to thirty; waiting
/// anywhere near that long would be a Mac that is slow to sleep because of
/// PushOS, which is worse than the problem this solves.
const DARKEN: Duration = Duration::from_millis(250);

/// How long the listening thread waits between checks that it should stop.
///
/// It is only ever asked to stop as PushOS finishes, so this is how long an
/// idle thread may outlive it, not a delay anyone waits for.
const CHECK_STOP: f64 = 5.0;

// iokit_common_msg(n) is sys_iokit | sub_iokit_common | n, and sys_iokit is
// err_system(0x38), which is 0x38 << 26. Values from IOMessage.h.
const SYSTEM_MESSAGE: u32 = 0x38 << 26;
/// Idle sleep is being considered. Must be answered, and is not refused here.
const CAN_SYSTEM_SLEEP: u32 = SYSTEM_MESSAGE | 0x270;
/// Sleep is going to happen. Must be answered once the lights are out.
const SYSTEM_WILL_SLEEP: u32 = SYSTEM_MESSAGE | 0x280;
/// The Mac is awake again.
const SYSTEM_HAS_POWERED_ON: u32 = SYSTEM_MESSAGE | 0x300;

type IoObject = u32;
type IoConnect = u32;
type IoNotificationPort = *mut c_void;
type CfRunLoop = *mut c_void;
type CfRunLoopSource = *mut c_void;
type CfString = *const c_void;
type InterestCallback =
    extern "C" fn(refcon: *mut c_void, service: IoObject, message: u32, argument: *mut c_void);

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IORegisterForSystemPower(
        refcon: *mut c_void,
        port: *mut IoNotificationPort,
        callback: InterestCallback,
        notifier: *mut IoObject,
    ) -> IoConnect;
    fn IODeregisterForSystemPower(notifier: *mut IoObject) -> i32;
    fn IOAllowPowerChange(kernel_port: IoConnect, notification: isize) -> i32;
    fn IONotificationPortGetRunLoopSource(port: IoNotificationPort) -> CfRunLoopSource;
    fn IONotificationPortDestroy(port: IoNotificationPort);
    fn IOServiceClose(connect: IoConnect) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: CfString;
    fn CFRunLoopGetCurrent() -> CfRunLoop;
    fn CFRunLoopAddSource(run_loop: CfRunLoop, source: CfRunLoopSource, mode: CfString);
    fn CFRunLoopRunInMode(mode: CfString, seconds: f64, return_after_source: u8) -> i32;
}

/// What the callback needs, owned by the listening thread for its lifetime.
struct Listening {
    power: watch::Sender<MachinePower>,
    /// The connection acknowledgements go back on, known once registered.
    root: AtomicU32,
}

/// Listens for the Mac sleeping and waking, for as long as it is kept.
#[derive(Debug)]
pub struct SleepWatch {
    stop: Arc<AtomicBool>,
}

/// Why the Mac's sleep cannot be listened for.
#[derive(Debug, thiserror::Error)]
pub enum SleepWatchError {
    /// macOS refused the registration.
    #[error("macOS would not report sleep and wake to PushOS")]
    Refused,
    /// The listening thread could not be started or ended before it was ready.
    #[error("the sleep listener could not be started")]
    NoThread,
}

impl SleepWatch {
    /// Starts listening, and returns the power state to follow.
    pub fn start() -> Result<(Self, watch::Receiver<MachinePower>), SleepWatchError> {
        let (power, following) = watch::channel(MachinePower::Running);
        let stop = Arc::new(AtomicBool::new(false));
        let (ready, registered) = std::sync::mpsc::sync_channel(1);

        let stopping = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("pushos-sleep".to_owned())
            .spawn(move || listen(power, &stopping, &ready))
            .map_err(|_| SleepWatchError::NoThread)?;

        match registered.recv() {
            Ok(true) => {
                info!("listening for the Mac sleeping, to put the Push 2 dark first");
                Ok((Self { stop }, following))
            }
            Ok(false) => Err(SleepWatchError::Refused),
            Err(_) => Err(SleepWatchError::NoThread),
        }
    }
}

impl Drop for SleepWatch {
    fn drop(&mut self) {
        // Only a flag. Reaching into the listening thread's run loop from here
        // would race with that thread ending and releasing it; the flag cannot.
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The listening thread: registers, runs until told to stop, and releases.
fn listen(
    power: watch::Sender<MachinePower>,
    stop: &AtomicBool,
    ready: &std::sync::mpsc::SyncSender<bool>,
) {
    let context = Box::into_raw(Box::new(Listening {
        power,
        root: AtomicU32::new(0),
    }));

    let mut port: IoNotificationPort = std::ptr::null_mut();
    let mut notifier: IoObject = 0;
    // The context lives until the end of this function, after deregistration,
    // so the callback never sees it freed.
    let root = unsafe {
        IORegisterForSystemPower(context.cast(), &raw mut port, on_power, &raw mut notifier)
    };
    if root == 0 {
        warn!("macOS refused to report sleep and wake");
        drop(unsafe { Box::from_raw(context) });
        let _ = ready.send(false);
        return;
    }

    unsafe {
        (*context).root.store(root, Ordering::Relaxed);
        CFRunLoopAddSource(
            CFRunLoopGetCurrent(),
            IONotificationPortGetRunLoopSource(port),
            kCFRunLoopDefaultMode,
        );
    }
    let _ = ready.send(true);

    // In bounded turns rather than once forever, so a stop that arrives before
    // the first turn begins is still seen.
    while !stop.load(Ordering::Relaxed) {
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, CHECK_STOP, 0) };
    }

    unsafe {
        IODeregisterForSystemPower(&raw mut notifier);
        IOServiceClose(root);
        IONotificationPortDestroy(port);
        drop(Box::from_raw(context));
    }
    debug!("stopped listening for sleep");
}

/// Called by `IOKit` on the listening thread for each power message.
///
/// Every message that expects an answer gets one, whatever else happens. An
/// unanswered sleep notification holds the Mac's sleep for thirty seconds.
extern "C" fn on_power(
    refcon: *mut c_void,
    _service: IoObject,
    message: u32,
    argument: *mut c_void,
) {
    // The context outlives registration; see `listen`.
    let listening = unsafe { &*refcon.cast::<Listening>() };
    let root = listening.root.load(Ordering::Relaxed);
    let notification = argument as isize;

    match message {
        CAN_SYSTEM_SLEEP => {
            unsafe { IOAllowPowerChange(root, notification) };
        }
        SYSTEM_WILL_SLEEP => {
            debug!("the Mac is going to sleep; putting the Push 2 dark");
            listening.power.send_replace(MachinePower::Sleeping);
            std::thread::sleep(DARKEN);
            unsafe { IOAllowPowerChange(root, notification) };
        }
        SYSTEM_HAS_POWERED_ON => {
            debug!("the Mac is awake");
            listening.power.send_replace(MachinePower::Running);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_numbers_match_iokit() {
        // IOMessage.h: kIOMessageCanSystemSleep 0xE0000270,
        // kIOMessageSystemWillSleep 0xE0000280,
        // kIOMessageSystemHasPoweredOn 0xE0000300.
        assert_eq!(CAN_SYSTEM_SLEEP, 0xE000_0270);
        assert_eq!(SYSTEM_WILL_SLEEP, 0xE000_0280);
        assert_eq!(SYSTEM_HAS_POWERED_ON, 0xE000_0300);
    }

    #[test]
    fn listening_starts_and_stops_on_this_mac() {
        // Registration is real: it asks this Mac's power management. Nothing
        // sleeps, and nothing is acknowledged, because nothing asks.
        let (watch, following) = SleepWatch::start().expect("macOS accepts the registration");
        assert_eq!(*following.borrow(), MachinePower::Running);
        drop(watch);
    }
}
