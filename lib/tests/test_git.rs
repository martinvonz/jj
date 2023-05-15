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

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Barrier};
use std::thread;

use git2::Oid;
use itertools::Itertools;
use jujutsu_lib::backend::{
    BackendError, ChangeId, CommitId, MillisSinceEpoch, ObjectId, Signature, Timestamp,
};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::git;
use jujutsu_lib::git::{GitFetchError, GitPushError, GitRefUpdate};
use jujutsu_lib::git_backend::GitBackend;
use jujutsu_lib::op_store::{BranchTarget, RefTarget};
use jujutsu_lib::repo::{MutableRepo, ReadonlyRepo, Repo};
use jujutsu_lib::settings::{GitSettings, UserSettings};
use maplit::{btreemap, hashset};
use tempfile::TempDir;
use testutils::{create_random_commit, load_repo_at_head, write_random_commit, TestRepo};

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

fn jj_id(commit: &git2::Commit) -> CommitId {
    CommitId::from_bytes(commit.id().as_bytes())
}

fn git_id(commit: &Commit) -> Oid {
    Oid::from_bytes(commit.id().as_bytes()).unwrap()
}

fn get_git_repo(repo: &Arc<ReadonlyRepo>) -> git2::Repository {
    repo.store()
        .backend_impl()
        .downcast_ref::<GitBackend>()
        .unwrap()
        .git_repo_clone()
}

#[test]
fn test_import_refs() {
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let git_repo = get_git_repo(repo);

    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_ref(&git_repo, "refs/remotes/origin/main", commit1.id());
    let commit2 = empty_git_commit(&git_repo, "refs/heads/main", &[&commit1]);
    let commit3 = empty_git_commit(&git_repo, "refs/heads/feature1", &[&commit2]);
    let commit4 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);
    let commit5 = empty_git_commit(&git_repo, "refs/tags/v1.0", &[&commit1]);
    let commit6 = empty_git_commit(&git_repo, "refs/remotes/origin/feature3", &[&commit1]);
    // Should not be imported
    empty_git_commit(&git_repo, "refs/notes/x", &[&commit2]);
    empty_git_commit(&git_repo, "refs/remotes/origin/HEAD", &[&commit2]);

    git_repo.set_head("refs/heads/main").unwrap();

    let git_repo = get_git_repo(repo);
    let mut tx = repo.start_transaction(&settings, "test");
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit();
    let view = repo.view();

    let expected_heads = hashset! {
        jj_id(&commit3),
        jj_id(&commit4),
        jj_id(&commit5),
        jj_id(&commit6)
    };
    assert_eq!(*view.heads(), expected_heads);

    let expected_main_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(jj_id(&commit2))),
        remote_targets: btreemap! {
          "origin".to_string() => RefTarget::Normal(jj_id(&commit1)),
        },
    };
    assert_eq!(
        view.branches().get("main"),
        Some(expected_main_branch).as_ref()
    );
    let expected_feature1_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(jj_id(&commit3))),
        remote_targets: btreemap! {},
    };
    assert_eq!(
        view.branches().get("feature1"),
        Some(expected_feature1_branch).as_ref()
    );
    let expected_feature2_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(jj_id(&commit4))),
        remote_targets: btreemap! {},
    };
    assert_eq!(
        view.branches().get("feature2"),
        Some(expected_feature2_branch).as_ref()
    );
    let expected_feature3_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(jj_id(&commit6))),
        remote_targets: btreemap! {
          "origin".to_string() => RefTarget::Normal(jj_id(&commit6)),
        },
    };
    assert_eq!(
        view.branches().get("feature3"),
        Some(expected_feature3_branch).as_ref()
    );

    assert_eq!(
        view.tags().get("v1.0"),
        Some(RefTarget::Normal(jj_id(&commit5))).as_ref()
    );

    assert_eq!(view.git_refs().len(), 6);
    assert_eq!(
        view.git_refs().get("refs/heads/main"),
        Some(RefTarget::Normal(jj_id(&commit2))).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/heads/feature1"),
        Some(RefTarget::Normal(jj_id(&commit3))).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/heads/feature2"),
        Some(RefTarget::Normal(jj_id(&commit4))).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/remotes/origin/main"),
        Some(RefTarget::Normal(jj_id(&commit1))).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/remotes/origin/feature3"),
        Some(RefTarget::Normal(jj_id(&commit6))).as_ref()
    );
    assert_eq!(
        view.git_refs().get("refs/tags/v1.0"),
        Some(RefTarget::Normal(jj_id(&commit5))).as_ref()
    );
    assert_eq!(view.git_head(), Some(&RefTarget::Normal(jj_id(&commit2))));
}

#[test]
fn test_import_refs_reimport() {
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_workspace = TestRepo::init(true);
    let repo = &test_workspace.repo;
    let git_repo = get_git_repo(repo);

    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_ref(&git_repo, "refs/remotes/origin/main", commit1.id());
    let commit2 = empty_git_commit(&git_repo, "refs/heads/main", &[&commit1]);
    let commit3 = empty_git_commit(&git_repo, "refs/heads/feature1", &[&commit2]);
    let commit4 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);
    let pgp_key_oid = git_repo.blob(b"my PGP key").unwrap();
    git_repo
        .reference("refs/tags/my-gpg-key", pgp_key_oid, false, "")
        .unwrap();

    let mut tx = repo.start_transaction(&settings, "test");
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit();

    let expected_heads = hashset! {
            jj_id(&commit3),
            jj_id(&commit4),
    };
    let view = repo.view();
    assert_eq!(*view.heads(), expected_heads);

    // Delete feature1 and rewrite feature2
    delete_git_ref(&git_repo, "refs/heads/feature1");
    delete_git_ref(&git_repo, "refs/heads/feature2");
    let commit5 = empty_git_commit(&git_repo, "refs/heads/feature2", &[&commit2]);

    // Also modify feature2 on the jj side
    let mut tx = repo.start_transaction(&settings, "test");
    let commit6 = create_random_commit(tx.mut_repo(), &settings)
        .set_parents(vec![jj_id(&commit2)])
        .write()
        .unwrap();
    tx.mut_repo().set_local_branch(
        "feature2".to_string(),
        RefTarget::Normal(commit6.id().clone()),
    );
    let repo = tx.commit();

    let mut tx = repo.start_transaction(&settings, "test");
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit();

    let view = repo.view();
    let expected_heads = hashset! {
            jj_id(&commit5),
            commit6.id().clone(),
    };
    assert_eq!(*view.heads(), expected_heads);

    assert_eq!(view.branches().len(), 2);
    let commit1_target = RefTarget::Normal(jj_id(&commit1));
    let commit2_target = RefTarget::Normal(jj_id(&commit2));
    let expected_main_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(jj_id(&commit2))),
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
            removes: vec![jj_id(&commit4)],
            adds: vec![commit6.id().clone(), jj_id(&commit5)],
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
    let commit5_target = RefTarget::Normal(jj_id(&commit5));
    assert_eq!(
        view.git_refs().get("refs/heads/feature2"),
        Some(commit5_target).as_ref()
    );
}

#[test]
fn test_import_refs_reimport_head_removed() {
    // Test that re-importing refs doesn't cause a deleted head to come back
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let git_repo = get_git_repo(repo);

    let commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    let mut tx = repo.start_transaction(&settings, "test");
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let commit_id = jj_id(&commit);
    // Test the setup
    assert!(tx.mut_repo().view().heads().contains(&commit_id));

    // Remove the head and re-import
    tx.mut_repo().remove_head(&commit_id);
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert!(!tx.mut_repo().view().heads().contains(&commit_id));
}

#[test]
fn test_import_refs_reimport_git_head_counts() {
    // Test that if a branch is removed but the Git HEAD points to the commit (or a
    // descendant of it), we still keep it alive.
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let git_repo = get_git_repo(repo);

    let commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_repo.set_head_detached(commit.id()).unwrap();

    let mut tx = repo.start_transaction(&settings, "test");
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();

    // Delete the branch and re-import. The commit should still be there since HEAD
    // points to it
    git_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .delete()
        .unwrap();
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert!(tx.mut_repo().view().heads().contains(&jj_id(&commit)));
}

