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

use jujube_lib::commit_builder::CommitBuilder;
use jujube_lib::revset::{resolve_symbol, RevsetError};
use jujube_lib::store::{MillisSinceEpoch, Signature, Timestamp};
use jujube_lib::testutils;
use test_case::test_case;

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_resolve_symbol_root(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    assert_eq!(
        resolve_symbol(repo.as_repo_ref(), "root").unwrap(),
        repo.store().root_commit()
    );
}

#[test]
fn test_resolve_symbol_commit_id() {
    let settings = testutils::user_settings();
    // Test only with git so we can get predictable commit ids
    let (_temp_dir, repo) = testutils::init_repo(&settings, true);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();
    let signature = Signature {
        name: "test".to_string(),
        email: "test".to_string(),
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(0),
            tz_offset: 0,
        },
    };

    let mut commits = vec![];
    for i in &[1, 167, 895] {
        let commit = CommitBuilder::for_new_commit(
            &settings,
            repo.store(),
            repo.store().empty_tree_id().clone(),
        )
        .set_description(format!("test {}", i))
        .set_author(signature.clone())
        .set_committer(signature.clone())
        .write_to_repo(mut_repo);
        commits.push(commit);
    }

    // Test the test setup
    assert_eq!(
        commits[0].id().hex(),
        "0454de3cae04c46cda37ba2e8873b4c17ff51dcb"
    );
    assert_eq!(
        commits[1].id().hex(),
        "045f56cd1b17e8abde86771e2705395dcde6a957"
    );
    assert_eq!(
        commits[2].id().hex(),
        "0468f7da8de2ce442f512aacf83411d26cd2e0cf"
    );

    // Test lookup by full commit id
    let repo_ref = mut_repo.as_repo_ref();
    assert_eq!(
        resolve_symbol(repo_ref, "0454de3cae04c46cda37ba2e8873b4c17ff51dcb").unwrap(),
        commits[0]
    );
    assert_eq!(
        resolve_symbol(repo_ref, "045f56cd1b17e8abde86771e2705395dcde6a957").unwrap(),
        commits[1]
    );
    assert_eq!(
        resolve_symbol(repo_ref, "0468f7da8de2ce442f512aacf83411d26cd2e0cf").unwrap(),
        commits[2]
    );

    // Test commit id prefix
    assert_eq!(resolve_symbol(repo_ref, "046").unwrap(), commits[2]);
    assert_eq!(
        resolve_symbol(repo_ref, "04"),
        Err(RevsetError::AmbiguousCommitIdPrefix("04".to_string()))
    );
    assert_eq!(
        resolve_symbol(repo_ref, ""),
        Err(RevsetError::AmbiguousCommitIdPrefix("".to_string()))
    );
    assert_eq!(
        resolve_symbol(repo_ref, "040"),
        Err(RevsetError::NoSuchRevision("040".to_string()))
    );

    tx.discard();
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_resolve_symbol_checkout(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    let commit1 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);
    let commit2 = testutils::create_random_commit(&settings, &repo).write_to_repo(mut_repo);

    mut_repo.set_checkout(commit1.id().clone());
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "@").unwrap(),
        commit1
    );
    mut_repo.set_checkout(commit2.id().clone());
    assert_eq!(
        resolve_symbol(mut_repo.as_repo_ref(), "@").unwrap(),
        commit2
    );

    tx.discard();
}
