use crate::fabric::{Candidate, Outcome};
use crate::EntityId;

/// The order-independent outcome of the Guard resolve pass.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Denied,
    Allowed { destination: EntityId },
    Unresolved,
}

/// Compare two candidates for salience. Returns true if `a` is MORE salient than `b`.
fn more_salient(a: &Candidate, b: &Candidate) -> bool {
    let (ra, rb) = (a.band.rank(), b.band.rank());
    if ra != rb {
        return ra < rb; // lower rank = more salient (Participant first)
    }
    if a.priority != b.priority {
        return a.priority > b.priority; // higher priority wins
    }
    // stable tie-break by self_id: Some(smaller id) beats larger; Some beats None
    match (&a.self_id, &b.self_id) {
        (Some(x), Some(y)) => x < y,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => false,
    }
}

/// Most salient candidate in `candidates`, or None if empty.
pub fn salient(candidates: &[Candidate]) -> Option<&Candidate> {
    candidates.iter().reduce(|best, c| if more_salient(c, best) { c } else { best })
}

/// Set-based outcome. Deny beats Traverse beats nothing.
pub fn resolve(candidates: &[Candidate]) -> Decision {
    if candidates.iter().any(|c| c.outcome == Outcome::Block) {
        return Decision::Denied;
    }
    let traversers: Vec<Candidate> = candidates
        .iter()
        .filter(|c| matches!(c.outcome, Outcome::Traverse(_)))
        .cloned()
        .collect();
    if let Some(winner) = salient(&traversers) {
        if let Outcome::Traverse(dest) = &winner.outcome {
            return Decision::Allowed { destination: dest.clone() };
        }
    }
    Decision::Unresolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fabric::Band;

    fn traverse(band: Band, priority: i32, self_id: Option<&str>, dest: &str) -> Candidate {
        Candidate {
            band,
            priority,
            self_id: self_id.map(|s| EntityId::new(s).unwrap()),
            outcome: Outcome::Traverse(EntityId::new(dest).unwrap()),
            effects: Vec::new(),
        }
    }

    fn block(band: Band, priority: i32, self_id: Option<&str>) -> Candidate {
        Candidate {
            band,
            priority,
            self_id: self_id.map(|s| EntityId::new(s).unwrap()),
            outcome: Outcome::Block,
            effects: Vec::new(),
        }
    }

    #[test]
    fn any_block_denies_even_with_traverse_present() {
        let cands = vec![
            traverse(Band::Structure, 0, None, "snakewood/old-well"),
            block(Band::Participant, 0, Some("snakewood/mob/goblin#1")),
        ];
        assert_eq!(resolve(&cands), Decision::Denied);
    }

    #[test]
    fn lone_traverse_allows_to_destination() {
        let cands = vec![traverse(Band::Structure, 0, None, "snakewood/old-well")];
        assert_eq!(
            resolve(&cands),
            Decision::Allowed { destination: EntityId::new("snakewood/old-well").unwrap() }
        );
    }

    #[test]
    fn empty_is_unresolved() {
        assert_eq!(resolve(&[]), Decision::Unresolved);
    }

    #[test]
    fn salient_prefers_participant_over_structure() {
        let cands = vec![
            block(Band::Structure, 0, None),
            block(Band::Participant, 0, Some("snakewood/mob/goblin#1")),
        ];
        let s = salient(&cands).unwrap();
        assert_eq!(s.band, Band::Participant);
    }

    #[test]
    fn salient_uses_priority_within_band() {
        let cands = vec![
            block(Band::Participant, 1, Some("snakewood/mob/a#1")),
            block(Band::Participant, 5, Some("snakewood/mob/b#1")),
        ];
        let s = salient(&cands).unwrap();
        assert_eq!(s.priority, 5);
    }

    #[test]
    fn resolve_picks_salient_traverser_destination_not_first() {
        // Two competing open exits: the more-salient (Participant) traverser wins
        // the destination, even though it is NOT first in the slice. This guards
        // against a regression that takes `traversers.first()` instead of
        // `salient(&traversers)`.
        let cands = vec![
            traverse(Band::Structure, 0, None, "snakewood/via-structure"),
            traverse(Band::Participant, 0, Some("snakewood/mob/guide#1"), "snakewood/via-participant"),
        ];
        assert_eq!(
            resolve(&cands),
            Decision::Allowed { destination: EntityId::new("snakewood/via-participant").unwrap() }
        );
    }
}
