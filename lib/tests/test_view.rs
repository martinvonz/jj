// Copyright 2020 The Jujutsu Authors
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

use std::collections::BTreeMap;

use jj_lib::op_store::{BranchTarget, RefTarget, RemoteRef, RemoteRefState, WorkspaceId};
use jj_lib::repo::Repo;
use maplit::{btreemap, hashset};
use test_case::test_case;
use testutils::{
    commit_transactions, create_random_commit, write_random_commit, CommitGraphBuilder, TestRepo,
};

#[test]
fn test_heads_empty() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    assert_eq!(
        *repo.view().heads(),
        hashset! {repo.store().root_commit_id().clone()}
    );
}

#[test]
fn test_heads_fork() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let mut tx = repo.start_transaction(&settings);

    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let initial = graph_builder.initial_commit();
    let child1 = graph_builder.commit_with_parents(&[&initial]);
    let child2 = graph_builder.commit_with_parents(&[&initial]);
    let repo = tx.commit("test");

    assert_eq!(
        *repo.view().heads(),
        hashset! {
            child1.id().clone(),
            child2.id().clone(),
        }
    );
}

#[test]
fn test_heads_merge() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;
    let mut tx = repo.start_transaction(&settings);

    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let initial = graph_builder.initial_commit();
    let child1 = graph_builder.commit_with_parents(&[&initial]);
    let child2 = graph_builder.commit_with_parents(&[&initial]);
    let merge = graph_builder.commit_with_parents(&[&child1, &child2]);
    let repo = tx.commit("test");

    assert_eq!(*repo.view().heads(), hashset! {merge.id().clone()});
}

#[test]
fn test_merge_views_heads() {
    // Tests merging of the view's heads (by performing concurrent operations).
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings);
    let mut_repo = tx.mut_repo();
    let head_unchanged = write_random_commit(mut_repo, &settings);
    let head_remove_tx1 = write_random_commit(mut_repo, &settings);
    let head_remove_tx2 = write_random_commit(mut_repo, &settings);
    let repo = tx.commit("test");

    let mut tx1 = repo.start_transaction(&settings);
    tx1.mut_repo().remove_head(head_remove_tx1.id());
    let head_add_tx1 = write_random_commit(tx1.mut_repo(), &settings);

    let mut tx2 = repo.start_transaction(&settings);
    tx2.mut_repo().remove_head(head_remove_tx2.id());
    let head_add_tx2 = write_random_commit(tx2.mut_repo(), &settings);

    let repo = commit_transactions(&settings, vec![tx1, tx2]);

    let expected_heads = hashset! {
        head_unchanged.id().clone(),
        head_add_tx1.id().clone(),
        head_add_tx2.id().clone(),
    };
    assert_eq!(repo.view().heads(), &expected_heads);
}

