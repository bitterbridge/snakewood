use serde::{Deserialize, Serialize};

use crate::fabric::Intent;
use crate::PresentationNode;

/// Which intent class an operator matches. Coarser than `Trigger` — operators
/// gate by class, not by exact direction.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentClass {
    Move,
    Look,
}

impl IntentClass {
    pub fn of(intent: &Intent) -> IntentClass {
        match intent {
            Intent::Move { .. } => IntentClass::Move,
            Intent::Look { .. } => IntentClass::Look,
        }
    }
}

/// The kind of a presentation node, for kind-based coalescing.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PresentationKind {
    RoomName,
    RoomDescription,
    Exits,
    Occupants,
    Line,
    Denied,
    Prompt,
}

impl PresentationKind {
    pub fn of(node: &PresentationNode) -> PresentationKind {
        match node {
            PresentationNode::RoomName(_) => PresentationKind::RoomName,
            PresentationNode::RoomDescription(_) => PresentationKind::RoomDescription,
            PresentationNode::Exits(_) => PresentationKind::Exits,
            PresentationNode::Occupants(_) => PresentationKind::Occupants,
            PresentationNode::Line(_) => PresentationKind::Line,
            PresentationNode::Denied(_) => PresentationKind::Denied,
            PresentationNode::Prompt => PresentationKind::Prompt,
        }
    }
}

/// The scope key an operator's state machine is bucketed by.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    PerActor,
}

/// A declarative stream operator, attached as data on the `Realm`. Each is a
/// deterministic, tick-quantized state machine (state held at runtime by the
/// host). See the M2 design spec §3.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Operator {
    /// Admit at most one matching intent per `per_ticks` window per scope key.
    RateLimit {
        on: IntentClass,
        per_ticks: u64,
        scope: Scope,
        #[serde(default)]
        deny: Option<String>,
    },
    /// Collapse redundant directed-presentation nodes of the listed kinds.
    /// M2 collapses within a single tick's batch (`within_ticks` reserved; see
    /// the M2 design spec §5 for why cross-tick redraw coalescing is unsafe).
    Coalesce {
        on: Vec<PresentationKind>,
        within_ticks: u64,
        scope: Scope,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fabric::Intent;
    use crate::{Direction, EntityId, PresentationNode};

    #[test]
    fn intent_class_maps_variants() {
        let mv = Intent::Move {
            actor: EntityId::new("player/anon-0").unwrap(),
            direction: Direction::North,
        };
        let look = Intent::Look {
            actor: EntityId::new("player/anon-0").unwrap(),
        };
        assert_eq!(IntentClass::of(&mv), IntentClass::Move);
        assert_eq!(IntentClass::of(&look), IntentClass::Look);
    }

    #[test]
    fn presentation_kind_maps_variants() {
        assert_eq!(
            PresentationKind::of(&PresentationNode::RoomName("x".into())),
            PresentationKind::RoomName
        );
        assert_eq!(
            PresentationKind::of(&PresentationNode::Denied("no".into())),
            PresentationKind::Denied
        );
        assert_eq!(
            PresentationKind::of(&PresentationNode::Prompt),
            PresentationKind::Prompt
        );
    }
}
