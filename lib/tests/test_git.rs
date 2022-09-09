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
use jujutsu_lib::testutils::{create_random_commit, TestRepo};
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
    CommitId::from_bytes(commit.id().as_bytes())
}

#[test]
fn test_import_refs() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let git_repo = repo.store().git_repo().unwrap();

    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_ref(&git_repo, "refs/remotes/origin/main", commit1.id());
    let commit2 = empty_git_commit(&git_repo, "refs/heads/main", &[&commit1]);
    let commit3 = empty_git_commit(&git_repo, "refs/heads/feature1", &[&commit2]);
    let commit4 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);
    let commit5 = empty_git_commit(&git_repo, "refs/tags/v1.0", &[&commit1]);
    // Should not be imported
    empty_git_commit(&git_repo, "refs/notes/x", &[&commit2]);
    empty_git_commit(&git_repo, "refs/remotes/origin/HEAD", &[&commit2]);

    git_repo.set_head("refs/heads/main").unwrap();

    let git_repo = repo.store().git_repo().unwrap();
    let mut tx = repo.start_transaction("test");
    git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit();
    let view = repo.view();

    let expected_heads = hashset! {
        commit_id(&commit3),
        commit_id(&commit4),
        commit_id(&commit5)
    };
    assert_eq!(*view.heads(), expected_heads);

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
    assert_eq!(view.git_head(), Some(commit_id(&commit2)));
}

