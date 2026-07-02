//! snakewood-daemon: the long-lived host around the pure `snakewood-core` engine.
//! Stage 3a is synchronous; async transports wrap this in later sub-stages.

pub mod clock;
pub mod engine;
pub mod session;

pub use clock::{Clock, ManualClock, SystemClock};
pub use engine::Engine;
pub use session::{Session, SessionId};