#[test]
fn test_merge_views_checkout() {
    // Tests merging of the view's checkout (by performing concurrent operations).
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Workspace 1 gets updated in both transactions.
    // Workspace 2 gets updated only in tx1.
    // Workspace 3 gets updated only in tx2.
    // Workspace 4 gets deleted in tx1 and modified in tx2.
    // Workspace 5 gets deleted in tx2 and modified in tx1.
    // Workspace 6 gets added in tx1.
    // Workspace 7 gets added in tx2.
    let mut initial_tx = repo.start_transaction(&settings);
    let commit1 = write_random_commit(initial_tx.mut_repo(), &settings);
    let commit2 = write_random_commit(initial_tx.mut_repo(), &settings);
    let commit3 = write_random_commit(initial_tx.mut_repo(), &settings);
    let ws1_id = WorkspaceId::new("ws1".to_string());
    let ws2_id = WorkspaceId::new("ws2".to_string());
    let ws3_id = WorkspaceId::new("ws3".to_string());
    let ws4_id = WorkspaceId::new("ws4".to_string());
    let ws5_id = WorkspaceId::new("ws5".to_string());
    let ws6_id = WorkspaceId::new("ws6".to_string());
    let ws7_id = WorkspaceId::new("ws7".to_string());
    initial_tx
        .mut_repo()
        .set_wc_commit(ws1_id.clone(), commit1.id().clone())
        .unwrap();
    initial_tx
        .mut_repo()
        .set_wc_commit(ws2_id.clone(), commit1.id().clone())
        .unwrap();
    initial_tx
        .mut_repo()
        .set_wc_commit(ws3_id.clone(), commit1.id().clone())
        .unwrap();
    initial_tx
        .mut_repo()
        .set_wc_commit(ws4_id.clone(), commit1.id().clone())
        .unwrap();
    initial_tx
        .mut_repo()
        .set_wc_commit(ws5_id.clone(), commit1.id().clone())
        .unwrap();
    let repo = initial_tx.commit("test");

    let mut tx1 = repo.start_transaction(&settings);
    tx1.mut_repo()
        .set_wc_commit(ws1_id.clone(), commit2.id().clone())
        .unwrap();
    tx1.mut_repo()
        .set_wc_commit(ws2_id.clone(), commit2.id().clone())
        .unwrap();
    tx1.mut_repo().remove_wc_commit(&ws4_id);
    tx1.mut_repo()
        .set_wc_commit(ws5_id.clone(), commit2.id().clone())
        .unwrap();
    tx1.mut_repo()
        .set_wc_commit(ws6_id.clone(), commit2.id().clone())
        .unwrap();

    let mut tx2 = repo.start_transaction(&settings);
    tx2.mut_repo()
        .set_wc_commit(ws1_id.clone(), commit3.id().clone())
        .unwrap();
    tx2.mut_repo()
        .set_wc_commit(ws3_id.clone(), commit3.id().clone())
        .unwrap();
    tx2.mut_repo()
        .set_wc_commit(ws4_id.clone(), commit3.id().clone())
        .unwrap();
    tx2.mut_repo().remove_wc_commit(&ws5_id);
    tx2.mut_repo()
        .set_wc_commit(ws7_id.clone(), commit3.id().clone())
        .unwrap();

    let repo = commit_transactions(&settings, vec![tx1, tx2]);

    // We currently arbitrarily pick the first transaction's working-copy commit
    // (first by transaction end time).
    assert_eq!(repo.view().get_wc_commit_id(&ws1_id), Some(commit2.id()));
    assert_eq!(repo.view().get_wc_commit_id(&ws2_id), Some(commit2.id()));
    assert_eq!(repo.view().get_wc_commit_id(&ws3_id), Some(commit3.id()));
    assert_eq!(repo.view().get_wc_commit_id(&ws4_id), None);
    assert_eq!(repo.view().get_wc_commit_id(&ws5_id), None);
    assert_eq!(repo.view().get_wc_commit_id(&ws6_id), Some(commit2.id()));
    assert_eq!(repo.view().get_wc_commit_id(&ws7_id), Some(commit3.id()));
}

#[test]
fn test_merge_views_branches() {
    // Tests merging of branches (by performing concurrent operations). See
    // test_refs.rs for tests of merging of individual ref targets.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings);
    let mut_repo = tx.mut_repo();
    let main_branch_local_tx0 = write_random_commit(mut_repo, &settings);
    let main_branch_origin_tx0 = write_random_commit(mut_repo, &settings);
    let main_branch_alternate_tx0 = write_random_commit(mut_repo, &settings);
    let main_branch_origin_tx0_remote_ref = RemoteRef {
        target: RefTarget::normal(main_branch_origin_tx0.id().clone()),
        state: RemoteRefState::New,
    };
    let main_branch_alternate_tx0_remote_ref = RemoteRef {
        target: RefTarget::normal(main_branch_alternate_tx0.id().clone()),
        state: RemoteRefState::Tracking,
    };
    mut_repo.set_local_branch_target(
        "main",
        RefTarget::normal(main_branch_local_tx0.id().clone()),
    );
    mut_repo.set_remote_branch("main", "origin", main_branch_origin_tx0_remote_ref);
    mut_repo.set_remote_branch(
        "main",
        "alternate",
        main_branch_alternate_tx0_remote_ref.clone(),
    );
    let feature_branch_local_tx0 = write_random_commit(mut_repo, &settings);
    mut_repo.set_local_branch_target(
        "feature",
        RefTarget::normal(feature_branch_local_tx0.id().clone()),
    );
    let repo = tx.commit("test");

    let mut tx1 = repo.start_transaction(&settings);
    let main_branch_local_tx1 = write_random_commit(tx1.mut_repo(), &settings);
    tx1.mut_repo().set_local_branch_target(
        "main",
        RefTarget::normal(main_branch_local_tx1.id().clone()),
    );
    let feature_branch_tx1 = write_random_commit(tx1.mut_repo(), &settings);
    tx1.mut_repo().set_local_branch_target(
        "feature",
        RefTarget::normal(feature_branch_tx1.id().clone()),
    );

    let mut tx2 = repo.start_transaction(&settings);
    let main_branch_local_tx2 = write_random_commit(tx2.mut_repo(), &settings);
    let main_branch_origin_tx2 = write_random_commit(tx2.mut_repo(), &settings);
    let main_branch_origin_tx2_remote_ref = RemoteRef {
        target: RefTarget::normal(main_branch_origin_tx2.id().clone()),
        state: RemoteRefState::Tracking,
    };
    tx2.mut_repo().set_local_branch_target(
        "main",
        RefTarget::normal(main_branch_local_tx2.id().clone()),
    );
    tx2.mut_repo()
        .set_remote_branch("main", "origin", main_branch_origin_tx2_remote_ref.clone());

    let repo = commit_transactions(&settings, vec![tx1, tx2]);
    let expected_main_branch = BranchTarget {
        local_target: &RefTarget::from_legacy_form(
            [main_branch_local_tx0.id().clone()],
            [
                main_branch_local_tx1.id().clone(),
                main_branch_local_tx2.id().clone(),
            ],
        ),
        remote_refs: vec![
            ("alternate", &main_branch_alternate_tx0_remote_ref),
            // tx1: unchanged, tx2: new -> tracking
            ("origin", &main_branch_origin_tx2_remote_ref),
        ],
    };
    let expected_feature_branch = BranchTarget {
        local_target: &RefTarget::normal(feature_branch_tx1.id().clone()),
        remote_refs: vec![],
    };
    assert_eq!(
        repo.view().branches().collect::<BTreeMap<_, _>>(),
        btreemap! {
            "main" => expected_main_branch,
            "feature" => expected_feature_branch,
        }
    );
}

