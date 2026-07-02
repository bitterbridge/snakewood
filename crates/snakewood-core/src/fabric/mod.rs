//! The event fabric: Intent -> Guard -> Commit -> Notify.

pub mod intent;

pub use intent::{Event, Intent};
