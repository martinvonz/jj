// Copyright 2021 Google LLC
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

use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::op_store::RefTarget;
use jujutsu_lib::repo::RepoRef;
use jujutsu_lib::revset::{parse, resolve_symbol, RevsetError};
use jujutsu_lib::store::{CommitId, MillisSinceEpoch, Signature, Timestamp};
use jujutsu_lib::testutils::CommitGraphBuilder;
use jujutsu_lib::{git, testutils};
use test_case::test_case;

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_resolve_symbol_root(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    assert_eq!(
        resolve_symbol(repo.as_repo_ref(), "root"),
        Ok(vec![repo.store().root_commit_id().clone()])
    );
}

#[test]
fn test_resolve_symbol_commit_id() {
    let settings = testutils::user_settings();
    // Test only with git so we can get predictable commit ids
    let (_temp_dir, repo) = testutils::init_repo(&settings, true);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let signature = Signature {
        name: "test".to_string(),
        email: "test".to_string(),
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(0),
            tz_offset: 0,
        },
    };

    let mut commits = vec![];
    for i in &[1, 167, 895] {
        let commit = CommitBuilder::for_new_commit(
            &settings,
            repo.store(),
            repo.store().empty_tree_id().clone(),
        )
        .set_description(format!("test {}", i))
        .set_author(signature.clone())
        .set_committer(signature.clone())
        .write_to_repo(mut_repo);
        commits.push(commit);
    }

    // Test the test setup
    assert_eq!(
        commits[0].id().hex(),
        "0454de3cae04c46cda37ba2e8873b4c17ff51dcb"
    );
    assert_eq!(
        commits[1].id().hex(),
        "045f56cd1b17e8abde86771e2705395dcde6a957"
    );
    assert_eq!(
        commits[2].id().hex(),
        "0468f7da8de2ce442f512aacf83411d26cd2e0cf"
    );

    // Test lookup by full commit id
    let repo_ref = mut_repo.as_repo_ref();
    assert_eq!(
        resolve_symbol(repo_ref, "0454de3cae04c46cda37ba2e8873b4c17ff51dcb"),
        Ok(vec![commits[0].id().clone()])
    );
    assert_eq!(
        resolve_symbol(repo_ref, "045f56cd1b17e8abde86771e2705395dcde6a957"),
        Ok(vec![commits[1].id().clone()])
    );
    assert_eq!(
        resolve_symbol(repo_ref, "0468f7da8de2ce442f512aacf83411d26cd2e0cf"),
        Ok(vec![commits[2].id().clone()])
    );

    // Test commit id prefix
    assert_eq!(
        resolve_symbol(repo_ref, "046"),
        Ok(vec![commits[2].id().clone()])
    );
    assert_eq!(
        resolve_symbol(repo_ref, "04"),
        Err(RevsetError::AmbiguousCommitIdPrefix("04".to_string()))
    );
    assert_eq!(
        resolve_symbol(repo_ref, ""),
        Err(RevsetError::AmbiguousCommitIdPrefix("".to_string()))
    );
    assert_eq!(
        resolve_symbol(repo_ref, "040"),
        Err(RevsetError::NoSuchRevision("040".to_string()))
    );

    // Test non-hex string
    assert_eq!(
        resolve_symbol(repo_ref, "foo"),
        Err(RevsetError::NoSuchRevision("foo".to_string()))
    );

    tx.discard();
}