#[test]
fn test_merge_views_tags() {
    // Tests merging of tags (by performing concurrent operations). See
    // test_refs.rs for tests of merging of individual ref targets.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings);
    let mut_repo = tx.mut_repo();
    let v1_tx0 = write_random_commit(mut_repo, &settings);
    mut_repo.set_tag_target("v1.0", RefTarget::normal(v1_tx0.id().clone()));
    let v2_tx0 = write_random_commit(mut_repo, &settings);
    mut_repo.set_tag_target("v2.0", RefTarget::normal(v2_tx0.id().clone()));
    let repo = tx.commit("test");

    let mut tx1 = repo.start_transaction(&settings);
    let v1_tx1 = write_random_commit(tx1.mut_repo(), &settings);
    tx1.mut_repo()
        .set_tag_target("v1.0", RefTarget::normal(v1_tx1.id().clone()));
    let v2_tx1 = write_random_commit(tx1.mut_repo(), &settings);
    tx1.mut_repo()
        .set_tag_target("v2.0", RefTarget::normal(v2_tx1.id().clone()));

    let mut tx2 = repo.start_transaction(&settings);
    let v1_tx2 = write_random_commit(tx2.mut_repo(), &settings);
    tx2.mut_repo()
        .set_tag_target("v1.0", RefTarget::normal(v1_tx2.id().clone()));

    let repo = commit_transactions(&settings, vec![tx1, tx2]);
    let expected_v1 = RefTarget::from_legacy_form(
        [v1_tx0.id().clone()],
        [v1_tx1.id().clone(), v1_tx2.id().clone()],
    );
    let expected_v2 = RefTarget::normal(v2_tx1.id().clone());
    assert_eq!(
        repo.view().tags(),
        &btreemap! {
            "v1.0".to_string() => expected_v1,
            "v2.0".to_string() => expected_v2,
        }
    );
}

#[test]
fn test_merge_views_git_refs() {
    // Tests merging of git refs (by performing concurrent operations). See
    // test_refs.rs for tests of merging of individual ref targets.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings);
    let mut_repo = tx.mut_repo();
    let main_branch_tx0 = write_random_commit(mut_repo, &settings);
    mut_repo.set_git_ref_target(
        "refs/heads/main",
        RefTarget::normal(main_branch_tx0.id().clone()),
    );
    let feature_branch_tx0 = write_random_commit(mut_repo, &settings);
    mut_repo.set_git_ref_target(
        "refs/heads/feature",
        RefTarget::normal(feature_branch_tx0.id().clone()),
    );
    let repo = tx.commit("test");

    let mut tx1 = repo.start_transaction(&settings);
    let main_branch_tx1 = write_random_commit(tx1.mut_repo(), &settings);
    tx1.mut_repo().set_git_ref_target(
        "refs/heads/main",
        RefTarget::normal(main_branch_tx1.id().clone()),
    );
    let feature_branch_tx1 = write_random_commit(tx1.mut_repo(), &settings);
    tx1.mut_repo().set_git_ref_target(
        "refs/heads/feature",
        RefTarget::normal(feature_branch_tx1.id().clone()),
    );

    let mut tx2 = repo.start_transaction(&settings);
    let main_branch_tx2 = write_random_commit(tx2.mut_repo(), &settings);
    tx2.mut_repo().set_git_ref_target(
        "refs/heads/main",
        RefTarget::normal(main_branch_tx2.id().clone()),
    );

    let repo = commit_transactions(&settings, vec![tx1, tx2]);
    let expected_main_branch = RefTarget::from_legacy_form(
        [main_branch_tx0.id().clone()],
        [main_branch_tx1.id().clone(), main_branch_tx2.id().clone()],
    );
    let expected_feature_branch = RefTarget::normal(feature_branch_tx1.id().clone());
    assert_eq!(
        repo.view().git_refs(),
        &btreemap! {
            "refs/heads/main".to_string() => expected_main_branch,
            "refs/heads/feature".to_string() => expected_feature_branch,
        }
    );
}

