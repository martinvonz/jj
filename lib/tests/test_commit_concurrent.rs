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

use std::sync::Arc;
use std::thread;

use jujube_lib::repo::ReadonlyRepo;
use jujube_lib::{dag_walk, testutils};
use test_case::test_case;

fn count_non_merge_operations(repo: &ReadonlyRepo) -> u32 {
    let op_store = repo.op_store();
    let op_id = repo.op_id().clone();
    let mut num_ops = 0;

    for op_id in dag_walk::bfs(
        vec![op_id],
        Box::new(|op_id| op_id.clone()),
        Box::new(|op_id| op_store.read_operation(&op_id).unwrap().parents),
    ) {
        if op_store.read_operation(&op_id).unwrap().parents.len() <= 1 {
            num_ops += 1;
        }
    }
    num_ops
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_commit_parallel(use_git: bool) {
    // This loads a Repo instance and creates and commits many concurrent
    // transactions from it. It then reloads the repo. That should merge all the
    // operations and all commits should be visible.
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut threads = vec![];
    for _ in 0..100 {
        let settings = settings.clone();
        let repo = repo.clone();
        let handle = thread::spawn(move || {
            testutils::create_random_commit(&settings, &repo)
                .write_to_new_transaction(&repo, "test");
        });
        threads.push(handle);
    }
    for thread in threads {
        thread.join().ok().unwrap();
    }
    Arc::get_mut(&mut repo).unwrap().reload();
    // One commit per thread plus the commit from the initial checkout on top of the
    // root commit
    assert_eq!(repo.view().heads().len(), 101);

    // One operation for initializing the repo (containing the root id and the
    // initial working copy commit).
    assert_eq!(count_non_merge_operations(&repo), 101);
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_commit_parallel_instances(use_git: bool) {
    // Like the test above but creates a new repo instance for every thread, which
    // makes it behave very similar to separate processes.
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut threads = vec![];
    for _ in 0..100 {
        let settings = settings.clone();
        let repo = ReadonlyRepo::load(&settings, repo.working_copy_path().clone()).unwrap();
        let handle = thread::spawn(move || {
            testutils::create_random_commit(&settings, &repo)
                .write_to_new_transaction(&repo, "test");
        });
        threads.push(handle);
    }
    for thread in threads {
        thread.join().ok().unwrap();
    }
    // One commit per thread plus the commit from the initial checkout on top of the
    // root commit
    let repo = ReadonlyRepo::load(&settings, repo.working_copy_path().clone()).unwrap();
    assert_eq!(repo.view().heads().len(), 101);

    // One operation for initializing the repo (containing the root id and the
    // initial working copy commit).
    assert_eq!(count_non_merge_operations(&repo), 101);
}