#[test]
fn test_import_refs_reimport_git_head_without_ref() {
    // Simulate external `git checkout` in colocated repo, from anonymous branch.
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let git_repo = get_git_repo(repo);

    // First, HEAD points to commit1.
    let mut tx = repo.start_transaction(&settings, "test");
    let commit1 = write_random_commit(tx.mut_repo(), &settings);
    let commit2 = write_random_commit(tx.mut_repo(), &settings);
    git_repo.set_head_detached(git_id(&commit1)).unwrap();

    // Import HEAD.
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert!(tx.mut_repo().view().heads().contains(commit1.id()));
    assert!(tx.mut_repo().view().heads().contains(commit2.id()));

    // Move HEAD to commit2 (by e.g. `git checkout` command)
    git_repo.set_head_detached(git_id(&commit2)).unwrap();

    // Reimport HEAD, which doesn't abandon the old HEAD branch because jj thinks it
    // would be moved by `git checkout` command. This isn't always true because the
    // detached HEAD commit could be rewritten by e.g. `git commit --amend` command,
    // but it should be safer than abandoning old checkout branch.
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert!(tx.mut_repo().view().heads().contains(commit1.id()));
    assert!(tx.mut_repo().view().heads().contains(commit2.id()));
}

#[test]
fn test_import_refs_reimport_git_head_with_moved_ref() {
    // Simulate external history rewriting in colocated repo.
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let git_repo = get_git_repo(repo);

    // First, both HEAD and main point to commit1.
    let mut tx = repo.start_transaction(&settings, "test");
    let commit1 = write_random_commit(tx.mut_repo(), &settings);
    let commit2 = write_random_commit(tx.mut_repo(), &settings);
    git_repo
        .reference("refs/heads/main", git_id(&commit1), true, "test")
        .unwrap();
    git_repo.set_head_detached(git_id(&commit1)).unwrap();

    // Import HEAD and main.
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert!(tx.mut_repo().view().heads().contains(commit1.id()));
    assert!(tx.mut_repo().view().heads().contains(commit2.id()));

    // Move both HEAD and main to commit2 (by e.g. `git commit --amend` command)
    git_repo
        .reference("refs/heads/main", git_id(&commit2), true, "test")
        .unwrap();
    git_repo.set_head_detached(git_id(&commit2)).unwrap();

    // Reimport HEAD and main, which abandons the old main branch.
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert!(!tx.mut_repo().view().heads().contains(commit1.id()));
    assert!(tx.mut_repo().view().heads().contains(commit2.id()));
}

#[test]
fn test_import_refs_reimport_with_deleted_remote_ref() {
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_workspace = TestRepo::init(true);
    let repo = &test_workspace.repo;
    let git_repo = get_git_repo(repo);

    let commit_base = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    let commit_main = empty_git_commit(&git_repo, "refs/heads/main", &[&commit_base]);
    let commit_remote_only = empty_git_commit(
        &git_repo,
        "refs/remotes/origin/feature-remote-only",
        &[&commit_base],
    );
    let commit_remote_and_local = empty_git_commit(
        &git_repo,
        "refs/remotes/origin/feature-remote-and-local",
        &[&commit_base],
    );
    git_ref(
        &git_repo,
        "refs/heads/feature-remote-and-local",
        commit_remote_and_local.id(),
    );

    let mut tx = repo.start_transaction(&settings, "test");
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit();

    let expected_heads = hashset! {
            jj_id(&commit_main),
            jj_id(&commit_remote_only),
            jj_id(&commit_remote_and_local),
    };
    let view = repo.view();
    assert_eq!(*view.heads(), expected_heads);
    assert_eq!(view.branches().len(), 3);
    assert_eq!(
        view.branches().get("feature-remote-only"),
        Some(&BranchTarget {
            // Even though the git repo does not have a local branch for `feature-remote-only`, jj
            // creates one. This follows the model explained in docs/branches.md.
            local_target: Some(RefTarget::Normal(jj_id(&commit_remote_only))),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::Normal(jj_id(&commit_remote_only))
            },
        }),
    );
    assert_eq!(
        view.branches().get("feature-remote-and-local"),
        Some(&BranchTarget {
            local_target: Some(RefTarget::Normal(jj_id(&commit_remote_and_local))),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::Normal(jj_id(&commit_remote_and_local))
            },
        }),
    );
    view.branches().get("main").unwrap(); // branch #3 of 3

    // Simulate fetching from a remote where feature-remote-only and
    // feature-remote-and-local branches were deleted. This leads to the
    // following import deleting the corresponding local branches.
    delete_git_ref(&git_repo, "refs/remotes/origin/feature-remote-only");
    delete_git_ref(&git_repo, "refs/remotes/origin/feature-remote-and-local");

    let mut tx = repo.start_transaction(&settings, "test");
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit();

    let view = repo.view();
    // The local branches were indeed deleted
    assert_eq!(view.branches().len(), 1);
    view.branches().get("main").unwrap(); // branch #1 of 1
    assert_eq!(view.branches().get("feature-remote-local"), None);
    assert_eq!(view.branches().get("feature-remote-and-local"), None);
    let expected_heads = hashset! {
            jj_id(&commit_main),
            // Neither commit_remote_only nor commit_remote_and_local should be
            // listed as a head. commit_remote_only was never affected by #864,
            // but commit_remote_and_local was.
    };
    assert_eq!(*view.heads(), expected_heads);
}

/// This test is nearly identical to the previous one, except the branches are
/// moved sideways instead of being deleted.
#[test]
fn test_import_refs_reimport_with_moved_remote_ref() {
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_workspace = TestRepo::init(true);
    let repo = &test_workspace.repo;
    let git_repo = get_git_repo(repo);

    let commit_base = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    let commit_main = empty_git_commit(&git_repo, "refs/heads/main", &[&commit_base]);
    let commit_remote_only = empty_git_commit(
        &git_repo,
        "refs/remotes/origin/feature-remote-only",
        &[&commit_base],
    );
    let commit_remote_and_local = empty_git_commit(
        &git_repo,
        "refs/remotes/origin/feature-remote-and-local",
        &[&commit_base],
    );
    git_ref(
        &git_repo,
        "refs/heads/feature-remote-and-local",
        commit_remote_and_local.id(),
    );

    let mut tx = repo.start_transaction(&settings, "test");
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit();

    let expected_heads = hashset! {
            jj_id(&commit_main),
            jj_id(dbg!(&commit_remote_only)),
            jj_id(dbg!(&commit_remote_and_local)),
    };
    let view = repo.view();
    assert_eq!(*view.heads(), expected_heads);
    assert_eq!(view.branches().len(), 3);
    assert_eq!(
        view.branches().get("feature-remote-only"),
        Some(&BranchTarget {
            // Even though the git repo does not have a local branch for `feature-remote-only`, jj
            // creates one. This follows the model explained in docs/branches.md.
            local_target: Some(RefTarget::Normal(jj_id(&commit_remote_only))),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::Normal(jj_id(&commit_remote_only))
            },
        }),
    );
    assert_eq!(
        view.branches().get("feature-remote-and-local"),
        Some(&BranchTarget {
            local_target: Some(RefTarget::Normal(jj_id(&commit_remote_and_local))),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::Normal(jj_id(&commit_remote_and_local))
            },
        }),
    );
    view.branches().get("main").unwrap(); // branch #3 of 3

    // Simulate fetching from a remote where feature-remote-only and
    // feature-remote-and-local branches were moved. This leads to the
    // following import moving the corresponding local branches.
    delete_git_ref(&git_repo, "refs/remotes/origin/feature-remote-only");
    delete_git_ref(&git_repo, "refs/remotes/origin/feature-remote-and-local");
    let new_commit_remote_only = empty_git_commit(
        &git_repo,
        "refs/remotes/origin/feature-remote-only",
        &[&commit_base],
    );
    let new_commit_remote_and_local = empty_git_commit(
        &git_repo,
        "refs/remotes/origin/feature-remote-and-local",
        &[&commit_base],
    );

    let mut tx = repo.start_transaction(&settings, "test");
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit();

    let view = repo.view();
    assert_eq!(view.branches().len(), 3);
    // The local branches are moved
    assert_eq!(
        view.branches().get("feature-remote-only"),
        Some(&BranchTarget {
            local_target: Some(RefTarget::Normal(jj_id(&new_commit_remote_only))),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::Normal(jj_id(&new_commit_remote_only))
            },
        }),
    );
    assert_eq!(
        view.branches().get("feature-remote-and-local"),
        Some(&BranchTarget {
            local_target: Some(RefTarget::Normal(jj_id(&new_commit_remote_and_local))),
            remote_targets: btreemap! {
                "origin".to_string() => RefTarget::Normal(jj_id(&new_commit_remote_and_local))
            },
        }),
    );
    view.branches().get("main").unwrap(); // branch #3 of 3
    let expected_heads = hashset! {
            jj_id(&commit_main),
            jj_id(&new_commit_remote_and_local),
            jj_id(&new_commit_remote_only),
            // Neither commit_remote_only nor commit_remote_and_local should be
            // listed as a head. commit_remote_only was never affected by #864,
            // but commit_remote_and_local was.
    };
    assert_eq!(*view.heads(), expected_heads);
}

