// Copyright 2020 Google LLC
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

use std::path::PathBuf;
use std::sync::Arc;

use git2::Oid;
use jujutsu_lib::backend::CommitId;
use jujutsu_lib::commit::Commit;
use jujutsu_lib::git::{GitFetchError, GitPushError, GitRefUpdate};
use jujutsu_lib::op_store::{BranchTarget, RefTarget};
use jujutsu_lib::repo::ReadonlyRepo;
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::testutils::create_random_commit;
use jujutsu_lib::{git, testutils};
use maplit::{btreemap, hashset};
use tempfile::TempDir;

fn empty_git_commit<'r>(
    git_repo: &'r git2::Repository,
    ref_name: &str,
    parents: &[&git2::Commit],
) -> git2::Commit<'r> {
    let signature = git2::Signature::now("Someone", "someone@example.com").unwrap();
    let empty_tree_id = Oid::from_str("4b825dc642cb6eb9a060e54bf8d69288fbee4904").unwrap();
    let empty_tree = git_repo.find_tree(empty_tree_id).unwrap();
    let oid = git_repo
        .commit(
            Some(ref_name),
            &signature,
            &signature,
            &format!("random commit {}", rand::random::<u32>()),
            &empty_tree,
            parents,
        )
        .unwrap();
    git_repo.find_commit(oid).unwrap()
}

fn commit_id(commit: &git2::Commit) -> CommitId {
    CommitId(commit.id().as_bytes().to_vec())
}

#[test]
fn test_import_refs() {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, true);
    let git_repo = repo.store().git_repo().unwrap();

    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_ref(&git_repo, "refs/remotes/origin/main", commit1.id());
    let commit2 = empty_git_commit(&git_repo, "refs/heads/main", &[&commit1]);
    let commit3 = empty_git_commit(&git_repo, "refs/heads/feature1", &[&commit2]);
    let commit4 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);
    let commit5 = empty_git_commit(&git_repo, "refs/tags/v1.0", &[&commit1]);
    // Should not be imported
    empty_git_commit(&git_repo, "refs/notes/x", &[&commit2]);

    let git_repo = repo.store().git_repo().unwrap();
    let mut tx = repo.start_transaction("test");
    jujutsu_lib::git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    let repo = tx.commit();
    let view = repo.view();

    let expected_heads = hashset! {
        view.checkout().clone(),
        commit_id(&commit3),
        commit_id(&commit4),
        commit_id(&commit5)
    };
    assert_eq!(*view.heads(), expected_heads);
    assert_eq!(*view.public_heads(), hashset!(commit_id(&commit1)));

    let expected_main_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(commit_id(&commit2))),
        remote_targets: btreemap! {
          "origin".to_string() => RefTarget::Normal(commit_id(&commit1)),
        },
    };
    assert_eq!(
        view.branches().get("main"),
        Some(expected_main_branch).as_ref()
    );
    let expected_feature1_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(commit_id(&commit3))),
        remote_targets: btreemap! {},
    };
    assert_eq!(
        view.branches().get("feature1"),
        Some(expected_feature1_branch).as_ref()
    );
    let expected_feature2_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(commit_id(&commit4))),
        remote_targets: btreemap! {},
    };
    assert_eq!(
        view.branches().get("feature2"),
        Some(expected_feature2_branch).as_ref()
    );

    assert_eq!(
        view.tags().get("v1.0"),
        Some(RefTarget::Normal(commit_id(&commit5))).as_ref()
    );

    assert_eq!(view.git_refs().len(), 5);
    assert_eq!(
        view.git_refs().get("refs/heads/main"),
        Some(RefTarget::Normal(commit_id(&commit2))).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/heads/feature1"),
        Some(RefTarget::Normal(commit_id(&commit3))).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/heads/feature2"),
        Some(RefTarget::Normal(commit_id(&commit4))).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/remotes/origin/main"),
        Some(RefTarget::Normal(commit_id(&commit1))).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/tags/v1.0"),
        Some(RefTarget::Normal(commit_id(&commit5))).as_ref()
    );
}

