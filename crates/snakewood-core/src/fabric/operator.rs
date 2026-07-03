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

    use crate::{from_ron, to_ron};
    use proptest::prelude::*;

    #[test]
    fn operator_ron_round_trips_and_is_readable() {
        let ops = vec![
            Operator::RateLimit {
                on: IntentClass::Move,
                per_ticks: 3,
                scope: Scope::PerActor,
                deny: Some("Slow down.".to_string()),
            },
            Operator::Coalesce {
                on: vec![PresentationKind::RoomName, PresentationKind::Exits],
                within_ticks: 1,
                scope: Scope::PerActor,
            },
        ];
        let text = to_ron(&ops);
        // Golden-ish: output is human-readable and names the operators.
        assert!(text.contains("RateLimit"), "RON:\n{text}");
        assert!(text.contains("per_ticks: 3"), "RON:\n{text}");
        assert!(text.contains("Coalesce"), "RON:\n{text}");
        // Round-trips losslessly.
        let back: Vec<Operator> = from_ron(&text).unwrap();
        assert_eq!(back, ops);
    }

    fn arb_intent_class() -> impl Strategy<Value = IntentClass> {
        prop_oneof![Just(IntentClass::Move), Just(IntentClass::Look)]
    }

    fn arb_scope() -> impl Strategy<Value = Scope> {
        prop_oneof![Just(Scope::Global), Just(Scope::PerActor)]
    }

    fn arb_kind() -> impl Strategy<Value = PresentationKind> {
        prop_oneof![
            Just(PresentationKind::RoomName),
            Just(PresentationKind::RoomDescription),
            Just(PresentationKind::Exits),
            Just(PresentationKind::Occupants),
            Just(PresentationKind::Line),
            Just(PresentationKind::Denied),
            Just(PresentationKind::Prompt),
        ]
    }

    fn arb_operator() -> impl Strategy<Value = Operator> {
        prop_oneof![
            (
                arb_intent_class(),
                1u64..20,
                arb_scope(),
                any::<Option<String>>()
            )
                .prop_map(|(on, per_ticks, scope, deny)| Operator::RateLimit {
                    on,
                    per_ticks,
                    scope,
                    deny
                }),
            (
                prop::collection::vec(arb_kind(), 0..7),
                1u64..5,
                arb_scope()
            )
                .prop_map(|(on, within_ticks, scope)| Operator::Coalesce {
                    on,
                    within_ticks,
                    scope
                }),
        ]
    }

    proptest! {
        #[test]
        fn any_operator_list_round_trips(ops in prop::collection::vec(arb_operator(), 0..8)) {
            let text = to_ron(&ops);
            let back: Vec<Operator> = from_ron(&text).unwrap();
            prop_assert_eq!(back, ops);
        }
    }
}