#[test]
fn test_merge_views_git_heads() {
    // Tests merging of git heads (by performing concurrent operations). See
    // test_refs.rs for tests of merging of individual ref targets.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx0 = repo.start_transaction(&settings);
    let tx0_head = write_random_commit(tx0.mut_repo(), &settings);
    tx0.mut_repo()
        .set_git_head_target(RefTarget::normal(tx0_head.id().clone()));
    let repo = tx0.commit("test");

    let mut tx1 = repo.start_transaction(&settings);
    let tx1_head = write_random_commit(tx1.mut_repo(), &settings);
    tx1.mut_repo()
        .set_git_head_target(RefTarget::normal(tx1_head.id().clone()));

    let mut tx2 = repo.start_transaction(&settings);
    let tx2_head = write_random_commit(tx2.mut_repo(), &settings);
    tx2.mut_repo()
        .set_git_head_target(RefTarget::normal(tx2_head.id().clone()));

    let repo = commit_transactions(&settings, vec![tx1, tx2]);
    let expected_git_head = RefTarget::from_legacy_form(
        [tx0_head.id().clone()],
        [tx1_head.id().clone(), tx2_head.id().clone()],
    );
    assert_eq!(repo.view().git_head(), &expected_git_head);
}

#[test]
fn test_merge_views_divergent() {
    // We start with just commit A. Operation 1 rewrites it as A2. Operation 2
    // rewrites it as A3.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();

    let mut tx = test_repo.repo.start_transaction(&settings);
    let commit_a = write_random_commit(tx.mut_repo(), &settings);
    let repo = tx.commit("test");

    let mut tx1 = repo.start_transaction(&settings);
    let commit_a2 = tx1
        .mut_repo()
        .rewrite_commit(&settings, &commit_a)
        .set_description("A2")
        .write()
        .unwrap();
    tx1.mut_repo().rebase_descendants(&settings).unwrap();

    let mut tx2 = repo.start_transaction(&settings);
    let commit_a3 = tx2
        .mut_repo()
        .rewrite_commit(&settings, &commit_a)
        .set_description("A3")
        .write()
        .unwrap();
    tx2.mut_repo().rebase_descendants(&settings).unwrap();

    let repo = commit_transactions(&settings, vec![tx1, tx2]);

    // A2 and A3 should be heads.
    assert_eq!(
        *repo.view().heads(),
        hashset! {commit_a2.id().clone(), commit_a3.id().clone()}
    );
}

#[test_case(false ; "rewrite first")]
#[test_case(true ; "add child first")]
fn test_merge_views_child_on_rewritten(child_first: bool) {
    // We start with just commit A. Operation 1 adds commit B on top. Operation 2
    // rewrites A as A2.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();

    let mut tx = test_repo.repo.start_transaction(&settings);
    let commit_a = write_random_commit(tx.mut_repo(), &settings);
    let repo = tx.commit("test");

    let mut tx1 = repo.start_transaction(&settings);
    let commit_b = create_random_commit(tx1.mut_repo(), &settings)
        .set_parents(vec![commit_a.id().clone()])
        .write()
        .unwrap();

    let mut tx2 = repo.start_transaction(&settings);
    let commit_a2 = tx2
        .mut_repo()
        .rewrite_commit(&settings, &commit_a)
        .set_description("A2")
        .write()
        .unwrap();
    tx2.mut_repo().rebase_descendants(&settings).unwrap();

    let repo = if child_first {
        commit_transactions(&settings, vec![tx1, tx2])
    } else {
        commit_transactions(&settings, vec![tx2, tx1])
    };

    // A new B2 commit (B rebased onto A2) should be the only head.
    let heads = repo.view().heads();
    assert_eq!(heads.len(), 1);
    let b2_id = heads.iter().next().unwrap();
    let commit_b2 = repo.store().get_commit(b2_id).unwrap();
    assert_eq!(commit_b2.change_id(), commit_b.change_id());
    assert_eq!(commit_b2.parent_ids(), vec![commit_a2.id().clone()]);
}