#[test]
fn test_import_refs_reimport_git_head_with_fixed_ref() {
    // Simulate external `git checkout` in colocated repo, from named branch.
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let git_repo = get_git_repo(repo);

    // First, both HEAD and main point to commit1.
    let mut tx = repo.start_transaction(&settings, "test");
    let commit1 = write_random_commit(tx.mut_repo(), &settings);
    let commit2 = write_random_commit(tx.mut_repo(), &settings);
    git_repo
        .reference("refs/heads/main", git_id(&commit1), true, "test")
        .unwrap();
    git_repo.set_head_detached(git_id(&commit1)).unwrap();

    // Import HEAD and main.
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert!(tx.mut_repo().view().heads().contains(commit1.id()));
    assert!(tx.mut_repo().view().heads().contains(commit2.id()));

    // Move only HEAD to commit2 (by e.g. `git checkout` command)
    git_repo.set_head_detached(git_id(&commit2)).unwrap();

    // Reimport HEAD, which shouldn't abandon the old HEAD branch.
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert!(tx.mut_repo().view().heads().contains(commit1.id()));
    assert!(tx.mut_repo().view().heads().contains(commit2.id()));
}

#[test]
fn test_import_refs_reimport_all_from_root_removed() {
    // Test that if a chain of commits all the way from the root gets unreferenced,
    // we abandon the whole stack, but not including the root commit.
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let git_repo = get_git_repo(repo);

    let commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    let mut tx = repo.start_transaction(&settings, "test");
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    // Test the setup
    assert!(tx.mut_repo().view().heads().contains(&jj_id(&commit)));

    // Remove all git refs and re-import
    git_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .delete()
        .unwrap();
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    assert!(!tx.mut_repo().view().heads().contains(&jj_id(&commit)));
}

#[test]
fn test_import_some_refs() {
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_workspace = TestRepo::init(true);
    let repo = &test_workspace.repo;
    let git_repo = get_git_repo(repo);

    let commit_main = empty_git_commit(&git_repo, "refs/remotes/origin/main", &[]);
    let commit_feat1 = empty_git_commit(&git_repo, "refs/remotes/origin/feature1", &[&commit_main]);
    let commit_feat2 =
        empty_git_commit(&git_repo, "refs/remotes/origin/feature2", &[&commit_feat1]);
    let commit_feat3 =
        empty_git_commit(&git_repo, "refs/remotes/origin/feature3", &[&commit_feat1]);
    let commit_feat4 =
        empty_git_commit(&git_repo, "refs/remotes/origin/feature4", &[&commit_feat3]);
    let commit_ign = empty_git_commit(&git_repo, "refs/remotes/origin/ignored", &[]);

    // Import branches feature1, feature2, and feature3.
    let mut tx = repo.start_transaction(&settings, "test");
    git::import_some_refs(tx.mut_repo(), &git_repo, &git_settings, |ref_name| {
        ref_name.starts_with("refs/remotes/origin/feature")
    })
    .unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit();

    // There are two heads, feature2 and feature4.
    let view = repo.view();
    let expected_heads = hashset! {
            jj_id(&commit_feat2),
            jj_id(&commit_feat4),
    };
    assert_eq!(*view.heads(), expected_heads);

    // Check that branches feature[1-4] have been locally imported and are known to
    // be present on origin as well.
    assert_eq!(view.branches().len(), 4);
    let commit_feat1_target = RefTarget::Normal(jj_id(&commit_feat1));
    let commit_feat2_target = RefTarget::Normal(jj_id(&commit_feat2));
    let commit_feat3_target = RefTarget::Normal(jj_id(&commit_feat3));
    let commit_feat4_target = RefTarget::Normal(jj_id(&commit_feat4));
    let expected_feature1_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(jj_id(&commit_feat1))),
        remote_targets: btreemap! { "origin".to_string() => commit_feat1_target },
    };
    assert_eq!(
        view.branches().get("feature1"),
        Some(expected_feature1_branch).as_ref()
    );
    let expected_feature2_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(jj_id(&commit_feat2))),
        remote_targets: btreemap! { "origin".to_string() => commit_feat2_target },
    };
    assert_eq!(
        view.branches().get("feature2"),
        Some(expected_feature2_branch).as_ref()
    );
    let expected_feature3_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(jj_id(&commit_feat3))),
        remote_targets: btreemap! { "origin".to_string() => commit_feat3_target },
    };
    assert_eq!(
        view.branches().get("feature3"),
        Some(expected_feature3_branch).as_ref()
    );
    let expected_feature4_branch = BranchTarget {
        local_target: Some(RefTarget::Normal(jj_id(&commit_feat4))),
        remote_targets: btreemap! { "origin".to_string() => commit_feat4_target },
    };
    assert_eq!(
        view.branches().get("feature4"),
        Some(expected_feature4_branch).as_ref()
    );
    assert_eq!(view.branches().get("main"), None,);
    assert!(!view.heads().contains(&jj_id(&commit_main)));
    assert_eq!(view.branches().get("ignored"), None,);
    assert!(!view.heads().contains(&jj_id(&commit_ign)));

    // Delete branch feature1, feature3 and feature4 in git repository and import
    // branch feature2 only. That should have no impact on the jj repository.
    delete_git_ref(&git_repo, "refs/remotes/origin/feature1");
    delete_git_ref(&git_repo, "refs/remotes/origin/feature3");
    delete_git_ref(&git_repo, "refs/remotes/origin/feature4");
    let mut tx = repo.start_transaction(&settings, "test");
    git::import_some_refs(tx.mut_repo(), &git_repo, &git_settings, |ref_name| {
        ref_name == "refs/remotes/origin/feature2"
    })
    .unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit();

    // feature2 and feature4 will still be heads, and all four branches should be
    // present.
    let view = repo.view();
    assert_eq!(view.branches().len(), 4);
    assert_eq!(*view.heads(), expected_heads);

    // Import feature1: this should cause the branch to be deleted, but the
    // corresponding commit should stay because it is reachable from feature2.
    let mut tx = repo.start_transaction(&settings, "test");
    git::import_some_refs(tx.mut_repo(), &git_repo, &git_settings, |ref_name| {
        ref_name == "refs/remotes/origin/feature1"
    })
    .unwrap();
    // No descendant should be rewritten.
    assert_eq!(tx.mut_repo().rebase_descendants(&settings).unwrap(), 0);
    let repo = tx.commit();

    // feature2 and feature4 should still be the heads, and all three branches
    // feature2, feature3, and feature3 should exist.
    let view = repo.view();
    assert_eq!(view.branches().len(), 3);
    assert_eq!(*view.heads(), expected_heads);

    // Import feature3: this should cause the branch to be deleted, but
    // feature4 should be left alone even though it is no longer in git.
    let mut tx = repo.start_transaction(&settings, "test");
    git::import_some_refs(tx.mut_repo(), &git_repo, &git_settings, |ref_name| {
        ref_name == "refs/remotes/origin/feature3"
    })
    .unwrap();
    // No descendant should be rewritten
    assert_eq!(tx.mut_repo().rebase_descendants(&settings).unwrap(), 0);
    let repo = tx.commit();

    // feature2 and feature4 should still be the heads, and both branches
    // should exist.
    let view = repo.view();
    assert_eq!(view.branches().len(), 2);
    assert_eq!(*view.heads(), expected_heads);

    // Import feature4: both the head and the branch will disappear.
    let mut tx = repo.start_transaction(&settings, "test");
    git::import_some_refs(tx.mut_repo(), &git_repo, &git_settings, |ref_name| {
        ref_name == "refs/remotes/origin/feature4"
    })
    .unwrap();
    // No descendant should be rewritten
    assert_eq!(tx.mut_repo().rebase_descendants(&settings).unwrap(), 0);
    let repo = tx.commit();

    // feature2 should now be the only head and only branch.
    let view = repo.view();
    assert_eq!(view.branches().len(), 1);
    let expected_heads = hashset! {
            jj_id(&commit_feat2),
    };
    assert_eq!(*view.heads(), expected_heads);
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
        let repo = ReadonlyRepo::init(
            &settings,
            &jj_repo_dir,
            |store_path| Box::new(GitBackend::init_external(store_path, &git_repo_dir)),
            ReadonlyRepo::default_op_store_factory(),
            ReadonlyRepo::default_op_heads_store_factory(),
            ReadonlyRepo::default_index_store_factory(),
            ReadonlyRepo::default_submodule_store_factory(),
        )
        .unwrap();
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
    let git_settings = GitSettings::default();
    let heads_before = test_data.repo.view().heads().clone();
    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    git::import_refs(tx.mut_repo(), &test_data.git_repo, &git_settings).unwrap();
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
    let git_settings = GitSettings::default();
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

    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    git::import_refs(tx.mut_repo(), &test_data.git_repo, &git_settings).unwrap();
    tx.mut_repo()
        .rebase_descendants(&test_data.settings)
        .unwrap();
    let repo = tx.commit();

    let expected_heads = hashset! { jj_id(&commit1) };
    assert_eq!(*repo.view().heads(), expected_heads);
    assert_eq!(repo.view().git_refs().len(), 0);
    assert_eq!(
        repo.view().git_head(),
        Some(&RefTarget::Normal(jj_id(&commit1)))
    );
}

