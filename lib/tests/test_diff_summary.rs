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

use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::testutils;
use jujutsu_lib::tree::DiffSummary;
use test_case::test_case;

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_types(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let clean_path = RepoPath::from_internal_string("clean");
    let modified_path = RepoPath::from_internal_string("modified");
    let added_path = RepoPath::from_internal_string("added");
    let removed_path = RepoPath::from_internal_string("removed");

    let tree1 = testutils::create_tree(
        &repo,
        &[
            (&clean_path, "clean"),
            (&modified_path, "contents before"),
            (&removed_path, "removed contents"),
        ],
    );

    let tree2 = testutils::create_tree(
        &repo,
        &[
            (&clean_path, "clean"),
            (&modified_path, "contents after"),
            (&added_path, "added contents"),
        ],
    );

    assert_eq!(
        tree1.diff_summary(&tree2),
        DiffSummary {
            modified: vec![modified_path],
            added: vec![added_path],
            removed: vec![removed_path]
        }
    );
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_tree_file_transition(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let dir_file_path = RepoPath::from_internal_string("dir/file");
    let dir_path = RepoPath::from_internal_string("dir");

    let tree1 = testutils::create_tree(&repo, &[(&dir_file_path, "contents")]);
    let tree2 = testutils::create_tree(&repo, &[(&dir_path, "contents")]);

    assert_eq!(
        tree1.diff_summary(&tree2),
        DiffSummary {
            modified: vec![],
            added: vec![dir_path.clone()],
            removed: vec![dir_file_path.clone()]
        }
    );
    assert_eq!(
        tree2.diff_summary(&tree1),
        DiffSummary {
            modified: vec![],
            added: vec![dir_file_path],
            removed: vec![dir_path]
        }
    );
}

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_sorting(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let a_path = RepoPath::from_internal_string("a");
    let b_path = RepoPath::from_internal_string("b");
    let f_a_path = RepoPath::from_internal_string("f/a");
    let f_b_path = RepoPath::from_internal_string("f/b");
    let f_f_a_path = RepoPath::from_internal_string("f/f/a");
    let f_f_b_path = RepoPath::from_internal_string("f/f/b");
    let n_path = RepoPath::from_internal_string("n");
    let s_b_path = RepoPath::from_internal_string("s/b");
    let z_path = RepoPath::from_internal_string("z");

    let tree1 = testutils::create_tree(
        &repo,
        &[
            (&a_path, "before"),
            (&f_a_path, "before"),
            (&f_f_a_path, "before"),
        ],
    );

    let tree2 = testutils::create_tree(
        &repo,
        &[
            (&a_path, "after"),
            (&b_path, "after"),
            (&f_a_path, "after"),
            (&f_b_path, "after"),
            (&f_f_a_path, "after"),
            (&f_f_b_path, "after"),
            (&n_path, "after"),
            (&s_b_path, "after"),
            (&z_path, "after"),
        ],
    );

    assert_eq!(
        tree1.diff_summary(&tree2),
        DiffSummary {
            modified: vec![a_path.clone(), f_a_path.clone(), f_f_a_path.clone()],
            added: vec![
                b_path.clone(),
                f_b_path.clone(),
                f_f_b_path.clone(),
                n_path.clone(),
                s_b_path.clone(),
                z_path.clone(),
            ],
            removed: vec![]
        }
    );
    assert_eq!(
        tree2.diff_summary(&tree1),
        DiffSummary {
            modified: vec![a_path, f_a_path, f_f_a_path],
            added: vec![],
            removed: vec![b_path, f_b_path, f_f_b_path, n_path, s_b_path, z_path]
        }
    );
}