#[test]
fn test_resolve_symbol_change_id() {
    let settings = testutils::user_settings();
    // Test only with git so we can get predictable change ids
    let (_temp_dir, repo) = testutils::init_repo(&settings, true);

    let git_repo = repo.store().git_repo().unwrap();
    // Add some commits that will end up having change ids with common prefixes
    let empty_tree_id = git_repo.treebuilder(None).unwrap().write().unwrap();
    let git_author = git2::Signature::new(
        "git author",
        "git.author@example.com",
        &git2::Time::new(1000, 60),
    )
    .unwrap();
    let git_committer = git2::Signature::new(
        "git committer",
        "git.committer@example.com",
        &git2::Time::new(2000, -480),
    )
    .unwrap();
    let git_tree = git_repo.find_tree(empty_tree_id).unwrap();
    let mut git_commit_ids = vec![];
    for i in &[133, 664, 840] {
        let git_commit_id = git_repo
            .commit(
                Some(&format!("refs/heads/branch{}", i)),
                &git_author,
                &git_committer,
                &format!("test {}", i),
                &git_tree,
                &[],
            )
            .unwrap();
        git_commit_ids.push(git_commit_id);
    }

    let mut tx = repo.start_transaction("test");
    git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    let repo = tx.commit();

    // Test the test setup
    assert_eq!(
        hex::encode(git_commit_ids[0].as_bytes()),
        // "04e12a5467bba790efb88a9870894ec208b16bf1" reversed
        "8fd68d104372910e19511df709e5dde62a548720"
    );
    assert_eq!(
        hex::encode(git_commit_ids[1].as_bytes()),
        // "040b3ba3a51d8edbc4c5855cbd09de71d4c29cca" reversed
        "5339432b8e7b90bd3aa1a323db71b8a5c5dcd020"
    );
    assert_eq!(
        hex::encode(git_commit_ids[2].as_bytes()),
        // "04e1c7082e4e34f3f371d8a1a46770b861b9b547" reversed
        "e2ad9d861d0ee625851b8ecfcf2c727410e38720"
    );

    // Test lookup by full change id
    let repo_ref = repo.as_repo_ref();
    assert_eq!(
        resolve_symbol(repo_ref, "04e12a5467bba790efb88a9870894ec2"),
        Ok(vec![CommitId::from_hex(
            "8fd68d104372910e19511df709e5dde62a548720"
        )])
    );
    assert_eq!(
        resolve_symbol(repo_ref, "040b3ba3a51d8edbc4c5855cbd09de71"),
        Ok(vec![CommitId::from_hex(
            "5339432b8e7b90bd3aa1a323db71b8a5c5dcd020"
        )])
    );
    assert_eq!(
        resolve_symbol(repo_ref, "04e1c7082e4e34f3f371d8a1a46770b8"),
        Ok(vec![CommitId::from_hex(
            "e2ad9d861d0ee625851b8ecfcf2c727410e38720"
        )])
    );

    // Test change id prefix
    assert_eq!(
        resolve_symbol(repo_ref, "04e12"),
        Ok(vec![CommitId::from_hex(
            "8fd68d104372910e19511df709e5dde62a548720"
        )])
    );
    assert_eq!(
        resolve_symbol(repo_ref, "04e1c"),
        Ok(vec![CommitId::from_hex(
            "e2ad9d861d0ee625851b8ecfcf2c727410e38720"
        )])
    );
    assert_eq!(
        resolve_symbol(repo_ref, "04e1"),
        Err(RevsetError::AmbiguousChangeIdPrefix("04e1".to_string()))
    );
    assert_eq!(
        resolve_symbol(repo_ref, ""),
        // Commit id is checked first, so this is considered an ambiguous commit id
        Err(RevsetError::AmbiguousCommitIdPrefix("".to_string()))
    );
    assert_eq!(
        resolve_symbol(repo_ref, "04e13"),
        Err(RevsetError::NoSuchRevision("04e13".to_string()))
    );

    // Test non-hex string
    assert_eq!(
        resolve_symbol(repo_ref, "foo"),
        Err(RevsetError::NoSuchRevision("foo".to_string()))
    );
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_resolve_symbol_checkout(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    let commit2 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);

    mut_repo.set_checkout(commit1.id().clone());
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "@"),
        Ok(vec![commit1.id().clone()])
    );
    mut_repo.set_checkout(commit2.id().clone());
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "@"),
        Ok(vec![commit2.id().clone()])
    );

    tx.discard();
}

