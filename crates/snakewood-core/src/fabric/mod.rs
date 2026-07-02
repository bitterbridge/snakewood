//! The event fabric: Intent -> Guard -> Commit -> Notify.

pub mod intent;
pub mod trigger;

pub use intent::{Event, Intent};
pub use trigger::Trigger;