#[test]
fn test_export_refs_no_detach() {
    // When exporting the branch that's current checked out, don't detach HEAD if
    // the target already matches
    let test_data = GitRepoData::create();
    let git_settings = GitSettings::default();
    let git_repo = test_data.git_repo;
    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_repo.set_head("refs/heads/main").unwrap();
    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let mut_repo = tx.mut_repo();
    git::import_refs(mut_repo, &git_repo, &git_settings).unwrap();
    mut_repo.rebase_descendants(&test_data.settings).unwrap();

    // Do an initial export to make sure `main` is considered
    assert_eq!(git::export_refs(mut_repo, &git_repo), Ok(vec![]));
    assert_eq!(
        mut_repo.get_git_ref("refs/heads/main"),
        Some(RefTarget::Normal(jj_id(&commit1)))
    );
    assert_eq!(git_repo.head().unwrap().name(), Some("refs/heads/main"));
    assert_eq!(
        git_repo.find_reference("refs/heads/main").unwrap().target(),
        Some(commit1.id())
    );
}

#[test]
fn test_export_refs_branch_changed() {
    // We can export a change to a branch
    let test_data = GitRepoData::create();
    let git_settings = GitSettings::default();
    let git_repo = test_data.git_repo;
    let commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_repo
        .reference("refs/heads/feature", commit.id(), false, "test")
        .unwrap();
    git_repo.set_head("refs/heads/feature").unwrap();

    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let mut_repo = tx.mut_repo();
    git::import_refs(mut_repo, &git_repo, &git_settings).unwrap();
    mut_repo.rebase_descendants(&test_data.settings).unwrap();
    assert_eq!(git::export_refs(mut_repo, &git_repo), Ok(vec![]));

    let new_commit = create_random_commit(mut_repo, &test_data.settings)
        .set_parents(vec![jj_id(&commit)])
        .write()
        .unwrap();
    mut_repo.set_local_branch(
        "main".to_string(),
        RefTarget::Normal(new_commit.id().clone()),
    );
    assert_eq!(git::export_refs(mut_repo, &git_repo), Ok(vec![]));
    assert_eq!(
        mut_repo.get_git_ref("refs/heads/main"),
        Some(RefTarget::Normal(new_commit.id().clone()))
    );
    assert_eq!(
        git_repo
            .find_reference("refs/heads/main")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id(),
        git_id(&new_commit)
    );
    // HEAD should be unchanged since its target branch didn't change
    assert_eq!(git_repo.head().unwrap().name(), Some("refs/heads/feature"));
}

#[test]
fn test_export_refs_current_branch_changed() {
    // If we update a branch that is checked out in the git repo, HEAD gets detached
    let test_data = GitRepoData::create();
    let git_settings = GitSettings::default();
    let git_repo = test_data.git_repo;
    let commit1 = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    git_repo.set_head("refs/heads/main").unwrap();
    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let mut_repo = tx.mut_repo();
    git::import_refs(mut_repo, &git_repo, &git_settings).unwrap();
    mut_repo.rebase_descendants(&test_data.settings).unwrap();
    assert_eq!(git::export_refs(mut_repo, &git_repo), Ok(vec![]));

    let new_commit = create_random_commit(mut_repo, &test_data.settings)
        .set_parents(vec![jj_id(&commit1)])
        .write()
        .unwrap();
    mut_repo.set_local_branch(
        "main".to_string(),
        RefTarget::Normal(new_commit.id().clone()),
    );
    assert_eq!(git::export_refs(mut_repo, &git_repo), Ok(vec![]));
    assert_eq!(
        mut_repo.get_git_ref("refs/heads/main"),
        Some(RefTarget::Normal(new_commit.id().clone()))
    );
    assert_eq!(
        git_repo
            .find_reference("refs/heads/main")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id(),
        git_id(&new_commit)
    );
    assert!(git_repo.head_detached().unwrap());
}

#[test]
fn test_export_refs_unborn_git_branch() {
    // Can export to an empty Git repo (we can handle Git's "unborn branch" state)
    let test_data = GitRepoData::create();
    let git_settings = GitSettings::default();
    let git_repo = test_data.git_repo;
    git_repo.set_head("refs/heads/main").unwrap();
    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let mut_repo = tx.mut_repo();
    git::import_refs(mut_repo, &git_repo, &git_settings).unwrap();
    mut_repo.rebase_descendants(&test_data.settings).unwrap();
    assert_eq!(git::export_refs(mut_repo, &git_repo), Ok(vec![]));

    let new_commit = write_random_commit(mut_repo, &test_data.settings);
    mut_repo.set_local_branch(
        "main".to_string(),
        RefTarget::Normal(new_commit.id().clone()),
    );
    assert_eq!(git::export_refs(mut_repo, &git_repo), Ok(vec![]));
    assert_eq!(
        mut_repo.get_git_ref("refs/heads/main"),
        Some(RefTarget::Normal(new_commit.id().clone()))
    );
    assert_eq!(
        git_repo
            .find_reference("refs/heads/main")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id(),
        git_id(&new_commit)
    );
    // It's weird that the head is still pointing to refs/heads/main, but
    // it doesn't seem that Git lets you be on an "unborn branch" while
    // also being in a "detached HEAD" state.
    assert!(!git_repo.head_detached().unwrap());
}

