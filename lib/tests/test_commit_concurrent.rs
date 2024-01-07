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

use std::cmp::max;
use std::sync::Arc;
use std::thread;

use jj_lib::dag_walk;
use jj_lib::repo::{ReadonlyRepo, Repo};
use test_case::test_case;
use testutils::{load_repo_at_head, write_random_commit, TestRepoBackend, TestWorkspace};

fn count_non_merge_operations(repo: &Arc<ReadonlyRepo>) -> usize {
    let op_store = repo.op_store();
    let op_id = repo.op_id().clone();
    let mut num_ops = 0;

    for op_id in dag_walk::dfs(
        vec![op_id],
        |op_id| op_id.clone(),
        |op_id| op_store.read_operation(op_id).unwrap().parents,
    ) {
        if op_store.read_operation(&op_id).unwrap().parents.len() <= 1 {
            num_ops += 1;
        }
    }
    num_ops
}

#[test_case(TestRepoBackend::Local ; "local backend")]
#[test_case(TestRepoBackend::Git ; "git backend")]
fn test_commit_parallel(backend: TestRepoBackend) {
    // This loads a Repo instance and creates and commits many concurrent
    // transactions from it. It then reloads the repo. That should merge all the
    // operations and all commits should be visible.
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init_with_backend(&settings, backend);
    let repo = &test_workspace.repo;

    let num_threads = max(num_cpus::get(), 4);
    thread::scope(|s| {
        for _ in 0..num_threads {
            let settings = settings.clone();
            let repo = repo.clone();
            s.spawn(move || {
                let mut tx = repo.start_transaction(&settings);
                write_random_commit(tx.mut_repo(), &settings);
                tx.commit("test");
            });
        }
    });
    let repo = repo.reload_at_head(&settings).unwrap();
    // One commit per thread plus the commit from the initial working-copy on top of
    // the root commit
    assert_eq!(repo.view().heads().len(), num_threads + 1);

    // One additional operation for the root commit, one for initializing the repo,
    // one for checking out the initial commit.
    assert_eq!(count_non_merge_operations(&repo), num_threads + 3);
}

#[test_case(TestRepoBackend::Local ; "local backend")]
#[test_case(TestRepoBackend::Git ; "git backend")]
fn test_commit_parallel_instances(backend: TestRepoBackend) {
    // Like the test above but creates a new repo instance for every thread, which
    // makes it behave very similar to separate processes.
    let settings = testutils::user_settings();
    let test_workspace = TestWorkspace::init_with_backend(&settings, backend);
    let repo = &test_workspace.repo;

    let num_threads = max(num_cpus::get(), 4);
    thread::scope(|s| {
        for _ in 0..num_threads {
            let settings = settings.clone();
            let repo = load_repo_at_head(&settings, repo.repo_path());
            s.spawn(move || {
                let mut tx = repo.start_transaction(&settings);
                write_random_commit(tx.mut_repo(), &settings);
                tx.commit("test");
            });
        }
    });
    // One commit per thread plus the commit from the initial working-copy commit on
    // top of the root commit
    let repo = load_repo_at_head(&settings, repo.repo_path());
    assert_eq!(repo.view().heads().len(), num_threads + 1);

    // One additional operation for the root commit, one for initializing the repo,
    // one for checking out the initial commit.
    assert_eq!(count_non_merge_operations(&repo), num_threads + 3);
}
