// Copyright 2022 The Jujutsu Authors
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

fn create_commit(
    test_env: &TestEnvironment,
    repo_path: &Path,
    name: &str,
    parents: &[&str],
    files: &[(&str, &str)],
) {
    if parents.is_empty() {
        test_env.jj_cmd_success(repo_path, &["new", "root", "-m", name]);
    } else {
        let mut args = vec!["new", "-m", name];
        args.extend(parents);
        test_env.jj_cmd_success(repo_path, &args);
    }
    for (name, content) in files {
        std::fs::write(repo_path.join(name), content).unwrap();
    }
    test_env.jj_cmd_success(repo_path, &["branch", "create", name]);
}

fn get_log_output(test_env: &TestEnvironment, repo_path: &Path) -> String {
    test_env.jj_cmd_success(repo_path, &["log", "-T", "branches"])
}

#[test]
fn test_resolution() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "base", &[], &[("file", "base\n")]);
    create_commit(&test_env, &repo_path, "a", &["base"], &[("file", "a\n")]);
    create_commit(&test_env, &repo_path, "b", &["base"], &[("file", "b\n")]);
    create_commit(&test_env, &repo_path, "conflict", &["a", "b"], &[]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
        @   conflict
        |\  
        o | b
        | o a
        |/  
        o base
        o 
        "###);
    insta::assert_snapshot!(
    std::fs::read_to_string(repo_path.join("file")).unwrap()
        , @r###"
            <<<<<<<
            %%%%%%%
            -base
            +a
            +++++++
            b
            >>>>>>>
            "###);

    let editor_script = test_env.set_up_fake_editor();
    // Check that output file starts out empty and resolve the conflict
    std::fs::write(&editor_script, "expect\n\0write\nresolution\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["resolve", "file"]);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff", "-r", "conflict"]), 
    @r###"
    Resolved conflict in file:
       1    1: <<<<<<<resolution
       2     : %%%%%%%
       3     : -base
       4     : +a
       5     : +++++++
       6     : b
       7     : >>>>>>>
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["status"]), 
    @r###"
    Parent commit: 77c5ed9eda54 a
    Parent commit: 7c4a3488ba53 b
    Working copy : 665f83829a6a conflict
    Working copy changes:
    M file
    "###);

    // Check that the output file starts with conflict markers if
    // `merge-tool-edits-conflict-markers=true`
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff", "-r", "conflict"]), 
    @"");
    std::fs::write(
        &editor_script,
        "expect
<<<<<<<
%%%%%%%
-base
+a
+++++++
b
>>>>>>>
\0write
resolution
",
    )
    .unwrap();
    test_env.jj_cmd_success(
        &repo_path,
        &[
            "resolve",
            "--config-toml",
            "merge-tools.fake-editor.merge-tool-edits-conflict-markers=true",
            "file",
        ],
    );
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff", "-r", "conflict"]), 
    @r###"
    Resolved conflict in file:
       1    1: <<<<<<<resolution
       2     : %%%%%%%
       3     : -base
       4     : +a
       5     : +++++++
       6     : b
       7     : >>>>>>>
    "###);

    // Check that if merge tool leaves conflict markers in output file and
    // `merge-tool-edits-conflict-markers=true`, these markers are properly parsed.
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff", "-r", "conflict"]), 
    @"");
    std::fs::write(
        &editor_script,
        "expect
<<<<<<<
%%%%%%%
-base
+a
+++++++
b
>>>>>>>
\0write
<<<<<<<
%%%%%%%
-some
+fake
+++++++
conflict
>>>>>>>
",
    )
    .unwrap();
    test_env.jj_cmd_success(
        &repo_path,
        &[
            "resolve",
            "--config-toml",
            "merge-tools.fake-editor.merge-tool-edits-conflict-markers=true",
            "file",
        ],
    );
    // Note the "Modified" below
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff", "-r", "conflict"]), 
    @r###"
    Modified conflict in file:
       1    1: <<<<<<<
       2    2: %%%%%%%
       3    3: -basesome
       4    4: +afake
       5    5: +++++++
       6    6: bconflict
       7    7: >>>>>>>
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["status"]), 
    @r###"
    Parent commit: 77c5ed9eda54 a
    Parent commit: 7c4a3488ba53 b
    Working copy : cbd3d65d2612 conflict
    Working copy changes:
    M file
    There are unresolved conflicts at these paths:
    file
    "###);

    // Check that if merge tool leaves conflict markers in output file but
    // `merge-tool-edits-conflict-markers=false` or is not specified,
    // `jj` considers the conflict resolved.
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff", "-r", "conflict"]), 
    @"");
    std::fs::write(
        &editor_script,
        "expect
\0write
<<<<<<<
%%%%%%%
-some
+fake
+++++++
conflict
>>>>>>>
",
    )
    .unwrap();
    test_env.jj_cmd_success(&repo_path, &["resolve", "file"]);
    // Note the "Resolved" below
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff", "-r", "conflict"]), 
    @r###"
    Resolved conflict in file:
       1    1: <<<<<<<
       2    2: %%%%%%%
       3    3: -basesome
       4    4: +afake
       5    5: +++++++
       6    6: bconflict
       7    7: >>>>>>>
    "###);
    // No "there are unresolved conflicts..." message below
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["status"]), 
    @r###"
    Parent commit: 77c5ed9eda54 a
    Parent commit: 7c4a3488ba53 b
    Working copy : d5b735448648 conflict
    Working copy changes:
    M file
    "###);
}