#[test]
fn test_export_import_sequence() {
    // Import a branch pointing to A, modify it in jj to point to B, export it,
    // modify it in git to point to C, then import it again. There should be no
    // conflict.
    let test_data = GitRepoData::create();
    let git_settings = GitSettings::default();
    let git_repo = test_data.git_repo;
    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let mut_repo = tx.mut_repo();
    let commit_a = write_random_commit(mut_repo, &test_data.settings);
    let commit_b = write_random_commit(mut_repo, &test_data.settings);
    let commit_c = write_random_commit(mut_repo, &test_data.settings);

    // Import the branch pointing to A
    git_repo
        .reference("refs/heads/main", git_id(&commit_a), true, "test")
        .unwrap();
    git::import_refs(mut_repo, &git_repo, &git_settings).unwrap();
    assert_eq!(
        mut_repo.get_git_ref("refs/heads/main"),
        Some(RefTarget::Normal(commit_a.id().clone()))
    );

    // Modify the branch in jj to point to B
    mut_repo.set_local_branch("main".to_string(), RefTarget::Normal(commit_b.id().clone()));

    // Export the branch to git
    assert_eq!(git::export_refs(mut_repo, &git_repo), Ok(vec![]));
    assert_eq!(
        mut_repo.get_git_ref("refs/heads/main"),
        Some(RefTarget::Normal(commit_b.id().clone()))
    );

    // Modify the branch in git to point to C
    git_repo
        .reference("refs/heads/main", git_id(&commit_c), true, "test")
        .unwrap();

    // Import from git
    git::import_refs(mut_repo, &git_repo, &git_settings).unwrap();
    assert_eq!(
        mut_repo.get_git_ref("refs/heads/main"),
        Some(RefTarget::Normal(commit_c.id().clone()))
    );
    assert_eq!(
        mut_repo.view().get_local_branch("main"),
        Some(RefTarget::Normal(commit_c.id().clone()))
    );
}

#[test]
fn test_import_export_no_auto_local_branch() {
    // Import a remote tracking branch and export it. We should not create a git
    // branch.
    let test_data = GitRepoData::create();
    let git_settings = GitSettings {
        auto_local_branch: false,
    };
    let git_repo = test_data.git_repo;
    let git_commit = empty_git_commit(&git_repo, "refs/remotes/origin/main", &[]);

    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let mut_repo = tx.mut_repo();

    git::import_refs(mut_repo, &git_repo, &git_settings).unwrap();

    let expected_branch = BranchTarget {
        local_target: None,
        remote_targets: btreemap! {
            "origin".to_string() => RefTarget::Normal(jj_id(&git_commit))
        },
    };
    assert_eq!(
        mut_repo.view().branches().get("main"),
        Some(expected_branch).as_ref()
    );
    assert_eq!(
        mut_repo.get_git_ref("refs/remotes/origin/main"),
        Some(RefTarget::Normal(jj_id(&git_commit)))
    );

    // Export the branch to git
    assert_eq!(git::export_refs(mut_repo, &git_repo), Ok(vec![]));
    assert_eq!(mut_repo.get_git_ref("refs/heads/main"), None);
}

#[test]
fn test_export_conflicts() {
    // We skip export of conflicted branches
    let test_data = GitRepoData::create();
    let git_repo = test_data.git_repo;
    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let mut_repo = tx.mut_repo();
    let commit_a = write_random_commit(mut_repo, &test_data.settings);
    let commit_b = write_random_commit(mut_repo, &test_data.settings);
    let commit_c = write_random_commit(mut_repo, &test_data.settings);
    mut_repo.set_local_branch("main".to_string(), RefTarget::Normal(commit_a.id().clone()));
    mut_repo.set_local_branch(
        "feature".to_string(),
        RefTarget::Normal(commit_a.id().clone()),
    );
    assert_eq!(git::export_refs(mut_repo, &git_repo), Ok(vec![]));

    // Create a conflict and export. It should not be exported, but other changes
    // should be.
    mut_repo.set_local_branch("main".to_string(), RefTarget::Normal(commit_b.id().clone()));
    mut_repo.set_local_branch(
        "feature".to_string(),
        RefTarget::Conflict {
            removes: vec![commit_a.id().clone()],
            adds: vec![commit_b.id().clone(), commit_c.id().clone()],
        },
    );
    assert_eq!(git::export_refs(mut_repo, &git_repo), Ok(vec![]));
    assert_eq!(
        git_repo
            .find_reference("refs/heads/feature")
            .unwrap()
            .target()
            .unwrap(),
        git_id(&commit_a)
    );
    assert_eq!(
        git_repo
            .find_reference("refs/heads/main")
            .unwrap()
            .target()
            .unwrap(),
        git_id(&commit_b)
    );
}

#[test]
fn test_export_partial_failure() {
    // Check that we skip branches that fail to export
    let test_data = GitRepoData::create();
    let git_repo = test_data.git_repo;
    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let mut_repo = tx.mut_repo();
    let commit_a = write_random_commit(mut_repo, &test_data.settings);
    let target = RefTarget::Normal(commit_a.id().clone());
    // Empty string is disallowed by Git
    mut_repo.set_local_branch("".to_string(), target.clone());
    mut_repo.set_local_branch("main".to_string(), target.clone());
    // `main/sub` will conflict with `main` in Git, at least when using loose ref
    // storage
    mut_repo.set_local_branch("main/sub".to_string(), target);
    assert_eq!(
        git::export_refs(mut_repo, &git_repo),
        Ok(vec!["".to_string(), "main/sub".to_string()])
    );

    // The `main` branch should have succeeded but the other should have failed
    assert!(git_repo.find_reference("refs/heads/").is_err());
    assert_eq!(
        git_repo
            .find_reference("refs/heads/main")
            .unwrap()
            .target()
            .unwrap(),
        git_id(&commit_a)
    );
    assert!(git_repo.find_reference("refs/heads/main/sub").is_err());

    // Now remove the `main` branch and make sure that the `main/sub` gets exported
    // even though it didn't change
    mut_repo.remove_local_branch("main");
    assert_eq!(
        git::export_refs(mut_repo, &git_repo),
        Ok(vec!["".to_string()])
    );
    assert!(git_repo.find_reference("refs/heads/").is_err());
    assert!(git_repo.find_reference("refs/heads/main").is_err());
    assert_eq!(
        git_repo
            .find_reference("refs/heads/main/sub")
            .unwrap()
            .target()
            .unwrap(),
        git_id(&commit_a)
    );
}