#[test]
fn test_resolve_symbol_git_refs() {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, true);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // Create some commits and refs to work with and so the repo is not empty
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    let commit2 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    let commit3 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    let commit4 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    let commit5 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    mut_repo.insert_git_ref(
        "refs/heads/branch1".to_string(),
        RefTarget::Normal(commit1.id().clone()),
    );
    mut_repo.insert_git_ref(
        "refs/heads/branch2".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    mut_repo.insert_git_ref(
        "refs/heads/conflicted".to_string(),
        RefTarget::Conflict {
            removes: vec![commit2.id().clone()],
            adds: vec![commit1.id().clone(), commit3.id().clone()],
        },
    );
    mut_repo.insert_git_ref(
        "refs/tags/tag1".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    mut_repo.insert_git_ref(
        "refs/tags/remotes/origin/branch1".to_string(),
        RefTarget::Normal(commit3.id().clone()),
    );

    // Non-existent ref
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "non-existent"),
        Err(RevsetError::NoSuchRevision("non-existent".to_string()))
    );

    // Full ref
    mut_repo.insert_git_ref(
        "refs/heads/branch".to_string(),
        RefTarget::Normal(commit4.id().clone()),
    );
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "refs/heads/branch"),
        Ok(vec![commit4.id().clone()])
    );

    // Qualified with only heads/
    mut_repo.insert_git_ref(
        "refs/heads/branch".to_string(),
        RefTarget::Normal(commit5.id().clone()),
    );
    mut_repo.insert_git_ref(
        "refs/tags/branch".to_string(),
        RefTarget::Normal(commit4.id().clone()),
    );
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "heads/branch"),
        Ok(vec![commit5.id().clone()])
    );

    // Unqualified branch name
    mut_repo.insert_git_ref(
        "refs/heads/branch".to_string(),
        RefTarget::Normal(commit3.id().clone()),
    );
    mut_repo.insert_git_ref(
        "refs/tags/branch".to_string(),
        RefTarget::Normal(commit4.id().clone()),
    );
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "branch"),
        Ok(vec![commit3.id().clone()])
    );

    // Unqualified tag name
    mut_repo.insert_git_ref(
        "refs/tags/tag".to_string(),
        RefTarget::Normal(commit4.id().clone()),
    );
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "tag"),
        Ok(vec![commit4.id().clone()])
    );

    // Unqualified remote-tracking branch name
    mut_repo.insert_git_ref(
        "refs/remotes/origin/remote-branch".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "origin/remote-branch"),
        Ok(vec![commit2.id().clone()])
    );

    // Cannot shadow checkout ("@") or root symbols
    mut_repo.insert_git_ref("@".to_string(), RefTarget::Normal(commit2.id().clone()));
    mut_repo.insert_git_ref("root".to_string(), RefTarget::Normal(commit3.id().clone()));
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "@"),
        Ok(vec![mut_repo.view().checkout().clone()])
    );
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "root"),
        Ok(vec![mut_repo.store().root_commit().id().clone()])
    );

    // Conflicted ref resolves to its "adds"
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "refs/heads/conflicted"),
        Ok(vec![commit1.id().clone(), commit3.id().clone()])
    );

    tx.discard();
}

