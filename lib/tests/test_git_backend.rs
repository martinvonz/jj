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

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures::executor::block_on_stream;
use jj_lib::backend::{CommitId, CopyRecord};
use jj_lib::commit::Commit;
use jj_lib::git_backend::GitBackend;
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::repo_path::{RepoPath, RepoPathBuf};
use jj_lib::settings::UserSettings;
use jj_lib::store::Store;
use jj_lib::transaction::Transaction;
use maplit::hashset;
use testutils::{create_random_commit, create_tree, CommitGraphBuilder, TestRepo, TestRepoBackend};

fn get_git_backend(repo: &Arc<ReadonlyRepo>) -> &GitBackend {
    repo.store()
        .backend_impl()
        .downcast_ref::<GitBackend>()
        .unwrap()
}

fn collect_no_gc_refs(git_repo_path: &Path) -> HashSet<CommitId> {
    // Load fresh git repo to isolate from false caching issue. Here we want to
    // ensure that the underlying data is correct. We could test the in-memory
    // data as well, but we don't have any special handling in our code.
    let git_repo = gix::open(git_repo_path).unwrap();
    let git_refs = git_repo.references().unwrap();
    let no_gc_refs_iter = git_refs.prefixed("refs/jj/keep/").unwrap();
    no_gc_refs_iter
        .map(|git_ref| CommitId::from_bytes(git_ref.unwrap().id().as_bytes()))
        .collect()
}

fn get_copy_records(
    store: &Store,
    paths: Option<&[RepoPathBuf]>,
    a: &Commit,
    b: &Commit,
) -> HashMap<String, String> {
    let stream = store.get_copy_records(paths, a.id(), b.id()).unwrap();
    let mut res: HashMap<String, String> = HashMap::new();
    for CopyRecord { target, source, .. } in block_on_stream(stream).filter_map(|r| r.ok()) {
        res.insert(
            target.as_internal_file_string().into(),
            source.as_internal_file_string().into(),
        );
    }
    res
}

fn make_commit(
    tx: &mut Transaction,
    settings: &UserSettings,
    parents: Vec<CommitId>,
    content: &[(&RepoPath, &str)],
) -> Commit {
    let tree = create_tree(tx.base_repo(), content);
    tx.mut_repo()
        .new_commit(settings, parents, tree.id())
        .write()
        .unwrap()
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
    let git_repo_path = get_git_backend(&repo).git_repo_path();
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
        collect_no_gc_refs(git_repo_path),
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
        collect_no_gc_refs(git_repo_path),
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

    // Don't rely on the exact system time because file modification time might
    // have lower precision for example.
    let now = || SystemTime::now() + Duration::from_secs(1);

    // All reachable: redundant no-gc refs will be removed
    repo.store().gc(repo.index(), now()).unwrap();
    assert_eq!(
        collect_no_gc_refs(git_repo_path),
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
    repo.store().gc(mut_index.as_index(), now()).unwrap();
    assert_eq!(
        collect_no_gc_refs(git_repo_path),
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
    repo.store().gc(mut_index.as_index(), now()).unwrap();
    assert_eq!(
        collect_no_gc_refs(git_repo_path),
        hashset! {
            commit_c.id().clone(),
            commit_f.id().clone(),
        },
    );

    // B|C|F are no longer reachable
    let mut mut_index = base_index.start_modification();
    mut_index.add_commit(&commit_a);
    repo.store().gc(mut_index.as_index(), now()).unwrap();
    assert_eq!(
        collect_no_gc_refs(git_repo_path),
        hashset! {
            commit_a.id().clone(),
        },
    );

    // All unreachable
    repo.store().gc(base_index.as_index(), now()).unwrap();
    assert_eq!(collect_no_gc_refs(git_repo_path), hashset! {});
}

#[test]
fn test_copy_detection() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init_with_backend(TestRepoBackend::Git);
    let repo = &test_repo.repo;

    let paths = &[
        RepoPathBuf::from_internal_string("file0"),
        RepoPathBuf::from_internal_string("file1"),
        RepoPathBuf::from_internal_string("file2"),
    ];

    let mut tx = repo.start_transaction(&settings);
    let commit_a = make_commit(
        &mut tx,
        &settings,
        vec![repo.store().root_commit_id().clone()],
        &[(&paths[0], "content")],
    );
    let commit_b = make_commit(
        &mut tx,
        &settings,
        vec![commit_a.id().clone()],
        &[(&paths[1], "content")],
    );
    let commit_c = make_commit(
        &mut tx,
        &settings,
        vec![commit_b.id().clone()],
        &[(&paths[2], "content")],
    );

    let store = repo.store();
    assert_eq!(
        get_copy_records(store, Some(paths), &commit_a, &commit_b),
        HashMap::from([("file1".to_string(), "file0".to_string())])
    );
    assert_eq!(
        get_copy_records(store, Some(paths), &commit_b, &commit_c),
        HashMap::from([("file2".to_string(), "file1".to_string())])
    );
    assert_eq!(
        get_copy_records(store, Some(paths), &commit_a, &commit_c),
        HashMap::from([("file2".to_string(), "file0".to_string())])
    );
    assert_eq!(
        get_copy_records(store, None, &commit_a, &commit_c),
        HashMap::from([("file2".to_string(), "file0".to_string())])
    );
    assert_eq!(
        get_copy_records(store, Some(&[paths[1].clone()]), &commit_a, &commit_c),
        HashMap::default(),
    );
    assert_eq!(
        get_copy_records(store, Some(paths), &commit_c, &commit_c),
        HashMap::default(),
    );
}
