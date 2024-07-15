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

use std::collections::HashMap;

use futures::executor::block_on_stream;
use jj_lib::backend::{Backend, CommitId, CopySource, CopySources};
use jj_lib::commit::Commit;
use jj_lib::git_backend::GitBackend;
use jj_lib::repo::Repo;
use jj_lib::repo_path::{RepoPath, RepoPathBuf};
use jj_lib::settings::UserSettings;
use jj_lib::transaction::Transaction;
use testutils::{create_tree, TestRepo, TestRepoBackend};

fn get_copy_records(
    backend: &GitBackend,
    paths: &[RepoPathBuf],
    a: &Commit,
    b: &Commit,
) -> HashMap<String, Vec<String>> {
    let stream = backend
        .get_copy_records(paths, &[a.id().clone()], &[b.id().clone()])
        .unwrap();
    let mut res: HashMap<String, Vec<String>> = HashMap::new();
    for copy_record in block_on_stream(stream).filter_map(|r| r.ok()) {
        res.insert(
            copy_record.target.as_internal_file_string().into(),
            match copy_record.sources {
                CopySources::Resolved(CopySource { path, .. }) => {
                    vec![path.as_internal_file_string().into()]
                }
                CopySources::Conflict(conflicting) => conflicting
                    .iter()
                    .map(|s| s.path.as_internal_file_string().into())
                    .collect(),
            },
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
fn test_git_detection() {
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

    let backend: &GitBackend = repo.store().backend_impl().downcast_ref().unwrap();
    assert_eq!(
        get_copy_records(backend, paths, &commit_a, &commit_b),
        HashMap::from([("file1".to_string(), vec!["file0".to_string()])])
    );
    assert_eq!(
        get_copy_records(backend, paths, &commit_b, &commit_c),
        HashMap::from([("file2".to_string(), vec!["file1".to_string()])])
    );
    assert_eq!(
        get_copy_records(backend, paths, &commit_a, &commit_c),
        HashMap::from([("file2".to_string(), vec!["file0".to_string()])])
    );
    assert_eq!(
        get_copy_records(backend, &[], &commit_a, &commit_c),
        HashMap::default(),
    );
    assert_eq!(
        get_copy_records(backend, paths, &commit_c, &commit_c),
        HashMap::default(),
    );
}