#[test]
fn test_import_refs_reimport() {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, true);
    let git_repo = repo.store().git_repo().unwrap();

    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_ref(&git_repo, "refs/remotes/origin/main", commit1.id());
    let commit2 = empty_git_commit(&git_repo, "refs/heads/main", &[&commit1]);
    let commit3 = empty_git_commit(&git_repo, "refs/heads/feature1", &[&commit2]);
    let commit4 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);
    let pgp_key_oid = git_repo.blob(b"my PGP key").unwrap();
    git_repo
        .reference("refs/tags/my-gpg-key", pgp_key_oid, false, "")
        .unwrap();

    let mut tx = repo.start_transaction("test");
    jujutsu_lib::git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    let repo = tx.commit();

    // Delete feature1 and rewrite feature2
    delete_git_ref(&git_repo, "refs/heads/feature1");
    delete_git_ref(&git_repo, "refs/heads/feature2");
    let commit5 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);

    // Also modify feature2 on the jj side
    let mut tx = repo.start_transaction("test");
    let commit6 = create_random_commit(&settings, &repo)
        .set_parents(vec![commit_id(&commit2)])
        .write_to_repo(tx.mut_repo());
    tx.mut_repo().set_local_branch(
        "feature2".to_string(),
        RefTarget::Normal(commit6.id().clone()),
    );
    let repo = tx.commit();

    let mut tx = repo.start_transaction("test");
    jujutsu_lib::git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    let repo = tx.commit();

    let view = repo.view();
    // TODO: commit3 and commit4 should probably be removed
    let expected_heads = hashset! {
            view.checkout().clone(),
            commit_id(&commit3),
            commit_id(&commit4),
            commit_id(&commit5),
            commit6.id().clone(),
    };
    assert_eq!(*view.heads(), expected_heads);

    assert_eq!(view.branches().len(), 2);
    let commit1_target = RefTarget::Normal(commit_id(&commit1));
    let commit2_target = RefTarget::Normal(commit_id(&commit2));
    let expected_main_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(commit_id(&commit2))),
        remote_targets: btreemap! {
          "origin".to_string() => commit1_target.clone(),
        },
    };
    assert_eq!(
        view.branches().get("main"),
        Some(expected_main_branch).as_ref()
    );
    let expected_feature2_branch = BranchTarget {
        local_target: Some(RefTarget::Conflict {
            removes: vec![commit_id(&commit4)],
            adds: vec![commit6.id().clone(), commit_id(&commit5)],
        }),
        remote_targets: btreemap! {},
    };
    assert_eq!(
        view.branches().get("feature2"),
        Some(expected_feature2_branch).as_ref()
    );

    assert!(view.tags().is_empty());

    assert_eq!(view.git_refs().len(), 3);
    assert_eq!(
        view.git_refs().get("refs/heads/main"),
        Some(commit2_target).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/remotes/origin/main"),
        Some(commit1_target).as_ref()
    );
    let commit5_target = RefTarget::Normal(commit_id(&commit5));
    assert_eq!(
        view.git_refs().get("refs/heads/feature2"),
        Some(commit5_target).as_ref()
    );
}

fn git_ref(git_repo: &git2::Repository, name: &str, target: Oid) {
    git_repo.reference(name, target, true, "").unwrap();
}

fn delete_git_ref(git_repo: &git2::Repository, name: &str) {
    git_repo.find_reference(name).unwrap().delete().unwrap();
}

