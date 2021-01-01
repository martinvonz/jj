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
use jj_lib::commit::Commit;
use jj_lib::git;
use jj_lib::git::{GitFetchError, GitImportError, GitPushError};
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::settings::UserSettings;
use jj_lib::store::CommitId;
use jj_lib::testutils;
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
    let temp_dir = tempfile::tempdir().unwrap();
    let git_repo_dir = temp_dir.path().join("git");
    let jj_repo_dir = temp_dir.path().join("jj");

    let git_repo = git2::Repository::init_bare(&git_repo_dir).unwrap();
    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    let commit2 = empty_git_commit(&git_repo, "refs/heads/main", &[&commit1]);
    let commit3 = empty_git_commit(&git_repo, "refs/heads/feature1", &[&commit2]);
    let commit4 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);

    std::fs::create_dir(&jj_repo_dir).unwrap();
    let repo = ReadonlyRepo::init_external_git(&settings, jj_repo_dir, git_repo_dir);
    let mut tx = repo.start_transaction("test");
    let heads_before: HashSet<_> = repo.view().heads().cloned().collect();
    jj_lib::git::import_refs(&mut tx).unwrap_or_default();
    let heads_after: HashSet<_> = tx.as_repo().view().heads().cloned().collect();
    let expected_heads: HashSet<_> = heads_before
        .union(&hashset!(commit_id(&commit3), commit_id(&commit4)))
        .cloned()
        .collect();
    assert_eq!(heads_after, expected_heads);
    tx.discard();
}

#[test]
fn test_import_refs_empty_git_repo() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let git_repo_dir = temp_dir.path().join("source");
    let jj_repo_dir = temp_dir.path().join("jj");

    git2::Repository::init_bare(&git_repo_dir).unwrap();

    std::fs::create_dir(&jj_repo_dir).unwrap();
    let repo = ReadonlyRepo::init_external_git(&settings, jj_repo_dir, git_repo_dir);
    let heads_before: HashSet<_> = repo.view().heads().cloned().collect();
    let mut tx = repo.start_transaction("test");
    jj_lib::git::import_refs(&mut tx).unwrap_or_default();
    let heads_after: HashSet<_> = tx.as_repo().view().heads().cloned().collect();
    assert_eq!(heads_before, heads_after);
    tx.discard();
}

#[test]
fn test_import_refs_non_git() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let jj_repo_dir = temp_dir.path().join("jj");

    std::fs::create_dir(&jj_repo_dir).unwrap();
    let repo = ReadonlyRepo::init_local(&settings, jj_repo_dir);
    let mut tx = repo.start_transaction("test");
    let result = jj_lib::git::import_refs(&mut tx);
    assert_eq!(result, Err(GitImportError::NotAGitRepo));
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
    let heads: HashSet<_> = repo.view().heads().cloned().collect();
    // The refs were *not* imported -- it's the caller's responsibility to import
    // any refs they care about.
    assert!(!heads.contains(&initial_commit_id));
}

#[test]
fn test_fetch_success() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let source_repo_dir = temp_dir.path().join("source");
    let clone_repo_dir = temp_dir.path().join("clone");
    let jj_repo_dir = temp_dir.path().join("jj");
    let git_repo = git2::Repository::init_bare(&source_repo_dir).unwrap();
    let initial_git_commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git2::Repository::clone(&source_repo_dir.to_str().unwrap(), &clone_repo_dir).unwrap();
    std::fs::create_dir(&jj_repo_dir).unwrap();
    ReadonlyRepo::init_external_git(&settings, jj_repo_dir.clone(), clone_repo_dir.clone());

    let new_git_commit = empty_git_commit(&git_repo, "refs/heads/main", &[&initial_git_commit]);

    // The new commit is not visible before git::fetch().
    let jj_repo = ReadonlyRepo::load(&settings, jj_repo_dir.clone());
    let heads: HashSet<_> = jj_repo.view().heads().cloned().collect();
    assert!(!heads.contains(&commit_id(&new_git_commit)));

    // The new commit is visible after git::fetch().
    let mut tx = jj_repo.start_transaction("test");
    git::fetch(&mut tx, "origin").unwrap();
    let heads: HashSet<_> = tx.as_repo().view().heads().cloned().collect();
    assert!(heads.contains(&commit_id(&new_git_commit)));

    tx.discard();
}

#[test]
fn test_fetch_non_git() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let jj_repo_dir = temp_dir.path().join("jj");
    std::fs::create_dir(&jj_repo_dir).unwrap();
    let jj_repo = ReadonlyRepo::init_local(&settings, jj_repo_dir);

    let mut tx = jj_repo.start_transaction("test");
    let result = git::fetch(&mut tx, "origin");
    assert_eq!(result, Err(GitFetchError::NotAGitRepo));

    tx.discard();
}

#[test]
fn test_fetch_no_such_remote() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let source_repo_dir = temp_dir.path().join("source");
    let jj_repo_dir = temp_dir.path().join("jj");
    git2::Repository::init_bare(&source_repo_dir).unwrap();
    std::fs::create_dir(&jj_repo_dir).unwrap();
    let jj_repo =
        ReadonlyRepo::init_external_git(&settings, jj_repo_dir.clone(), source_repo_dir.clone());

    let mut tx = jj_repo.start_transaction("test");
    let result = git::fetch(&mut tx, "invalid-remote");
    assert!(matches!(result, Err(GitFetchError::NoSuchRemote(_))));

    tx.discard();
}

struct PushTestSetup {
    source_repo_dir: PathBuf,
    clone_repo_dir: PathBuf,
    jj_repo: Arc<ReadonlyRepo>,
    new_commit: Commit,
}

fn set_up_push_repos(settings: &UserSettings, temp_dir: &TempDir) -> PushTestSetup {
    let source_repo_dir = temp_dir.path().join("source");
    let clone_repo_dir = temp_dir.path().join("clone");
    let jj_repo_dir = temp_dir.path().join("jj");
    let git_repo = git2::Repository::init_bare(&source_repo_dir).unwrap();
    let initial_git_commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
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
        clone_repo_dir,
        jj_repo,
        new_commit,
    }
}

#[test]
fn test_push_commit_success() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let result = git::push_commit(&setup.new_commit, "origin", "main");
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
    let clone_repo = git2::Repository::open(&setup.clone_repo_dir).unwrap();
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
    let mut jj_repo = set_up_push_repos(&settings, &temp_dir).jj_repo;
    let new_commit = testutils::create_random_commit(&settings, &jj_repo)
        .write_to_new_transaction(&jj_repo, "test");
    Arc::get_mut(&mut jj_repo).unwrap().reload();
    let result = git::push_commit(&new_commit, "origin", "main");
    assert_eq!(result, Err(GitPushError::NotFastForward));
}

#[test]
fn test_push_commit_no_such_remote() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let result = git::push_commit(&setup.new_commit, "invalid-remote", "main");
    assert!(matches!(result, Err(GitPushError::NoSuchRemote(_))));
}

#[test]
fn test_push_commit_invalid_remote() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let result = git::push_commit(&setup.new_commit, "http://invalid-remote", "main");
    assert!(matches!(result, Err(GitPushError::NoSuchRemote(_))));
}

#[test]
fn test_push_commit_non_git() {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, false);
    let commit =
        testutils::create_random_commit(&settings, &repo).write_to_new_transaction(&repo, "test");
    let result = git::push_commit(&commit, "origin", "main");
    assert_eq!(result, Err(GitPushError::NotAGitRepo));
}
