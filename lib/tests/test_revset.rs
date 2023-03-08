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

use std::path::Path;

use assert_matches::assert_matches;
use jujutsu_lib::backend::{CommitId, MillisSinceEpoch, ObjectId, Signature, Timestamp};
use jujutsu_lib::default_revset_engine::resolve_symbol;
use jujutsu_lib::git;
use jujutsu_lib::op_store::{RefTarget, WorkspaceId};
use jujutsu_lib::repo::Repo;
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::revset::{
    optimize, parse, RevsetAliasesMap, RevsetError, RevsetExpression, RevsetFilterPredicate,
    RevsetIteratorExt, RevsetWorkspaceContext,
};
use jujutsu_lib::settings::GitSettings;
use jujutsu_lib::workspace::Workspace;
use test_case::test_case;
use testutils::{
    create_random_commit, write_random_commit, CommitGraphBuilder, TestRepo, TestWorkspace,
};

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_resolve_symbol_root(use_git: bool) {
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    assert_matches!(
        resolve_symbol(repo, "root", None),
        Ok(v) if v == vec![repo.store().root_commit_id().clone()]
    );
}

#[test]
fn test_resolve_symbol_commit_id() {
    let settings = testutils::user_settings();
    // Test only with git so we can get predictable commit ids
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
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
        let commit = mut_repo
            .new_commit(
                &settings,
                vec![repo.store().root_commit_id().clone()],
                repo.store().empty_tree_id().clone(),
            )
            .set_description(format!("test {i}"))
            .set_author(signature.clone())
            .set_committer(signature.clone())
            .write()
            .unwrap();
        commits.push(commit);
    }
    let repo = tx.commit();

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

    // Change ids should never have prefix "04"
    insta::assert_snapshot!(commits[0].change_id().hex(), @"781199f9d55d18e855a7aa84c5e4b40d");
    insta::assert_snapshot!(commits[1].change_id().hex(), @"a2c96fc88f32e487328f04927f20c4b1");
    insta::assert_snapshot!(commits[2].change_id().hex(), @"4399e4f3123763dfe7d68a2809ecc01b");

    // Test lookup by full commit id
    assert_eq!(
        resolve_symbol(&repo, "0454de3cae04c46cda37ba2e8873b4c17ff51dcb", None).unwrap(),
        vec![commits[0].id().clone()]
    );
    assert_eq!(
        resolve_symbol(&repo, "045f56cd1b17e8abde86771e2705395dcde6a957", None).unwrap(),
        vec![commits[1].id().clone()]
    );
    assert_eq!(
        resolve_symbol(&repo, "0468f7da8de2ce442f512aacf83411d26cd2e0cf", None).unwrap(),
        vec![commits[2].id().clone()]
    );

    // Test empty commit id
    assert_matches!(
        resolve_symbol(&repo, "", None),
        Err(RevsetError::AmbiguousIdPrefix(s)) if s.is_empty()
    );

    // Test commit id prefix
    assert_eq!(
        resolve_symbol(&repo, "046", None).unwrap(),
        vec![commits[2].id().clone()]
    );
    assert_matches!(
        resolve_symbol(&repo, "04", None),
        Err(RevsetError::AmbiguousIdPrefix(s)) if s == "04"
    );
    assert_matches!(
        resolve_symbol(&repo, "", None),
        Err(RevsetError::AmbiguousIdPrefix(s)) if s.is_empty()
    );
    assert_matches!(
        resolve_symbol(&repo, "040", None),
        Err(RevsetError::NoSuchRevision(s)) if s == "040"
    );

    // Test non-hex string
    assert_matches!(
        resolve_symbol(&repo, "foo", None),
        Err(RevsetError::NoSuchRevision(s)) if s == "foo"
    );

    // Test present() suppresses only NoSuchRevision error
    assert_eq!(resolve_commit_ids(&repo, "present(foo)"), []);
    assert_matches!(
        optimize(parse("present(04)", &RevsetAliasesMap::new(), None).unwrap())
            .evaluate(&repo, None)
            .map(|_| ()),
        Err(RevsetError::AmbiguousIdPrefix(s)) if s == "04"
    );
    assert_eq!(
        resolve_commit_ids(&repo, "present(046)"),
        vec![commits[2].id().clone()]
    );
}

