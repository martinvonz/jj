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

use jujube_lib::commit::Commit;
use jujube_lib::commit_builder::CommitBuilder;
use jujube_lib::evolution::{evolve, EvolveListener};
use jujube_lib::repo::{MutableRepo, ReadonlyRepo};
use jujube_lib::repo_path::FileRepoPath;
use jujube_lib::settings::UserSettings;
use jujube_lib::testutils;
use test_case::test_case;

#[must_use]
fn child_commit(settings: &UserSettings, repo: &ReadonlyRepo, commit: &Commit) -> CommitBuilder {
    testutils::create_random_commit(&settings, repo).set_parents(vec![commit.id().clone()])
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_obsolete_and_orphan(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // A commit without successors should not be obsolete and not an orphan.
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    assert!(!mut_repo.evolution().is_obsolete(original.id()));
    assert!(!mut_repo.evolution().is_orphan(original.id()));

    // A commit with a successor with a different change_id should not be obsolete.
    let child = child_commit(&settings, &repo, &original).write_to_repo(mut_repo);
    let grandchild = child_commit(&settings, &repo, &child).write_to_repo(mut_repo);
    let cherry_picked = child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    assert!(!mut_repo.evolution().is_obsolete(original.id()));
    assert!(!mut_repo.evolution().is_orphan(original.id()));
    assert!(!mut_repo.evolution().is_obsolete(child.id()));
    assert!(!mut_repo.evolution().is_orphan(child.id()));

    // A commit with a successor with the same change_id should be obsolete.
    let rewritten = child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    assert!(mut_repo.evolution().is_obsolete(original.id()));
    assert!(!mut_repo.evolution().is_obsolete(child.id()));
    assert!(mut_repo.evolution().is_orphan(child.id()));
    assert!(mut_repo.evolution().is_orphan(grandchild.id()));
    assert!(!mut_repo.evolution().is_obsolete(cherry_picked.id()));
    assert!(!mut_repo.evolution().is_orphan(cherry_picked.id()));
    assert!(!mut_repo.evolution().is_obsolete(rewritten.id()));
    assert!(!mut_repo.evolution().is_orphan(rewritten.id()));

    // It should no longer be obsolete if we remove the successor.
    mut_repo.remove_head(&rewritten);
    assert!(!mut_repo.evolution().is_obsolete(original.id()));
    assert!(!mut_repo.evolution().is_orphan(child.id()));
    assert!(!mut_repo.evolution().is_orphan(grandchild.id()));
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_divergent(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // A single commit should not be divergent
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    assert!(!mut_repo.evolution().is_divergent(original.change_id()));

    // Commits with the same change id are divergent, including the original commit
    // (it's the change that's divergent)
    child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    assert!(mut_repo.evolution().is_divergent(original.change_id()));
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_divergent_pruned(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);

    // Pruned commits are also divergent (because it's unclear where descendants
    // should be evolved to).
    child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .set_pruned(true)
        .write_to_repo(mut_repo);
    child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .set_pruned(true)
        .write_to_repo(mut_repo);
    assert!(mut_repo.evolution().is_divergent(original.change_id()));
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_divergent_duplicate(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // Successors with different change id are not divergent
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let cherry_picked1 = child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    let cherry_picked2 = child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    assert!(!mut_repo.evolution().is_divergent(original.change_id()));
    assert!(!mut_repo
        .evolution()
        .is_divergent(cherry_picked1.change_id()));
    assert!(!mut_repo
        .evolution()
        .is_divergent(cherry_picked2.change_id()));
    tx.discard();
}

// TODO: Create a #[repo_test] proc macro that injects the `settings` and `repo`
// variables into the test function
#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_new_parent_rewritten(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // After a simple rewrite, the new parent is the successor.
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let rewritten = child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    assert_eq!(
        mut_repo
            .evolution()
            .new_parent(mut_repo.as_repo_ref(), original.id()),
        vec![rewritten.id().clone()].into_iter().collect()
    );
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_new_parent_cherry_picked(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // A successor with a different change id has no effect.
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let _cherry_picked = child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    assert_eq!(
        mut_repo
            .evolution()
            .new_parent(mut_repo.as_repo_ref(), original.id()),
        vec![original.id().clone()].into_iter().collect()
    );
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_new_parent_is_pruned(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // If a commit's successor is pruned, the new parent is the parent of the
    // pruned commit.
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let new_parent = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let _rewritten = child_commit(&settings, &repo, &new_parent)
        .set_pruned(true)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    assert_eq!(
        mut_repo
            .evolution()
            .new_parent(mut_repo.as_repo_ref(), original.id()),
        vec![new_parent.id().clone()].into_iter().collect()
    );
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_new_parent_divergent(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // If a commit has multiple successors, then they will all be returned.
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let rewritten1 = child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    let rewritten2 = child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    let rewritten3 = child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    assert_eq!(
        mut_repo
            .evolution()
            .new_parent(mut_repo.as_repo_ref(), original.id()),
        vec![
            rewritten1.id().clone(),
            rewritten2.id().clone(),
            rewritten3.id().clone()
        ]
        .into_iter()
        .collect()
    );
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_new_parent_divergent_one_not_pruned(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // If a commit has multiple successors, then they will all be returned, even if
    // all but one are pruned (the parents of the pruned commits, not the pruned
    // commits themselves).
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let rewritten1 = child_commit(&settings, &repo, &root_commit)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    let parent2 = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let _rewritten2 = child_commit(&settings, &repo, &parent2)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .set_pruned(true)
        .write_to_repo(mut_repo);
    let parent3 = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let _rewritten3 = child_commit(&settings, &repo, &parent3)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .set_pruned(true)
        .write_to_repo(mut_repo);
    assert_eq!(
        mut_repo
            .evolution()
            .new_parent(mut_repo.as_repo_ref(), original.id()),
        vec![
            rewritten1.id().clone(),
            parent2.id().clone(),
            parent3.id().clone()
        ]
        .into_iter()
        .collect()
    );
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_new_parent_divergent_all_pruned(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // If a commit has multiple successors, then they will all be returned, even if
    // they are all pruned (the parents of the pruned commits, not the pruned
    // commits themselves).
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let parent1 = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let _rewritten1 = child_commit(&settings, &repo, &parent1)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .set_pruned(true)
        .write_to_repo(mut_repo);
    let parent2 = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let _rewritten2 = child_commit(&settings, &repo, &parent2)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .set_pruned(true)
        .write_to_repo(mut_repo);
    let parent3 = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let _rewritten3 = child_commit(&settings, &repo, &parent3)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .set_pruned(true)
        .write_to_repo(mut_repo);
    assert_eq!(
        mut_repo
            .evolution()
            .new_parent(mut_repo.as_repo_ref(), original.id()),
        vec![
            parent1.id().clone(),
            parent2.id().clone(),
            parent3.id().clone()
        ]
        .into_iter()
        .collect()
    );
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_new_parent_split(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // If a commit was split, the new parent is the tip-most rewritten
    // commit. Here we let the middle commit inherit the change id, but it shouldn't
    // matter which one inherits it.
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let new_parent = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let rewritten1 = child_commit(&settings, &repo, &new_parent)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    let rewritten2 = child_commit(&settings, &repo, &rewritten1)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    let rewritten3 = child_commit(&settings, &repo, &rewritten2)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    assert_eq!(
        mut_repo
            .evolution()
            .new_parent(mut_repo.as_repo_ref(), original.id()),
        vec![rewritten3.id().clone()].into_iter().collect()
    );
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_new_parent_split_pruned_descendant(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // If a commit was split and the tip-most successor became pruned,
    // we use that that descendant's parent.
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let new_parent = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let rewritten1 = child_commit(&settings, &repo, &new_parent)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    let rewritten2 = child_commit(&settings, &repo, &rewritten1)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    let rewritten3 = child_commit(&settings, &repo, &rewritten2)
        .set_pruned(true)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    let _rewritten4 = child_commit(&settings, &repo, &rewritten3)
        .set_pruned(true)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    assert_eq!(
        mut_repo
            .evolution()
            .new_parent(mut_repo.as_repo_ref(), original.id()),
        vec![rewritten2.id().clone()].into_iter().collect()
    );
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_new_parent_split_forked(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // If a commit was split and the successors were split up across topological
    // branches, we return only the descendants from the branch with the same
    // change id (we can't tell a split from two unrelated rewrites and cherry-picks
    // anyway).
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let new_parent = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let rewritten1 = child_commit(&settings, &repo, &new_parent)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    let rewritten2 = child_commit(&settings, &repo, &rewritten1)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    let rewritten3 = child_commit(&settings, &repo, &rewritten1)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    let _rewritten4 = child_commit(&settings, &repo, &original)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    assert_eq!(
        mut_repo
            .evolution()
            .new_parent(mut_repo.as_repo_ref(), original.id()),
        vec![rewritten2.id().clone(), rewritten3.id().clone()]
            .into_iter()
            .collect()
    );
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_new_parent_split_forked_pruned(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // If a commit was split and the successors were split up across topological
    // branches and some commits were pruned, we won't return a parent of the pruned
    // commit if the parent is an ancestor of another commit we'd return.
    let original = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let new_parent = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let rewritten1 = child_commit(&settings, &repo, &new_parent)
        .set_predecessors(vec![original.id().clone()])
        .set_change_id(original.change_id().clone())
        .write_to_repo(mut_repo);
    let rewritten2 = child_commit(&settings, &repo, &rewritten1)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    let rewritten3 = child_commit(&settings, &repo, &rewritten2)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    let _rewritten4 = child_commit(&settings, &repo, &rewritten1)
        .set_pruned(true)
        .set_predecessors(vec![original.id().clone()])
        .write_to_repo(mut_repo);
    assert_eq!(
        mut_repo
            .evolution()
            .new_parent(mut_repo.as_repo_ref(), original.id()),
        vec![rewritten3.id().clone()].into_iter().collect()
    );
    tx.discard();
}

struct RecordingEvolveListener {
    evolved_orphans: Vec<(Commit, Commit)>,
    evolved_divergents: Vec<(Vec<Commit>, Commit)>,
}

impl Default for RecordingEvolveListener {
    fn default() -> Self {
        RecordingEvolveListener {
            evolved_orphans: Default::default(),
            evolved_divergents: Default::default(),
        }
    }
}

impl EvolveListener for RecordingEvolveListener {
    fn orphan_evolved(
        &mut self,
        _mut_repo: &mut MutableRepo,
        orphan: &Commit,
        new_commit: &Commit,
    ) {
        self.evolved_orphans
            .push((orphan.clone(), new_commit.clone()));
    }

    fn orphan_target_ambiguous(&mut self, _mut_repo: &mut MutableRepo, _orphan: &Commit) {
        // TODO: Record this too and add tests
        panic!("unexpected call to orphan_target_ambiguous");
    }

    fn divergent_resolved(
        &mut self,
        _mut_repo: &mut MutableRepo,
        sources: &[Commit],
        resolved: &Commit,
    ) {
        self.evolved_divergents
            .push((sources.to_vec(), resolved.clone()));
    }

    fn divergent_no_common_predecessor(
        &mut self,
        _mut_repo: &mut MutableRepo,
        _commit1: &Commit,
        _commit2: &Commit,
    ) {
        // TODO: Record this too and add tests
        panic!("unexpected call to divergent_no_common_predecessor");
    }
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_evolve_orphan(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let initial = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let child = child_commit(&settings, &repo, &initial).write_to_repo(mut_repo);
    let grandchild = child_commit(&settings, &repo, &child).write_to_repo(mut_repo);

    let rewritten = CommitBuilder::for_rewrite_from(&settings, repo.store(), &initial)
        .set_description("rewritten".to_string())
        .write_to_repo(mut_repo);

    let mut listener = RecordingEvolveListener::default();
    evolve(&settings, mut_repo, &mut listener);
    assert_eq!(listener.evolved_divergents.len(), 0);
    assert_eq!(listener.evolved_orphans.len(), 2);
    assert_eq!(&listener.evolved_orphans[0].0, &child);
    assert_eq!(&listener.evolved_orphans[0].1.parents(), &vec![rewritten]);
    assert_eq!(&listener.evolved_orphans[1].0, &grandchild);
    assert_eq!(
        &listener.evolved_orphans[1].1.parents(),
        &vec![listener.evolved_orphans[0].1.clone()]
    );
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_evolve_pruned_orphan(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let initial = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    // Create a pruned child and a non-pruned child to show that the pruned one does
    // not get evolved (the non-pruned one is there to show that the setup is not
    // broken).
    let child = child_commit(&settings, &repo, &initial).write_to_repo(mut_repo);
    let _pruned_child = child_commit(&settings, &repo, &initial)
        .set_pruned(true)
        .write_to_repo(mut_repo);
    let _rewritten = CommitBuilder::for_rewrite_from(&settings, repo.store(), &initial)
        .set_description("rewritten".to_string())
        .write_to_repo(mut_repo);

    let mut listener = RecordingEvolveListener::default();
    evolve(&settings, mut_repo, &mut listener);
    assert_eq!(listener.evolved_divergents.len(), 0);
    assert_eq!(listener.evolved_orphans.len(), 1);
    assert_eq!(listener.evolved_orphans[0].0.id(), child.id());

    tx.discard();
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_evolve_multiple_orphans(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let root_commit = repo.store().root_commit();

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let initial = child_commit(&settings, &repo, &root_commit).write_to_repo(mut_repo);
    let child = child_commit(&settings, &repo, &initial).write_to_repo(mut_repo);
    let grandchild = child_commit(&settings, &repo, &child).write_to_repo(mut_repo);
    let grandchild2 = child_commit(&settings, &repo, &child).write_to_repo(mut_repo);

    let rewritten = CommitBuilder::for_rewrite_from(&settings, repo.store(), &initial)
        .set_description("rewritten".to_string())
        .write_to_repo(mut_repo);

    let mut listener = RecordingEvolveListener::default();
    evolve(&settings, mut_repo, &mut listener);
    assert_eq!(listener.evolved_divergents.len(), 0);
    assert_eq!(listener.evolved_orphans.len(), 3);
    assert_eq!(&listener.evolved_orphans[0].0, &child);
    assert_eq!(&listener.evolved_orphans[0].1.parents(), &vec![rewritten]);
    assert_eq!(&listener.evolved_orphans[1].0, &grandchild);
    assert_eq!(
        &listener.evolved_orphans[1].1.parents(),
        &vec![listener.evolved_orphans[0].1.clone()]
    );
    assert_eq!(&listener.evolved_orphans[2].0, &grandchild2);
    assert_eq!(
        &listener.evolved_orphans[2].1.parents(),
        &vec![listener.evolved_orphans[0].1.clone()]
    );
    tx.discard();
}

#[test_case(false ; "local store")]
// #[test_case(true ; "git store")]
fn test_evolve_divergent(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let store = repo.store();
    let root_commit = store.root_commit();

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // Set up a repo like this:
    //
    // x 6 add files X and Z (divergent commit 2)
    // o 5 add file A, contents C
    // | x 4 add files X and Y (divergent commit 1)
    // | o 3 add file A, contents B
    // |/
    // | x 2 add file X (source of divergence)
    // | o 1 add file A, contents A
    // |/
    // o root
    //
    // Resolving the divergence should result in a new commit on top of 5 (because
    // commit 6 has a later commit time than commit 4). It should have files C,
    // X, Y, Z.

    let path_a = FileRepoPath::from("A");
    let path_x = FileRepoPath::from("X");
    let path_y = FileRepoPath::from("Y");
    let path_z = FileRepoPath::from("Z");
    let tree1 = testutils::create_tree(&repo, &[(&path_a, "A")]);
    let tree2 = testutils::create_tree(&repo, &[(&path_a, "A"), (&path_x, "X")]);
    let tree3 = testutils::create_tree(&repo, &[(&path_a, "B")]);
    let tree4 = testutils::create_tree(&repo, &[(&path_a, "B"), (&path_x, "X"), (&path_y, "Y")]);
    let tree5 = testutils::create_tree(&repo, &[(&path_a, "C")]);
    let tree6 = testutils::create_tree(&repo, &[(&path_a, "C"), (&path_x, "X"), (&path_z, "Z")]);

    let commit1 = CommitBuilder::for_new_commit(&settings, repo.store(), tree1.id().clone())
        .set_parents(vec![root_commit.id().clone()])
        .set_description("add file A, contents A".to_string())
        .write_to_repo(mut_repo);
    let commit3 = CommitBuilder::for_new_commit(&settings, repo.store(), tree3.id().clone())
        .set_parents(vec![root_commit.id().clone()])
        .set_description("add file A, contents B".to_string())
        .write_to_repo(mut_repo);
    let commit5 = CommitBuilder::for_new_commit(&settings, repo.store(), tree5.id().clone())
        .set_parents(vec![root_commit.id().clone()])
        .set_description("add file A, contents C".to_string())
        .write_to_repo(mut_repo);
    let commit2 = CommitBuilder::for_new_commit(&settings, repo.store(), tree2.id().clone())
        .set_parents(vec![commit1.id().clone()])
        .set_description("add file X".to_string())
        .write_to_repo(mut_repo);
    let commit4 = CommitBuilder::for_rewrite_from(&settings, repo.store(), &commit2)
        .set_parents(vec![commit3.id().clone()])
        .set_tree(tree4.id().clone())
        .set_description("add files X and Y".to_string())
        .write_to_repo(mut_repo);
    let mut later_time = commit4.committer().clone();
    later_time.timestamp.timestamp.0 += 1;
    let commit6 = CommitBuilder::for_rewrite_from(&settings, repo.store(), &commit2)
        .set_parents(vec![commit5.id().clone()])
        .set_tree(tree6.id().clone())
        .set_description("add files X and Z".to_string())
        .set_committer(later_time)
        .write_to_repo(mut_repo);

    let mut listener = RecordingEvolveListener::default();
    evolve(&settings, mut_repo, &mut listener);
    assert_eq!(listener.evolved_orphans.len(), 0);
    assert_eq!(listener.evolved_divergents.len(), 1);
    assert_eq!(
        listener.evolved_divergents[0].0,
        &[commit6.clone(), commit4.clone()]
    );
    let resolved = listener.evolved_divergents[0].1.clone();
    assert_eq!(resolved.predecessors(), &[commit6, commit4]);

    let tree = resolved.tree();
    let entries: Vec<_> = tree.entries().collect();
    assert_eq!(entries.len(), 4);
    assert_eq!(tree.value("A").unwrap(), tree5.value("A").unwrap());
    assert_eq!(tree.value("X").unwrap(), tree2.value("X").unwrap());
    assert_eq!(tree.value("Y").unwrap(), tree4.value("Y").unwrap());
    assert_eq!(tree.value("Z").unwrap(), tree6.value("Z").unwrap());

    tx.discard();
}
