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

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use crate::common::TestEnvironment;

fn append_to_file(file_path: &Path, contents: &str) {
    let mut options = OpenOptions::new();
    options.append(true);
    let mut file = options.open(file_path).unwrap();
    writeln!(file, "{contents}").unwrap();
}

#[test]
fn test_annotate_linear() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file.txt"), "line1\n").unwrap();
    test_env.jj_cmd_ok(
        &repo_path,
        &["describe", "-m=initial", "--author=Foo <foo@example.org>"],
    );

    test_env.jj_cmd_ok(&repo_path, &["new", "-m=next"]);
    append_to_file(&repo_path.join("file.txt"), "new text from new commit");

    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "annotate", "file.txt"]);
    insta::assert_snapshot!(stdout, @r"
    qpvuntsm foo      2001-02-03 08:05:08    1: line1
    kkmpptxz test.use 2001-02-03 08:05:10    2: new text from new commit
    ");
}

#[test]
fn test_annotate_merge() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file.txt"), "line1\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=initial"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "initial"]);

    test_env.jj_cmd_ok(&repo_path, &["new", "-m=commit1"]);
    append_to_file(&repo_path.join("file.txt"), "new text from new commit 1");
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "commit1"]);

    test_env.jj_cmd_ok(&repo_path, &["new", "-m=commit2", "initial"]);
    append_to_file(&repo_path.join("file.txt"), "new text from new commit 2");
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "commit2"]);

    // create a (conflicted) merge
    test_env.jj_cmd_ok(&repo_path, &["new", "-m=merged", "commit1", "commit2"]);
    // resolve conflicts
    std::fs::write(
        repo_path.join("file.txt"),
        "line1\nnew text from new commit 1\nnew text from new commit 2\n",
    )
    .unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "annotate", "file.txt"]);
    insta::assert_snapshot!(stdout, @r"
    qpvuntsm test.use 2001-02-03 08:05:08    1: line1
    zsuskuln test.use 2001-02-03 08:05:11    2: new text from new commit 1
    royxmykx test.use 2001-02-03 08:05:13    3: new text from new commit 2
    ");
}

#[test]
fn test_annotate_conflicted() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file.txt"), "line1\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=initial"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "initial"]);

    test_env.jj_cmd_ok(&repo_path, &["new", "-m=commit1"]);
    append_to_file(&repo_path.join("file.txt"), "new text from new commit 1");
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "commit1"]);

    test_env.jj_cmd_ok(&repo_path, &["new", "-m=commit2", "initial"]);
    append_to_file(&repo_path.join("file.txt"), "new text from new commit 2");
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "commit2"]);

    // create a (conflicted) merge
    test_env.jj_cmd_ok(&repo_path, &["new", "-m=merged", "commit1", "commit2"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);

    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "annotate", "file.txt"]);
    insta::assert_snapshot!(stdout, @r"
    qpvuntsm test.use 2001-02-03 08:05:08    1: line1
    yostqsxw test.use 2001-02-03 08:05:15    2: <<<<<<< Conflict 1 of 1
    yostqsxw test.use 2001-02-03 08:05:15    3: %%%%%%% Changes from base to side #1
    yostqsxw test.use 2001-02-03 08:05:15    4: +new text from new commit 1
    yostqsxw test.use 2001-02-03 08:05:15    5: +++++++ Contents of side #2
    royxmykx test.use 2001-02-03 08:05:13    6: new text from new commit 2
    yostqsxw test.use 2001-02-03 08:05:15    7: >>>>>>> Conflict 1 of 1 ends
    ");
}

#[test]
fn test_annotate_merge_one_sided_conflict_resolution() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file.txt"), "line1\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["describe", "-m=initial"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "initial"]);

    test_env.jj_cmd_ok(&repo_path, &["new", "-m=commit1"]);
    append_to_file(&repo_path.join("file.txt"), "new text from new commit 1");
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "commit1"]);

    test_env.jj_cmd_ok(&repo_path, &["new", "-m=commit2", "initial"]);
    append_to_file(&repo_path.join("file.txt"), "new text from new commit 2");
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "commit2"]);

    // create a (conflicted) merge
    test_env.jj_cmd_ok(&repo_path, &["new", "-m=merged", "commit1", "commit2"]);
    // resolve conflicts
    std::fs::write(
        repo_path.join("file.txt"),
        "line1\nnew text from new commit 1\n",
    )
    .unwrap();

    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "annotate", "file.txt"]);
    insta::assert_snapshot!(stdout, @r"
    qpvuntsm test.use 2001-02-03 08:05:08    1: line1
    zsuskuln test.use 2001-02-03 08:05:11    2: new text from new commit 1
    ");
}