fn check_resolve_produces_input_file(
    test_env: &mut TestEnvironment,
    repo_path: &Path,
    role: &str,
    expected_content: &str,
) {
    let editor_script = test_env.set_up_fake_editor();
    std::fs::write(editor_script, format!("expect\n{expected_content}")).unwrap();

    let merge_arg_config = format!(r#"merge-tools.fake-editor.merge-args = ["${role}"]"#);
    let error = test_env.jj_cmd_failure(
        repo_path,
        &["resolve", "--config-toml", &merge_arg_config, "file"],
    );
    // This error means that fake-editor exited successfully but did not modify the
    // output file.
    insta::assert_snapshot!(error, @r###"
        Error: Failed to use external tool to resolve: The output file is either unchanged or empty after the editor quit.
        "###);
}

#[test]
fn test_normal_conflict_input_files() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "base", &[], &[("file", "base\n")]);
    create_commit(&test_env, &repo_path, "a", &["base"], &[("file", "a\n")]);
    create_commit(&test_env, &repo_path, "b", &["base"], &[("file", "b\n")]);
    create_commit(&test_env, &repo_path, "conflict", &["a", "b"], &[]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
        @   conflict
        |\  
        o | b
        | o a
        |/  
        o base
        o 
        "###);
    insta::assert_snapshot!(
    std::fs::read_to_string(repo_path.join("file")).unwrap()
        , @r###"
            <<<<<<<
            %%%%%%%
            -base
            +a
            +++++++
            b
            >>>>>>>
            "###);

    check_resolve_produces_input_file(&mut test_env, &repo_path, "base", "base\n");
    check_resolve_produces_input_file(&mut test_env, &repo_path, "left", "a\n");
    check_resolve_produces_input_file(&mut test_env, &repo_path, "right", "b\n");
}

#[test]
fn test_baseless_conflict_input_files() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "base", &[], &[]);
    create_commit(&test_env, &repo_path, "a", &["base"], &[("file", "a\n")]);
    create_commit(&test_env, &repo_path, "b", &["base"], &[("file", "b\n")]);
    create_commit(&test_env, &repo_path, "conflict", &["a", "b"], &[]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
        @   conflict
        |\  
        o | b
        | o a
        |/  
        o base
        o 
        "###);
    insta::assert_snapshot!(
    std::fs::read_to_string(repo_path.join("file")).unwrap()
        , @r###"
    <<<<<<<
    +++++++
    a
    +++++++
    b
    >>>>>>>
    "###);

    check_resolve_produces_input_file(&mut test_env, &repo_path, "base", "");
    check_resolve_produces_input_file(&mut test_env, &repo_path, "left", "a\n");
    check_resolve_produces_input_file(&mut test_env, &repo_path, "right", "b\n");
}

#[test]
fn test_too_many_parents() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "base", &[], &[("file", "base\n")]);
    create_commit(&test_env, &repo_path, "a", &["base"], &[("file", "a\n")]);
    create_commit(&test_env, &repo_path, "b", &["base"], &[("file", "b\n")]);
    create_commit(&test_env, &repo_path, "c", &["base"], &[("file", "c\n")]);
    create_commit(&test_env, &repo_path, "conflict", &["a", "b", "c"], &[]);

    let error = test_env.jj_cmd_failure(&repo_path, &["resolve", "file"]);
    insta::assert_snapshot!(error, @r###"
    Error: Failed to use external tool to resolve: The conflict at "file" has 2 removes and 3 adds.
    At most 1 remove and 2 adds are supported.
    "###);
}

#[test]
fn test_edit_delete_conflict_input_files() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "base", &[], &[("file", "base\n")]);
    create_commit(&test_env, &repo_path, "a", &["base"], &[("file", "a\n")]);
    create_commit(&test_env, &repo_path, "b", &["base"], &[]);
    std::fs::remove_file(repo_path.join("file")).unwrap();
    create_commit(&test_env, &repo_path, "conflict", &["a", "b"], &[]);
    // Test the setup
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
        @   conflict
        |\  
        o | b
        | o a
        |/  
        o base
        o 
        "###);
    insta::assert_snapshot!(
    std::fs::read_to_string(repo_path.join("file")).unwrap()
        , @r###"
    <<<<<<<
    %%%%%%%
    -base
    +a
    >>>>>>>
    "###);

    check_resolve_produces_input_file(&mut test_env, &repo_path, "base", "base\n");
    check_resolve_produces_input_file(&mut test_env, &repo_path, "left", "");
    // Note that `a` ended up in "right" rather than "left". It's unclear if this
    // can or should be fixed.
    check_resolve_produces_input_file(&mut test_env, &repo_path, "right", "a\n");
}

#[test]
fn test_file_vs_dir() {
    let test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(&test_env, &repo_path, "base", &[], &[("file", "base\n")]);
    create_commit(&test_env, &repo_path, "a", &["base"], &[("file", "a\n")]);
    create_commit(&test_env, &repo_path, "b", &["base"], &[]);
    std::fs::remove_file(repo_path.join("file")).unwrap();
    std::fs::create_dir(repo_path.join("file")).unwrap();
    // Without a placeholder file, `jj` ignores an empty directory
    std::fs::write(repo_path.join("file").join("placeholder"), "").unwrap();
    create_commit(&test_env, &repo_path, "conflict", &["a", "b"], &[]);

    let error = test_env.jj_cmd_failure(&repo_path, &["resolve", "file"]);
    insta::assert_snapshot!(error, @r###"
    Error: Failed to use external tool to resolve: Only conflicts that involve normal files (not symlinks, not executable, etc.) are supported. Conflict summary:
     Conflict:
      Removing file with id df967b96a579e45a18b8251732d16804b2e56a55
      Adding file with id 78981922613b2afb6025042ff6bd878ac1994e85
      Adding tree with id 133bb38fc4e4bf6b551f1f04db7e48f04cac2877

    "###);
}
