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

use git2::Oid;
use jujube_lib::commit::Commit;
use jujube_lib::git;
use jujube_lib::git::{GitFetchError, GitPushError};
use jujube_lib::repo::{ReadonlyRepo, Repo};
use jujube_lib::settings::UserSettings;
use jujube_lib::store::CommitId;
use jujube_lib::testutils;
use maplit::hashset;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
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
            &format!("commit on {}", ref_name),
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
    let commit2 = empty_git_commit(&git_repo, "refs/heads/main", &[&commit1]);
    let commit3 = empty_git_commit(&git_repo, "refs/heads/feature1", &[&commit2]);
    let commit4 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);
    let commit5 = empty_git_commit(&git_repo, "refs/remotes/origin/main", &[&commit2]);
    // Should not be imported
    empty_git_commit(&git_repo, "refs/notes/x", &[&commit2]);

    let git_repo = repo.store().git_repo().unwrap();
    let mut tx = repo.start_transaction("test");
    let heads_before: HashSet<_> = repo.view().heads().clone();
    jujube_lib::git::import_refs(&mut tx, &git_repo).unwrap_or_default();
    let view = tx.as_repo().view();
    let expected_heads: HashSet<_> = heads_before
        .union(&hashset!(
            commit_id(&commit3),
            commit_id(&commit4),
            commit_id(&commit5)
        ))
        .cloned()
        .collect();
    assert_eq!(*view.heads(), expected_heads);
    assert_eq!(*view.public_heads(), hashset!(commit_id(&commit5)));
    assert_eq!(view.git_refs().len(), 4);
    assert_eq!(
        view.git_refs().get("refs/heads/main"),
        Some(commit_id(&commit2)).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/heads/feature1"),
        Some(commit_id(&commit3)).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/heads/feature2"),
        Some(commit_id(&commit4)).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/remotes/origin/main"),
        Some(commit_id(&commit5)).as_ref()
    );
    tx.discard();
}

#[test]
fn test_import_refs_reimport() {
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, true);
    let git_repo = repo.store().git_repo().unwrap();

    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    let commit2 = empty_git_commit(&git_repo, "refs/heads/main", &[&commit1]);
    let commit3 = empty_git_commit(&git_repo, "refs/heads/feature1", &[&commit2]);
    let commit4 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);

    let heads_before = repo.view().heads().clone();
    let mut tx = repo.start_transaction("test");
    jujube_lib::git::import_refs(&mut tx, &git_repo).unwrap_or_default();
    tx.commit();

    // Delete feature1 and rewrite feature2
    delete_git_ref(&git_repo, "refs/heads/feature1");
    delete_git_ref(&git_repo, "refs/heads/feature2");
    let commit5 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);

    Arc::get_mut(&mut repo).unwrap().reload();
    let mut tx = repo.start_transaction("test");
    jujube_lib::git::import_refs(&mut tx, &git_repo).unwrap_or_default();

    let view = tx.as_repo().view();
    // TODO: commit3 and commit4 should probably be removed
    let expected_heads: HashSet<_> = heads_before
        .union(&hashset!(
            commit_id(&commit3),
            commit_id(&commit4),
            commit_id(&commit5)
        ))
        .cloned()
        .collect();
    assert_eq!(*view.heads(), expected_heads);
    assert_eq!(view.git_refs().len(), 2);
    assert_eq!(
        view.git_refs().get("refs/heads/main"),
        Some(commit_id(&commit2)).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/heads/feature2"),
        Some(commit_id(&commit5)).as_ref()
    );
    tx.discard();
}

fn git_ref(git_repo: &git2::Repository, name: &str, target: Oid) {
    git_repo.reference(name, target, true, "").unwrap();
}

fn delete_git_ref(git_repo: &git2::Repository, name: &str) {
    git_repo.find_reference(name).unwrap().delete().unwrap();
}