fn resolve_commit_ids(repo: RepoRef, revset_str: &str) -> Vec<CommitId> {
    let expression = parse(revset_str).unwrap();
    expression
        .evaluate(repo)
        .unwrap()
        .iter()
        .map(|entry| entry.commit_id())
        .collect()
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_root_and_checkout(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    let root_commit = repo.store().root_commit();
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);

    // Can find the root commit
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "root"),
        vec![root_commit.id().clone()]
    );

    // Can find the current checkout
    mut_repo.set_checkout(commit1.id().clone());
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "@"),
        vec![commit1.id().clone()]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_parents(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.initial_commit();
    let commit4 = graph_builder.commit_with_parents(&[&commit2, &commit3]);
    let commit5 = graph_builder.commit_with_parents(&[&commit2]);

    // The root commit has no parents
    assert_eq!(resolve_commit_ids(mut_repo.as_repo_ref(), ":root"), vec![]);

    // Can find parents of the current checkout
    mut_repo.set_checkout(commit2.id().clone());
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), ":@"),
        vec![commit1.id().clone()]
    );

    // Can find parents of a merge commit
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), &format!(":{}", commit4.id().hex())),
        vec![commit3.id().clone(), commit2.id().clone()]
    );

    // Parents of all commits in input are returned
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(":({} | {})", commit2.id().hex(), commit3.id().hex())
        ),
        vec![commit1.id().clone(), root_commit.id().clone()]
    );

    // Parents already in input set are returned
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(":({} | {})", commit1.id().hex(), commit2.id().hex())
        ),
        vec![commit1.id().clone(), root_commit.id().clone()]
    );

    // Parents shared among commits in input are not repeated
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(":({} | {})", commit4.id().hex(), commit5.id().hex())
        ),
        vec![commit3.id().clone(), commit2.id().clone()]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_children(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    let wc_commit = repo.working_copy_locked().current_commit();
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    let commit2 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit1.id().clone()])
        .write_to_repo(mut_repo);
    let commit3 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit1.id().clone()])
        .set_predecessors(vec![commit2.id().clone()])
        .set_change_id(commit2.change_id().clone())
        .write_to_repo(mut_repo);
    let commit4 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit3.id().clone()])
        .write_to_repo(mut_repo);
    let commit5 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit1.id().clone()])
        .write_to_repo(mut_repo);
    let commit6 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit4.id().clone(), commit5.id().clone()])
        .write_to_repo(mut_repo);

    // Can find children of the root commit
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "root:"),
        vec![commit1.id().clone(), wc_commit.id().clone()]
    );

    // Children do not include hidden commits (commit2)
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), &format!("{}:", commit1.id().hex())),
        vec![commit5.id().clone(), commit3.id().clone()]
    );

    // Children of all commits in input are returned, including those already in the
    // input set
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("({} | {}):", commit1.id().hex(), commit3.id().hex())
        ),
        vec![
            commit5.id().clone(),
            commit4.id().clone(),
            commit3.id().clone()
        ]
    );

    // Children shared among commits in input are not repeated
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("({} | {}):", commit4.id().hex(), commit5.id().hex())
        ),
        vec![commit6.id().clone()]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_ancestors(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit1, &commit3]);

    // The ancestors of the root commit is just the root commit itself
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), ",,root"),
        vec![root_commit.id().clone()]
    );

    // Can find ancestors of a specific commit. Commits reachable via multiple paths
    // are not repeated.
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), &format!(",,{}", commit4.id().hex())),
        vec![
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone(),
            root_commit.id().clone(),
        ]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_range(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit1, &commit3]);

    // The range from the root to the root is empty (because the left side of the
    // range is exclusive)
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "root,,,root"),
        vec![]
    );

    // Linear range
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("{},,,{}", commit1.id().hex(), commit3.id().hex())
        ),
        vec![commit3.id().clone(), commit2.id().clone()]
    );

    // Empty range (descendant first)
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("{},,,{}", commit3.id().hex(), commit1.id().hex())
        ),
        vec![]
    );

    // Range including a merge
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("{},,,{}", commit1.id().hex(), commit4.id().hex())
        ),
        vec![
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone()
        ]
    );

    // Sibling commits
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("{},,,{}", commit2.id().hex(), commit3.id().hex())
        ),
        vec![commit3.id().clone()]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_dag_range(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit1]);
    let commit5 = graph_builder.commit_with_parents(&[&commit3, &commit4]);

    // Can get DAG range of just the root commit
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "root,,root"),
        vec![root_commit.id().clone(),]
    );

    // Linear range
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("{},,{}", root_commit.id().hex(), commit2.id().hex())
        ),
        vec![
            commit2.id().clone(),
            commit1.id().clone(),
            root_commit.id().clone(),
        ]
    );

    // Empty range
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("{},,{}", commit2.id().hex(), commit4.id().hex())
        ),
        vec![]
    );

    // Including a merge
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("{},,{}", commit1.id().hex(), commit5.id().hex())
        ),
        vec![
            commit5.id().clone(),
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone(),
        ]
    );

    // Including a merge, but only ancestors only from one side
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("{},,{}", commit2.id().hex(), commit5.id().hex())
        ),
        vec![
            commit5.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
        ]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_descendants(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    let root_commit = repo.store().root_commit();
    let wc_commit = repo.working_copy_locked().current_commit();
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    let commit2 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit1.id().clone()])
        .write_to_repo(mut_repo);
    let commit3 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit1.id().clone()])
        .set_predecessors(vec![commit2.id().clone()])
        .set_change_id(commit2.change_id().clone())
        .write_to_repo(mut_repo);
    let commit4 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit3.id().clone()])
        .write_to_repo(mut_repo);
    let commit5 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit1.id().clone()])
        .write_to_repo(mut_repo);
    let commit6 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit4.id().clone(), commit5.id().clone()])
        .write_to_repo(mut_repo);

    // The descendants of the root commit is all the non-hidden commits in the repo
    // (commit2 is excluded)
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "root,,"),
        vec![
            commit6.id().clone(),
            commit5.id().clone(),
            commit4.id().clone(),
            commit3.id().clone(),
            commit1.id().clone(),
            wc_commit.id().clone(),
            root_commit.id().clone(),
        ]
    );

    // Can find ancestors of a specific commit
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(
                "{},,\
        ",
                commit3.id().hex()
            )
        ),
        vec![
            commit6.id().clone(),
            commit4.id().clone(),
            commit3.id().clone(),
        ]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_all_heads(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let wc_commit = repo.working_copy_locked().current_commit();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);

    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "all_heads()"),
        vec![commit2.id().clone(), wc_commit.id().clone()]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_public_heads(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.initial_commit();

    // Can get public heads with root commit as only public head
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "public_heads()"),
        vec![root_commit.id().clone()]
    );
    // Can get public heads with a single public head
    mut_repo.add_public_head(&commit1);
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "public_heads()"),
        vec![commit1.id().clone()]
    );
    // Can get public heads with multiple public head
    mut_repo.add_public_head(&commit2);
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "public_heads()"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_git_refs(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    let commit2 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    let commit3 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    let commit4 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);

    // Can get git refs when there are none
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "git_refs()"),
        vec![]
    );
    // Can get a mix of git refs
    mut_repo.insert_git_ref(
        "refs/heads/branch1".to_string(),
        RefTarget::Normal(commit1.id().clone()),
    );
    mut_repo.insert_git_ref(
        "refs/tags/tag1".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "git_refs()"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    // Two refs pointing to the same commit does not result in a duplicate in the
    // revset
    mut_repo.insert_git_ref(
        "refs/tags/tag2".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "git_refs()"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    // Can get git refs when there are conflicted refs
    mut_repo.insert_git_ref(
        "refs/heads/branch1".to_string(),
        RefTarget::Conflict {
            removes: vec![commit1.id().clone()],
            adds: vec![commit2.id().clone(), commit3.id().clone()],
        },
    );
    mut_repo.insert_git_ref(
        "refs/tags/tag1".to_string(),
        RefTarget::Conflict {
            removes: vec![commit2.id().clone()],
            adds: vec![commit3.id().clone(), commit4.id().clone()],
        },
    );
    mut_repo.remove_git_ref("refs/tags/tag2");
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "git_refs()"),
        vec![
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone()
        ]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_obsolete(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    let root_commit = repo.store().root_commit();
    let wc_commit = repo.working_copy_locked().current_commit();
    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    let commit2 = testutils::create_random_commit(&settings, &repo)
        .set_predecessors(vec![commit1.id().clone()])
        .set_change_id(commit1.change_id().clone())
        .write_to_repo(mut_repo);
    let commit3 = testutils::create_random_commit(&settings, &repo)
        .set_predecessors(vec![commit2.id().clone()])
        .set_change_id(commit2.change_id().clone())
        .write_to_repo(mut_repo);
    let commit4 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit3.id().clone()])
        .set_pruned(true)
        .write_to_repo(mut_repo);

    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "non_obsolete_heads()"),
        vec![commit3.id().clone(), wc_commit.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("non_obsolete_heads({})", commit4.id().hex())
        ),
        vec![commit3.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("non_obsolete_heads({})", commit1.id().hex())
        ),
        vec![root_commit.id().clone()]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_merges(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.initial_commit();
    let commit3 = graph_builder.initial_commit();
    let commit4 = graph_builder.commit_with_parents(&[&commit1, &commit2]);
    let commit5 = graph_builder.commit_with_parents(&[&commit1, &commit2, &commit3]);

    // Finds all merges by default
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "merges()"),
        vec![commit5.id().clone(), commit4.id().clone(),]
    );
    // Searches only among candidates if specified
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("merges(,,{})", commit5.id().hex())
        ),
        vec![commit5.id().clone()]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_description(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    let commit1 = testutils::create_random_commit(&settings, &repo)
        .set_description("commit 1".to_string())
        .write_to_repo(mut_repo);
    let commit2 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit1.id().clone()])
        .set_description("commit 2".to_string())
        .write_to_repo(mut_repo);
    let commit3 = testutils::create_random_commit(&settings, &repo)
        .set_parents(vec![commit2.id().clone()])
        .set_description("commit 3".to_string())
        .write_to_repo(mut_repo);

    // Can find multiple matches
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "description(commit)"),
        vec![
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone()
        ]
    );
    // Can find a unique match
    assert_eq!(
        resolve_commit_ids(mut_repo.as_repo_ref(), "description(\"commit 2\")"),
        vec![commit2.id().clone()]
    );
    // Searches only among candidates if specified
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            "description(\"commit 2\",all_heads())"
        ),
        vec![]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_union(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit3]);
    let commit5 = graph_builder.commit_with_parents(&[&commit2]);

    // Union between ancestors
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(",,{} | ,,{}", commit4.id().hex(), commit5.id().hex())
        ),
        vec![
            commit5.id().clone(),
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone(),
            root_commit.id().clone()
        ]
    );

    // Unioning can add back commits removed by difference
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(
                "(,,{} - ,,{}) | ,,{}",
                commit4.id().hex(),
                commit2.id().hex(),
                commit5.id().hex()
            )
        ),
        vec![
            commit5.id().clone(),
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone(),
            root_commit.id().clone(),
        ]
    );

    // Unioning of disjoint sets
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(
                "(,,{} - ,,{}) | {}",
                commit4.id().hex(),
                commit2.id().hex(),
                commit5.id().hex(),
            )
        ),
        vec![
            commit5.id().clone(),
            commit4.id().clone(),
            commit3.id().clone()
        ]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_intersection(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit3]);
    let commit5 = graph_builder.commit_with_parents(&[&commit2]);

    // Intersection between ancestors
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(",,{} & ,,{}", commit4.id().hex(), commit5.id().hex())
        ),
        vec![
            commit2.id().clone(),
            commit1.id().clone(),
            root_commit.id().clone()
        ]
    );

    // Intersection of disjoint sets
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!("{} & {}", commit4.id().hex(), commit2.id().hex())
        ),
        vec![]
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evaluate_expression_difference(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit3]);
    let commit5 = graph_builder.commit_with_parents(&[&commit2]);

    // Difference between ancestors
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(",,{} - ,,{}", commit4.id().hex(), commit5.id().hex())
        ),
        vec![commit4.id().clone(), commit3.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(",,{} - ,,{}", commit5.id().hex(), commit4.id().hex())
        ),
        vec![commit5.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(",,{} - ,,{}", commit4.id().hex(), commit2.id().hex())
        ),
        vec![commit4.id().clone(), commit3.id().clone()]
    );

    // Associativity
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(
                ",,{} - {} - {}",
                commit4.id().hex(),
                commit2.id().hex(),
                commit3.id().hex()
            )
        ),
        vec![
            commit4.id().clone(),
            commit1.id().clone(),
            root_commit.id().clone(),
        ]
    );

    // Subtracting a difference does not add back any commits
    assert_eq!(
        resolve_commit_ids(
            mut_repo.as_repo_ref(),
            &format!(
                "(,,{} - ,,{}) - (,,{} - ,,{})",
                commit4.id().hex(),
                commit1.id().hex(),
                commit3.id().hex(),
                commit1.id().hex(),
            )
        ),
        vec![commit4.id().clone()]
    );

    tx.discard();
}
