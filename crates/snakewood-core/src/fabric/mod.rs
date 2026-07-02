//! The event fabric: Intent -> Guard -> Commit -> Notify.

pub mod gather;
pub mod handler;
pub mod intent;
pub mod predicate;
pub mod resolve;
pub mod trigger;

pub use gather::{gather, Candidate};
pub use handler::{Band, Effect, Outcome, Responder, Rule};
pub use intent::{Event, Intent};
pub use predicate::{eval as eval_predicate, Party, Predicate};
pub use resolve::{resolve, salient, Decision};
pub use trigger::Trigger;
