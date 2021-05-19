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

use jujutsu_lib::repo::ReadonlyRepo;
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::testutils;
use test_case::test_case;

#[test]
fn test_init_local() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let wc_path = temp_dir.path().to_owned();
    let repo = ReadonlyRepo::init_local(&settings, wc_path.clone()).unwrap();
    assert!(repo.store().git_repo().is_none());
    assert_eq!(repo.working_copy_path(), &wc_path);
    assert_eq!(repo.repo_path(), &wc_path.join(".jj"));

    // Just test that we write a commit to the store
    let mut tx = repo.start_transaction("test");
    testutils::create_random_commit(&settings, &repo).write_to_repo(tx.mut_repo());
    tx.discard();
}

#[test]
fn test_init_internal_git() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let wc_path = temp_dir.path().to_owned();
    let repo = ReadonlyRepo::init_internal_git(&settings, wc_path.clone()).unwrap();
    assert!(repo.store().git_repo().is_some());
    assert_eq!(repo.working_copy_path(), &wc_path);
    assert_eq!(repo.repo_path(), &wc_path.join(".jj"));

    // Just test that we write a commit to the store
    let mut tx = repo.start_transaction("test");
    testutils::create_random_commit(&settings, &repo).write_to_repo(tx.mut_repo());
    tx.discard();
}

#[test]
fn test_init_external_git() {
    let settings = testutils::user_settings();
    let temp_dir = tempfile::tempdir().unwrap();
    let git_repo_path = temp_dir.path().join("git");
    git2::Repository::init(&git_repo_path).unwrap();
    let wc_path = temp_dir.path().join("jj");
    std::fs::create_dir(&wc_path).unwrap();
    let repo = ReadonlyRepo::init_external_git(&settings, wc_path.clone(), git_repo_path).unwrap();
    assert!(repo.store().git_repo().is_some());
    assert_eq!(repo.working_copy_path(), &wc_path);
    assert_eq!(repo.repo_path(), &wc_path.join(".jj"));

    // Just test that we write a commit to the store
    let mut tx = repo.start_transaction("test");
    testutils::create_random_commit(&settings, &repo).write_to_repo(tx.mut_repo());
    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_init_no_config_set(use_git: bool) {
    // Test that we can create a repo without setting any config
    let settings = UserSettings::from_config(config::Config::new());
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);
    let wc_commit = repo.working_copy_locked().current_commit();
    assert_eq!(wc_commit.author().name, "(no name configured)".to_string());
    assert_eq!(
        wc_commit.author().email,
        "(no email configured)".to_string()
    );
    assert_eq!(
        wc_commit.committer().name,
        "(no name configured)".to_string()
    );
    assert_eq!(
        wc_commit.committer().email,
        "(no email configured)".to_string()
    );
}