#[test]
fn test_import_refs_reimport() {
    let settings = testutils::user_settings();
    let test_workspace = TestRepo::init(true);
    let repo = &test_workspace.repo;
    let git_repo = repo.store().git_repo().unwrap();

    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_ref(&git_repo, "refs/remotes/origin/main", commit1.id());
    let commit2 = empty_git_commit(&git_repo, "refs/heads/main", &[&commit1]);
    let _commit3 = empty_git_commit(&git_repo, "refs/heads/feature1", &[&commit2]);
    let commit4 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);
    let pgp_key_oid = git_repo.blob(b"my PGP key").unwrap();
    git_repo
        .reference("refs/tags/my-gpg-key", pgp_key_oid, false, "")
        .unwrap();

    let mut tx = repo.start_transaction("test");
    git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
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
    git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit();

    let view = repo.view();
    let expected_heads = hashset! {
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

#[test]
fn test_import_refs_reimport_head_removed() {
    // Test that re-importing refs doesn't cause a deleted head to come back
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let git_repo = repo.store().git_repo().unwrap();

    let commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    let mut tx = repo.start_transaction("test");
    git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let commit_id = CommitId::from_bytes(commit.id().as_bytes());
    // Test the setup
    assert!(tx.mut_repo().view().heads().contains(&commit_id));

    // Remove the head and re-import
    tx.mut_repo().remove_head(&commit_id);
    git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert!(!tx.mut_repo().view().heads().contains(&commit_id));
}

fn git_ref(git_repo: &git2::Repository, name: &str, target: Oid) {
    git_repo.reference(name, target, true, "").unwrap();
}

fn delete_git_ref(git_repo: &git2::Repository, name: &str) {
    git_repo.find_reference(name).unwrap().delete().unwrap();
}

struct GitRepoData {
    settings: UserSettings,
    _temp_dir: TempDir,
    origin_repo: git2::Repository,
    git_repo: git2::Repository,
    repo: Arc<ReadonlyRepo>,
}

impl GitRepoData {
    fn create() -> Self {
        let settings = testutils::user_settings();
        let temp_dir = testutils::new_temp_dir();
        let origin_repo_dir = temp_dir.path().join("source");
        let origin_repo = git2::Repository::init_bare(&origin_repo_dir).unwrap();
        let git_repo_dir = temp_dir.path().join("git");
        let git_repo =
            git2::Repository::clone(origin_repo_dir.to_str().unwrap(), &git_repo_dir).unwrap();
        let jj_repo_dir = temp_dir.path().join("jj");
        std::fs::create_dir(&jj_repo_dir).unwrap();
        let repo = ReadonlyRepo::init_external_git(&settings, jj_repo_dir, git_repo_dir);
        Self {
            settings,
            _temp_dir: temp_dir,
            origin_repo,
            git_repo,
            repo,
        }
    }
}

#[test]
fn test_import_refs_empty_git_repo() {
    let test_data = GitRepoData::create();
    let heads_before = test_data.repo.view().heads().clone();
    let mut tx = test_data.repo.start_transaction("test");
    git::import_refs(tx.mut_repo(), &test_data.git_repo).unwrap();
    tx.mut_repo()
        .rebase_descendants(&test_data.settings)
        .unwrap();
    let repo = tx.commit();
    assert_eq!(*repo.view().heads(), heads_before);
    assert_eq!(repo.view().branches().len(), 0);
    assert_eq!(repo.view().tags().len(), 0);
    assert_eq!(repo.view().git_refs().len(), 0);
    assert_eq!(repo.view().git_head(), None);
}

#[test]
fn test_import_refs_detached_head() {
    let test_data = GitRepoData::create();
    let commit1 = empty_git_commit(&test_data.git_repo, "refs/heads/main", &[]);
    // Delete the reference. Check that the detached HEAD commit still gets added to
    // the set of heads
    test_data
        .git_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .delete()
        .unwrap();
    test_data.git_repo.set_head_detached(commit1.id()).unwrap();

    let mut tx = test_data.repo.start_transaction("test");
    git::import_refs(tx.mut_repo(), &test_data.git_repo).unwrap();
    tx.mut_repo()
        .rebase_descendants(&test_data.settings)
        .unwrap();
    let repo = tx.commit();

    let expected_heads = hashset! {
            commit_id(&commit1),
    };
    assert_eq!(*repo.view().heads(), expected_heads);
    assert_eq!(repo.view().git_refs().len(), 0);
    assert_eq!(
        repo.view().git_head(),
        Some(CommitId::from_bytes(commit1.id().as_bytes()))
    );
}

#[test]
fn test_export_refs_initial() {
    // The first export doesn't do anything
    let mut test_data = GitRepoData::create();
    let git_repo = test_data.git_repo;
    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_repo.set_head("refs/heads/main").unwrap();
    let mut tx = test_data.repo.start_transaction("test");
    git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    tx.mut_repo()
        .rebase_descendants(&test_data.settings)
        .unwrap();
    test_data.repo = tx.commit();

    // The first export shouldn't do anything
    assert_eq!(git::export_refs(&test_data.repo, &git_repo), Ok(()));
    assert_eq!(git_repo.head().unwrap().name(), Some("refs/heads/main"));
    assert_eq!(
        git_repo.find_reference("refs/heads/main").unwrap().target(),
        Some(commit1.id())
    );
}

#[test]
fn test_export_refs_no_op() {
    // Nothing changes on the git side if nothing changed on the jj side
    let mut test_data = GitRepoData::create();
    let git_repo = test_data.git_repo;
    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_repo.set_head("refs/heads/main").unwrap();

    let mut tx = test_data.repo.start_transaction("test");
    git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    tx.mut_repo()
        .rebase_descendants(&test_data.settings)
        .unwrap();
    test_data.repo = tx.commit();

    assert_eq!(git::export_refs(&test_data.repo, &git_repo), Ok(()));
    // The export should be a no-op since nothing changed on the jj side since last
    // export
    assert_eq!(git::export_refs(&test_data.repo, &git_repo), Ok(()));
    assert_eq!(git_repo.head().unwrap().name(), Some("refs/heads/main"));
    assert_eq!(
        git_repo.find_reference("refs/heads/main").unwrap().target(),
        Some(commit1.id())
    );
}

#[test]
fn test_export_refs_branch_changed() {
    // We can export a change to a branch
    let mut test_data = GitRepoData::create();
    let git_repo = test_data.git_repo;
    let commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_repo
        .reference("refs/heads/feature", commit.id(), false, "test")
        .unwrap();
    git_repo.set_head("refs/heads/feature").unwrap();

    let mut tx = test_data.repo.start_transaction("test");
    git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    tx.mut_repo()
        .rebase_descendants(&test_data.settings)
        .unwrap();
    test_data.repo = tx.commit();

    assert_eq!(git::export_refs(&test_data.repo, &git_repo), Ok(()));
    let mut tx = test_data.repo.start_transaction("test");
    let new_commit = testutils::create_random_commit(&test_data.settings, &test_data.repo)
        .set_parents(vec![CommitId::from_bytes(commit.id().as_bytes())])
        .write_to_repo(tx.mut_repo());
    tx.mut_repo().set_local_branch(
        "main".to_string(),
        RefTarget::Normal(new_commit.id().clone()),
    );
    test_data.repo = tx.commit();
    assert_eq!(git::export_refs(&test_data.repo, &git_repo), Ok(()));
    assert_eq!(
        git_repo
            .find_reference("refs/heads/main")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id(),
        Oid::from_bytes(new_commit.id().as_bytes()).unwrap()
    );
    // HEAD should be unchanged since its target branch didn't change
    assert_eq!(git_repo.head().unwrap().name(), Some("refs/heads/feature"));
}

#[test]
fn test_export_refs_current_branch_changed() {
    // If we update a branch that is checked out in the git repo, HEAD gets detached
    let mut test_data = GitRepoData::create();
    let git_repo = test_data.git_repo;
    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_repo.set_head("refs/heads/main").unwrap();
    let mut tx = test_data.repo.start_transaction("test");
    git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    tx.mut_repo()
        .rebase_descendants(&test_data.settings)
        .unwrap();
    test_data.repo = tx.commit();

    assert_eq!(git::export_refs(&test_data.repo, &git_repo), Ok(()));
    let mut tx = test_data.repo.start_transaction("test");
    let new_commit = testutils::create_random_commit(&test_data.settings, &test_data.repo)
        .set_parents(vec![CommitId::from_bytes(commit1.id().as_bytes())])
        .write_to_repo(tx.mut_repo());
    tx.mut_repo().set_local_branch(
        "main".to_string(),
        RefTarget::Normal(new_commit.id().clone()),
    );
    test_data.repo = tx.commit();
    assert_eq!(git::export_refs(&test_data.repo, &git_repo), Ok(()));
    assert_eq!(
        git_repo
            .find_reference("refs/heads/main")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id(),
        Oid::from_bytes(new_commit.id().as_bytes()).unwrap()
    );
    assert!(git_repo.head_detached().unwrap());
}

#[test]
fn test_export_refs_unborn_git_branch() {
    // Can export to an empty Git repo (we can handle Git's "unborn branch" state)
    let mut test_data = GitRepoData::create();
    let git_repo = test_data.git_repo;
    git_repo.set_head("refs/heads/main").unwrap();
    let mut tx = test_data.repo.start_transaction("test");
    git::import_refs(tx.mut_repo(), &git_repo).unwrap();
    tx.mut_repo()
        .rebase_descendants(&test_data.settings)
        .unwrap();
    test_data.repo = tx.commit();

    assert_eq!(git::export_refs(&test_data.repo, &git_repo), Ok(()));
    let mut tx = test_data.repo.start_transaction("test");
    let new_commit = testutils::create_random_commit(&test_data.settings, &test_data.repo)
        .write_to_repo(tx.mut_repo());
    tx.mut_repo().set_local_branch(
        "main".to_string(),
        RefTarget::Normal(new_commit.id().clone()),
    );
    test_data.repo = tx.commit();
    assert_eq!(git::export_refs(&test_data.repo, &git_repo), Ok(()));
    assert_eq!(
        git_repo
            .find_reference("refs/heads/main")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id(),
        Oid::from_bytes(new_commit.id().as_bytes()).unwrap()
    );
    // It's weird that the head is still pointing to refs/heads/main, but
    // it doesn't seem that Git lets you be on an "unborn branch" while
    // also being in a "detached HEAD" state.
    assert!(!git_repo.head_detached().unwrap());
}

#[test]
fn test_init() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
    let git_repo_dir = temp_dir.path().join("git");
    let jj_repo_dir = temp_dir.path().join("jj");
    let git_repo = git2::Repository::init_bare(&git_repo_dir).unwrap();
    let initial_git_commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    let initial_commit_id = commit_id(&initial_git_commit);
    std::fs::create_dir(&jj_repo_dir).unwrap();
    let repo = ReadonlyRepo::init_external_git(&settings, jj_repo_dir, git_repo_dir);
    // The refs were *not* imported -- it's the caller's responsibility to import
    // any refs they care about.
    assert!(!repo.view().heads().contains(&initial_commit_id));
}

