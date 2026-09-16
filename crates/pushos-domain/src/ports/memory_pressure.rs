//! What the machine says about how much memory it has left.
//!
//! PushOS decides how many coding sessions may run at once. A number chosen in
//! configuration cannot know what else the operator is running, so the machine
//! is asked as well: when it says memory is short, fewer sessions are left
//! running, and they come back when it says the pressure has passed.
//!
//! Asked rather than calculated. How much memory is really free is not a
//! number anyone can work out from process sizes: the system compresses,
//! shares and reclaims pages continuously, and only it knows.

use async_trait::async_trait;

/// How much memory the machine says it is short of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Pressure {
    /// There is room. Nothing needs to give anything back.
    #[default]
    Normal,
    /// The machine has started asking for memory back.
    Warning,
    /// The machine is about to swap, or is swapping.
    Critical,
}

/// Asks the machine how it is doing for memory.
#[async_trait]
pub trait MemoryPressure: Send + Sync + std::fmt::Debug {
    /// How much memory the machine says it is short of, right now.
    ///
    /// Never fails: a machine that will not say is one PushOS treats as having
    /// room, because guessing the other way would put away sessions the
    /// operator is using on no evidence at all.
    async fn now(&self) -> Pressure;
}

/// A machine that always says it has room, for tests and for anything that
/// does not know how to ask.
#[derive(Clone, Copy, Debug, Default)]
pub struct RoomToSpare;

#[async_trait]
impl MemoryPressure for RoomToSpare {
    async fn now(&self) -> Pressure {
        Pressure::Normal
    }
}