#[test]
fn test_export_reexport_transitions() {
    // Test exporting after making changes on the jj side, or the git side, or both
    let test_data = GitRepoData::create();
    let git_repo = test_data.git_repo;
    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let mut_repo = tx.mut_repo();
    let commit_a = write_random_commit(mut_repo, &test_data.settings);
    let commit_b = write_random_commit(mut_repo, &test_data.settings);
    let commit_c = write_random_commit(mut_repo, &test_data.settings);
    // Create a few branches whose names indicate how they change in jj in git. The
    // first letter represents the branch's target in the last export. The second
    // letter represents the branch's target in jj. The third letter represents the
    // branch's target in git. "X" means that the branch doesn't exist. "A", "B", or
    // "C" means that the branch points to that commit.
    //
    // AAB: Branch modified in git
    // AAX: Branch deleted in git
    // ABA: Branch modified in jj
    // ABB: Branch modified in both jj and git, pointing to same target
    // ABC: Branch modified in both jj and git, pointing to different targets
    // ABX: Branch modified in jj, deleted in git
    // AXA: Branch deleted in jj
    // AXB: Branch deleted in jj, modified in git
    // AXX: Branch deleted in both jj and git
    // XAA: Branch added in both jj and git, pointing to same target
    // XAB: Branch added in both jj and git, pointing to different targets
    // XAX: Branch added in jj
    // XXA: Branch added in git

    // Create initial state and export it
    for branch in [
        "AAB", "AAX", "ABA", "ABB", "ABC", "ABX", "AXA", "AXB", "AXX",
    ] {
        mut_repo.set_local_branch(branch.to_string(), RefTarget::Normal(commit_a.id().clone()));
    }
    assert_eq!(git::export_refs(mut_repo, &git_repo), Ok(vec![]));

    // Make changes on the jj side
    for branch in ["AXA", "AXB", "AXX"] {
        mut_repo.remove_local_branch(branch);
    }
    for branch in ["XAA", "XAB", "XAX"] {
        mut_repo.set_local_branch(branch.to_string(), RefTarget::Normal(commit_a.id().clone()));
    }
    for branch in ["ABA", "ABB", "ABC", "ABX"] {
        mut_repo.set_local_branch(branch.to_string(), RefTarget::Normal(commit_b.id().clone()));
    }

    // Make changes on the git side
    for branch in ["AAX", "ABX", "AXX"] {
        git_repo
            .find_reference(&format!("refs/heads/{branch}"))
            .unwrap()
            .delete()
            .unwrap();
    }
    for branch in ["XAA", "XXA"] {
        git_repo
            .reference(&format!("refs/heads/{branch}"), git_id(&commit_a), true, "")
            .unwrap();
    }
    for branch in ["AAB", "ABB", "AXB", "XAB"] {
        git_repo
            .reference(&format!("refs/heads/{branch}"), git_id(&commit_b), true, "")
            .unwrap();
    }
    let branch = "ABC";
    git_repo
        .reference(&format!("refs/heads/{branch}"), git_id(&commit_c), true, "")
        .unwrap();

    // TODO: The branches that we made conflicting changes to should have failed to
    // export. They should have been unchanged in git and in
    // mut_repo.view().git_refs().
    assert_eq!(
        git::export_refs(mut_repo, &git_repo),
        Ok(["AXB", "ABC", "ABX", "XAB"]
            .into_iter()
            .map(String::from)
            .collect_vec())
    );
    for branch in ["AAX", "ABX", "AXA", "AXX"] {
        assert!(
            git_repo
                .find_reference(&format!("refs/heads/{branch}"))
                .is_err(),
            "{branch} should not exist"
        );
    }
    for branch in ["XAA", "XAX", "XXA"] {
        assert_eq!(
            git_repo
                .find_reference(&format!("refs/heads/{branch}"))
                .unwrap()
                .target(),
            Some(git_id(&commit_a)),
            "{branch} should point to commit A"
        );
    }
    for branch in ["AAB", "ABA", "AAB", "ABB", "AXB", "XAB"] {
        assert_eq!(
            git_repo
                .find_reference(&format!("refs/heads/{branch}"))
                .unwrap()
                .target(),
            Some(git_id(&commit_b)),
            "{branch} should point to commit B"
        );
    }
    let branch = "ABC";
    assert_eq!(
        git_repo
            .find_reference(&format!("refs/heads/{branch}"))
            .unwrap()
            .target(),
        Some(git_id(&commit_c)),
        "{branch} should point to commit C"
    );
    assert_eq!(
        *mut_repo.view().git_refs(),
        btreemap! {
            "refs/heads/AAX".to_string() => RefTarget::Normal(commit_a.id().clone()),
            "refs/heads/AAB".to_string() => RefTarget::Normal(commit_a.id().clone()),
            "refs/heads/ABA".to_string() => RefTarget::Normal(commit_b.id().clone()),
            "refs/heads/ABB".to_string() => RefTarget::Normal(commit_b.id().clone()),
            "refs/heads/ABC".to_string() => RefTarget::Normal(commit_a.id().clone()),
            "refs/heads/ABX".to_string() => RefTarget::Normal(commit_a.id().clone()),
            "refs/heads/AXB".to_string() => RefTarget::Normal(commit_a.id().clone()),
            "refs/heads/XAA".to_string() => RefTarget::Normal(commit_a.id().clone()),
            "refs/heads/XAX".to_string() => RefTarget::Normal(commit_a.id().clone()),
        }
    );
}

#[test]
fn test_init() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
    let git_repo_dir = temp_dir.path().join("git");
    let jj_repo_dir = temp_dir.path().join("jj");
    let git_repo = git2::Repository::init_bare(&git_repo_dir).unwrap();
    let initial_git_commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    std::fs::create_dir(&jj_repo_dir).unwrap();
    let repo = ReadonlyRepo::init(
        &settings,
        &jj_repo_dir,
        |store_path| Box::new(GitBackend::init_external(store_path, &git_repo_dir)),
        ReadonlyRepo::default_op_store_factory(),
        ReadonlyRepo::default_op_heads_store_factory(),
        ReadonlyRepo::default_index_store_factory(),
        ReadonlyRepo::default_submodule_store_factory(),
    )
    .unwrap();
    // The refs were *not* imported -- it's the caller's responsibility to import
    // any refs they care about.
    assert!(!repo.view().heads().contains(&jj_id(&initial_git_commit)));
}

#[test]
fn test_fetch_empty_repo() {
    let test_data = GitRepoData::create();
    let git_settings = GitSettings::default();

    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let default_branch = git::fetch(
        tx.mut_repo(),
        &test_data.git_repo,
        "origin",
        None,
        git::RemoteCallbacks::default(),
        &git_settings,
    )
    .unwrap();
    // No default branch and no refs
    assert_eq!(default_branch, None);
    assert_eq!(*tx.mut_repo().view().git_refs(), btreemap! {});
    assert_eq!(*tx.mut_repo().view().branches(), btreemap! {});
}

#[test]
fn test_fetch_initial_commit() {
    let test_data = GitRepoData::create();
    let git_settings = GitSettings::default();
    let initial_git_commit = empty_git_commit(&test_data.origin_repo, "refs/heads/main", &[]);

    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let default_branch = git::fetch(
        tx.mut_repo(),
        &test_data.git_repo,
        "origin",
        None,
        git::RemoteCallbacks::default(),
        &git_settings,
    )
    .unwrap();
    // No default branch because the origin repo's HEAD wasn't set
    assert_eq!(default_branch, None);
    let repo = tx.commit();
    // The initial commit is visible after git::fetch().
    let view = repo.view();
    assert!(view.heads().contains(&jj_id(&initial_git_commit)));
    let initial_commit_target = RefTarget::Normal(jj_id(&initial_git_commit));
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
    let git_settings = GitSettings::default();
    let initial_git_commit = empty_git_commit(&test_data.origin_repo, "refs/heads/main", &[]);

    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    git::fetch(
        tx.mut_repo(),
        &test_data.git_repo,
        "origin",
        None,
        git::RemoteCallbacks::default(),
        &git_settings,
    )
    .unwrap();
    test_data.repo = tx.commit();

    test_data.origin_repo.set_head("refs/heads/main").unwrap();
    let new_git_commit = empty_git_commit(
        &test_data.origin_repo,
        "refs/heads/main",
        &[&initial_git_commit],
    );

    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let default_branch = git::fetch(
        tx.mut_repo(),
        &test_data.git_repo,
        "origin",
        None,
        git::RemoteCallbacks::default(),
        &git_settings,
    )
    .unwrap();
    // The default branch is "main"
    assert_eq!(default_branch, Some("main".to_string()));
    let repo = tx.commit();
    // The new commit is visible after we fetch again
    let view = repo.view();
    assert!(view.heads().contains(&jj_id(&new_git_commit)));
    let new_commit_target = RefTarget::Normal(jj_id(&new_git_commit));
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
    let git_settings = GitSettings::default();
    empty_git_commit(&test_data.git_repo, "refs/heads/main", &[]);

    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    git::fetch(
        tx.mut_repo(),
        &test_data.git_repo,
        "origin",
        None,
        git::RemoteCallbacks::default(),
        &git_settings,
    )
    .unwrap();
    // Test the setup
    assert!(tx.mut_repo().get_branch("main").is_some());

    test_data
        .git_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .delete()
        .unwrap();
    // After re-fetching, the branch should be deleted
    git::fetch(
        tx.mut_repo(),
        &test_data.git_repo,
        "origin",
        None,
        git::RemoteCallbacks::default(),
        &git_settings,
    )
    .unwrap();
    assert!(tx.mut_repo().get_branch("main").is_none());
}

