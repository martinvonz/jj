// Copyright 2023 The Jujutsu Authors
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

use std::path::Path;

use crate::common::TestEnvironment;

pub mod common;

fn create_commit(test_env: &TestEnvironment, repo_path: &Path, name: &str, parents: &[&str]) {
    if parents.is_empty() {
        test_env.jj_cmd_success(repo_path, &["new", "root", "-m", name]);
    } else {
        let mut args = vec!["new", "-m", name];
        args.extend(parents);
        test_env.jj_cmd_success(repo_path, &args);
    }
    std::fs::write(repo_path.join(name), format!("{name}\n")).unwrap();
    test_env.jj_cmd_success(repo_path, &["branch", "create", name]);
}

#[test]
fn test_duplicate() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &[]);
    create_commit(&test_env, &repo_path, "c", &["a", "b"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   17a00fc21654   c
    |\  
    o | d370aee184ba   b
    | o 2443ea76b0b1   a
    |/  
    o 000000000000   (no description set)
    "###);

    let stderr = test_env.jj_cmd_failure(&repo_path, &["duplicate", "root"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Cannot rewrite the root commit
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    {Commit { id: CommitId("2443ea76b0b1c531326908326aab7020abab8e6c") }}
    Duplicated 2443ea76b0b1 as 2f6dc5a1ffc2 a
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o 2f6dc5a1ffc2   a
    | @   17a00fc21654   c
    | |\  
    | o | d370aee184ba   b
    |/ /  
    | o 2443ea76b0b1   a
    |/  
    o 000000000000   (no description set)
    "###);

    test_env.jj_cmd_success(&repo_path, &["undo"]);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate" /* duplicates `c` */]);
    insta::assert_snapshot!(stdout, @r###"
    {Commit { id: CommitId("17a00fc216544096edd9e195f7af4962e0f93d2b") }}
    Duplicated 17a00fc21654 as 1dd099ea963c c
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   1dd099ea963c   c
    |\  
    | | o 2f6dc5a1ffc2   a
    | | | @   17a00fc21654   c
    | | | |\  
    | |_|/ /  
    |/| | |   
    | | |/    
    | |/|     
    o | | d370aee184ba   b
    | |/  
    |/|   
    | o 2443ea76b0b1   a
    |/  
    o 000000000000   (no description set)
    "###);
}

// https://github.com/martinvonz/jj/issues/694
#[test]
fn test_rebase_duplicates() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    @ 1394f625cbbd   b @ 2001-02-03 04:05:11.000 +07:00
    o 2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    o 000000000000   (no description set) @ 1970-01-01 00:00:00.000 +00:00
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    {Commit { id: CommitId("1394f625cbbddc4245af6505f4ef56b77dc27ba9") }}
    Duplicated 1394f625cbbd as fdaaf3950f07 b
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b"]);
    insta::assert_snapshot!(stdout, @r###"
    {Commit { id: CommitId("1394f625cbbddc4245af6505f4ef56b77dc27ba9") }}
    Duplicated 1394f625cbbd as 870cf438ccbb b
    "###);
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    o 870cf438ccbb   b @ 2001-02-03 04:05:14.000 +07:00
    | o fdaaf3950f07   b @ 2001-02-03 04:05:13.000 +07:00
    |/  
    | @ 1394f625cbbd   b @ 2001-02-03 04:05:11.000 +07:00
    |/  
    o 2443ea76b0b1   a @ 2001-02-03 04:05:09.000 +07:00
    o 000000000000   (no description set) @ 1970-01-01 00:00:00.000 +00:00
    "###);

    let stdout = test_env.jj_cmd_success(&repo_path, &["rebase", "-s", "a", "-d", "a-"]);
    insta::assert_snapshot!(stdout, @r###"
    Rebased 4 commits
    Working copy now at: 29bd36b60e60 b
    "###);
    // One of the duplicate commit's timestamps was changed a little to make it have
    // a different commit id from the other.
    insta::assert_snapshot!(get_log_output_with_ts(&test_env, &repo_path), @r###"
    o b43fe7354758   b @ 2001-02-03 04:05:14.000 +07:00
    | o 08beb14c3ead   b @ 2001-02-03 04:05:15.000 +07:00
    |/  
    | @ 29bd36b60e60   b @ 2001-02-03 04:05:16.000 +07:00
    |/  
    o 2f6dc5a1ffc2   a @ 2001-02-03 04:05:16.000 +07:00
    o 000000000000   (no description set) @ 1970-01-01 00:00:00.000 +00:00
    "###);
}

#[test]
fn test_duplicate_many() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "a", &[]);
    create_commit(&test_env, &repo_path, "b", &["a"]);
    create_commit(&test_env, &repo_path, "c", &["a"]);
    create_commit(&test_env, &repo_path, "d", &["c"]);
    create_commit(&test_env, &repo_path, "e", &["b", "d"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @   921dde6e55c0   e
    |\  
    o | ebd06dba20ec   d
    o | c0cb3a0b73e7   c
    | o 1394f625cbbd   b
    |/  
    o 2443ea76b0b1   a
    o 000000000000   (no description set)
    "###);

    // BUG! Copy of e shouldn't have b as a parent!
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b:"]);
    insta::assert_snapshot!(stdout, @r###"
    {Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("1394f625cbbddc4245af6505f4ef56b77dc27ba9") }}
    {Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }}
    Duplicated 1394f625cbbd as 3b74d9691015 b
    Duplicated 921dde6e55c0 as 8348ddcec733 e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   8348ddcec733   e
    |\  
    o | 3b74d9691015   b
    | | @   921dde6e55c0   e
    | | |\  
    | |/ /  
    | o | ebd06dba20ec   d
    | o | c0cb3a0b73e7   c
    |/ /  
    | o 1394f625cbbd   b
    |/  
    o 2443ea76b0b1   a
    o 000000000000   (no description set)
    "###);

    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log"]), @r###"
    @ ed679e396ff6 test-username@host.example.com 2001-02-03 04:05:19.000 +07:00 - 2001-02-03 04:05:19.000 +07:00
    | duplicating 2 commit(s)
    | args: jj duplicate b:
    o 3744137a8223 test-username@host.example.com 2001-02-03 04:05:17.000 +07:00 - 2001-02-03 04:05:17.000 +07:00
    | create branch e pointing to commit 921dde6e55c035fc748f34fd6422dd81587a4ad2
    | args: jj branch create e
    o 89fd0441d523 test-username@host.example.com 2001-02-03 04:05:17.000 +07:00 - 2001-02-03 04:05:17.000 +07:00
    | commit working copy
    o de545ab8b02e test-username@host.example.com 2001-02-03 04:05:16.000 +07:00 - 2001-02-03 04:05:16.000 +07:00
    | new empty commit
    | args: jj new -m e b d
    o f74e66a811c8 test-username@host.example.com 2001-02-03 04:05:15.000 +07:00 - 2001-02-03 04:05:15.000 +07:00
    | create branch d pointing to commit ebd06dba20ec6d52e6eb903fd33f78179d38d0fe
    | args: jj branch create d
    o 38ee3946a5d2 test-username@host.example.com 2001-02-03 04:05:15.000 +07:00 - 2001-02-03 04:05:15.000 +07:00
    | commit working copy
    o da637c5d3dd3 test-username@host.example.com 2001-02-03 04:05:14.000 +07:00 - 2001-02-03 04:05:14.000 +07:00
    | new empty commit
    | args: jj new -m d c
    o daf702c3bf56 test-username@host.example.com 2001-02-03 04:05:13.000 +07:00 - 2001-02-03 04:05:13.000 +07:00
    | create branch c pointing to commit c0cb3a0b73e72dfe9d3c871d59a17558f4c304e1
    | args: jj branch create c
    o 4d8268ae2ad4 test-username@host.example.com 2001-02-03 04:05:13.000 +07:00 - 2001-02-03 04:05:13.000 +07:00
    | commit working copy
    o 5c04532c1a5b test-username@host.example.com 2001-02-03 04:05:12.000 +07:00 - 2001-02-03 04:05:12.000 +07:00
    | new empty commit
    | args: jj new -m c a
    o 153fd636c0c9 test-username@host.example.com 2001-02-03 04:05:11.000 +07:00 - 2001-02-03 04:05:11.000 +07:00
    | create branch b pointing to commit 1394f625cbbddc4245af6505f4ef56b77dc27ba9
    | args: jj branch create b
    o 1def250aa177 test-username@host.example.com 2001-02-03 04:05:11.000 +07:00 - 2001-02-03 04:05:11.000 +07:00
    | commit working copy
    o 2e1648567398 test-username@host.example.com 2001-02-03 04:05:10.000 +07:00 - 2001-02-03 04:05:10.000 +07:00
    | new empty commit
    | args: jj new -m b a
    o 0923ccefb5cb test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    | create branch a pointing to commit 2443ea76b0b1c531326908326aab7020abab8e6c
    | args: jj branch create a
    o 04db325bf42c test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    | commit working copy
    o 682c21f25d34 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    | new empty commit
    | args: jj new root -m a
    o a99a3fd5c51e test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    | add workspace 'default'
    o 56b94dfc38e7 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
      initialize repo
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["undo"]), @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log"]), @r###"
    @ ed679e396ff6 test-username@host.example.com 2001-02-03 04:05:19.000 +07:00 - 2001-02-03 04:05:19.000 +07:00
    | duplicating 2 commit(s)
    | args: jj duplicate b:
    o 3744137a8223 test-username@host.example.com 2001-02-03 04:05:17.000 +07:00 - 2001-02-03 04:05:17.000 +07:00
    | create branch e pointing to commit 921dde6e55c035fc748f34fd6422dd81587a4ad2
    | args: jj branch create e
    o 89fd0441d523 test-username@host.example.com 2001-02-03 04:05:17.000 +07:00 - 2001-02-03 04:05:17.000 +07:00
    | commit working copy
    o de545ab8b02e test-username@host.example.com 2001-02-03 04:05:16.000 +07:00 - 2001-02-03 04:05:16.000 +07:00
    | new empty commit
    | args: jj new -m e b d
    o f74e66a811c8 test-username@host.example.com 2001-02-03 04:05:15.000 +07:00 - 2001-02-03 04:05:15.000 +07:00
    | create branch d pointing to commit ebd06dba20ec6d52e6eb903fd33f78179d38d0fe
    | args: jj branch create d
    o 38ee3946a5d2 test-username@host.example.com 2001-02-03 04:05:15.000 +07:00 - 2001-02-03 04:05:15.000 +07:00
    | commit working copy
    o da637c5d3dd3 test-username@host.example.com 2001-02-03 04:05:14.000 +07:00 - 2001-02-03 04:05:14.000 +07:00
    | new empty commit
    | args: jj new -m d c
    o daf702c3bf56 test-username@host.example.com 2001-02-03 04:05:13.000 +07:00 - 2001-02-03 04:05:13.000 +07:00
    | create branch c pointing to commit c0cb3a0b73e72dfe9d3c871d59a17558f4c304e1
    | args: jj branch create c
    o 4d8268ae2ad4 test-username@host.example.com 2001-02-03 04:05:13.000 +07:00 - 2001-02-03 04:05:13.000 +07:00
    | commit working copy
    o 5c04532c1a5b test-username@host.example.com 2001-02-03 04:05:12.000 +07:00 - 2001-02-03 04:05:12.000 +07:00
    | new empty commit
    | args: jj new -m c a
    o 153fd636c0c9 test-username@host.example.com 2001-02-03 04:05:11.000 +07:00 - 2001-02-03 04:05:11.000 +07:00
    | create branch b pointing to commit 1394f625cbbddc4245af6505f4ef56b77dc27ba9
    | args: jj branch create b
    o 1def250aa177 test-username@host.example.com 2001-02-03 04:05:11.000 +07:00 - 2001-02-03 04:05:11.000 +07:00
    | commit working copy
    o 2e1648567398 test-username@host.example.com 2001-02-03 04:05:10.000 +07:00 - 2001-02-03 04:05:10.000 +07:00
    | new empty commit
    | args: jj new -m b a
    o 0923ccefb5cb test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    | create branch a pointing to commit 2443ea76b0b1c531326908326aab7020abab8e6c
    | args: jj branch create a
    o 04db325bf42c test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    | commit working copy
    o 682c21f25d34 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    | new empty commit
    | args: jj new root -m a
    o a99a3fd5c51e test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    | add workspace 'default'
    o 56b94dfc38e7 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
      initialize repo
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "b:", "d"]);
    insta::assert_snapshot!(stdout, @r###"
    {Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("1394f625cbbddc4245af6505f4ef56b77dc27ba9") }, Commit { id: CommitId("ebd06dba20ec6d52e6eb903fd33f78179d38d0fe") }}
    {Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("ebd06dba20ec6d52e6eb903fd33f78179d38d0fe") }}
    {Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }}
    Duplicated 1394f625cbbd as 48a1420a9b84 b
    Duplicated ebd06dba20ec as 23e98e872621 d
    Duplicated 921dde6e55c0 as a6afd04bb7a6 e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   a6afd04bb7a6   e
    |\  
    o | 23e98e872621   d
    | o 48a1420a9b84   b
    | | o   8348ddcec733   e
    | | |\  
    | | o | 3b74d9691015   b
    | |/ /  
    | | | @   921dde6e55c0   e
    | | | |\  
    | | |/ /  
    | | o | ebd06dba20ec   d
    | |/ /  
    |/| |   
    o | | c0cb3a0b73e7   c
    |/ /  
    | o 1394f625cbbd   b
    |/  
    o 2443ea76b0b1   a
    o 000000000000   (no description set)
    "###);

    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log"]), @r###"
    @ 17f9f6b6e622 test-username@host.example.com 2001-02-03 04:05:24.000 +07:00 - 2001-02-03 04:05:24.000 +07:00
    | duplicating 3 commit(s)
    | args: jj duplicate b: d
    o ed679e396ff6 test-username@host.example.com 2001-02-03 04:05:19.000 +07:00 - 2001-02-03 04:05:19.000 +07:00
    | duplicating 2 commit(s)
    | args: jj duplicate b:
    o 3744137a8223 test-username@host.example.com 2001-02-03 04:05:17.000 +07:00 - 2001-02-03 04:05:17.000 +07:00
    | create branch e pointing to commit 921dde6e55c035fc748f34fd6422dd81587a4ad2
    | args: jj branch create e
    o 89fd0441d523 test-username@host.example.com 2001-02-03 04:05:17.000 +07:00 - 2001-02-03 04:05:17.000 +07:00
    | commit working copy
    o de545ab8b02e test-username@host.example.com 2001-02-03 04:05:16.000 +07:00 - 2001-02-03 04:05:16.000 +07:00
    | new empty commit
    | args: jj new -m e b d
    o f74e66a811c8 test-username@host.example.com 2001-02-03 04:05:15.000 +07:00 - 2001-02-03 04:05:15.000 +07:00
    | create branch d pointing to commit ebd06dba20ec6d52e6eb903fd33f78179d38d0fe
    | args: jj branch create d
    o 38ee3946a5d2 test-username@host.example.com 2001-02-03 04:05:15.000 +07:00 - 2001-02-03 04:05:15.000 +07:00
    | commit working copy
    o da637c5d3dd3 test-username@host.example.com 2001-02-03 04:05:14.000 +07:00 - 2001-02-03 04:05:14.000 +07:00
    | new empty commit
    | args: jj new -m d c
    o daf702c3bf56 test-username@host.example.com 2001-02-03 04:05:13.000 +07:00 - 2001-02-03 04:05:13.000 +07:00
    | create branch c pointing to commit c0cb3a0b73e72dfe9d3c871d59a17558f4c304e1
    | args: jj branch create c
    o 4d8268ae2ad4 test-username@host.example.com 2001-02-03 04:05:13.000 +07:00 - 2001-02-03 04:05:13.000 +07:00
    | commit working copy
    o 5c04532c1a5b test-username@host.example.com 2001-02-03 04:05:12.000 +07:00 - 2001-02-03 04:05:12.000 +07:00
    | new empty commit
    | args: jj new -m c a
    o 153fd636c0c9 test-username@host.example.com 2001-02-03 04:05:11.000 +07:00 - 2001-02-03 04:05:11.000 +07:00
    | create branch b pointing to commit 1394f625cbbddc4245af6505f4ef56b77dc27ba9
    | args: jj branch create b
    o 1def250aa177 test-username@host.example.com 2001-02-03 04:05:11.000 +07:00 - 2001-02-03 04:05:11.000 +07:00
    | commit working copy
    o 2e1648567398 test-username@host.example.com 2001-02-03 04:05:10.000 +07:00 - 2001-02-03 04:05:10.000 +07:00
    | new empty commit
    | args: jj new -m b a
    o 0923ccefb5cb test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    | create branch a pointing to commit 2443ea76b0b1c531326908326aab7020abab8e6c
    | args: jj branch create a
    o 04db325bf42c test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    | commit working copy
    o 682c21f25d34 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    | new empty commit
    | args: jj new root -m a
    o a99a3fd5c51e test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    | add workspace 'default'
    o 56b94dfc38e7 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
      initialize repo
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["undo"]), @r###"
    Nothing changed.
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["op", "log"]), @r###"
    @ 17f9f6b6e622 test-username@host.example.com 2001-02-03 04:05:24.000 +07:00 - 2001-02-03 04:05:24.000 +07:00
    | duplicating 3 commit(s)
    | args: jj duplicate b: d
    o ed679e396ff6 test-username@host.example.com 2001-02-03 04:05:19.000 +07:00 - 2001-02-03 04:05:19.000 +07:00
    | duplicating 2 commit(s)
    | args: jj duplicate b:
    o 3744137a8223 test-username@host.example.com 2001-02-03 04:05:17.000 +07:00 - 2001-02-03 04:05:17.000 +07:00
    | create branch e pointing to commit 921dde6e55c035fc748f34fd6422dd81587a4ad2
    | args: jj branch create e
    o 89fd0441d523 test-username@host.example.com 2001-02-03 04:05:17.000 +07:00 - 2001-02-03 04:05:17.000 +07:00
    | commit working copy
    o de545ab8b02e test-username@host.example.com 2001-02-03 04:05:16.000 +07:00 - 2001-02-03 04:05:16.000 +07:00
    | new empty commit
    | args: jj new -m e b d
    o f74e66a811c8 test-username@host.example.com 2001-02-03 04:05:15.000 +07:00 - 2001-02-03 04:05:15.000 +07:00
    | create branch d pointing to commit ebd06dba20ec6d52e6eb903fd33f78179d38d0fe
    | args: jj branch create d
    o 38ee3946a5d2 test-username@host.example.com 2001-02-03 04:05:15.000 +07:00 - 2001-02-03 04:05:15.000 +07:00
    | commit working copy
    o da637c5d3dd3 test-username@host.example.com 2001-02-03 04:05:14.000 +07:00 - 2001-02-03 04:05:14.000 +07:00
    | new empty commit
    | args: jj new -m d c
    o daf702c3bf56 test-username@host.example.com 2001-02-03 04:05:13.000 +07:00 - 2001-02-03 04:05:13.000 +07:00
    | create branch c pointing to commit c0cb3a0b73e72dfe9d3c871d59a17558f4c304e1
    | args: jj branch create c
    o 4d8268ae2ad4 test-username@host.example.com 2001-02-03 04:05:13.000 +07:00 - 2001-02-03 04:05:13.000 +07:00
    | commit working copy
    o 5c04532c1a5b test-username@host.example.com 2001-02-03 04:05:12.000 +07:00 - 2001-02-03 04:05:12.000 +07:00
    | new empty commit
    | args: jj new -m c a
    o 153fd636c0c9 test-username@host.example.com 2001-02-03 04:05:11.000 +07:00 - 2001-02-03 04:05:11.000 +07:00
    | create branch b pointing to commit 1394f625cbbddc4245af6505f4ef56b77dc27ba9
    | args: jj branch create b
    o 1def250aa177 test-username@host.example.com 2001-02-03 04:05:11.000 +07:00 - 2001-02-03 04:05:11.000 +07:00
    | commit working copy
    o 2e1648567398 test-username@host.example.com 2001-02-03 04:05:10.000 +07:00 - 2001-02-03 04:05:10.000 +07:00
    | new empty commit
    | args: jj new -m b a
    o 0923ccefb5cb test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    | create branch a pointing to commit 2443ea76b0b1c531326908326aab7020abab8e6c
    | args: jj branch create a
    o 04db325bf42c test-username@host.example.com 2001-02-03 04:05:09.000 +07:00 - 2001-02-03 04:05:09.000 +07:00
    | commit working copy
    o 682c21f25d34 test-username@host.example.com 2001-02-03 04:05:08.000 +07:00 - 2001-02-03 04:05:08.000 +07:00
    | new empty commit
    | args: jj new root -m a
    o a99a3fd5c51e test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
    | add workspace 'default'
    o 56b94dfc38e7 test-username@host.example.com 2001-02-03 04:05:07.000 +07:00 - 2001-02-03 04:05:07.000 +07:00
      initialize repo
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   a6afd04bb7a6   e
    |\  
    o | 23e98e872621   d
    | o 48a1420a9b84   b
    | | o   8348ddcec733   e
    | | |\  
    | | o | 3b74d9691015   b
    | |/ /  
    | | | @   921dde6e55c0   e
    | | | |\  
    | | |/ /  
    | | o | ebd06dba20ec   d
    | |/ /  
    |/| |   
    o | | c0cb3a0b73e7   c
    |/ /  
    | o 1394f625cbbd   b
    |/  
    o 2443ea76b0b1   a
    o 000000000000   (no description set)
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "d:", "a"]);
    insta::assert_snapshot!(stdout, @r###"
    {Commit { id: CommitId("8348ddcec73318e5134484f3254b13d7b650561f") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("ebd06dba20ec6d52e6eb903fd33f78179d38d0fe") }, Commit { id: CommitId("2443ea76b0b1c531326908326aab7020abab8e6c") }}
    {Commit { id: CommitId("8348ddcec73318e5134484f3254b13d7b650561f") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("2443ea76b0b1c531326908326aab7020abab8e6c") }}
    {Commit { id: CommitId("2443ea76b0b1c531326908326aab7020abab8e6c") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }}
    {Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }}
    Duplicated ebd06dba20ec as 9c5096adc8df d
    Duplicated 8348ddcec733 as 39803584a348 e
    Duplicated 2443ea76b0b1 as ebb6a2da924c a
    Duplicated 921dde6e55c0 as ddca8d0e4238 e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   ddca8d0e4238   e
    |\  
    | | o ebb6a2da924c   a
    | | | o   39803584a348   e
    | | | |\  
    | |_|/ /  
    |/| | |   
    o | | | 9c5096adc8df   d
    | | | | o   a6afd04bb7a6   e
    | | | | |\  
    | | | | o | 23e98e872621   d
    | |_|_|/ /  
    |/| | | |   
    | | | | o 48a1420a9b84   b
    | | | | | o   8348ddcec733   e
    | | | | | |\  
    | | | | |/ /  
    | | | |/| |   
    | | | o | | 3b74d9691015   b
    | | | |/ /  
    | | | | | @   921dde6e55c0   e
    | | | | | |\  
    | | | | |/ /  
    | | |_|_|/    
    | |/| | |     
    | | | | o ebd06dba20ec   d
    | |_|_|/  
    |/| | |   
    o | | | c0cb3a0b73e7   c
    | |_|/  
    |/| |   
    | o | 1394f625cbbd   b
    |/ /  
    o | 2443ea76b0b1   a
    |/  
    o 000000000000   (no description set)
    "###);

    // Check for BUG -- makes too many 'a'-s, etc.
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   ddca8d0e4238   e
    |\  
    | | o ebb6a2da924c   a
    | | | o   39803584a348   e
    | | | |\  
    | |_|/ /  
    |/| | |   
    o | | | 9c5096adc8df   d
    | | | | o   a6afd04bb7a6   e
    | | | | |\  
    | | | | o | 23e98e872621   d
    | |_|_|/ /  
    |/| | | |   
    | | | | o 48a1420a9b84   b
    | | | | | o   8348ddcec733   e
    | | | | | |\  
    | | | | |/ /  
    | | | |/| |   
    | | | o | | 3b74d9691015   b
    | | | |/ /  
    | | | | | @   921dde6e55c0   e
    | | | | | |\  
    | | | | |/ /  
    | | |_|_|/    
    | |/| | |     
    | | | | o ebd06dba20ec   d
    | |_|_|/  
    |/| | |   
    o | | | c0cb3a0b73e7   c
    | |_|/  
    |/| |   
    | o | 1394f625cbbd   b
    |/ /  
    o | 2443ea76b0b1   a
    |/  
    o 000000000000   (no description set)
    "###);
    let stdout = test_env.jj_cmd_success(&repo_path, &["duplicate", "a:"]);
    insta::assert_snapshot!(stdout, @r###"
    {Commit { id: CommitId("ddca8d0e4238412326d6fe5273593cc1797986c3") }, Commit { id: CommitId("39803584a3480fd6d775cb15fd95a320f6fc34ee") }, Commit { id: CommitId("9c5096adc8dfa57879effc64dbf3d810c3e9df29") }, Commit { id: CommitId("a6afd04bb7a65ff710d72b8b4945940279c94194") }, Commit { id: CommitId("23e98e8726212e4cdff205184f9459b08b189bfb") }, Commit { id: CommitId("48a1420a9b848a455ac340cc38902e0e789d65d4") }, Commit { id: CommitId("8348ddcec73318e5134484f3254b13d7b650561f") }, Commit { id: CommitId("3b74d9691015a88a7b6dde6d953b37bf90603da4") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("ebd06dba20ec6d52e6eb903fd33f78179d38d0fe") }, Commit { id: CommitId("c0cb3a0b73e72dfe9d3c871d59a17558f4c304e1") }, Commit { id: CommitId("1394f625cbbddc4245af6505f4ef56b77dc27ba9") }, Commit { id: CommitId("2443ea76b0b1c531326908326aab7020abab8e6c") }}
    {Commit { id: CommitId("ddca8d0e4238412326d6fe5273593cc1797986c3") }, Commit { id: CommitId("39803584a3480fd6d775cb15fd95a320f6fc34ee") }, Commit { id: CommitId("9c5096adc8dfa57879effc64dbf3d810c3e9df29") }, Commit { id: CommitId("a6afd04bb7a65ff710d72b8b4945940279c94194") }, Commit { id: CommitId("23e98e8726212e4cdff205184f9459b08b189bfb") }, Commit { id: CommitId("48a1420a9b848a455ac340cc38902e0e789d65d4") }, Commit { id: CommitId("8348ddcec73318e5134484f3254b13d7b650561f") }, Commit { id: CommitId("3b74d9691015a88a7b6dde6d953b37bf90603da4") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("ebd06dba20ec6d52e6eb903fd33f78179d38d0fe") }, Commit { id: CommitId("c0cb3a0b73e72dfe9d3c871d59a17558f4c304e1") }, Commit { id: CommitId("1394f625cbbddc4245af6505f4ef56b77dc27ba9") }}
    {Commit { id: CommitId("ddca8d0e4238412326d6fe5273593cc1797986c3") }, Commit { id: CommitId("39803584a3480fd6d775cb15fd95a320f6fc34ee") }, Commit { id: CommitId("9c5096adc8dfa57879effc64dbf3d810c3e9df29") }, Commit { id: CommitId("a6afd04bb7a65ff710d72b8b4945940279c94194") }, Commit { id: CommitId("23e98e8726212e4cdff205184f9459b08b189bfb") }, Commit { id: CommitId("1394f625cbbddc4245af6505f4ef56b77dc27ba9") }, Commit { id: CommitId("8348ddcec73318e5134484f3254b13d7b650561f") }, Commit { id: CommitId("3b74d9691015a88a7b6dde6d953b37bf90603da4") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("ebd06dba20ec6d52e6eb903fd33f78179d38d0fe") }, Commit { id: CommitId("c0cb3a0b73e72dfe9d3c871d59a17558f4c304e1") }}
    {Commit { id: CommitId("ddca8d0e4238412326d6fe5273593cc1797986c3") }, Commit { id: CommitId("39803584a3480fd6d775cb15fd95a320f6fc34ee") }, Commit { id: CommitId("9c5096adc8dfa57879effc64dbf3d810c3e9df29") }, Commit { id: CommitId("a6afd04bb7a65ff710d72b8b4945940279c94194") }, Commit { id: CommitId("23e98e8726212e4cdff205184f9459b08b189bfb") }, Commit { id: CommitId("c0cb3a0b73e72dfe9d3c871d59a17558f4c304e1") }, Commit { id: CommitId("8348ddcec73318e5134484f3254b13d7b650561f") }, Commit { id: CommitId("3b74d9691015a88a7b6dde6d953b37bf90603da4") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("ebd06dba20ec6d52e6eb903fd33f78179d38d0fe") }}
    {Commit { id: CommitId("ddca8d0e4238412326d6fe5273593cc1797986c3") }, Commit { id: CommitId("39803584a3480fd6d775cb15fd95a320f6fc34ee") }, Commit { id: CommitId("9c5096adc8dfa57879effc64dbf3d810c3e9df29") }, Commit { id: CommitId("a6afd04bb7a65ff710d72b8b4945940279c94194") }, Commit { id: CommitId("23e98e8726212e4cdff205184f9459b08b189bfb") }, Commit { id: CommitId("ebd06dba20ec6d52e6eb903fd33f78179d38d0fe") }, Commit { id: CommitId("8348ddcec73318e5134484f3254b13d7b650561f") }, Commit { id: CommitId("3b74d9691015a88a7b6dde6d953b37bf90603da4") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }}
    {Commit { id: CommitId("ddca8d0e4238412326d6fe5273593cc1797986c3") }, Commit { id: CommitId("39803584a3480fd6d775cb15fd95a320f6fc34ee") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("a6afd04bb7a65ff710d72b8b4945940279c94194") }, Commit { id: CommitId("23e98e8726212e4cdff205184f9459b08b189bfb") }, Commit { id: CommitId("ebd06dba20ec6d52e6eb903fd33f78179d38d0fe") }, Commit { id: CommitId("8348ddcec73318e5134484f3254b13d7b650561f") }, Commit { id: CommitId("3b74d9691015a88a7b6dde6d953b37bf90603da4") }}
    {Commit { id: CommitId("3b74d9691015a88a7b6dde6d953b37bf90603da4") }, Commit { id: CommitId("39803584a3480fd6d775cb15fd95a320f6fc34ee") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("a6afd04bb7a65ff710d72b8b4945940279c94194") }, Commit { id: CommitId("23e98e8726212e4cdff205184f9459b08b189bfb") }, Commit { id: CommitId("ebd06dba20ec6d52e6eb903fd33f78179d38d0fe") }, Commit { id: CommitId("8348ddcec73318e5134484f3254b13d7b650561f") }}
    {Commit { id: CommitId("8348ddcec73318e5134484f3254b13d7b650561f") }, Commit { id: CommitId("39803584a3480fd6d775cb15fd95a320f6fc34ee") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("a6afd04bb7a65ff710d72b8b4945940279c94194") }, Commit { id: CommitId("23e98e8726212e4cdff205184f9459b08b189bfb") }, Commit { id: CommitId("ebd06dba20ec6d52e6eb903fd33f78179d38d0fe") }}
    {Commit { id: CommitId("8348ddcec73318e5134484f3254b13d7b650561f") }, Commit { id: CommitId("ebd06dba20ec6d52e6eb903fd33f78179d38d0fe") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("a6afd04bb7a65ff710d72b8b4945940279c94194") }, Commit { id: CommitId("23e98e8726212e4cdff205184f9459b08b189bfb") }}
    {Commit { id: CommitId("8348ddcec73318e5134484f3254b13d7b650561f") }, Commit { id: CommitId("23e98e8726212e4cdff205184f9459b08b189bfb") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }, Commit { id: CommitId("a6afd04bb7a65ff710d72b8b4945940279c94194") }}
    {Commit { id: CommitId("a6afd04bb7a65ff710d72b8b4945940279c94194") }, Commit { id: CommitId("23e98e8726212e4cdff205184f9459b08b189bfb") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }}
    {Commit { id: CommitId("a6afd04bb7a65ff710d72b8b4945940279c94194") }, Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }}
    {Commit { id: CommitId("921dde6e55c035fc748f34fd6422dd81587a4ad2") }}
    Duplicated 2443ea76b0b1 as 818bc8141482 a
    Duplicated 48a1420a9b84 as d31c14797d72 b
    Duplicated 1394f625cbbd as 31cf6dd07141 b
    Duplicated c0cb3a0b73e7 as b39115a3d620 c
    Duplicated 9c5096adc8df as 7230a438db20 d
    Duplicated ddca8d0e4238 as 5a28494554c5 e
    Duplicated 3b74d9691015 as d0df45cf5144 b
    Duplicated 39803584a348 as ae308b018c52 e
    Duplicated ebd06dba20ec as b4ca0f0f5f5c d
    Duplicated 8348ddcec733 as a6217611e818 e
    Duplicated 23e98e872621 as 1db107c754b3 d
    Duplicated a6afd04bb7a6 as 2afbc89762c0 e
    Duplicated 921dde6e55c0 as 875fd6d690e2 e
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    o   875fd6d690e2   e
    |\  
    | | o   2afbc89762c0   e
    | | |\  
    | | o | 1db107c754b3   d
    | | | | o   a6217611e818   e
    | | | | |\  
    | |_|_|/ /  
    |/| | | |   
    o | | | | b4ca0f0f5f5c   d
    | |/ / /  
    |/| | |   
    | | | | o   ae308b018c52   e
    | | | | |\  
    | | | |/ /  
    | | | o | d0df45cf5144   b
    | | | | | o   5a28494554c5   e
    | | | | | |\  
    | | | | |/ /  
    | | |_|_|/    
    | |/| | |     
    | | | | o 7230a438db20   d
    | |_|_|/  
    |/| | |   
    o | | | b39115a3d620   c
    | |_|/  
    |/| |   
    | o | 31cf6dd07141   b
    |/ /  
    | o d31c14797d72   b
    |/  
    o 818bc8141482   a
    | o   ddca8d0e4238   e
    | |\  
    | | | o ebb6a2da924c   a
    | |_|/  
    |/| |   
    | | | o   39803584a348   e
    | | | |\  
    | | |/ /  
    | |/| |   
    | o | | 9c5096adc8df   d
    | | | | o   a6afd04bb7a6   e
    | | | | |\  
    | | | | o | 23e98e872621   d
    | | |_|/ /  
    | |/| | |   
    | | | | o 48a1420a9b84   b
    | | | | | o   8348ddcec733   e
    | | | | | |\  
    | | | | |/ /  
    | | | |/| |   
    | | | o | | 3b74d9691015   b
    | | | |/ /  
    | | | | | @   921dde6e55c0   e
    | | | | | |\  
    | | | | |/ /  
    | | | |_|/    
    | | |/| |     
    | | | | o ebd06dba20ec   d
    | | |_|/  
    | |/| |   
    | o | | c0cb3a0b73e7   c
    | | |/  
    | |/|   
    | | o 1394f625cbbd   b
    | |/  
    | o 2443ea76b0b1   a
    |/  
    o 000000000000   (no description set)
    "###);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &[
            "log",
            "-T",
            r#"commit_id.short() "   " description.first_line() if(divergent, " !divergence!")"#,
        ],
    )
}

fn get_log_output_with_ts(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(
        repo_path,
        &[
            "log",
            "-T",
            r#"commit_id.short() "   " description.first_line() " @ " committer.timestamp()"#,
        ],
    )
}