#[test]
fn test_import_refs_empty_git_repo() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let git_repo_dir = temp_dir.path().join("source");
    let jj_repo_dir = temp_dir.path().join("jj");

    let git_repo = git2::Repository::init_bare(&git_repo_dir).unwrap();

    std::fs::create_dir(&jj_repo_dir).unwrap();
    let repo = ReadonlyRepo::init_external_git(&settings, jj_repo_dir, git_repo_dir).unwrap();
    let heads_before = repo.view().heads().clone();
    let mut tx = repo.start_transaction("test");
    jujutsu_lib::git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    let repo = tx.commit();
    assert_eq!(*repo.view().heads(), heads_before);
    assert_eq!(repo.view().branches().len(), 0);
    assert_eq!(repo.view().tags().len(), 0);
    assert_eq!(repo.view().git_refs().len(), 0);
}

#[test]
fn test_init() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let git_repo_dir = temp_dir.path().join("git");
    let jj_repo_dir = temp_dir.path().join("jj");
    let git_repo = git2::Repository::init_bare(&git_repo_dir).unwrap();
    let initial_git_commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    let initial_commit_id = commit_id(&initial_git_commit);
    std::fs::create_dir(&jj_repo_dir).unwrap();
    let repo = ReadonlyRepo::init_external_git(&settings, jj_repo_dir, git_repo_dir).unwrap();
    // The refs were *not* imported -- it's the caller's responsibility to import
    // any refs they care about.
    assert!(!repo.view().heads().contains(&initial_commit_id));
}

#[test]
fn test_fetch_success() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let source_repo_dir = temp_dir.path().join("source");
    let clone_repo_dir = temp_dir.path().join("clone");
    let jj_repo_dir = temp_dir.path().join("jj");
    let source_git_repo = git2::Repository::init_bare(&source_repo_dir).unwrap();
    let initial_git_commit = empty_git_commit(&source_git_repo, "refs/heads/main", &[]);
    let clone_git_repo =
        git2::Repository::clone(source_repo_dir.to_str().unwrap(), &clone_repo_dir).unwrap();
    std::fs::create_dir(&jj_repo_dir).unwrap();
    let jj_repo = ReadonlyRepo::init_external_git(&settings, jj_repo_dir, clone_repo_dir).unwrap();

    let new_git_commit =
        empty_git_commit(&source_git_repo, "refs/heads/main", &[&initial_git_commit]);

    // The new commit is not visible before git::fetch() even if we reload the repo.
    assert!(!jj_repo
        .reload()
        .view()
        .heads()
        .contains(&commit_id(&new_git_commit)));

    let mut tx = jj_repo.start_transaction("test");
    let default_branch = git::fetch(tx.mut_repo(), &clone_git_repo, "origin").unwrap();
    // The default branch is "main"
    assert_eq!(default_branch, Some("main".to_string()));
    let repo = tx.commit();
    // The new commit is visible after git::fetch().
    assert!(repo.view().heads().contains(&commit_id(&new_git_commit)));
}

#[test]
fn test_fetch_no_default_branch() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let source_repo_dir = temp_dir.path().join("source");
    let clone_repo_dir = temp_dir.path().join("clone");
    let jj_repo_dir = temp_dir.path().join("jj");
    let source_git_repo = git2::Repository::init_bare(&source_repo_dir).unwrap();
    let initial_git_commit = empty_git_commit(&source_git_repo, "refs/heads/main", &[]);
    let clone_git_repo =
        git2::Repository::clone(source_repo_dir.to_str().unwrap(), &clone_repo_dir).unwrap();
    std::fs::create_dir(&jj_repo_dir).unwrap();
    let jj_repo = ReadonlyRepo::init_external_git(&settings, jj_repo_dir, clone_repo_dir).unwrap();

    empty_git_commit(&source_git_repo, "refs/heads/main", &[&initial_git_commit]);
    // It's actually not enough to have a detached HEAD, it also needs to point to a
    // commit without a commit (that's possibly a bug in Git *and* libgit2), so
    // we point it to initial_git_commit.
    source_git_repo
        .set_head_detached(initial_git_commit.id())
        .unwrap();

    let mut tx = jj_repo.start_transaction("test");
    let default_branch = git::fetch(tx.mut_repo(), &clone_git_repo, "origin").unwrap();
    // There is no default branch
    assert_eq!(default_branch, None);
    tx.discard();
}