#[test]
fn test_fetch_empty_repo() {
    let test_data = GitRepoData::create();

    let mut tx = test_data.repo.start_transaction("test");
    let default_branch = git::fetch(tx.mut_repo(), &test_data.git_repo, "origin").unwrap();
    // No default branch and no refs
    assert_eq!(default_branch, None);
    assert_eq!(*tx.mut_repo().view().git_refs(), btreemap! {});
    assert_eq!(*tx.mut_repo().view().branches(), btreemap! {});
}

#[test]
fn test_fetch_initial_commit() {
    let test_data = GitRepoData::create();
    let initial_git_commit = empty_git_commit(&test_data.origin_repo, "refs/heads/main", &[]);

    let mut tx = test_data.repo.start_transaction("test");
    let default_branch = git::fetch(tx.mut_repo(), &test_data.git_repo, "origin").unwrap();
    // No default branch because the origin repo's HEAD wasn't set
    assert_eq!(default_branch, None);
    let repo = tx.commit();
    // The initial commit is visible after git::fetch().
    let view = repo.view();
    assert!(view.heads().contains(&commit_id(&initial_git_commit)));
    let initial_commit_target = RefTarget::Normal(commit_id(&initial_git_commit));
    assert_eq!(
        *view.git_refs(),
        btreemap! {
            "refs/remotes/origin/main".to_string() => initial_commit_target.clone(),
        }
    );
    assert_eq!(
        *view.branches(),
        btreemap! {
            "main".to_string() => BranchTarget {
                local_target: Some(initial_commit_target.clone()),
                remote_targets: btreemap! {"origin".to_string() => initial_commit_target}
            },
        }
    );
}

