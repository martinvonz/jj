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

use jj_lib::secret_backend::SecretBackend;

use crate::common::{get_stderr_string, get_stdout_string, TestEnvironment};

#[test]
fn test_diff() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::create_dir(repo_path.join("dir")).unwrap();
    std::fs::write(repo_path.join("a-first"), "foo\n").unwrap();
    std::fs::write(repo_path.join("deleted-secret"), "foo\n").unwrap();
    std::fs::write(repo_path.join("dir").join("secret"), "foo\n").unwrap();
    std::fs::write(repo_path.join("modified-secret"), "foo\n").unwrap();
    std::fs::write(repo_path.join("z-last"), "foo\n").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("a-first"), "bar\n").unwrap();
    std::fs::remove_file(repo_path.join("deleted-secret")).unwrap();
    std::fs::write(repo_path.join("added-secret"), "bar\n").unwrap();
    std::fs::write(repo_path.join("dir").join("secret"), "bar\n").unwrap();
    std::fs::write(repo_path.join("modified-secret"), "bar\n").unwrap();
    std::fs::write(repo_path.join("z-last"), "bar\n").unwrap();

    SecretBackend::adopt_git_repo(&repo_path);

    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--color-words"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r###"
    Modified regular file a-first:
       1    1: foobar
    Access denied to added-secret: No access
    Access denied to deleted-secret: No access
    Access denied to dir/secret: No access
    Access denied to modified-secret: No access
    Modified regular file z-last:
       1    1: foobar
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--summary"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r###"
    M a-first
    A added-secret
    D deleted-secret
    M dir/secret
    M modified-secret
    M z-last
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--types"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r###"
    FF a-first
    -F added-secret
    F- deleted-secret
    FF dir/secret
    FF modified-secret
    FF z-last
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["diff", "--stat"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r###"
    a-first         | 2 +-
    added-secret    | 1 +
    deleted-secret  | 1 -
    dir/secret      | 0
    modified-secret | 0
    z-last          | 2 +-
    6 files changed, 3 insertions(+), 3 deletions(-)
    "###);
    let assert = test_env
        .jj_cmd(&repo_path, &["diff", "--git"])
        .assert()
        .failure();
    insta::assert_snapshot!(get_stdout_string(&assert).replace('\\', "/"), @r###"
    diff --git a/a-first b/a-first
    index 257cc5642c..5716ca5987 100644
    --- a/a-first
    +++ b/a-first
    @@ -1,1 +1,1 @@
    -foo
    +bar
    "###);
    insta::assert_snapshot!(get_stderr_string(&assert), @r###"
    Error: Access denied to added-secret: No access
    Caused by: No access
    "###);

    // TODO: Test external tool
}

#[test]
fn test_cat() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("a-first"), "foo\n").unwrap();
    std::fs::write(repo_path.join("secret"), "bar\n").unwrap();
    std::fs::write(repo_path.join("z-last"), "baz\n").unwrap();

    SecretBackend::adopt_git_repo(&repo_path);

    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["file", "show", "."]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r###"
    foo
    baz
    "###);
    insta::assert_snapshot!(stderr.replace('\\', "/"), @r###"
    Warning: Path 'secret' exists but access is denied: No access
    "###);
}
