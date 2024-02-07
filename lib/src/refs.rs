// Copyright 2021 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![allow(missing_docs)]

use itertools::EitherOrBoth;

use crate::backend::CommitId;
use crate::index::Index;
use crate::merge::{trivial_merge, Merge};
use crate::op_store::{RefTarget, RemoteRef};

/// Compares `refs1` and `refs2` targets, yields entry if they differ.
///
/// `refs1` and `refs2` must be sorted by `K`.
pub fn diff_named_ref_targets<'a, 'b, K: Ord>(
    refs1: impl IntoIterator<Item = (K, &'a RefTarget)>,
    refs2: impl IntoIterator<Item = (K, &'b RefTarget)>,
) -> impl Iterator<Item = (K, (&'a RefTarget, &'b RefTarget))> {
    iter_named_pairs(
        refs1,
        refs2,
        || RefTarget::absent_ref(),
        || RefTarget::absent_ref(),
    )
    .filter(|(_, (target1, target2))| target1 != target2)
}

/// Compares remote `refs1` and `refs2` pairs, yields entry if they differ.
///
/// `refs1` and `refs2` must be sorted by `K`.
pub fn diff_named_remote_refs<'a, 'b, K: Ord>(
    refs1: impl IntoIterator<Item = (K, &'a RemoteRef)>,
    refs2: impl IntoIterator<Item = (K, &'b RemoteRef)>,
) -> impl Iterator<Item = (K, (&'a RemoteRef, &'b RemoteRef))> {
    iter_named_pairs(
        refs1,
        refs2,
        || RemoteRef::absent_ref(),
        || RemoteRef::absent_ref(),
    )
    .filter(|(_, (ref1, ref2))| ref1 != ref2)
}

/// Iterates local `refs1` and remote `refs2` pairs by name.
///
/// `refs1` and `refs2` must be sorted by `K`.
pub fn iter_named_local_remote_refs<'a, 'b, K: Ord>(
    refs1: impl IntoIterator<Item = (K, &'a RefTarget)>,
    refs2: impl IntoIterator<Item = (K, &'b RemoteRef)>,
) -> impl Iterator<Item = (K, (&'a RefTarget, &'b RemoteRef))> {
    iter_named_pairs(
        refs1,
        refs2,
        || RefTarget::absent_ref(),
        || RemoteRef::absent_ref(),
    )
}

fn iter_named_pairs<K: Ord, V1, V2>(
    refs1: impl IntoIterator<Item = (K, V1)>,
    refs2: impl IntoIterator<Item = (K, V2)>,
    absent_ref1: impl Fn() -> V1,
    absent_ref2: impl Fn() -> V2,
) -> impl Iterator<Item = (K, (V1, V2))> {
    itertools::merge_join_by(refs1, refs2, |(name1, _), (name2, _)| name1.cmp(name2)).map(
        move |entry| match entry {
            EitherOrBoth::Both((name, target1), (_, target2)) => (name, (target1, target2)),
            EitherOrBoth::Left((name, target1)) => (name, (target1, absent_ref2())),
            EitherOrBoth::Right((name, target2)) => (name, (absent_ref1(), target2)),
        },
    )
}

pub fn merge_ref_targets(
    index: &dyn Index,
    left: &RefTarget,
    base: &RefTarget,
    right: &RefTarget,
) -> RefTarget {
    if let Some(&resolved) = trivial_merge(&[base], &[left, right]) {
        return resolved.clone();
    }

    let mut merge = Merge::from_vec(vec![
        left.as_merge().clone(),
        base.as_merge().clone(),
        right.as_merge().clone(),
    ])
    .flatten()
    .simplify();
    if !merge.is_resolved() {
        merge_ref_targets_non_trivial(index, &mut merge);
    }
    RefTarget::from_merge(merge)
}

pub fn merge_remote_refs(
    index: &dyn Index,
    left: &RemoteRef,
    base: &RemoteRef,
    right: &RemoteRef,
) -> RemoteRef {
    // Just merge target and state fields separately. Strictly speaking, merging
    // target-only change and state-only change shouldn't automatically mark the
    // new target as tracking. However, many faulty merges will end up in local
    // or remote target conflicts (since fast-forwardable move can be safely
    // "tracked"), and the conflicts will require user intervention anyway. So
    // there wouldn't be much reason to handle these merges precisely.
    let target = merge_ref_targets(index, &left.target, &base.target, &right.target);
    // Merged state shouldn't conflict atm since we only have two states, but if
    // it does, keep the original state. The choice is arbitrary.
    let state = *trivial_merge(&[base.state], &[left.state, right.state]).unwrap_or(&base.state);
    RemoteRef { target, state }
}

fn merge_ref_targets_non_trivial(index: &dyn Index, conflict: &mut Merge<Option<CommitId>>) {
    while let Some((remove_index, add_index)) = find_pair_to_remove(index, conflict) {
        conflict.swap_remove(remove_index, add_index);
    }
}

fn find_pair_to_remove(
    index: &dyn Index,
    conflict: &Merge<Option<CommitId>>,
) -> Option<(usize, usize)> {
    // If a "remove" is an ancestor of two different "adds" and one of the
    // "adds" is an ancestor of the other, then pick the descendant.
    for (add_index1, add1) in conflict.adds().enumerate() {
        for (add_index2, add2) in conflict.adds().enumerate().skip(add_index1 + 1) {
            // TODO: Instead of relying on the list order, maybe ((add1, add2), remove)
            // combination should be somehow weighted?
            let (add_index, add_id) = match (add1, add2) {
                (Some(id1), Some(id2)) if id1 == id2 => (add_index1, id1),
                (Some(id1), Some(id2)) if index.is_ancestor(id1, id2) => (add_index1, id1),
                (Some(id1), Some(id2)) if index.is_ancestor(id2, id1) => (add_index2, id2),
                _ => continue,
            };
            if let Some(remove_index) = conflict.removes().position(|remove| match remove {
                Some(id) => index.is_ancestor(id, add_id),
                None => true, // Absent ref can be considered a root
            }) {
                return Some((remove_index, add_index));
            }
        }
    }

    None
}

/// Pair of local and remote targets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocalAndRemoteRef<'a> {
    pub local_target: &'a RefTarget,
    pub remote_ref: &'a RemoteRef,
}

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct BranchPushUpdate {
    pub old_target: Option<CommitId>,
    pub new_target: Option<CommitId>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum BranchPushAction {
    Update(BranchPushUpdate),
    AlreadyMatches,
    LocalConflicted,
    RemoteConflicted,
    RemoteUntracked,
}

/// Figure out what changes (if any) need to be made to the remote when pushing
/// this branch.
pub fn classify_branch_push_action(targets: LocalAndRemoteRef) -> BranchPushAction {
    let local_target = targets.local_target;
    let remote_target = targets.remote_ref.tracking_target();
    if local_target == remote_target {
        BranchPushAction::AlreadyMatches
    } else if local_target.has_conflict() {
        BranchPushAction::LocalConflicted
    } else if remote_target.has_conflict() {
        BranchPushAction::RemoteConflicted
    } else if targets.remote_ref.is_present() && !targets.remote_ref.is_tracking() {
        BranchPushAction::RemoteUntracked
    } else {
        BranchPushAction::Update(BranchPushUpdate {
            old_target: remote_target.as_normal().cloned(),
            new_target: local_target.as_normal().cloned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op_store::RemoteRefState;

    fn new_remote_ref(target: RefTarget) -> RemoteRef {
        RemoteRef {
            target,
            state: RemoteRefState::New,
        }
    }

    fn tracking_remote_ref(target: RefTarget) -> RemoteRef {
        RemoteRef {
            target,
            state: RemoteRefState::Tracking,
        }
    }

    #[test]
    fn test_classify_branch_push_action_unchanged() {
        let commit_id1 = CommitId::from_hex("11");
        let targets = LocalAndRemoteRef {
            local_target: &RefTarget::normal(commit_id1.clone()),
            remote_ref: &tracking_remote_ref(RefTarget::normal(commit_id1)),
        };
        assert_eq!(
            classify_branch_push_action(targets),
            BranchPushAction::AlreadyMatches
        );
    }

    #[test]
    fn test_classify_branch_push_action_added() {
        let commit_id1 = CommitId::from_hex("11");
        let targets = LocalAndRemoteRef {
            local_target: &RefTarget::normal(commit_id1.clone()),
            remote_ref: RemoteRef::absent_ref(),
        };
        assert_eq!(
            classify_branch_push_action(targets),
            BranchPushAction::Update(BranchPushUpdate {
                old_target: None,
                new_target: Some(commit_id1),
            })
        );
    }

    #[test]
    fn test_classify_branch_push_action_removed() {
        let commit_id1 = CommitId::from_hex("11");
        let targets = LocalAndRemoteRef {
            local_target: RefTarget::absent_ref(),
            remote_ref: &tracking_remote_ref(RefTarget::normal(commit_id1.clone())),
        };
        assert_eq!(
            classify_branch_push_action(targets),
            BranchPushAction::Update(BranchPushUpdate {
                old_target: Some(commit_id1),
                new_target: None,
            })
        );
    }

    #[test]
    fn test_classify_branch_push_action_updated() {
        let commit_id1 = CommitId::from_hex("11");
        let commit_id2 = CommitId::from_hex("22");
        let targets = LocalAndRemoteRef {
            local_target: &RefTarget::normal(commit_id2.clone()),
            remote_ref: &tracking_remote_ref(RefTarget::normal(commit_id1.clone())),
        };
        assert_eq!(
            classify_branch_push_action(targets),
            BranchPushAction::Update(BranchPushUpdate {
                old_target: Some(commit_id1),
                new_target: Some(commit_id2),
            })
        );
    }

    #[test]
    fn test_classify_branch_push_action_removed_untracked() {
        // This is not RemoteUntracked error since non-tracking remote branches
        // have no relation to local branches, and there's nothing to push.
        let commit_id1 = CommitId::from_hex("11");
        let targets = LocalAndRemoteRef {
            local_target: RefTarget::absent_ref(),
            remote_ref: &new_remote_ref(RefTarget::normal(commit_id1.clone())),
        };
        assert_eq!(
            classify_branch_push_action(targets),
            BranchPushAction::AlreadyMatches
        );
    }

    #[test]
    fn test_classify_branch_push_action_updated_untracked() {
        let commit_id1 = CommitId::from_hex("11");
        let commit_id2 = CommitId::from_hex("22");
        let targets = LocalAndRemoteRef {
            local_target: &RefTarget::normal(commit_id2.clone()),
            remote_ref: &new_remote_ref(RefTarget::normal(commit_id1.clone())),
        };
        assert_eq!(
            classify_branch_push_action(targets),
            BranchPushAction::RemoteUntracked
        );
    }

    #[test]
    fn test_classify_branch_push_action_local_conflicted() {
        let commit_id1 = CommitId::from_hex("11");
        let commit_id2 = CommitId::from_hex("22");
        let targets = LocalAndRemoteRef {
            local_target: &RefTarget::from_legacy_form([], [commit_id1.clone(), commit_id2]),
            remote_ref: &tracking_remote_ref(RefTarget::normal(commit_id1)),
        };
        assert_eq!(
            classify_branch_push_action(targets),
            BranchPushAction::LocalConflicted
        );
    }

    #[test]
    fn test_classify_branch_push_action_remote_conflicted() {
        let commit_id1 = CommitId::from_hex("11");
        let commit_id2 = CommitId::from_hex("22");
        let targets = LocalAndRemoteRef {
            local_target: &RefTarget::normal(commit_id1.clone()),
            remote_ref: &tracking_remote_ref(RefTarget::from_legacy_form(
                [],
                [commit_id1, commit_id2],
            )),
        };
        assert_eq!(
            classify_branch_push_action(targets),
            BranchPushAction::RemoteConflicted
        );
    }
}