#[test]
fn test_import_refs_merge() {
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, true);
    let git_repo = repo.store().git_repo().unwrap();

    // Set up the following refs and update them as follows:
    // sideways-unchanged: one operation rewrites the ref
    // unchanged-sideways: the other operation rewrites the ref
    // remove-unchanged: one operation removes the ref
    // unchanged-remove: the other operation removes the ref
    // forward-forward: two operations move forward by different amounts
    // sideways-sideways: two operations rewrite the ref
    // forward-remove: one operation moves forward, the other operation removes
    // remove-forward: one operation removes, the other operation moves
    // add-add: two operations add the ref with different target
    //
    // The above cases distinguish between refs moving forward and sideways (and
    // there are no tests for refs moving backward) because we may want to treat
    // the cases differently, although that's still unclear.
    //
    // TODO: Consider adding more systematic testing to cover
    // all state transitions. For example, the above does not include a case
    // where a ref is added on both sides and one is an ancestor of the other
    // (we should probably resolve that in favor of the descendant).
    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    let commit2 = empty_git_commit(&git_repo, "refs/heads/main", &[&commit1]);
    let commit3 = empty_git_commit(&git_repo, "refs/heads/main", &[&commit2]);
    let commit4 = empty_git_commit(&git_repo, "refs/heads/feature1", &[&commit2]);
    let commit5 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);
    git_ref(&git_repo, "refs/heads/sideways-unchanged", commit3.id());
    git_ref(&git_repo, "refs/heads/unchanged-sideways", commit3.id());
    git_ref(&git_repo, "refs/heads/remove-unchanged", commit3.id());
    git_ref(&git_repo, "refs/heads/unchanged-remove", commit3.id());
    git_ref(&git_repo, "refs/heads/sideways-sideways", commit3.id());
    git_ref(&git_repo, "refs/heads/forward-forward", commit1.id());
    git_ref(&git_repo, "refs/heads/forward-remove", commit1.id());
    git_ref(&git_repo, "refs/heads/remove-forward", commit1.id());
    let mut tx = repo.start_transaction("initial import");
    jujube_lib::git::import_refs(&mut tx, &git_repo).unwrap_or_default();
    tx.commit();
    Arc::get_mut(&mut repo).unwrap().reload();

    // One of the concurrent operations:
    git_ref(&git_repo, "refs/heads/sideways-unchanged", commit4.id());
    delete_git_ref(&git_repo, "refs/heads/remove-unchanged");
    git_ref(&git_repo, "refs/heads/sideways-sideways", commit4.id());
    git_ref(&git_repo, "refs/heads/forward-forward", commit2.id());
    git_ref(&git_repo, "refs/heads/forward-remove", commit2.id());
    delete_git_ref(&git_repo, "refs/heads/remove-forward");
    git_ref(&git_repo, "refs/heads/add-add", commit3.id());
    let mut tx1 = repo.start_transaction("concurrent import 1");
    jujube_lib::git::import_refs(&mut tx1, &git_repo).unwrap_or_default();
    tx1.commit();

    // The other concurrent operation:
    git_ref(&git_repo, "refs/heads/unchanged-sideways", commit4.id());
    delete_git_ref(&git_repo, "refs/heads/unchanged-remove");
    git_ref(&git_repo, "refs/heads/sideways-sideways", commit5.id());
    git_ref(&git_repo, "refs/heads/forward-forward", commit3.id());
    delete_git_ref(&git_repo, "refs/heads/forward-remove");
    git_ref(&git_repo, "refs/heads/remove-forward", commit2.id());
    git_ref(&git_repo, "refs/heads/add-add", commit4.id());
    let mut tx2 = repo.start_transaction("concurrent import 2");
    jujube_lib::git::import_refs(&mut tx2, &git_repo).unwrap_or_default();
    tx2.commit();

    // Reload the repo, causing the operations to be merged.
    Arc::get_mut(&mut repo).unwrap().reload();

    let view = repo.view();
    let git_refs = view.git_refs();
    assert_eq!(git_refs.len(), 9);
    assert_eq!(
        git_refs.get("refs/heads/sideways-unchanged"),
        Some(commit_id(&commit4)).as_ref()
    );
    assert_eq!(
        git_refs.get("refs/heads/unchanged-sideways"),
        Some(commit_id(&commit4)).as_ref()
    );
    assert_eq!(git_refs.get("refs/heads/remove-unchanged"), None);
    assert_eq!(git_refs.get("refs/heads/unchanged-remove"), None);
    // TODO: Perhaps we should automatically resolve this to the descendant-most
    // commit? (We currently do get the descendant-most, but that's only because we
    // let the later operation overwrite.)
    assert_eq!(
        git_refs.get("refs/heads/forward-forward"),
        Some(commit_id(&commit3)).as_ref()
    );
    // TODO: The rest of these should be conflicts (however we decide to represent
    // that).
    assert_eq!(
        git_refs.get("refs/heads/sideways-sideways"),
        Some(commit_id(&commit5)).as_ref()
    );
    assert_eq!(git_refs.get("refs/heads/forward-remove"), None);
    assert_eq!(
        git_refs.get("refs/heads/remove-forward"),
        Some(commit_id(&commit2)).as_ref()
    );
    assert_eq!(
        git_refs.get("refs/heads/add-add"),
        Some(commit_id(&commit4)).as_ref()
    );
}

