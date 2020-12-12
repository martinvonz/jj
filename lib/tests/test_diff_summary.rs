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

use jj_lib::repo_path::FileRepoPath;
use jj_lib::testutils;
use jj_lib::tree::DiffSummary;
use test_case::test_case;

#[test_case(false ; "local store")]
#[test_case(true ; "git store")]
fn test_types(use_git: bool) {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, use_git);

    let clean_path = FileRepoPath::from("clean");
    let modified_path = FileRepoPath::from("modified");
    let added_path = FileRepoPath::from("added");
    let removed_path = FileRepoPath::from("removed");

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

    let dir_file_path = FileRepoPath::from("dir/file");
    let dir_path = FileRepoPath::from("dir");

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

    let a_path = FileRepoPath::from("a");
    let b_path = FileRepoPath::from("b");
    let f_a_path = FileRepoPath::from("f/a");
    let f_b_path = FileRepoPath::from("f/b");
    let f_f_a_path = FileRepoPath::from("f/f/a");
    let f_f_b_path = FileRepoPath::from("f/f/b");
    let n_path = FileRepoPath::from("n");
    let s_b_path = FileRepoPath::from("s/b");
    let z_path = FileRepoPath::from("z");

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
                n_path.clone(),
                z_path.clone(),
                f_b_path.clone(),
                f_f_b_path.clone(),
                s_b_path.clone(),
            ],
            removed: vec![]
        }
    );
    assert_eq!(
        tree2.diff_summary(&tree1),
        DiffSummary {
            modified: vec![a_path, f_a_path, f_f_a_path],
            added: vec![],
            removed: vec![b_path, n_path, z_path, f_b_path, f_f_b_path, s_b_path,]
        }
    );
}