#[test]
fn test_fetch_no_such_remote() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let source_repo_dir = temp_dir.path().join("source");
    let jj_repo_dir = temp_dir.path().join("jj");
    let git_repo = git2::Repository::init_bare(&source_repo_dir).unwrap();
    std::fs::create_dir(&jj_repo_dir).unwrap();
    let jj_repo = ReadonlyRepo::init_external_git(&settings, jj_repo_dir, source_repo_dir).unwrap();

    let mut tx = jj_repo.start_transaction("test");
    let result = git::fetch(tx.mut_repo(), &git_repo, "invalid-remote");
    assert!(matches!(result, Err(GitFetchError::NoSuchRemote(_))));
    tx.discard();
}

struct PushTestSetup {
    source_repo_dir: PathBuf,
    jj_repo: Arc<ReadonlyRepo>,
    new_commit: Commit,
}

fn set_up_push_repos(settings: &UserSettings, temp_dir: &TempDir) -> PushTestSetup {
    let source_repo_dir = temp_dir.path().join("source");
    let clone_repo_dir = temp_dir.path().join("clone");
    let jj_repo_dir = temp_dir.path().join("jj");
    let source_repo = git2::Repository::init_bare(&source_repo_dir).unwrap();
    let initial_git_commit = empty_git_commit(&source_repo, "refs/heads/main", &[]);
    let initial_commit_id = commit_id(&initial_git_commit);
    git2::Repository::clone(source_repo_dir.to_str().unwrap(), &clone_repo_dir).unwrap();
    std::fs::create_dir(&jj_repo_dir).unwrap();
    let jj_repo = ReadonlyRepo::init_external_git(settings, jj_repo_dir, clone_repo_dir).unwrap();
    let mut tx = jj_repo.start_transaction("test");
    let new_commit = testutils::create_random_commit(settings, &jj_repo)
        .set_parents(vec![initial_commit_id])
        .write_to_repo(tx.mut_repo());
    let jj_repo = tx.commit();
    PushTestSetup {
        source_repo_dir,
        jj_repo,
        new_commit,
    }
}

#[test]
fn test_push_updates_success() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let clone_repo = setup.jj_repo.store().git_repo().unwrap();
    let result = git::push_updates(
        &clone_repo,
        "origin",
        &[GitRefUpdate {
            qualified_name: "refs/heads/main".to_string(),
            force: false,
            new_target: Some(setup.new_commit.id().clone()),
        }],
    );
    assert_eq!(result, Ok(()));

    // Check that the ref got updated in the source repo
    let source_repo = git2::Repository::open(&setup.source_repo_dir).unwrap();
    let new_target = source_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .target();
    let new_oid = Oid::from_bytes(&setup.new_commit.id().0).unwrap();
    assert_eq!(new_target, Some(new_oid));

    // Check that the ref got updated in the cloned repo. This just tests our
    // assumptions about libgit2 because we want the refs/remotes/origin/main
    // branch to be updated.
    let new_target = clone_repo
        .find_reference("refs/remotes/origin/main")
        .unwrap()
        .target();
    assert_eq!(new_target, Some(new_oid));
}

#[test]
fn test_push_updates_deletion() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let clone_repo = setup.jj_repo.store().git_repo().unwrap();

    let source_repo = git2::Repository::open(&setup.source_repo_dir).unwrap();
    // Test the setup
    assert!(source_repo.find_reference("refs/heads/main").is_ok());

    let result = git::push_updates(
        &setup.jj_repo.store().git_repo().unwrap(),
        "origin",
        &[GitRefUpdate {
            qualified_name: "refs/heads/main".to_string(),
            force: false,
            new_target: None,
        }],
    );
    assert_eq!(result, Ok(()));

    // Check that the ref got deleted in the source repo
    assert!(source_repo.find_reference("refs/heads/main").is_err());

    // Check that the ref got deleted in the cloned repo. This just tests our
    // assumptions about libgit2 because we want the refs/remotes/origin/main
    // branch to be deleted.
    assert!(clone_repo
        .find_reference("refs/remotes/origin/main")
        .is_err());
}

