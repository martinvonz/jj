// Copyright 2024 The Jujutsu Authors
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

use std::collections::HashSet;
use std::process::Command;
use std::sync::Arc;
use std::time::SystemTime;

use jj_lib::backend::CommitId;
use jj_lib::git_backend::GitBackend;
use jj_lib::repo::{ReadonlyRepo, Repo};
use maplit::hashset;
use testutils::{create_random_commit, CommitGraphBuilder, TestRepo, TestRepoBackend};

fn get_git_backend(repo: &Arc<ReadonlyRepo>) -> &GitBackend {
    repo.store()
        .backend_impl()
        .downcast_ref::<GitBackend>()
        .unwrap()
}

fn get_git_repo(repo: &Arc<ReadonlyRepo>) -> gix::Repository {
    get_git_backend(repo).git_repo()
}

fn collect_no_gc_refs(git_repo: &gix::Repository) -> HashSet<CommitId> {
    let git_refs = git_repo.references().unwrap();
    let no_gc_refs_iter = git_refs.prefixed("refs/jj/keep/").unwrap();
    no_gc_refs_iter
        .map(|git_ref| CommitId::from_bytes(git_ref.unwrap().id().as_bytes()))
        .collect()
}

#[test]
fn test_gc() {
    // TODO: Better way to disable the test if git command couldn't be executed
    if Command::new("git").arg("--version").status().is_err() {
        eprintln!("Skipping because git command might fail to run");
        return;
    }

    let settings = testutils::user_settings();
    let test_repo = TestRepo::init_with_backend(TestRepoBackend::Git);
    let repo = test_repo.repo;
    let git_repo = get_git_repo(&repo);
    let base_index = repo.readonly_index();

    // Set up commits:
    //
    //     H (predecessor: D)
    //   G |
    //   |\|
    //   | F
    //   E |
    // D | |
    // C |/
    // |/
    // B
    // A
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_g = graph_builder.commit_with_parents(&[&commit_e, &commit_f]);
    let commit_h = create_random_commit(tx.mut_repo(), &settings)
        .set_parents(vec![commit_f.id().clone()])
        .set_predecessors(vec![commit_d.id().clone()])
        .write()
        .unwrap();
    let repo = tx.commit("test");
    assert_eq!(
        *repo.view().heads(),
        hashset! {
            commit_d.id().clone(),
            commit_g.id().clone(),
            commit_h.id().clone(),
        },
    );

    // At first, all commits have no-gc refs
    assert_eq!(
        collect_no_gc_refs(&git_repo),
        hashset! {
            commit_a.id().clone(),
            commit_b.id().clone(),
            commit_c.id().clone(),
            commit_d.id().clone(),
            commit_e.id().clone(),
            commit_f.id().clone(),
            commit_g.id().clone(),
            commit_h.id().clone(),
        },
    );

    // Empty index, but all kept by file modification time
    // (Beware that this invokes "git gc" and refs will be packed.)
    repo.store()
        .gc(base_index.as_index(), SystemTime::UNIX_EPOCH)
        .unwrap();
    assert_eq!(
        collect_no_gc_refs(&git_repo),
        hashset! {
            commit_a.id().clone(),
            commit_b.id().clone(),
            commit_c.id().clone(),
            commit_d.id().clone(),
            commit_e.id().clone(),
            commit_f.id().clone(),
            commit_g.id().clone(),
            commit_h.id().clone(),
        },
    );

    // All reachable: redundant no-gc refs will be removed
    let now = SystemTime::now();
    repo.store().gc(repo.index(), now).unwrap();
    assert_eq!(
        collect_no_gc_refs(&git_repo),
        hashset! {
            commit_d.id().clone(),
            commit_g.id().clone(),
            commit_h.id().clone(),
        },
    );

    // G is no longer reachable
    let mut mut_index = base_index.start_modification();
    mut_index.add_commit(&commit_a);
    mut_index.add_commit(&commit_b);
    mut_index.add_commit(&commit_c);
    mut_index.add_commit(&commit_d);
    mut_index.add_commit(&commit_e);
    mut_index.add_commit(&commit_f);
    mut_index.add_commit(&commit_h);
    repo.store().gc(mut_index.as_index(), now).unwrap();
    assert_eq!(
        collect_no_gc_refs(&git_repo),
        hashset! {
            commit_d.id().clone(),
            commit_e.id().clone(),
            commit_h.id().clone(),
        },
    );

    // D|E|H are no longer reachable
    let mut mut_index = base_index.start_modification();
    mut_index.add_commit(&commit_a);
    mut_index.add_commit(&commit_b);
    mut_index.add_commit(&commit_c);
    mut_index.add_commit(&commit_f);
    repo.store().gc(mut_index.as_index(), now).unwrap();
    assert_eq!(
        collect_no_gc_refs(&git_repo),
        hashset! {
            commit_c.id().clone(),
            commit_f.id().clone(),
        },
    );

    // B|C|F are no longer reachable
    let mut mut_index = base_index.start_modification();
    mut_index.add_commit(&commit_a);
    repo.store().gc(mut_index.as_index(), now).unwrap();
    assert_eq!(
        collect_no_gc_refs(&git_repo),
        hashset! {
            commit_a.id().clone(),
        },
    );

    // All unreachable
    repo.store().gc(base_index.as_index(), now).unwrap();
    assert_eq!(collect_no_gc_refs(&git_repo), hashset! {});
}
