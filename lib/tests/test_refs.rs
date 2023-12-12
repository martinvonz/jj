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

use jj_lib::merge::Merge;
use jj_lib::op_store::RefTarget;
use jj_lib::refs::merge_ref_targets;
use jj_lib::repo::Repo;
use testutils::{CommitGraphBuilder, TestWorkspace};

#[test]
fn test_merge_ref_targets() {
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init(&settings);
    let repo = &test_workspace.repo;

    // 6 7
    // |/
    // 5
    // | 3 4
    // | |/
    // | 2
    // |/
    // 1
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit2]);
    let commit5 = graph_builder.commit_with_parents(&[&commit1]);
    let commit6 = graph_builder.commit_with_parents(&[&commit5]);
    let commit7 = graph_builder.commit_with_parents(&[&commit5]);
    let repo = tx.commit("test");

    let target1 = RefTarget::normal(commit1.id().clone());
    let target2 = RefTarget::normal(commit2.id().clone());
    let target3 = RefTarget::normal(commit3.id().clone());
    let target4 = RefTarget::normal(commit4.id().clone());
    let target5 = RefTarget::normal(commit5.id().clone());
    let target6 = RefTarget::normal(commit6.id().clone());
    let _target7 = RefTarget::normal(commit7.id().clone());

    let index = repo.index();

    // Left moved forward
    assert_eq!(
        merge_ref_targets(index, &target3, &target1, &target1),
        target3
    );

    // Right moved forward
    assert_eq!(
        merge_ref_targets(index, &target1, &target1, &target3),
        target3
    );

    // Left moved backward
    assert_eq!(
        merge_ref_targets(index, &target1, &target3, &target3),
        target1
    );

    // Right moved backward
    assert_eq!(
        merge_ref_targets(index, &target3, &target3, &target1),
        target1
    );

    // Left moved sideways
    assert_eq!(
        merge_ref_targets(index, &target4, &target3, &target3),
        target4
    );

    // Right moved sideways
    assert_eq!(
        merge_ref_targets(index, &target3, &target3, &target4),
        target4
    );

    // Both added same target
    assert_eq!(
        merge_ref_targets(index, &target3, RefTarget::absent_ref(), &target3),
        target3
    );

    // Left added target, right added descendant target
    assert_eq!(
        merge_ref_targets(index, &target2, RefTarget::absent_ref(), &target3),
        target3
    );

    // Right added target, left added descendant target
    assert_eq!(
        merge_ref_targets(index, &target3, RefTarget::absent_ref(), &target2),
        target3
    );

    // Both moved forward to same target
    assert_eq!(
        merge_ref_targets(index, &target3, &target1, &target3),
        target3
    );

    // Both moved forward, left moved further
    assert_eq!(
        merge_ref_targets(index, &target3, &target1, &target2),
        target3
    );

    // Both moved forward, right moved further
    assert_eq!(
        merge_ref_targets(index, &target2, &target1, &target3),
        target3
    );

    // Left and right moved forward to divergent targets
    assert_eq!(
        merge_ref_targets(index, &target3, &target1, &target4),
        RefTarget::from_legacy_form(
            [commit1.id().clone()],
            [commit3.id().clone(), commit4.id().clone()]
        )
    );

    // Left moved back, right moved forward
    assert_eq!(
        merge_ref_targets(index, &target1, &target2, &target3),
        RefTarget::from_legacy_form(
            [commit2.id().clone()],
            [commit1.id().clone(), commit3.id().clone()]
        )
    );

    // Right moved back, left moved forward
    assert_eq!(
        merge_ref_targets(index, &target3, &target2, &target1),
        RefTarget::from_legacy_form(
            [commit2.id().clone()],
            [commit3.id().clone(), commit1.id().clone()]
        )
    );

    // Left removed
    assert_eq!(
        merge_ref_targets(index, RefTarget::absent_ref(), &target3, &target3),
        RefTarget::absent()
    );

    // Right removed
    assert_eq!(
        merge_ref_targets(index, &target3, &target3, RefTarget::absent_ref()),
        RefTarget::absent()
    );

    // Left removed, right moved forward
    assert_eq!(
        merge_ref_targets(index, RefTarget::absent_ref(), &target1, &target3),
        RefTarget::from_merge(Merge::from_vec(vec![
            None,
            Some(commit1.id().clone()),
            Some(commit3.id().clone()),
        ]))
    );

    // Right removed, left moved forward
    assert_eq!(
        merge_ref_targets(index, &target3, &target1, RefTarget::absent_ref()),
        RefTarget::from_merge(Merge::from_vec(vec![
            Some(commit3.id().clone()),
            Some(commit1.id().clone()),
            None,
        ]))
    );

    // Left became conflicted, right moved forward
    assert_eq!(
        merge_ref_targets(
            index,
            &RefTarget::from_legacy_form(
                [commit2.id().clone()],
                [commit3.id().clone(), commit4.id().clone()]
            ),
            &target1,
            &target3
        ),
        // TODO: "removes" should have commit 2, just like it does in the next test case
        RefTarget::from_legacy_form(
            [commit1.id().clone()],
            [commit3.id().clone(), commit4.id().clone()]
        )
    );

    // Right became conflicted, left moved forward
    assert_eq!(
        merge_ref_targets(
            index,
            &target3,
            &target1,
            &RefTarget::from_legacy_form(
                [commit2.id().clone()],
                [commit3.id().clone(), commit4.id().clone()]
            ),
        ),
        RefTarget::from_legacy_form(
            [commit2.id().clone()],
            [commit4.id().clone(), commit3.id().clone()]
        )
    );

    // Existing conflict on left, right moves an "add" sideways
    //
    // Under the hood, the conflict is simplified as below:
    // ```
    // 3 4 5      3 4 5      5 4
    //  2 /   =>   2 3   =>   2
    //   3
    // ```
    assert_eq!(
        merge_ref_targets(
            index,
            &RefTarget::from_legacy_form(
                [commit2.id().clone()],
                [commit3.id().clone(), commit4.id().clone()]
            ),
            &target3,
            &target5
        ),
        RefTarget::from_legacy_form(
            [commit2.id().clone()],
            [commit5.id().clone(), commit4.id().clone()]
        )
    );

    // Existing conflict on right, left moves an "add" sideways
    //
    // Under the hood, the conflict is simplified as below:
    // ```
    // 5 3 4      5 3 4      5 4
    //  \ 2   =>   3 2   =>   2
    //   3
    // ```
    assert_eq!(
        merge_ref_targets(
            index,
            &target5,
            &target3,
            &RefTarget::from_legacy_form(
                [commit2.id().clone()],
                [commit3.id().clone(), commit4.id().clone()]
            ),
        ),
        RefTarget::from_legacy_form(
            [commit2.id().clone()],
            [commit5.id().clone(), commit4.id().clone()]
        )
    );

    // Existing conflict on left, right moves an "add" backwards, past point of
    // divergence
    //
    // Under the hood, the conflict is simplified as below:
    // ```
    // 3 4 1      3 4 1      1 4
    //  2 /   =>   2 3   =>   2
    //   3
    // ```
    assert_eq!(
        merge_ref_targets(
            index,
            &RefTarget::from_legacy_form(
                [commit2.id().clone()],
                [commit3.id().clone(), commit4.id().clone()]
            ),
            &target3,
            &target1
        ),
        RefTarget::from_legacy_form(
            [commit2.id().clone()],
            [commit1.id().clone(), commit4.id().clone()]
        )
    );

    // Existing conflict on right, left moves an "add" backwards, past point of
    // divergence
    //
    // Under the hood, the conflict is simplified as below:
    // ```
    // 1 3 4      1 3 4      1 4
    //  \ 2   =>   3 2   =>   2
    //   3
    // ```
    assert_eq!(
        merge_ref_targets(
            index,
            &target1,
            &target3,
            &RefTarget::from_legacy_form(
                [commit2.id().clone()],
                [commit3.id().clone(), commit4.id().clone()]
            ),
        ),
        RefTarget::from_legacy_form(
            [commit2.id().clone()],
            [commit1.id().clone(), commit4.id().clone()]
        )
    );

    // Existing conflict on left, right undoes one side of conflict
    assert_eq!(
        merge_ref_targets(
            index,
            &RefTarget::from_legacy_form(
                [commit2.id().clone()],
                [commit3.id().clone(), commit4.id().clone()]
            ),
            &target3,
            &target2
        ),
        target4
    );

    // Existing conflict on right, left undoes one side of conflict
    assert_eq!(
        merge_ref_targets(
            index,
            &target2,
            &target3,
            &RefTarget::from_legacy_form(
                [commit2.id().clone()],
                [commit3.id().clone(), commit4.id().clone()]
            ),
        ),
        target4
    );

    // Existing conflict on left, right makes unrelated update
    assert_eq!(
        merge_ref_targets(
            index,
            &RefTarget::from_legacy_form(
                [commit2.id().clone()],
                [commit3.id().clone(), commit4.id().clone()]
            ),
            &target5,
            &target6
        ),
        RefTarget::from_legacy_form(
            [commit2.id().clone(), commit5.id().clone()],
            [
                commit3.id().clone(),
                commit4.id().clone(),
                commit6.id().clone()
            ]
        )
    );

    // Existing conflict on right, left makes unrelated update
    assert_eq!(
        merge_ref_targets(
            index,
            &target6,
            &target5,
            &RefTarget::from_legacy_form(
                [commit2.id().clone()],
                [commit3.id().clone(), commit4.id().clone()]
            ),
        ),
        RefTarget::from_legacy_form(
            [commit5.id().clone(), commit2.id().clone()],
            [
                commit6.id().clone(),
                commit3.id().clone(),
                commit4.id().clone()
            ]
        )
    );
}