#[test_case(false, false ; "add child on unchanged, rewrite first")]
#[test_case(false, true ; "add child on unchanged, add child first")]
#[test_case(true, false ; "add child on rewritten, rewrite first")]
#[test_case(true, true ; "add child on rewritten, add child first")]
fn test_merge_views_child_on_rewritten_divergent(on_rewritten: bool, child_first: bool) {
    // We start with divergent commits A2 and A3. Operation 1 adds commit B on top
    // of A2 or A3. Operation 2 rewrites A2 as A4. The result should be that B
    // gets rebased onto A4 if it was based on A2 before, but if it was based on
    // A3, it should remain there.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();

    let mut tx = test_repo.repo.start_transaction(&settings);
    let commit_a2 = write_random_commit(tx.mut_repo(), &settings);
    let commit_a3 = create_random_commit(tx.mut_repo(), &settings)
        .set_change_id(commit_a2.change_id().clone())
        .write()
        .unwrap();
    let repo = tx.commit("test");

    let mut tx1 = repo.start_transaction(&settings);
    let parent = if on_rewritten { &commit_a2 } else { &commit_a3 };
    let commit_b = create_random_commit(tx1.mut_repo(), &settings)
        .set_parents(vec![parent.id().clone()])
        .write()
        .unwrap();

    let mut tx2 = repo.start_transaction(&settings);
    let commit_a4 = tx2
        .mut_repo()
        .rewrite_commit(&settings, &commit_a2)
        .set_description("A4")
        .write()
        .unwrap();
    tx2.mut_repo().rebase_descendants(&settings).unwrap();

    let repo = if child_first {
        commit_transactions(&settings, vec![tx1, tx2])
    } else {
        commit_transactions(&settings, vec![tx2, tx1])
    };

    if on_rewritten {
        // A3 should remain as a head. The other head should be B2 (B rebased onto A4).
        let mut heads = repo.view().heads().clone();
        assert_eq!(heads.len(), 2);
        assert!(heads.remove(commit_a3.id()));
        let b2_id = heads.iter().next().unwrap();
        let commit_b2 = repo.store().get_commit(b2_id).unwrap();
        assert_eq!(commit_b2.change_id(), commit_b.change_id());
        assert_eq!(commit_b2.parent_ids(), vec![commit_a4.id().clone()]);
    } else {
        // No rebases should happen, so B and A4 should be the heads.
        let mut heads = repo.view().heads().clone();
        assert_eq!(heads.len(), 2);
        assert!(heads.remove(commit_b.id()));
        assert!(heads.remove(commit_a4.id()));
    }
}

#[test_case(false ; "abandon first")]
#[test_case(true ; "add child first")]
fn test_merge_views_child_on_abandoned(child_first: bool) {
    // We start with commit B on top of commit A. Operation 1 adds commit C on top.
    // Operation 2 abandons B.
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();

    let mut tx = test_repo.repo.start_transaction(&settings);
    let commit_a = write_random_commit(tx.mut_repo(), &settings);
    let commit_b = create_random_commit(tx.mut_repo(), &settings)
        .set_parents(vec![commit_a.id().clone()])
        .write()
        .unwrap();
    let repo = tx.commit("test");

    let mut tx1 = repo.start_transaction(&settings);
    let commit_c = create_random_commit(tx1.mut_repo(), &settings)
        .set_parents(vec![commit_b.id().clone()])
        .write()
        .unwrap();

    let mut tx2 = repo.start_transaction(&settings);
    tx2.mut_repo()
        .record_abandoned_commit(commit_b.id().clone());
    tx2.mut_repo().rebase_descendants(&settings).unwrap();

    let repo = if child_first {
        commit_transactions(&settings, vec![tx1, tx2])
    } else {
        commit_transactions(&settings, vec![tx2, tx1])
    };

    // A new C2 commit (C rebased onto A) should be the only head.
    let heads = repo.view().heads();
    assert_eq!(heads.len(), 1);
    let id_c2 = heads.iter().next().unwrap();
    let commit_c2 = repo.store().get_commit(id_c2).unwrap();
    assert_eq!(commit_c2.change_id(), commit_c.change_id());
    assert_eq!(commit_c2.parent_ids(), vec![commit_a.id().clone()]);
}