#[test]
fn test_fetch_no_default_branch() {
    let test_data = GitRepoData::create();
    let git_settings = GitSettings::default();
    let initial_git_commit = empty_git_commit(&test_data.origin_repo, "refs/heads/main", &[]);

    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    git::fetch(
        tx.mut_repo(),
        &test_data.git_repo,
        "origin",
        None,
        git::RemoteCallbacks::default(),
        &git_settings,
    )
    .unwrap();

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

    let default_branch = git::fetch(
        tx.mut_repo(),
        &test_data.git_repo,
        "origin",
        None,
        git::RemoteCallbacks::default(),
        &git_settings,
    )
    .unwrap();
    // There is no default branch
    assert_eq!(default_branch, None);
}

#[test]
fn test_fetch_no_such_remote() {
    let test_data = GitRepoData::create();
    let git_settings = GitSettings::default();
    let mut tx = test_data
        .repo
        .start_transaction(&test_data.settings, "test");
    let result = git::fetch(
        tx.mut_repo(),
        &test_data.git_repo,
        "invalid-remote",
        None,
        git::RemoteCallbacks::default(),
        &git_settings,
    );
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
    git2::Repository::clone(source_repo_dir.to_str().unwrap(), &clone_repo_dir).unwrap();
    std::fs::create_dir(&jj_repo_dir).unwrap();
    let jj_repo = ReadonlyRepo::init(
        settings,
        &jj_repo_dir,
        |store_path| Box::new(GitBackend::init_external(store_path, &clone_repo_dir)),
        ReadonlyRepo::default_op_store_factory(),
        ReadonlyRepo::default_op_heads_store_factory(),
        ReadonlyRepo::default_index_store_factory(),
        ReadonlyRepo::default_submodule_store_factory(),
    )
    .unwrap();
    let mut tx = jj_repo.start_transaction(settings, "test");
    let new_commit = create_random_commit(tx.mut_repo(), settings)
        .set_parents(vec![jj_id(&initial_git_commit)])
        .write()
        .unwrap();
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
    let clone_repo = get_git_repo(&setup.jj_repo);
    let result = git::push_updates(
        &clone_repo,
        "origin",
        &[GitRefUpdate {
            qualified_name: "refs/heads/main".to_string(),
            force: false,
            new_target: Some(setup.new_commit.id().clone()),
        }],
        git::RemoteCallbacks::default(),
    );
    assert_eq!(result, Ok(()));

    // Check that the ref got updated in the source repo
    let source_repo = git2::Repository::open(&setup.source_repo_dir).unwrap();
    let new_target = source_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .target();
    let new_oid = git_id(&setup.new_commit);
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
    let clone_repo = get_git_repo(&setup.jj_repo);

    let source_repo = git2::Repository::open(&setup.source_repo_dir).unwrap();
    // Test the setup
    assert!(source_repo.find_reference("refs/heads/main").is_ok());

    let result = git::push_updates(
        &get_git_repo(&setup.jj_repo),
        "origin",
        &[GitRefUpdate {
            qualified_name: "refs/heads/main".to_string(),
            force: false,
            new_target: None,
        }],
        git::RemoteCallbacks::default(),
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
    let clone_repo = get_git_repo(&setup.jj_repo);
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
        git::RemoteCallbacks::default(),
    );
    assert_eq!(result, Ok(()));

    // Check that the topic ref got updated in the source repo
    let source_repo = git2::Repository::open(&setup.source_repo_dir).unwrap();
    let new_target = source_repo
        .find_reference("refs/heads/topic")
        .unwrap()
        .target();
    assert_eq!(new_target, Some(git_id(&setup.new_commit)));

    // Check that the main ref got deleted in the source repo
    assert!(source_repo.find_reference("refs/heads/main").is_err());
}

#[test]
fn test_push_updates_not_fast_forward() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
    let mut setup = set_up_push_repos(&settings, &temp_dir);
    let mut tx = setup.jj_repo.start_transaction(&settings, "test");
    let new_commit = write_random_commit(tx.mut_repo(), &settings);
    setup.jj_repo = tx.commit();
    let result = git::push_updates(
        &get_git_repo(&setup.jj_repo),
        "origin",
        &[GitRefUpdate {
            qualified_name: "refs/heads/main".to_string(),
            force: false,
            new_target: Some(new_commit.id().clone()),
        }],
        git::RemoteCallbacks::default(),
    );
    assert_eq!(result, Err(GitPushError::NotFastForward));
}

#[test]
fn test_push_updates_not_fast_forward_with_force() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
    let mut setup = set_up_push_repos(&settings, &temp_dir);
    let mut tx = setup.jj_repo.start_transaction(&settings, "test");
    let new_commit = write_random_commit(tx.mut_repo(), &settings);
    setup.jj_repo = tx.commit();
    let result = git::push_updates(
        &get_git_repo(&setup.jj_repo),
        "origin",
        &[GitRefUpdate {
            qualified_name: "refs/heads/main".to_string(),
            force: true,
            new_target: Some(new_commit.id().clone()),
        }],
        git::RemoteCallbacks::default(),
    );
    assert_eq!(result, Ok(()));

    // Check that the ref got updated in the source repo
    let source_repo = git2::Repository::open(&setup.source_repo_dir).unwrap();
    let new_target = source_repo
        .find_reference("refs/heads/main")
        .unwrap()
        .target();
    assert_eq!(new_target, Some(git_id(&new_commit)));
}

#[test]
fn test_push_updates_no_such_remote() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let result = git::push_updates(
        &get_git_repo(&setup.jj_repo),
        "invalid-remote",
        &[GitRefUpdate {
            qualified_name: "refs/heads/main".to_string(),
            force: false,
            new_target: Some(setup.new_commit.id().clone()),
        }],
        git::RemoteCallbacks::default(),
    );
    assert!(matches!(result, Err(GitPushError::NoSuchRemote(_))));
}

#[test]
fn test_push_updates_invalid_remote() {
    let settings = testutils::user_settings();
    let temp_dir = testutils::new_temp_dir();
    let setup = set_up_push_repos(&settings, &temp_dir);
    let result = git::push_updates(
        &get_git_repo(&setup.jj_repo),
        "http://invalid-remote",
        &[GitRefUpdate {
            qualified_name: "refs/heads/main".to_string(),
            force: false,
            new_target: Some(setup.new_commit.id().clone()),
        }],
        git::RemoteCallbacks::default(),
    );
    assert!(matches!(result, Err(GitPushError::NoSuchRemote(_))));
}

#[test]
fn test_bulk_update_extra_on_import_refs() {
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let git_repo = get_git_repo(repo);

    let count_extra_tables = || {
        let extra_dir = repo.repo_path().join("store").join("extra");
        extra_dir
            .read_dir()
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().metadata().unwrap().is_file())
            .count()
    };
    let import_refs = |repo: &Arc<ReadonlyRepo>| {
        let mut tx = repo.start_transaction(&settings, "test");
        git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
        tx.mut_repo().rebase_descendants(&settings).unwrap();
        tx.commit()
    };

    // Extra metadata table shouldn't be created per read_commit() call. The number
    // of the table files should be way smaller than the number of the heads.
    let mut commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    for _ in 1..10 {
        commit = empty_git_commit(&git_repo, "refs/heads/main", &[&commit]);
    }
    let repo = import_refs(repo);
    assert_eq!(count_extra_tables(), 2); // empty + imported_heads == 2

    // Noop import shouldn't create a table file.
    let repo = import_refs(&repo);
    assert_eq!(count_extra_tables(), 2);

    // Importing new head should add exactly one table file.
    for _ in 0..10 {
        commit = empty_git_commit(&git_repo, "refs/heads/main", &[&commit]);
    }
    let repo = import_refs(&repo);
    assert_eq!(count_extra_tables(), 3);

    drop(repo); // silence clippy
}