#[test]
fn test_import_refs_empty_git_repo() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let git_repo_dir = temp_dir.path().join("source");
    let jj_repo_dir = temp_dir.path().join("jj");

    let git_repo = git2::Repository::init_bare(&git_repo_dir).unwrap();

    std::fs::create_dir(&jj_repo_dir).unwrap();
    let repo = ReadonlyRepo::init_external_git(&settings, jj_repo_dir, git_repo_dir);
    let heads_before = repo.view().heads().clone();
    let mut tx = repo.start_transaction("test");
    jujube_lib::git::import_refs(&mut tx, &git_repo).unwrap_or_default();
    let view = tx.as_repo().view();
    assert_eq!(*view.heads(), heads_before);
    assert_eq!(view.git_refs().len(), 0);
    tx.discard();
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
    let repo = ReadonlyRepo::init_external_git(&settings, jj_repo_dir.clone(), git_repo_dir);
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
        git2::Repository::clone(&source_repo_dir.to_str().unwrap(), &clone_repo_dir).unwrap();
    std::fs::create_dir(&jj_repo_dir).unwrap();
    ReadonlyRepo::init_external_git(&settings, jj_repo_dir.clone(), clone_repo_dir.clone());

    let new_git_commit =
        empty_git_commit(&source_git_repo, "refs/heads/main", &[&initial_git_commit]);

    // The new commit is not visible before git::fetch().
    let jj_repo = ReadonlyRepo::load(&settings, jj_repo_dir.clone()).unwrap();
    assert!(!jj_repo.view().heads().contains(&commit_id(&new_git_commit)));

    // The new commit is visible after git::fetch().
    let mut tx = jj_repo.start_transaction("test");
    git::fetch(&mut tx, &clone_git_repo, "origin").unwrap();
    assert!(tx
        .as_repo()
        .view()
        .heads()
        .contains(&commit_id(&new_git_commit)));

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
    let jj_repo =
        ReadonlyRepo::init_external_git(&settings, jj_repo_dir.clone(), source_repo_dir.clone());

    let mut tx = jj_repo.start_transaction("test");
    let result = git::fetch(&mut tx, &git_repo, "invalid-remote");
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
    git2::Repository::clone(&source_repo_dir.to_str().unwrap(), &clone_repo_dir).unwrap();
    std::fs::create_dir(&jj_repo_dir).unwrap();
    let mut jj_repo =
        ReadonlyRepo::init_external_git(&settings, jj_repo_dir.clone(), clone_repo_dir.clone());
    let new_commit = testutils::create_random_commit(&settings, &jj_repo)
        .set_parents(vec![initial_commit_id.clone()])
        .write_to_new_transaction(&jj_repo, "test");
    Arc::get_mut(&mut jj_repo).unwrap().reload();
    PushTestSetup {
        source_repo_dir,
        jj_repo,
        new_commit,
    }
}

#[test]
fn test_push_commit_success() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let clone_repo = setup.jj_repo.store().git_repo().unwrap();
    let result = git::push_commit(&clone_repo, &setup.new_commit, "origin", "main");
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
fn test_push_commit_not_fast_forward() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let mut setup = set_up_push_repos(&settings, &temp_dir);
    let new_commit = testutils::create_random_commit(&settings, &setup.jj_repo)
        .write_to_new_transaction(&setup.jj_repo, "test");
    Arc::get_mut(&mut setup.jj_repo).unwrap().reload();
    let result = git::push_commit(
        &setup.jj_repo.store().git_repo().unwrap(),
        &new_commit,
        "origin",
        "main",
    );
    assert_eq!(result, Err(GitPushError::NotFastForward));
}

#[test]
fn test_push_commit_no_such_remote() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let result = git::push_commit(
        &setup.jj_repo.store().git_repo().unwrap(),
        &setup.new_commit,
        "invalid-remote",
        "main",
    );
    assert!(matches!(result, Err(GitPushError::NoSuchRemote(_))));
}

#[test]
fn test_push_commit_invalid_remote() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let result = git::push_commit(
        &setup.jj_repo.store().git_repo().unwrap(),
        &setup.new_commit,
        "http://invalid-remote",
        "main",
    );
    assert!(matches!(result, Err(GitPushError::NoSuchRemote(_))));
}
