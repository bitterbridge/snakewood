//! The event fabric: Intent -> Guard -> Commit -> Notify.

pub mod dispatch;
pub mod gather;
pub mod handler;
pub mod intent;
pub mod operator;
pub mod predicate;
pub mod resolve;
pub mod trigger;

pub use dispatch::{dispatch, Dispatch};
pub use gather::{gather, Candidate};
pub use handler::{Band, Effect, Outcome, Responder, Rule};
pub use intent::{Event, Intent};
pub use operator::{
    coalesce, Admission, IntentClass, Operator, PresentationKind, RateLimiterState, Scope,
};
pub use predicate::{eval as eval_predicate, Party, Predicate};
pub use resolve::{resolve, salient, Decision};
pub use trigger::Trigger;
