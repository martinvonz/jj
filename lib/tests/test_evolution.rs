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

#![feature(assert_matches)]

use jujutsu_lib::commit::Commit;
use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::repo::ReadonlyRepo;
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::testutils;
use test_case::test_case;

#[must_use]
fn child_commit(settings: &UserSettings, repo: &ReadonlyRepo, commit: &Commit) -> CommitBuilder {
    testutils::create_random_commit(settings, repo).set_parents(vec![commit.id().clone()])
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
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
    mut_repo.remove_head(rewritten.id());
    assert!(!mut_repo.evolution().is_obsolete(original.id()));
    assert!(!mut_repo.evolution().is_orphan(child.id()));
    assert!(!mut_repo.evolution().is_orphan(grandchild.id()));
    tx.discard();
}

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
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

#[test_case(false ; "local backend")]
#[test_case(true ; "git backend")]
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
