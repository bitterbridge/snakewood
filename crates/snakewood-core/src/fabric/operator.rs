use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::fabric::Intent;
use crate::{EntityId, PresentationNode};

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

/// The outcome of a rate-limit admission check.
#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    Admit,
    Drop { deny: Option<String> },
}

/// Runtime admission state for all `RateLimit` operators. Keyed by
/// `(operator index, scope key)` -> last admitted tick. Held by the host
/// (runtime-only, never persisted).
#[derive(Debug, Default, Clone)]
pub struct RateLimiterState {
    last_admitted: BTreeMap<(usize, Option<EntityId>), u64>,
}

fn scope_key(scope: Scope, actor: &EntityId) -> Option<EntityId> {
    match scope {
        Scope::Global => None,
        Scope::PerActor => Some(actor.clone()),
    }
}

impl RateLimiterState {
    /// Decide whether an intent of `class` by `actor` is admitted at `tick`,
    /// against the `RateLimit` operators in `operators`. Records the admission
    /// (updating internal state) only when admitted. On a block, returns the
    /// first blocking operator's `deny` text.
    pub fn admit(
        &mut self,
        operators: &[Operator],
        class: IntentClass,
        actor: &EntityId,
        tick: u64,
    ) -> Admission {
        // Pass 1: is any matching operator currently blocking?
        for (idx, op) in operators.iter().enumerate() {
            if let Operator::RateLimit {
                on,
                per_ticks,
                scope,
                deny,
            } = op
            {
                if *on != class {
                    continue;
                }
                let key = (idx, scope_key(*scope, actor));
                if let Some(&last) = self.last_admitted.get(&key) {
                    if tick < last + *per_ticks {
                        return Admission::Drop { deny: deny.clone() };
                    }
                }
            }
        }
        // Pass 2: admit — record the admission for every matching operator.
        for (idx, op) in operators.iter().enumerate() {
            if let Operator::RateLimit { on, scope, .. } = op {
                if *on != class {
                    continue;
                }
                self.last_admitted
                    .insert((idx, scope_key(*scope, actor)), tick);
            }
        }
        Admission::Admit
    }
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

    fn move_rl(per_ticks: u64, scope: Scope) -> Vec<Operator> {
        vec![Operator::RateLimit {
            on: IntentClass::Move,
            per_ticks,
            scope,
            deny: Some("Slow down.".to_string()),
        }]
    }

    #[test]
    fn rate_limit_admits_one_per_window_then_drops() {
        let ops = move_rl(3, Scope::PerActor);
        let a = EntityId::new("player/anon-0").unwrap();
        let mut rl = RateLimiterState::default();
        // First admission at tick 0 always allowed.
        assert!(matches!(
            rl.admit(&ops, IntentClass::Move, &a, 0),
            Admission::Admit
        ));
        // Within the window (ticks 1, 2) -> dropped with the configured deny.
        match rl.admit(&ops, IntentClass::Move, &a, 1) {
            Admission::Drop { deny } => assert_eq!(deny.as_deref(), Some("Slow down.")),
            Admission::Admit => panic!("should be dropped inside window"),
        }
        assert!(matches!(
            rl.admit(&ops, IntentClass::Move, &a, 2),
            Admission::Drop { .. }
        ));
        // At tick 3 the window reopens.
        assert!(matches!(
            rl.admit(&ops, IntentClass::Move, &a, 3),
            Admission::Admit
        ));
    }

    #[test]
    fn per_actor_scope_has_independent_buckets() {
        let ops = move_rl(3, Scope::PerActor);
        let a = EntityId::new("player/anon-0").unwrap();
        let b = EntityId::new("player/anon-1").unwrap();
        let mut rl = RateLimiterState::default();
        assert!(matches!(
            rl.admit(&ops, IntentClass::Move, &a, 0),
            Admission::Admit
        ));
        // b is a different bucket -> admitted at the same tick.
        assert!(matches!(
            rl.admit(&ops, IntentClass::Move, &b, 0),
            Admission::Admit
        ));
        // a is still limited.
        assert!(matches!(
            rl.admit(&ops, IntentClass::Move, &a, 1),
            Admission::Drop { .. }
        ));
    }

    #[test]
    fn global_scope_shares_one_bucket() {
        let ops = move_rl(3, Scope::Global);
        let a = EntityId::new("player/anon-0").unwrap();
        let b = EntityId::new("player/anon-1").unwrap();
        let mut rl = RateLimiterState::default();
        assert!(matches!(
            rl.admit(&ops, IntentClass::Move, &a, 0),
            Admission::Admit
        ));
        // Same global bucket -> b is dropped even though it's a different actor.
        assert!(matches!(
            rl.admit(&ops, IntentClass::Move, &b, 1),
            Admission::Drop { .. }
        ));
    }

    #[test]
    fn non_matching_class_is_always_admitted() {
        let ops = move_rl(3, Scope::PerActor);
        let a = EntityId::new("player/anon-0").unwrap();
        let mut rl = RateLimiterState::default();
        // Look is not rate-limited by a Move operator.
        assert!(matches!(
            rl.admit(&ops, IntentClass::Look, &a, 0),
            Admission::Admit
        ));
        assert!(matches!(
            rl.admit(&ops, IntentClass::Look, &a, 0),
            Admission::Admit
        ));
    }
}
