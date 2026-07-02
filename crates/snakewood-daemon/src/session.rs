use snakewood_core::{EntityId, PresentationNode};

/// Identifies a connected session (one per client connection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);

/// A connected session: the actor it drives and its pending outbound view.
#[derive(Debug, Clone)]
pub struct Session {
    pub actor: EntityId,
    pub outbox: Vec<PresentationNode>,
}

impl Session {
    pub fn new(actor: EntityId) -> Session {
        Session { actor, outbox: Vec::new() }
    }
}
