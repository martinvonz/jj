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
    test_env.jj_cmd_success(&repo_path, &["resolve"]);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
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
    Working copy : 665f83829a6a conflict
    Working copy changes:
    M file
    "###);

    // Check that the output file starts with conflict markers if
    // `merge-tool-edits-conflict-markers=true`
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
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
        ],
    );
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
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
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
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
        ],
    );
    // Note the "Modified" below
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
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
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
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
    test_env.jj_cmd_success(&repo_path, &["resolve"]);
    // Note the "Resolved" below
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
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
    Working copy : d5b735448648 conflict
    Working copy changes:
    M file
    "###);

    // TODO: Check that running `jj new` and then `jj resolve -r conflict` works
    // correctly.
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
    let error =
        test_env.jj_cmd_failure(repo_path, &["resolve", "--config-toml", &merge_arg_config]);
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

    let error = test_env.jj_cmd_failure(&repo_path, &["resolve"]);
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

    let error = test_env.jj_cmd_failure(&repo_path, &["resolve"]);
    insta::assert_snapshot!(error, @r###"
    Error: Failed to use external tool to resolve: Only conflicts that involve normal files (not symlinks, not executable, etc.) are supported. Conflict summary for "file":
    Conflict:
      Removing file with id df967b96a579e45a18b8251732d16804b2e56a55
      Adding file with id 78981922613b2afb6025042ff6bd878ac1994e85
      Adding tree with id 133bb38fc4e4bf6b551f1f04db7e48f04cac2877

    "###);
}

#[test]
fn test_multiple_conflicts() {
    let mut test_env = TestEnvironment::default();
    test_env.jj_cmd_success(test_env.env_root(), &["init", "repo", "--git"]);
    let repo_path = test_env.env_root().join("repo");

    create_commit(
        &test_env,
        &repo_path,
        "base",
        &[],
        &[("file1", "first base\n"), ("file2", "second base\n")],
    );
    create_commit(
        &test_env,
        &repo_path,
        "a",
        &["base"],
        &[("file1", "first a\n"), ("file2", "second a\n")],
    );
    create_commit(
        &test_env,
        &repo_path,
        "b",
        &["base"],
        &[("file1", "first b\n"), ("file2", "second b\n")],
    );
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
    std::fs::read_to_string(repo_path.join("file1")).unwrap()
        , @r###"
    <<<<<<<
    %%%%%%%
    -first base
    +first a
    +++++++
    first b
    >>>>>>>
    "###);
    insta::assert_snapshot!(
    std::fs::read_to_string(repo_path.join("file2")).unwrap()
        , @r###"
    <<<<<<<
    %%%%%%%
    -second base
    +second a
    +++++++
    second b
    >>>>>>>
    "###);

    let editor_script = test_env.set_up_fake_editor();

    // Check that we can manually pick which of the conflicts to resolve first
    std::fs::write(&editor_script, "expect\n\0write\nresolution file2\n").unwrap();
    test_env.jj_cmd_success(&repo_path, &["resolve", "file2"]);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
    @r###"
    Resolved conflict in file2:
       1     : <<<<<<<
       2     : %%%%%%%
       3    1: -secondresolution basefile2
       4     : +second a
       5     : +++++++
       6     : second b
       7     : >>>>>>>
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["status"]), 
    @r###"
    Parent commit: 9a25592b7aee a
    Working copy : 00dac0bec4b1 conflict
    Working copy changes:
    M file1
    M file2
    There are unresolved conflicts at these paths:
    file1
    "###);

    // For the rest of the test, we call `jj resolve` several times in a row to
    // resolve each conflict in the order it chooses.
    test_env.jj_cmd_success(&repo_path, &["undo"]);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
    @"");
    std::fs::write(
        &editor_script,
        "expect\n\0write\nfirst resolution for auto-chosen file\n",
    )
    .unwrap();
    test_env.jj_cmd_success(&repo_path, &["resolve"]);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
    @r###"
    Resolved conflict in file1:
       1     : <<<<<<<
       2     : %%%%%%%
       3    1: -first base
       4    1: +firstresolution a
       5     : +++++++
       6    1: firstfor bauto-chosen file
       7     : >>>>>>>
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["status"]), 
    @r###"
    Parent commit: 9a25592b7aee a
    Working copy : f06196060882 conflict
    Working copy changes:
    M file1
    M file2
    There are unresolved conflicts at these paths:
    file2
    "###);
    std::fs::write(
        &editor_script,
        "expect\n\0write\nsecond resolution for auto-chosen file\n",
    )
    .unwrap();

    test_env.jj_cmd_success(&repo_path, &["resolve"]);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["diff"]), 
    @r###"
    Resolved conflict in file1:
       1     : <<<<<<<
       2     : %%%%%%%
       3    1: -first base
       4    1: +firstresolution a
       5     : +++++++
       6    1: firstfor bauto-chosen file
       7     : >>>>>>>
    Resolved conflict in file2:
       1     : <<<<<<<
       2     : %%%%%%%
       3    1: -second base
       4    1: +secondresolution a
       5     : +++++++
       6    1: secondfor bauto-chosen file
       7     : >>>>>>>
    "###);
    insta::assert_snapshot!(test_env.jj_cmd_success(&repo_path, &["status"]), 
    @r###"
    Parent commit: 9a25592b7aee a
    Working copy : 0fafecce390d conflict
    Working copy changes:
    M file1
    M file2
    "###);

    insta::assert_snapshot!(test_env.jj_cmd_cli_error(&repo_path, &["resolve"]), 
    @r###"
    Error: No conflicts found at this revision
    "###);
}