#[test_case(false ; "mutable")]
#[test_case(true ; "readonly")]
fn test_resolve_symbol_change_id(readonly: bool) {
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    // Test only with git so we can get predictable change ids
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;

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
    for i in &[133, 664, 840, 5085] {
        let git_commit_id = git_repo
            .commit(
                Some(&format!("refs/heads/branch{i}")),
                &git_author,
                &git_committer,
                &format!("test {i}"),
                &git_tree,
                &[],
            )
            .unwrap();
        git_commit_ids.push(git_commit_id);
    }

    let mut tx = repo.start_transaction(&settings, "test");
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();

    // Test the test setup
    assert_eq!(
        hex::encode(git_commit_ids[0]),
        // "04e12a5467bba790efb88a9870894ec208b16bf1" reversed
        "8fd68d104372910e19511df709e5dde62a548720"
    );
    assert_eq!(
        hex::encode(git_commit_ids[1]),
        // "040b3ba3a51d8edbc4c5855cbd09de71d4c29cca" reversed
        "5339432b8e7b90bd3aa1a323db71b8a5c5dcd020"
    );
    assert_eq!(
        hex::encode(git_commit_ids[2]),
        // "04e1c7082e4e34f3f371d8a1a46770b861b9b547" reversed
        "e2ad9d861d0ee625851b8ecfcf2c727410e38720"
    );
    assert_eq!(
        hex::encode(git_commit_ids[3]),
        // "911d7e52fd5ba04b8f289e14c3d30b52d38c0020" reversed
        "040031cb4ad0cbc3287914f1d205dabf4a7eb889"
    );

    let _readonly_repo;
    let repo: &dyn Repo = if readonly {
        _readonly_repo = tx.commit();
        &_readonly_repo
    } else {
        tx.mut_repo()
    };

    // Test lookup by full change id
    assert_eq!(
        resolve_symbol(repo, "zvlyxpuvtsoopsqzlkorrpqrszrqvlnx", None).unwrap(),
        vec![CommitId::from_hex(
            "8fd68d104372910e19511df709e5dde62a548720"
        )]
    );
    assert_eq!(
        resolve_symbol(repo, "zvzowopwpuymrlmonvnuruunomzqmlsy", None).unwrap(),
        vec![CommitId::from_hex(
            "5339432b8e7b90bd3aa1a323db71b8a5c5dcd020"
        )]
    );
    assert_eq!(
        resolve_symbol(repo, "zvlynszrxlvlwvkwkwsymrpypvtsszor", None).unwrap(),
        vec![CommitId::from_hex(
            "e2ad9d861d0ee625851b8ecfcf2c727410e38720"
        )]
    );

    // Test change id prefix
    assert_eq!(
        resolve_symbol(repo, "zvlyx", None).unwrap(),
        vec![CommitId::from_hex(
            "8fd68d104372910e19511df709e5dde62a548720"
        )]
    );
    assert_eq!(
        resolve_symbol(repo, "zvlyn", None).unwrap(),
        vec![CommitId::from_hex(
            "e2ad9d861d0ee625851b8ecfcf2c727410e38720"
        )]
    );
    assert_matches!(
        resolve_symbol(repo, "zvly", None),
        Err(RevsetError::AmbiguousIdPrefix(s)) if s == "zvly"
    );
    assert_matches!(
        resolve_symbol(repo, "", None),
        Err(RevsetError::AmbiguousIdPrefix(s)) if s.is_empty()
    );
    assert_matches!(
        resolve_symbol(repo, "zvlyw", None),
        Err(RevsetError::NoSuchRevision(s)) if s == "zvlyw"
    );

    // Test that commit and changed id don't conflict ("040" and "zvz" are the
    // same).
    assert_eq!(
        resolve_symbol(repo, "040", None).unwrap(),
        vec![CommitId::from_hex(
            "040031cb4ad0cbc3287914f1d205dabf4a7eb889"
        )]
    );
    assert_eq!(
        resolve_symbol(repo, "zvz", None).unwrap(),
        vec![CommitId::from_hex(
            "5339432b8e7b90bd3aa1a323db71b8a5c5dcd020"
        )]
    );

    // Test non-hex string
    assert_matches!(
        resolve_symbol(repo, "foo", None),
        Err(RevsetError::NoSuchRevision(s)) if s == "foo"
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_resolve_symbol_checkout(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    let commit1 = write_random_commit(mut_repo, &settings);
    let commit2 = write_random_commit(mut_repo, &settings);

    let ws1 = WorkspaceId::new("ws1".to_string());
    let ws2 = WorkspaceId::new("ws2".to_string());

    // With no workspaces, no variation can be resolved
    assert_matches!(
        resolve_symbol(mut_repo, "@", None),
        Err(RevsetError::NoSuchRevision(s)) if s == "@"
    );
    assert_matches!(
        resolve_symbol(mut_repo, "@", Some(&ws1)),
        Err(RevsetError::NoSuchRevision(s)) if s == "@"
    );
    assert_matches!(
        resolve_symbol(mut_repo, "ws1@", Some(&ws1)),
        Err(RevsetError::NoSuchRevision(s)) if s == "ws1@"
    );

    // Add some workspaces
    mut_repo
        .set_wc_commit(ws1.clone(), commit1.id().clone())
        .unwrap();
    mut_repo.set_wc_commit(ws2, commit2.id().clone()).unwrap();
    // @ cannot be resolved without a default workspace ID
    assert_matches!(
        resolve_symbol(mut_repo, "@", None),
        Err(RevsetError::NoSuchRevision(s)) if s == "@"
    );
    // Can resolve "@" shorthand with a default workspace ID
    assert_eq!(
        resolve_symbol(mut_repo, "@", Some(&ws1)).unwrap(),
        vec![commit1.id().clone()]
    );
    // Can resolve an explicit checkout
    assert_eq!(
        resolve_symbol(mut_repo, "ws2@", Some(&ws1)).unwrap(),
        vec![commit2.id().clone()]
    );
}

#[test]
fn test_resolve_symbol_git_refs() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    // Create some commits and refs to work with and so the repo is not empty
    let commit1 = write_random_commit(mut_repo, &settings);
    let commit2 = write_random_commit(mut_repo, &settings);
    let commit3 = write_random_commit(mut_repo, &settings);
    let commit4 = write_random_commit(mut_repo, &settings);
    let commit5 = write_random_commit(mut_repo, &settings);
    mut_repo.set_git_ref(
        "refs/heads/branch1".to_string(),
        RefTarget::Normal(commit1.id().clone()),
    );
    mut_repo.set_git_ref(
        "refs/heads/branch2".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    mut_repo.set_git_ref(
        "refs/heads/conflicted".to_string(),
        RefTarget::Conflict {
            removes: vec![commit2.id().clone()],
            adds: vec![commit1.id().clone(), commit3.id().clone()],
        },
    );
    mut_repo.set_git_ref(
        "refs/tags/tag1".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    mut_repo.set_git_ref(
        "refs/tags/remotes/origin/branch1".to_string(),
        RefTarget::Normal(commit3.id().clone()),
    );

    // Nonexistent ref
    assert_matches!(
        resolve_symbol(mut_repo, "nonexistent", None),
        Err(RevsetError::NoSuchRevision(s)) if s == "nonexistent"
    );

    // Full ref
    mut_repo.set_git_ref(
        "refs/heads/branch".to_string(),
        RefTarget::Normal(commit4.id().clone()),
    );
    assert_eq!(
        resolve_symbol(mut_repo, "refs/heads/branch", None).unwrap(),
        vec![commit4.id().clone()]
    );

    // Qualified with only heads/
    mut_repo.set_git_ref(
        "refs/heads/branch".to_string(),
        RefTarget::Normal(commit5.id().clone()),
    );
    mut_repo.set_git_ref(
        "refs/tags/branch".to_string(),
        RefTarget::Normal(commit4.id().clone()),
    );
    assert_eq!(
        resolve_symbol(mut_repo, "heads/branch", None).unwrap(),
        vec![commit5.id().clone()]
    );

    // Unqualified branch name
    mut_repo.set_git_ref(
        "refs/heads/branch".to_string(),
        RefTarget::Normal(commit3.id().clone()),
    );
    mut_repo.set_git_ref(
        "refs/tags/branch".to_string(),
        RefTarget::Normal(commit4.id().clone()),
    );
    assert_eq!(
        resolve_symbol(mut_repo, "branch", None).unwrap(),
        vec![commit3.id().clone()]
    );

    // Unqualified tag name
    mut_repo.set_git_ref(
        "refs/tags/tag".to_string(),
        RefTarget::Normal(commit4.id().clone()),
    );
    assert_eq!(
        resolve_symbol(mut_repo, "tag", None).unwrap(),
        vec![commit4.id().clone()]
    );

    // Unqualified remote-tracking branch name
    mut_repo.set_git_ref(
        "refs/remotes/origin/remote-branch".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    assert_eq!(
        resolve_symbol(mut_repo, "origin/remote-branch", None).unwrap(),
        vec![commit2.id().clone()]
    );

    // Cannot shadow checkout ("@") or root symbols
    let ws_id = WorkspaceId::default();
    mut_repo
        .set_wc_commit(ws_id.clone(), commit1.id().clone())
        .unwrap();
    mut_repo.set_git_ref("@".to_string(), RefTarget::Normal(commit2.id().clone()));
    mut_repo.set_git_ref("root".to_string(), RefTarget::Normal(commit3.id().clone()));
    assert_eq!(
        resolve_symbol(mut_repo, "@", Some(&ws_id)).unwrap(),
        vec![mut_repo.view().get_wc_commit_id(&ws_id).unwrap().clone()]
    );
    assert_eq!(
        resolve_symbol(mut_repo, "root", None).unwrap(),
        vec![mut_repo.store().root_commit().id().clone()]
    );

    // Conflicted ref resolves to its "adds"
    assert_eq!(
        resolve_symbol(mut_repo, "refs/heads/conflicted", None).unwrap(),
        vec![commit1.id().clone(), commit3.id().clone()]
    );
}

fn resolve_commit_ids(repo: &dyn Repo, revset_str: &str) -> Vec<CommitId> {
    let expression = optimize(parse(revset_str, &RevsetAliasesMap::new(), None).unwrap());
    expression
        .evaluate(repo, None)
        .unwrap()
        .iter()
        .commit_ids()
        .collect()
}

fn resolve_commit_ids_in_workspace(
    repo: &dyn Repo,
    revset_str: &str,
    workspace: &Workspace,
    cwd: Option<&Path>,
) -> Vec<CommitId> {
    let workspace_ctx = RevsetWorkspaceContext {
        cwd: cwd.unwrap_or_else(|| workspace.workspace_root()),
        workspace_id: workspace.workspace_id(),
        workspace_root: workspace.workspace_root(),
    };
    let expression =
        optimize(parse(revset_str, &RevsetAliasesMap::new(), Some(&workspace_ctx)).unwrap());
    expression
        .evaluate(repo, Some(&workspace_ctx))
        .unwrap()
        .iter()
        .commit_ids()
        .collect()
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_root_and_checkout(use_git: bool) {
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init(&settings, use_git);
    let repo = &test_workspace.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    let root_commit = repo.store().root_commit();
    let commit1 = write_random_commit(mut_repo, &settings);

    // Can find the root commit
    assert_eq!(
        resolve_commit_ids(mut_repo, "root"),
        vec![root_commit.id().clone()]
    );

    // Can find the current working-copy commit
    mut_repo
        .set_wc_commit(WorkspaceId::default(), commit1.id().clone())
        .unwrap();
    assert_eq!(
        resolve_commit_ids_in_workspace(mut_repo, "@", &test_workspace.workspace, None),
        vec![commit1.id().clone()]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_heads(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);

    // Heads of an empty set is an empty set
    assert_eq!(resolve_commit_ids(mut_repo, "heads(none())"), vec![]);

    // Heads of the root is the root
    assert_eq!(
        resolve_commit_ids(mut_repo, "heads(root)"),
        vec![root_commit.id().clone()]
    );

    // Heads of a single commit is that commit
    assert_eq!(
        resolve_commit_ids(mut_repo, &format!("heads({})", commit2.id().hex())),
        vec![commit2.id().clone()]
    );

    // Heads of a parent and a child is the child
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("heads({} | {})", commit2.id().hex(), commit3.id().hex())
        ),
        vec![commit3.id().clone()]
    );

    // Heads of a grandparent and a grandchild is the grandchild (unlike Mercurial's
    // heads() revset, which would include both)
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("heads({} | {})", commit1.id().hex(), commit3.id().hex())
        ),
        vec![commit3.id().clone()]
    );

    // Heads of all commits is the set of heads in the repo
    assert_eq!(
        resolve_commit_ids(mut_repo, "heads(all())"),
        resolve_commit_ids(mut_repo, "heads()")
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_roots(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);

    // Roots of an empty set is an empty set
    assert_eq!(resolve_commit_ids(mut_repo, "roots(none())"), vec![]);

    // Roots of the root is the root
    assert_eq!(
        resolve_commit_ids(mut_repo, "roots(root)"),
        vec![root_commit.id().clone()]
    );

    // Roots of a single commit is that commit
    assert_eq!(
        resolve_commit_ids(mut_repo, &format!("roots({})", commit2.id().hex())),
        vec![commit2.id().clone()]
    );

    // Roots of a parent and a child is the parent
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("roots({} | {})", commit2.id().hex(), commit3.id().hex())
        ),
        vec![commit2.id().clone()]
    );

    // Roots of a grandparent and a grandchild is the grandparent (unlike
    // Mercurial's roots() revset, which would include both)
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("roots({} | {})", commit1.id().hex(), commit3.id().hex())
        ),
        vec![commit1.id().clone()]
    );

    // Roots of all commits is the root commit
    assert_eq!(
        resolve_commit_ids(mut_repo, "roots(all())"),
        vec![root_commit.id().clone()]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_parents(use_git: bool) {
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init(&settings, use_git);
    let repo = &test_workspace.repo;

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.initial_commit();
    let commit4 = graph_builder.commit_with_parents(&[&commit2, &commit3]);
    let commit5 = graph_builder.commit_with_parents(&[&commit2]);

    // The root commit has no parents
    assert_eq!(resolve_commit_ids(mut_repo, "root-"), vec![]);

    // Can find parents of the current working-copy commit
    mut_repo
        .set_wc_commit(WorkspaceId::default(), commit2.id().clone())
        .unwrap();
    assert_eq!(
        resolve_commit_ids_in_workspace(mut_repo, "@-", &test_workspace.workspace, None,),
        vec![commit1.id().clone()]
    );

    // Can find parents of a merge commit
    assert_eq!(
        resolve_commit_ids(mut_repo, &format!("{}-", commit4.id().hex())),
        vec![commit3.id().clone(), commit2.id().clone()]
    );

    // Parents of all commits in input are returned
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("({} | {})-", commit2.id().hex(), commit3.id().hex())
        ),
        vec![commit1.id().clone(), root_commit.id().clone()]
    );

    // Parents already in input set are returned
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("({} | {})-", commit1.id().hex(), commit2.id().hex())
        ),
        vec![commit1.id().clone(), root_commit.id().clone()]
    );

    // Parents shared among commits in input are not repeated
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("({} | {})-", commit4.id().hex(), commit5.id().hex())
        ),
        vec![commit3.id().clone(), commit2.id().clone()]
    );

    // Can find parents of parents, which may be optimized to single query
    assert_eq!(
        resolve_commit_ids(mut_repo, &format!("{}--", commit4.id().hex())),
        vec![commit1.id().clone(), root_commit.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("({} | {})--", commit4.id().hex(), commit5.id().hex())
        ),
        vec![commit1.id().clone(), root_commit.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("({} | {})--", commit4.id().hex(), commit2.id().hex())
        ),
        vec![commit1.id().clone(), root_commit.id().clone()]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_children(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    let commit1 = write_random_commit(mut_repo, &settings);
    let commit2 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit1.id().clone()])
        .write()
        .unwrap();
    let commit3 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit2.id().clone()])
        .write()
        .unwrap();
    let commit4 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit1.id().clone()])
        .write()
        .unwrap();
    let commit5 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit3.id().clone(), commit4.id().clone()])
        .write()
        .unwrap();

    // Can find children of the root commit
    assert_eq!(
        resolve_commit_ids(mut_repo, "root+"),
        vec![commit1.id().clone()]
    );

    // Children of all commits in input are returned, including those already in the
    // input set
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("({} | {})+", commit1.id().hex(), commit2.id().hex())
        ),
        vec![
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone()
        ]
    );

    // Children shared among commits in input are not repeated
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("({} | {})+", commit3.id().hex(), commit4.id().hex())
        ),
        vec![commit5.id().clone()]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_ancestors(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit1, &commit3]);

    // The ancestors of the root commit is just the root commit itself
    assert_eq!(
        resolve_commit_ids(mut_repo, ":root"),
        vec![root_commit.id().clone()]
    );

    // Can find ancestors of a specific commit. Commits reachable via multiple paths
    // are not repeated.
    assert_eq!(
        resolve_commit_ids(mut_repo, &format!(":{}", commit4.id().hex())),
        vec![
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone(),
            root_commit.id().clone(),
        ]
    );

    // Can find ancestors of parents or parents of ancestors, which may be optimized
    // to single query
    assert_eq!(
        resolve_commit_ids(mut_repo, &format!(":({}-)", commit4.id().hex()),),
        vec![
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone(),
            root_commit.id().clone(),
        ]
    );
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("(:({}|{}))-", commit3.id().hex(), commit2.id().hex()),
        ),
        vec![
            commit2.id().clone(),
            commit1.id().clone(),
            root_commit.id().clone(),
        ]
    );
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!(":(({}|{})-)", commit3.id().hex(), commit2.id().hex()),
        ),
        vec![
            commit2.id().clone(),
            commit1.id().clone(),
            root_commit.id().clone(),
        ]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_range(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit1, &commit3]);

    // The range from the root to the root is empty (because the left side of the
    // range is exclusive)
    assert_eq!(resolve_commit_ids(mut_repo, "root..root"), vec![]);

    // Linear range
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("{}..{}", commit1.id().hex(), commit3.id().hex())
        ),
        vec![commit3.id().clone(), commit2.id().clone()]
    );

    // Empty range (descendant first)
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("{}..{}", commit3.id().hex(), commit1.id().hex())
        ),
        vec![]
    );

    // Range including a merge
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("{}..{}", commit1.id().hex(), commit4.id().hex())
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
            mut_repo,
            &format!("{}..{}", commit2.id().hex(), commit3.id().hex())
        ),
        vec![commit3.id().clone()]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_dag_range(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let root_commit_id = repo.store().root_commit_id().clone();
    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit1]);
    let commit5 = graph_builder.commit_with_parents(&[&commit3, &commit4]);

    // Can get DAG range of just the root commit
    assert_eq!(
        resolve_commit_ids(mut_repo, "root:root"),
        vec![root_commit_id.clone()]
    );

    // Linear range
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("{}:{}", root_commit_id.hex(), commit2.id().hex())
        ),
        vec![commit2.id().clone(), commit1.id().clone(), root_commit_id]
    );

    // Empty range
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("{}:{}", commit2.id().hex(), commit4.id().hex())
        ),
        vec![]
    );

    // Including a merge
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("{}:{}", commit1.id().hex(), commit5.id().hex())
        ),
        vec![
            commit5.id().clone(),
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone(),
        ]
    );

    // Including a merge, but ancestors only from one side
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("{}:{}", commit2.id().hex(), commit5.id().hex())
        ),
        vec![
            commit5.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
        ]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_connected(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let root_commit_id = repo.store().root_commit_id().clone();
    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit1]);
    let commit5 = graph_builder.commit_with_parents(&[&commit3, &commit4]);

    // Connecting an empty set yields an empty set
    assert_eq!(resolve_commit_ids(mut_repo, "connected(none())"), vec![]);

    // Can connect just the root commit
    assert_eq!(
        resolve_commit_ids(mut_repo, "connected(root)"),
        vec![root_commit_id.clone()]
    );

    // Can connect linearly
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!(
                "connected({} | {})",
                root_commit_id.hex(),
                commit2.id().hex()
            )
        ),
        vec![commit2.id().clone(), commit1.id().clone(), root_commit_id]
    );

    // Siblings don't get connected
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("connected({} | {})", commit2.id().hex(), commit4.id().hex())
        ),
        vec![commit4.id().clone(), commit2.id().clone()]
    );

    // Including a merge
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("connected({} | {})", commit1.id().hex(), commit5.id().hex())
        ),
        vec![
            commit5.id().clone(),
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone(),
        ]
    );

    // Including a merge, but ancestors only from one side
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("connected({} | {})", commit2.id().hex(), commit5.id().hex())
        ),
        vec![
            commit5.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
        ]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_descendants(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    let root_commit_id = repo.store().root_commit_id().clone();
    let commit1 = write_random_commit(mut_repo, &settings);
    let commit2 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit1.id().clone()])
        .write()
        .unwrap();
    let commit3 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit2.id().clone()])
        .write()
        .unwrap();
    let commit4 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit1.id().clone()])
        .write()
        .unwrap();
    let commit5 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit3.id().clone(), commit4.id().clone()])
        .write()
        .unwrap();

    // The descendants of the root commit are all the commits in the repo
    assert_eq!(
        resolve_commit_ids(mut_repo, "root:"),
        vec![
            commit5.id().clone(),
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone(),
            root_commit_id,
        ]
    );

    // Can find descendants of a specific commit
    assert_eq!(
        resolve_commit_ids(mut_repo, &format!("{}:", commit2.id().hex())),
        vec![
            commit5.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
        ]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_none(use_git: bool) {
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    // none() is empty (doesn't include the checkout, for example)
    assert_eq!(resolve_commit_ids(repo, "none()"), vec![]);
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_all(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    let root_commit_id = repo.store().root_commit_id().clone();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit1]);
    let commit4 = graph_builder.commit_with_parents(&[&commit2, &commit3]);

    assert_eq!(
        resolve_commit_ids(mut_repo, "all()"),
        vec![
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone(),
            root_commit_id,
        ]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_visible_heads(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit1]);

    assert_eq!(
        resolve_commit_ids(mut_repo, "heads()"),
        vec![commit3.id().clone(), commit2.id().clone()]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_public_heads(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.initial_commit();

    // Can get public heads with root commit as only public head
    assert_eq!(
        resolve_commit_ids(mut_repo, "public_heads()"),
        vec![root_commit.id().clone()]
    );
    // Can get public heads with a single public head
    mut_repo.add_public_head(&commit1);
    assert_eq!(
        resolve_commit_ids(mut_repo, "public_heads()"),
        vec![commit1.id().clone()]
    );
    // Can get public heads with multiple public head
    mut_repo.add_public_head(&commit2);
    assert_eq!(
        resolve_commit_ids(mut_repo, "public_heads()"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_git_refs(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    let commit1 = write_random_commit(mut_repo, &settings);
    let commit2 = write_random_commit(mut_repo, &settings);
    let commit3 = write_random_commit(mut_repo, &settings);
    let commit4 = write_random_commit(mut_repo, &settings);

    // Can get git refs when there are none
    assert_eq!(resolve_commit_ids(mut_repo, "git_refs()"), vec![]);
    // Can get a mix of git refs
    mut_repo.set_git_ref(
        "refs/heads/branch1".to_string(),
        RefTarget::Normal(commit1.id().clone()),
    );
    mut_repo.set_git_ref(
        "refs/tags/tag1".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, "git_refs()"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    // Two refs pointing to the same commit does not result in a duplicate in the
    // revset
    mut_repo.set_git_ref(
        "refs/tags/tag2".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, "git_refs()"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    // Can get git refs when there are conflicted refs
    mut_repo.set_git_ref(
        "refs/heads/branch1".to_string(),
        RefTarget::Conflict {
            removes: vec![commit1.id().clone()],
            adds: vec![commit2.id().clone(), commit3.id().clone()],
        },
    );
    mut_repo.set_git_ref(
        "refs/tags/tag1".to_string(),
        RefTarget::Conflict {
            removes: vec![commit2.id().clone()],
            adds: vec![commit3.id().clone(), commit4.id().clone()],
        },
    );
    mut_repo.remove_git_ref("refs/tags/tag2");
    assert_eq!(
        resolve_commit_ids(mut_repo, "git_refs()"),
        vec![
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone()
        ]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_git_head(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    let commit1 = write_random_commit(mut_repo, &settings);

    // Can get git head when it's not set
    assert_eq!(resolve_commit_ids(mut_repo, "git_head()"), vec![]);
    mut_repo.set_git_head(RefTarget::Normal(commit1.id().clone()));
    assert_eq!(
        resolve_commit_ids(mut_repo, "git_head()"),
        vec![commit1.id().clone()]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_branches(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    let commit1 = write_random_commit(mut_repo, &settings);
    let commit2 = write_random_commit(mut_repo, &settings);
    let commit3 = write_random_commit(mut_repo, &settings);
    let commit4 = write_random_commit(mut_repo, &settings);

    // Can get branches when there are none
    assert_eq!(resolve_commit_ids(mut_repo, "branches()"), vec![]);
    // Can get a few branches
    mut_repo.set_local_branch(
        "branch1".to_string(),
        RefTarget::Normal(commit1.id().clone()),
    );
    mut_repo.set_local_branch(
        "branch2".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, "branches()"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    // Can get branches with matching names
    assert_eq!(
        resolve_commit_ids(mut_repo, "branches(branch1)"),
        vec![commit1.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, "branches(branch)"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    // Can silently resolve to an empty set if there's no matches
    assert_eq!(resolve_commit_ids(mut_repo, "branches(branch3)"), vec![]);
    // Two branches pointing to the same commit does not result in a duplicate in
    // the revset
    mut_repo.set_local_branch(
        "branch3".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, "branches()"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    // Can get branches when there are conflicted refs
    mut_repo.set_local_branch(
        "branch1".to_string(),
        RefTarget::Conflict {
            removes: vec![commit1.id().clone()],
            adds: vec![commit2.id().clone(), commit3.id().clone()],
        },
    );
    mut_repo.set_local_branch(
        "branch2".to_string(),
        RefTarget::Conflict {
            removes: vec![commit2.id().clone()],
            adds: vec![commit3.id().clone(), commit4.id().clone()],
        },
    );
    mut_repo.remove_local_branch("branch3");
    assert_eq!(
        resolve_commit_ids(mut_repo, "branches()"),
        vec![
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone()
        ]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_remote_branches(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    let commit1 = write_random_commit(mut_repo, &settings);
    let commit2 = write_random_commit(mut_repo, &settings);
    let commit3 = write_random_commit(mut_repo, &settings);
    let commit4 = write_random_commit(mut_repo, &settings);

    // Can get branches when there are none
    assert_eq!(resolve_commit_ids(mut_repo, "remote_branches()"), vec![]);
    // Can get a few branches
    mut_repo.set_remote_branch(
        "branch1".to_string(),
        "origin".to_string(),
        RefTarget::Normal(commit1.id().clone()),
    );
    mut_repo.set_remote_branch(
        "branch2".to_string(),
        "private".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, "remote_branches()"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    // Can get branches with matching names
    assert_eq!(
        resolve_commit_ids(mut_repo, "remote_branches(branch1)"),
        vec![commit1.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, "remote_branches(branch)"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    // Can get branches from matching remotes
    assert_eq!(
        resolve_commit_ids(mut_repo, r#"remote_branches("", origin)"#),
        vec![commit1.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, r#"remote_branches("", ri)"#),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    // Can get branches with matching names from matching remotes
    assert_eq!(
        resolve_commit_ids(mut_repo, "remote_branches(branch1, ri)"),
        vec![commit1.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, r#"remote_branches(branch, private)"#),
        vec![commit2.id().clone()]
    );
    // Can silently resolve to an empty set if there's no matches
    assert_eq!(
        resolve_commit_ids(mut_repo, "remote_branches(branch3)"),
        vec![]
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, r#"remote_branches("", upstream)"#),
        vec![]
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, r#"remote_branches(branch1, private)"#),
        vec![]
    );
    // Two branches pointing to the same commit does not result in a duplicate in
    // the revset
    mut_repo.set_remote_branch(
        "branch3".to_string(),
        "origin".to_string(),
        RefTarget::Normal(commit2.id().clone()),
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, "remote_branches()"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    // The commits don't have to be in the current set of heads to be included.
    mut_repo.remove_head(commit2.id());
    assert_eq!(
        resolve_commit_ids(mut_repo, "remote_branches()"),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    // Can get branches when there are conflicted refs
    mut_repo.set_remote_branch(
        "branch1".to_string(),
        "origin".to_string(),
        RefTarget::Conflict {
            removes: vec![commit1.id().clone()],
            adds: vec![commit2.id().clone(), commit3.id().clone()],
        },
    );
    mut_repo.set_remote_branch(
        "branch2".to_string(),
        "private".to_string(),
        RefTarget::Conflict {
            removes: vec![commit2.id().clone()],
            adds: vec![commit3.id().clone(), commit4.id().clone()],
        },
    );
    mut_repo.remove_remote_branch("branch3", "origin");
    assert_eq!(
        resolve_commit_ids(mut_repo, "remote_branches()"),
        vec![
            commit4.id().clone(),
            commit3.id().clone(),
            commit2.id().clone()
        ]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_merges(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.initial_commit();
    let commit3 = graph_builder.initial_commit();
    let commit4 = graph_builder.commit_with_parents(&[&commit1, &commit2]);
    let commit5 = graph_builder.commit_with_parents(&[&commit1, &commit2, &commit3]);

    // Finds all merges by default
    assert_eq!(
        resolve_commit_ids(mut_repo, "merges()"),
        vec![commit5.id().clone(), commit4.id().clone(),]
    );
    // Searches only among candidates if specified
    assert_eq!(
        resolve_commit_ids(mut_repo, &format!(":{} & merges()", commit5.id().hex())),
        vec![commit5.id().clone()]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_description(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    let commit1 = create_random_commit(mut_repo, &settings)
        .set_description("commit 1")
        .write()
        .unwrap();
    let commit2 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit1.id().clone()])
        .set_description("commit 2")
        .write()
        .unwrap();
    let commit3 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit2.id().clone()])
        .set_description("commit 3")
        .write()
        .unwrap();

    // Can find multiple matches
    assert_eq!(
        resolve_commit_ids(mut_repo, "description(commit)"),
        vec![
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone()
        ]
    );
    // Can find a unique match
    assert_eq!(
        resolve_commit_ids(mut_repo, "description(\"commit 2\")"),
        vec![commit2.id().clone()]
    );
    // Searches only among candidates if specified
    assert_eq!(
        resolve_commit_ids(mut_repo, "heads() & description(\"commit 2\")"),
        vec![]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_author(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    let timestamp = Timestamp {
        timestamp: MillisSinceEpoch(0),
        tz_offset: 0,
    };
    let commit1 = create_random_commit(mut_repo, &settings)
        .set_author(Signature {
            name: "name1".to_string(),
            email: "email1".to_string(),
            timestamp: timestamp.clone(),
        })
        .write()
        .unwrap();
    let commit2 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit1.id().clone()])
        .set_author(Signature {
            name: "name2".to_string(),
            email: "email2".to_string(),
            timestamp: timestamp.clone(),
        })
        .write()
        .unwrap();
    let commit3 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit2.id().clone()])
        .set_author(Signature {
            name: "name3".to_string(),
            email: "email3".to_string(),
            timestamp,
        })
        .write()
        .unwrap();

    // Can find multiple matches
    assert_eq!(
        resolve_commit_ids(mut_repo, "author(name)"),
        vec![
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone()
        ]
    );
    // Can find a unique match by either name or email
    assert_eq!(
        resolve_commit_ids(mut_repo, "author(\"name2\")"),
        vec![commit2.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, "author(\"name3\")"),
        vec![commit3.id().clone()]
    );
    // Searches only among candidates if specified
    assert_eq!(
        resolve_commit_ids(mut_repo, "heads() & author(\"name2\")"),
        vec![]
    );
    // Filter by union of pure predicate and set
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("root.. & (author(name1) | {})", commit3.id().hex())
        ),
        vec![commit3.id().clone(), commit1.id().clone()]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_committer(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    let timestamp = Timestamp {
        timestamp: MillisSinceEpoch(0),
        tz_offset: 0,
    };
    let commit1 = create_random_commit(mut_repo, &settings)
        .set_committer(Signature {
            name: "name1".to_string(),
            email: "email1".to_string(),
            timestamp: timestamp.clone(),
        })
        .write()
        .unwrap();
    let commit2 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit1.id().clone()])
        .set_committer(Signature {
            name: "name2".to_string(),
            email: "email2".to_string(),
            timestamp: timestamp.clone(),
        })
        .write()
        .unwrap();
    let commit3 = create_random_commit(mut_repo, &settings)
        .set_parents(vec![commit2.id().clone()])
        .set_committer(Signature {
            name: "name3".to_string(),
            email: "email3".to_string(),
            timestamp,
        })
        .write()
        .unwrap();

    // Can find multiple matches
    assert_eq!(
        resolve_commit_ids(mut_repo, "committer(name)"),
        vec![
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone()
        ]
    );
    // Can find a unique match by either name or email
    assert_eq!(
        resolve_commit_ids(mut_repo, "committer(\"name2\")"),
        vec![commit2.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(mut_repo, "committer(\"name3\")"),
        vec![commit3.id().clone()]
    );
    // Searches only among candidates if specified
    assert_eq!(
        resolve_commit_ids(mut_repo, "heads() & committer(\"name2\")"),
        vec![]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_union(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings, "test");
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
            mut_repo,
            &format!(":{} | :{}", commit4.id().hex(), commit5.id().hex())
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
            mut_repo,
            &format!(
                "(:{} ~ :{}) | :{}",
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
            mut_repo,
            &format!(
                "(:{} ~ :{}) | {}",
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
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_intersection(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings, "test");
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
            mut_repo,
            &format!(":{} & :{}", commit4.id().hex(), commit5.id().hex())
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
            mut_repo,
            &format!("{} & {}", commit4.id().hex(), commit2.id().hex())
        ),
        vec![]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_difference(use_git: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(use_git);
    let repo = &test_repo.repo;

    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit1 = graph_builder.initial_commit();
    let commit2 = graph_builder.commit_with_parents(&[&commit1]);
    let commit3 = graph_builder.commit_with_parents(&[&commit2]);
    let commit4 = graph_builder.commit_with_parents(&[&commit3]);
    let commit5 = graph_builder.commit_with_parents(&[&commit2]);

    // Difference from all
    assert_eq!(
        resolve_commit_ids(mut_repo, &format!("~:{}", commit5.id().hex())),
        vec![commit4.id().clone(), commit3.id().clone()]
    );

    // Difference between ancestors
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!(":{} ~ :{}", commit4.id().hex(), commit5.id().hex())
        ),
        vec![commit4.id().clone(), commit3.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!(":{} ~ :{}", commit5.id().hex(), commit4.id().hex())
        ),
        vec![commit5.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!("~:{} & :{}", commit4.id().hex(), commit5.id().hex())
        ),
        vec![commit5.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!(":{} ~ :{}", commit4.id().hex(), commit2.id().hex())
        ),
        vec![commit4.id().clone(), commit3.id().clone()]
    );

    // Associativity
    assert_eq!(
        resolve_commit_ids(
            mut_repo,
            &format!(
                ":{} ~ {} ~ {}",
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
            mut_repo,
            &format!(
                "(:{} ~ :{}) ~ (:{} ~ :{})",
                commit4.id().hex(),
                commit1.id().hex(),
                commit3.id().hex(),
                commit1.id().hex(),
            )
        ),
        vec![commit4.id().clone()]
    );
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
fn test_evaluate_expression_file(use_git: bool) {
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init(&settings, use_git);
    let repo = &test_workspace.repo;

    let mut tx = repo.start_transaction(&settings, "test");
    let mut_repo = tx.mut_repo();

    let added_clean_clean = RepoPath::from_internal_string("added_clean_clean");
    let added_modified_clean = RepoPath::from_internal_string("added_modified_clean");
    let added_modified_removed = RepoPath::from_internal_string("added_modified_removed");
    let tree1 = testutils::create_tree(
        repo,
        &[
            (&added_clean_clean, "1"),
            (&added_modified_clean, "1"),
            (&added_modified_removed, "1"),
        ],
    );
    let tree2 = testutils::create_tree(
        repo,
        &[
            (&added_clean_clean, "1"),
            (&added_modified_clean, "2"),
            (&added_modified_removed, "2"),
        ],
    );
    let tree3 = testutils::create_tree(
        repo,
        &[
            (&added_clean_clean, "1"),
            (&added_modified_clean, "2"),
            // added_modified_removed,
        ],
    );
    let commit1 = mut_repo
        .new_commit(
            &settings,
            vec![repo.store().root_commit_id().clone()],
            tree1.id().clone(),
        )
        .write()
        .unwrap();
    let commit2 = mut_repo
        .new_commit(&settings, vec![commit1.id().clone()], tree2.id().clone())
        .write()
        .unwrap();
    let commit3 = mut_repo
        .new_commit(&settings, vec![commit2.id().clone()], tree3.id().clone())
        .write()
        .unwrap();
    let commit4 = mut_repo
        .new_commit(&settings, vec![commit3.id().clone()], tree3.id().clone())
        .write()
        .unwrap();

    let resolve = |file_path: &RepoPath| -> Vec<CommitId> {
        let mut_repo = &*mut_repo;
        let expression =
            RevsetExpression::filter(RevsetFilterPredicate::File(Some(vec![file_path.clone()])));
        let revset = expression.evaluate(mut_repo, None).unwrap();
        let commit_ids = revset.iter().commit_ids().collect();
        commit_ids
    };

    assert_eq!(resolve(&added_clean_clean), vec![commit1.id().clone()]);
    assert_eq!(
        resolve(&added_modified_clean),
        vec![commit2.id().clone(), commit1.id().clone()]
    );
    assert_eq!(
        resolve(&added_modified_removed),
        vec![
            commit3.id().clone(),
            commit2.id().clone(),
            commit1.id().clone()
        ]
    );

    // file() revset:
    assert_eq!(
        resolve_commit_ids_in_workspace(
            mut_repo,
            r#"file("repo/added_clean_clean")"#,
            &test_workspace.workspace,
            Some(test_workspace.workspace.workspace_root().parent().unwrap()),
        ),
        vec![commit1.id().clone()]
    );
    assert_eq!(
        resolve_commit_ids_in_workspace(
            mut_repo,
            &format!(r#"{}: & file("added_modified_clean")"#, commit2.id().hex()),
            &test_workspace.workspace,
            Some(test_workspace.workspace.workspace_root()),
        ),
        vec![commit2.id().clone()]
    );

    // empty() revset, which is identical to ~file(".")
    assert_eq!(
        resolve_commit_ids(mut_repo, &format!("{}: & empty()", commit1.id().hex())),
        vec![commit4.id().clone()]
    );
}
