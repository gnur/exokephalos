use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::{Conflict, Head, NoteRevision, RevisionId};

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedNote {
    pub winning_revision: RevisionId,
    pub visible: Option<NoteRevision>,
    pub conflict: Option<Conflict>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RevisionGraphError {
    #[error("stored revision ID does not match revision content: {0}")]
    IdentityMismatch(RevisionId),
    #[error("revision {revision} has missing predecessor {predecessor}")]
    MissingPredecessor {
        revision: RevisionId,
        predecessor: RevisionId,
    },
    #[error("revision {revision} references a predecessor from another note")]
    CrossNotePredecessor { revision: RevisionId },
}

pub fn validate_revision_graph(
    revisions: &BTreeMap<RevisionId, NoteRevision>,
) -> Result<(), RevisionGraphError> {
    for (revision_id, revision) in revisions {
        if revision.id().ok().as_ref() != Some(revision_id) {
            return Err(RevisionGraphError::IdentityMismatch(revision_id.clone()));
        }
        for predecessor_id in &revision.predecessors {
            let predecessor = revisions.get(predecessor_id).ok_or_else(|| {
                RevisionGraphError::MissingPredecessor {
                    revision: revision_id.clone(),
                    predecessor: predecessor_id.clone(),
                }
            })?;
            if predecessor.note_id != revision.note_id {
                return Err(RevisionGraphError::CrossNotePredecessor {
                    revision: revision_id.clone(),
                });
            }
        }
    }
    Ok(())
}

/// Resolve asserted per-author heads using HLC then actor identity ordering.
#[must_use]
pub fn resolve_heads(
    revisions: &BTreeMap<RevisionId, NoteRevision>,
    heads: &[Head],
) -> Option<ResolvedNote> {
    let mut candidates: Vec<(&RevisionId, &NoteRevision)> = heads
        .iter()
        .filter_map(|head| {
            let (revision_id, revision) = revisions.get_key_value(&head.revision_id)?;
            (head.note_id == revision.note_id).then_some((revision_id, revision))
        })
        .collect();

    candidates.sort_by(|(left_id, left), (right_id, right)| {
        left.hlc.cmp(&right.hlc).then_with(|| left_id.cmp(right_id))
    });
    let (winning_id, winning) = candidates.last().copied()?;

    let concurrent_revisions = candidates
        .iter()
        .filter_map(|(candidate_id, _)| {
            if *candidate_id != winning_id && !is_ancestor(candidate_id, winning_id, revisions) {
                Some((*candidate_id).clone())
            } else {
                None
            }
        })
        .collect::<BTreeSet<_>>();

    let conflict = (!concurrent_revisions.is_empty()).then(|| Conflict {
        note_id: winning.note_id.clone(),
        winning_revision: winning_id.clone(),
        concurrent_revisions,
    });

    Some(ResolvedNote {
        winning_revision: winning_id.clone(),
        visible: (!winning.deleted).then(|| winning.clone()),
        conflict,
    })
}

fn is_ancestor(
    possible_ancestor: &RevisionId,
    descendant: &RevisionId,
    revisions: &BTreeMap<RevisionId, NoteRevision>,
) -> bool {
    let mut pending = vec![descendant.clone()];
    let mut visited = BTreeSet::new();
    while let Some(current) = pending.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        let Some(revision) = revisions.get(&current) else {
            continue;
        };
        for predecessor in &revision.predecessors {
            if predecessor == possible_ancestor {
                return true;
            }
            pending.push(predecessor.clone());
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Frontmatter;
    use crate::{ActorId, Hlc, NoteId, SchemaVersion};

    fn revision(actor: &str, logical: u32, predecessors: BTreeSet<RevisionId>) -> NoteRevision {
        NoteRevision {
            schema: SchemaVersion(1),
            note_id: NoteId::new("note002"),
            frontmatter: Frontmatter::new(),
            body: actor.to_owned(),
            materialized_path: "notes/note.md".to_owned(),
            hlc: Hlc {
                physical_ms: 100,
                logical,
                actor_id: ActorId::new(actor),
            },
            author_id: ActorId::new(actor),
            predecessors,
            deleted: false,
        }
    }

    #[test]
    fn concurrent_heads_choose_deterministically_and_report_conflict() {
        let a = revision("a", 0, BTreeSet::new());
        let b = revision("b", 0, BTreeSet::new());
        let a_id = a.id().unwrap();
        let b_id = b.id().unwrap();
        let revisions = BTreeMap::from([(a_id.clone(), a), (b_id.clone(), b)]);
        let heads = vec![
            Head {
                note_id: NoteId::new("note002"),
                author_id: ActorId::new("a"),
                revision_id: a_id.clone(),
            },
            Head {
                note_id: NoteId::new("note002"),
                author_id: ActorId::new("b"),
                revision_id: b_id.clone(),
            },
        ];

        let resolved = resolve_heads(&revisions, &heads).unwrap();
        assert_eq!(resolved.winning_revision, b_id);
        assert_eq!(
            resolved.conflict.unwrap().concurrent_revisions,
            BTreeSet::from([a_id])
        );
    }

    #[test]
    fn predecessor_is_history_not_a_conflict() {
        let base = revision("a", 0, BTreeSet::new());
        let base_id = base.id().unwrap();
        let next = revision("a", 1, BTreeSet::from([base_id.clone()]));
        let next_id = next.id().unwrap();
        let revisions = BTreeMap::from([(base_id.clone(), base), (next_id.clone(), next)]);
        let heads = vec![
            Head {
                note_id: NoteId::new("note002"),
                author_id: ActorId::new("old"),
                revision_id: base_id,
            },
            Head {
                note_id: NoteId::new("note002"),
                author_id: ActorId::new("a"),
                revision_id: next_id,
            },
        ];
        assert!(
            resolve_heads(&revisions, &heads)
                .unwrap()
                .conflict
                .is_none()
        );
    }

    #[test]
    fn resolution_is_independent_of_head_arrival_order() {
        let a = revision("a", 0, BTreeSet::new());
        let b = revision("b", 0, BTreeSet::new());
        let c = revision("c", 0, BTreeSet::new());
        let a_id = a.id().unwrap();
        let b_id = b.id().unwrap();
        let c_id = c.id().unwrap();
        let revisions = BTreeMap::from([(a_id.clone(), a), (b_id.clone(), b), (c_id.clone(), c)]);
        let heads = [
            Head {
                note_id: NoteId::new("note002"),
                author_id: ActorId::new("a"),
                revision_id: a_id,
            },
            Head {
                note_id: NoteId::new("note002"),
                author_id: ActorId::new("b"),
                revision_id: b_id,
            },
            Head {
                note_id: NoteId::new("note002"),
                author_id: ActorId::new("c"),
                revision_id: c_id.clone(),
            },
        ];
        let permutations = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];
        for order in permutations {
            let ordered = order.map(|index| heads[index].clone());
            assert_eq!(
                resolve_heads(&revisions, &ordered)
                    .unwrap()
                    .winning_revision,
                c_id
            );
        }
    }

    #[test]
    fn graph_validation_rejects_missing_predecessors() {
        let revision = revision("a", 1, BTreeSet::from([RevisionId::new("missing")]));
        let revision_id = revision.id().unwrap();
        let graph = BTreeMap::from([(revision_id.clone(), revision)]);
        assert_eq!(
            validate_revision_graph(&graph),
            Err(RevisionGraphError::MissingPredecessor {
                revision: revision_id,
                predecessor: RevisionId::new("missing"),
            })
        );
    }
}