#[test]
fn test_rewrite_imported_commit() {
    let settings = testutils::user_settings();
    let git_settings = GitSettings::default();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;
    let git_repo = get_git_repo(repo);

    // Import git commit, which generates change id from the commit id.
    let git_commit = empty_git_commit(&git_repo, "refs/heads/main", &[]);
    let mut tx = repo.start_transaction(&settings, "test");
    git::import_refs(tx.mut_repo(), &git_repo, &git_settings).unwrap();
    tx.mut_repo().rebase_descendants(&settings).unwrap();
    let repo = tx.commit();
    let imported_commit = repo.store().get_commit(&jj_id(&git_commit)).unwrap();

    // Try to create identical commit with different change id.
    let mut tx = repo.start_transaction(&settings, "test");
    let authored_commit = tx
        .mut_repo()
        .new_commit(
            &settings,
            imported_commit.parent_ids().to_vec(),
            imported_commit.tree_id().clone(),
        )
        .set_author(imported_commit.author().clone())
        .set_committer(imported_commit.committer().clone())
        .set_description(imported_commit.description())
        .write()
        .unwrap();
    let repo = tx.commit();

    // Imported commit shouldn't be reused, and the timestamp of the authored
    // commit should be adjusted to create new commit.
    assert_ne!(imported_commit.id(), authored_commit.id());
    assert_ne!(
        imported_commit.committer().timestamp,
        authored_commit.committer().timestamp,
    );

    // The index should be consistent with the store.
    assert_eq!(
        repo.resolve_change_id(imported_commit.change_id()),
        Some(vec![imported_commit.id().clone()]),
    );
    assert_eq!(
        repo.resolve_change_id(authored_commit.change_id()),
        Some(vec![authored_commit.id().clone()]),
    );
}

#[test]
fn test_concurrent_write_commit() {
    let settings = &testutils::user_settings();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;

    // Try to create identical commits with different change ids. Timestamp of the
    // commits should be adjusted such that each commit has a unique commit id.
    let num_thread = 8;
    let (sender, receiver) = mpsc::channel();
    thread::scope(|s| {
        let barrier = Arc::new(Barrier::new(num_thread));
        for i in 0..num_thread {
            let repo = load_repo_at_head(settings, repo.repo_path()); // unshare loader
            let barrier = barrier.clone();
            let sender = sender.clone();
            s.spawn(move || {
                barrier.wait();
                let mut tx = repo.start_transaction(settings, &format!("writer {i}"));
                let commit = create_rooted_commit(tx.mut_repo(), settings)
                    .set_description("racy commit")
                    .write()
                    .unwrap();
                tx.commit();
                sender
                    .send((commit.id().clone(), commit.change_id().clone()))
                    .unwrap();
            });
        }
    });

    drop(sender);
    let mut commit_change_ids: BTreeMap<CommitId, HashSet<ChangeId>> = BTreeMap::new();
    for (commit_id, change_id) in receiver {
        commit_change_ids
            .entry(commit_id)
            .or_default()
            .insert(change_id);
    }

    // Ideally, each commit should have unique commit/change ids.
    assert_eq!(commit_change_ids.len(), num_thread);

    // All unique commits should be preserved.
    let repo = repo.reload_at_head(settings).unwrap();
    for (commit_id, change_ids) in &commit_change_ids {
        let commit = repo.store().get_commit(commit_id).unwrap();
        assert_eq!(commit.id(), commit_id);
        assert!(change_ids.contains(commit.change_id()));
    }

    // The index should be consistent with the store.
    for commit_id in commit_change_ids.keys() {
        assert!(repo.index().has_id(commit_id));
        let commit = repo.store().get_commit(commit_id).unwrap();
        assert_eq!(
            repo.resolve_change_id(commit.change_id()),
            Some(vec![commit_id.clone()]),
        );
    }
}

#[test]
fn test_concurrent_read_write_commit() {
    let settings = &testutils::user_settings();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;

    // Create unique commits and load them concurrently. In this test, we assume
    // that writer doesn't fall back to timestamp adjustment, so the expected
    // commit ids are static. If reader could interrupt in the timestamp
    // adjustment loop, this assumption wouldn't apply.
    let commit_ids = [
        "c5c6efd6ac240102e7f047234c3cade55eedd621",
        "9f7a96a6c9d044b228f3321a365bdd3514e6033a",
        "aa7867ad0c566df5bbb708d8d6ddc88eefeea0ff",
        "930a76e333d5cc17f40a649c3470cb99aae24a0c",
        "88e9a719df4f0cc3daa740b814e271341f6ea9f4",
        "4883bdc57448a53b4eef1af85e34b85b9ee31aee",
        "308345f8d058848e83beed166704faac2ecd4541",
        "9e35ff61ea8d1d4ef7f01edc5fd23873cc301b30",
        "8010ac8c65548dd619e7c83551d983d724dda216",
        "bbe593d556ea31acf778465227f340af7e627b2b",
        "2f6800f4b8e8fc4c42dc0e417896463d13673654",
        "a3a7e4fcddeaa11bb84f66f3428f107f65eb3268",
        "96e17ff3a7ee1b67ddfa5619b2bf5380b80f619a",
        "34613f7609524c54cc990ada1bdef3dcad0fd29f",
        "95867e5aed6b62abc2cd6258da9fee8873accfd3",
        "7635ce107ae7ba71821b8cd74a1405ca6d9e49ac",
    ]
    .into_iter()
    .map(CommitId::from_hex)
    .collect_vec();
    let num_reader_thread = 8;
    thread::scope(|s| {
        let barrier = Arc::new(Barrier::new(commit_ids.len() + num_reader_thread));

        // Writer assigns random change id
        for (i, commit_id) in commit_ids.iter().enumerate() {
            let repo = load_repo_at_head(settings, repo.repo_path()); // unshare loader
            let barrier = barrier.clone();
            s.spawn(move || {
                barrier.wait();
                let mut tx = repo.start_transaction(settings, &format!("writer {i}"));
                let commit = create_rooted_commit(tx.mut_repo(), settings)
                    .set_description(format!("commit {i}"))
                    .write()
                    .unwrap();
                tx.commit();
                assert_eq!(commit.id(), commit_id);
            });
        }

        // Reader may generate change id (if not yet assigned by the writer)
        for i in 0..num_reader_thread {
            let mut repo = load_repo_at_head(settings, repo.repo_path()); // unshare loader
            let barrier = barrier.clone();
            let mut pending_commit_ids = commit_ids.clone();
            pending_commit_ids.rotate_left(i); // start lookup from different place
            s.spawn(move || {
                barrier.wait();
                while !pending_commit_ids.is_empty() {
                    repo = repo.reload_at_head(settings).unwrap();
                    let mut tx = repo.start_transaction(settings, &format!("reader {i}"));
                    pending_commit_ids = pending_commit_ids
                        .into_iter()
                        .filter_map(|commit_id| {
                            match repo.store().get_commit(&commit_id) {
                                Ok(commit) => {
                                    // update index as git::import_refs() would do
                                    tx.mut_repo().add_head(&commit);
                                    None
                                }
                                Err(BackendError::ObjectNotFound { .. }) => Some(commit_id),
                                Err(err) => panic!("unexpected error: {err}"),
                            }
                        })
                        .collect_vec();
                    if tx.mut_repo().has_changes() {
                        tx.commit();
                    }
                    thread::yield_now();
                }
            });
        }
    });

    // The index should be consistent with the store.
    let repo = repo.reload_at_head(settings).unwrap();
    for commit_id in &commit_ids {
        assert!(repo.index().has_id(commit_id));
        let commit = repo.store().get_commit(commit_id).unwrap();
        assert_eq!(
            repo.resolve_change_id(commit.change_id()),
            Some(vec![commit_id.clone()]),
        );
    }
}

fn create_rooted_commit<'repo>(
    mut_repo: &'repo mut MutableRepo,
    settings: &UserSettings,
) -> CommitBuilder<'repo> {
    let signature = Signature {
        name: "Test User".to_owned(),
        email: "test.user@example.com".to_owned(),
        timestamp: Timestamp {
            // avoid underflow during timestamp adjustment
            timestamp: MillisSinceEpoch(1_000_000),
            tz_offset: 0,
        },
    };
    mut_repo
        .new_commit(
            settings,
            vec![mut_repo.store().root_commit_id().clone()],
            mut_repo.store().empty_tree_id().clone(),
        )
        .set_author(signature.clone())
        .set_committer(signature)
}