#[test]
fn test_fetch_success() {
    let mut test_data = GitRepoData::create();
    let initial_git_commit = empty_git_commit(&test_data.origin_repo, "refs/heads/main", &[]);

    let mut tx = test_data.repo.start_transaction("test");
    git::fetch(tx.mut_repo(), &test_data.git_repo, "origin").unwrap();
    test_data.repo = tx.commit();

    test_data.origin_repo.set_head("refs/heads/main").unwrap();
    let new_git_commit = empty_git_commit(
        &test_data.origin_repo,
        "refs/heads/main",
        &[&initial_git_commit],
    );

    let mut tx = test_data.repo.start_transaction("test");
    let default_branch = git::fetch(tx.mut_repo(), &test_data.git_repo, "origin").unwrap();
    // The default branch is "main"
    assert_eq!(default_branch, Some("main".to_string()));
    let repo = tx.commit();
    // The new commit is visible after we fetch again
    let view = repo.view();
    assert!(view.heads().contains(&commit_id(&new_git_commit)));
    let new_commit_target = RefTarget::Normal(commit_id(&new_git_commit));
    assert_eq!(
        *view.git_refs(),
        btreemap! {
            "refs/remotes/origin/main".to_string() => new_commit_target.clone(),
        }
    );
    assert_eq!(
        *view.branches(),
        btreemap! {
            "main".to_string() => BranchTarget {
                local_target: Some(new_commit_target.clone()),
                remote_targets: btreemap! {"origin".to_string() => new_commit_target}
            },
        }
    );
}

#[test]
fn test_fetch_prune_deleted_ref() {
    let test_data = GitRepoData::create();
    empty_git_commit(&test_data.git_repo, "refs/heads/main", &[]);

    let mut tx = test_data.repo.start_transaction("test");
    git::fetch(tx.mut_repo(), &test_data.git_repo, "origin").unwrap();
    // Test the setup
    assert!(tx.mut_repo().get_branch("main").is_some());

    test_data
        .git_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .delete()
        .unwrap();
    // After re-fetching, the branch should be deleted
    git::fetch(tx.mut_repo(), &test_data.git_repo, "origin").unwrap();
    assert!(tx.mut_repo().get_branch("main").is_none());
}

#[test]
fn test_fetch_no_default_branch() {
    let test_data = GitRepoData::create();
    let initial_git_commit = empty_git_commit(&test_data.origin_repo, "refs/heads/main", &[]);

    let mut tx = test_data.repo.start_transaction("test");
    git::fetch(tx.mut_repo(), &test_data.git_repo, "origin").unwrap();

    empty_git_commit(
        &test_data.origin_repo,
        "refs/heads/main",
        &[&initial_git_commit],
    );
    // It's actually not enough to have a detached HEAD, it also needs to point to a
    // commit without a branch (that's possibly a bug in Git *and* libgit2), so
    // we point it to initial_git_commit.
    test_data
        .origin_repo
        .set_head_detached(initial_git_commit.id())
        .unwrap();

    let default_branch = git::fetch(tx.mut_repo(), &test_data.git_repo, "origin").unwrap();
    // There is no default branch
    assert_eq!(default_branch, None);
}

#[test]
fn test_fetch_no_such_remote() {
    let test_data = GitRepoData::create();

    let mut tx = test_data.repo.start_transaction("test");
    let result = git::fetch(tx.mut_repo(), &test_data.git_repo, "invalid-remote");
    assert!(matches!(result, Err(GitFetchError::NoSuchRemote(_))));
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
    let jj_repo = ReadonlyRepo::init_external_git(settings, jj_repo_dir, clone_repo_dir);
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
    let temp_dir = testutils::new_temp_dir();
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
    let new_oid = Oid::from_bytes(setup.new_commit.id().as_bytes()).unwrap();
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
    let temp_dir = testutils::new_temp_dir();
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
    let temp_dir = testutils::new_temp_dir();
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
    let new_oid = Oid::from_bytes(setup.new_commit.id().as_bytes()).unwrap();
    assert_eq!(new_target, Some(new_oid));

    // Check that the main ref got deleted in the source repo
    assert!(source_repo.find_reference("refs/heads/main").is_err());
}

#[test]
fn test_push_updates_not_fast_forward() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
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
    let temp_dir = testutils::new_temp_dir();
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
    let new_oid = Oid::from_bytes(new_commit.id().as_bytes()).unwrap();
    assert_eq!(new_target, Some(new_oid));
}

#[test]
fn test_push_updates_no_such_remote() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
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
    let temp_dir = testutils::new_temp_dir();
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