#[test]
fn test_push_updates_mixed_deletion_and_addition() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let clone_repo = setup.jj_repo.store().git_repo().unwrap();
    let result = git::push_updates(
        &clone_repo,
        "origin",
        &[
            GitRefUpdate {
                qualified_name: "refs/heads/main".to_string(),
                force: false,
                new_target: None,
            },
            GitRefUpdate {
                qualified_name: "refs/heads/topic".to_string(),
                force: false,
                new_target: Some(setup.new_commit.id().clone()),
            },
        ],
    );
    assert_eq!(result, Ok(()));

    // Check that the topic ref got updated in the source repo
    let source_repo = git2::Repository::open(&setup.source_repo_dir).unwrap();
    let new_target = source_repo
        .find_reference("refs/heads/topic")
        .unwrap()
        .target();
    let new_oid = Oid::from_bytes(&setup.new_commit.id().0).unwrap();
    assert_eq!(new_target, Some(new_oid));

    // Check that the main ref got deleted in the source repo
    assert!(source_repo.find_reference("refs/heads/main").is_err());
}

#[test]
fn test_push_updates_not_fast_forward() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let mut setup = set_up_push_repos(&settings, &temp_dir);
    let mut tx = setup.jj_repo.start_transaction("test");
    let new_commit =
        testutils::create_random_commit(&settings, &setup.jj_repo).write_to_repo(tx.mut_repo());
    setup.jj_repo = tx.commit();
    let result = git::push_updates(
        &setup.jj_repo.store().git_repo().unwrap(),
        "origin",
        &[GitRefUpdate {
            qualified_name: "refs/heads/main".to_string(),
            force: false,
            new_target: Some(new_commit.id().clone()),
        }],
    );
    assert_eq!(result, Err(GitPushError::NotFastForward));
}

#[test]
fn test_push_updates_not_fast_forward_with_force() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let mut setup = set_up_push_repos(&settings, &temp_dir);
    let mut tx = setup.jj_repo.start_transaction("test");
    let new_commit =
        testutils::create_random_commit(&settings, &setup.jj_repo).write_to_repo(tx.mut_repo());
    setup.jj_repo = tx.commit();
    let result = git::push_updates(
        &setup.jj_repo.store().git_repo().unwrap(),
        "origin",
        &[GitRefUpdate {
            qualified_name: "refs/heads/main".to_string(),
            force: true,
            new_target: Some(new_commit.id().clone()),
        }],
    );
    assert_eq!(result, Ok(()));

    // Check that the ref got updated in the source repo
    let source_repo = git2::Repository::open(&setup.source_repo_dir).unwrap();
    let new_target = source_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .target();
    let new_oid = Oid::from_bytes(&new_commit.id().0).unwrap();
    assert_eq!(new_target, Some(new_oid));
}

#[test]
fn test_push_updates_no_such_remote() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let result = git::push_updates(
        &setup.jj_repo.store().git_repo().unwrap(),
        "invalid-remote",
        &[GitRefUpdate {
            qualified_name: "refs/heads/main".to_string(),
            force: false,
            new_target: Some(setup.new_commit.id().clone()),
        }],
    );
    assert!(matches!(result, Err(GitPushError::NoSuchRemote(_))));
}

#[test]
fn test_push_updates_invalid_remote() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let result = git::push_updates(
        &setup.jj_repo.store().git_repo().unwrap(),
        "http://invalid-remote",
        &[GitRefUpdate {
            qualified_name: "refs/heads/main".to_string(),
            force: false,
            new_target: Some(setup.new_commit.id().clone()),
        }],
    );
    assert!(matches!(result, Err(GitPushError::NoSuchRemote(_))));
}
