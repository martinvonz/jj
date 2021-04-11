// Copyright 2021 Google LLC
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

use jujube_lib::repo::{ReadonlyRepo, RepoLoadError, RepoLoader};
use jujube_lib::testutils;
use test_case::test_case;

#[test]
fn test_load_bad_path() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let wc_path = temp_dir.path().to_owned();
    // We haven't created a repo in the wc_path, so it should fail to load.
    let result = ReadonlyRepo::load(&settings, wc_path.clone());
    assert_eq!(result.err(), Some(RepoLoadError::NoRepoHere(wc_path)));
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_load_at_operation(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, mut repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("add commit");
    let commit = testutils::create_random_commit(&settings, &repo).write_to_repo(tx.mut_repo());
    let op = tx.commit();
    repo = repo.reload().unwrap();

    let mut tx = repo.start_transaction("remove commit");
    tx.mut_repo().remove_head(&commit);
    tx.commit();

    // If we load the repo at head, we should not see the commit since it was
    // removed
    let loader = RepoLoader::init(&settings, repo.working_copy_path().clone()).unwrap();
    let head_repo = loader.load_at_head().unwrap();
    assert!(!head_repo.view().heads().contains(commit.id()));

    // If we load the repo at the previous operation, we should see the commit since
    // it has not been removed yet
    let loader = RepoLoader::init(&settings, repo.working_copy_path().clone()).unwrap();
    let old_repo = loader.load_at(&op).unwrap();
    assert!(old_repo.view().heads().contains(commit.id()));
}
