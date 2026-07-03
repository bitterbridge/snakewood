use serde::{Deserialize, Serialize};

use crate::Direction;

/// Semantic role of a span of text. A growing vocabulary; the core emits only
/// `Default`/`Actor` in M2. Transports map roles to medium-specific styling.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Default,
    Actor,
}

/// A run of text carrying one semantic role.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub role: Role,
}

impl Span {
    pub fn plain(text: impl Into<String>) -> Span {
        Span {
            text: text.into(),
            role: Role::Default,
        }
    }
    pub fn actor(text: impl Into<String>) -> Span {
        Span {
            text: text.into(),
            role: Role::Actor,
        }
    }
}

/// A single `Default`-role span — the common case for plain/authored text.
pub fn plain_text(text: impl Into<String>) -> Vec<Span> {
    vec![Span::plain(text)]
}

/// A semantic unit of output. Transports render these (telnet) or pass them as
/// structured data (the command API); the core never emits formatted text.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum PresentationNode {
    RoomName(String),
    RoomDescription(Vec<Span>),
    Exits(Vec<Direction>),
    Occupants(Vec<Span>),
    Line(Vec<Span>),
    Denied(Vec<Span>),
    Prompt,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn presentation_node_round_trips_via_serde() {
        let node = PresentationNode::Exits(vec![Direction::North, Direction::Down]);
        let text = ron::ser::to_string(&node).unwrap();
        let back: PresentationNode = ron::from_str(&text).unwrap();
        assert_eq!(back, node);

        let line = PresentationNode::Line(plain_text("hello"));
        let back2: PresentationNode = ron::from_str(&ron::ser::to_string(&line).unwrap()).unwrap();
        assert_eq!(back2, line);
    }

    #[test]
    fn span_helpers_and_roles_round_trip() {
        assert_eq!(
            Span::plain("hi"),
            Span {
                text: "hi".to_string(),
                role: Role::Default
            }
        );
        assert_eq!(
            Span::actor("a goblin"),
            Span {
                text: "a goblin".to_string(),
                role: Role::Actor
            }
        );
        assert_eq!(plain_text("x"), vec![Span::plain("x")]);

        // serde round-trip for a styled span vec
        let spans = vec![Span::plain("You see "), Span::actor("a goblin")];
        let text = ron::ser::to_string(&spans).unwrap();
        let back: Vec<Span> = ron::from_str(&text).unwrap();
        assert_eq!(back, spans);
    }

    fn arb_role() -> impl Strategy<Value = Role> {
        prop_oneof![Just(Role::Default), Just(Role::Actor)]
    }

    fn arb_span() -> impl Strategy<Value = Span> {
        (any::<String>(), arb_role()).prop_map(|(text, role)| Span { text, role })
    }

    proptest! {
        #[test]
        fn any_span_vec_round_trips(spans in prop::collection::vec(arb_span(), 0..8)) {
            let text = ron::ser::to_string(&spans).unwrap();
            let back: Vec<Span> = ron::from_str(&text).unwrap();
            prop_assert_eq!(back, spans);
        }
    }
}
