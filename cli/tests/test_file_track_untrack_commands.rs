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

use std::path::PathBuf;

use crate::common::TestEnvironment;

#[test]
fn test_track_untrack() {
    let test_env = TestEnvironment::default();
    test_env.add_config(r#"ui.allow-init-native = true"#);
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "initial").unwrap();
    std::fs::write(repo_path.join("file1.bak"), "initial").unwrap();
    std::fs::write(repo_path.join("file2.bak"), "initial").unwrap();
    let target_dir = repo_path.join("target");
    std::fs::create_dir(&target_dir).unwrap();
    std::fs::write(target_dir.join("file2"), "initial").unwrap();
    std::fs::write(target_dir.join("file3"), "initial").unwrap();

    // Run a command so all the files get tracked, then add "*.bak" to the ignore
    // patterns
    test_env.jj_cmd_ok(&repo_path, &["st"]);
    std::fs::write(repo_path.join(".gitignore"), "*.bak\n").unwrap();
    let files_before = test_env.jj_cmd_success(&repo_path, &["file", "list"]);

    // Errors out when not run at the head operation
    let stderr =
        test_env.jj_cmd_failure(&repo_path, &["file", "untrack", "file1", "--at-op", "@-"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: This command must be able to update the working copy.
    Hint: Don't use --at-op.
    "###);
    // Errors out when no path is specified
    let stderr = test_env.jj_cmd_cli_error(&repo_path, &["file", "untrack"]);
    insta::assert_snapshot!(stderr, @r###"
    error: the following required arguments were not provided:
      <FILESETS>...

    Usage: jj file untrack <FILESETS>...

    For more information, try '--help'.
    "###);
    // Errors out when a specified file is not ignored
    let stderr = test_env.jj_cmd_failure(&repo_path, &["file", "untrack", "file1", "file1.bak"]);
    insta::assert_snapshot!(stderr, @r###"
    Error: 'file1' is not ignored.
    Hint: Files that are not ignored will be added back by the next command.
    Make sure they're ignored, then try again.
    "###);
    let files_after = test_env.jj_cmd_success(&repo_path, &["file", "list"]);
    // There should be no changes to the state when there was an error
    assert_eq!(files_after, files_before);

    // Can untrack a single file
    assert!(files_before.contains("file1.bak\n"));
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["untrack", "file1.bak"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Warning: `jj untrack` is deprecated; use `jj file untrack` instead, which is equivalent
    Warning: `jj untrack` will be removed in a future version, and this will be a hard error
    "###);
    let files_after = test_env.jj_cmd_success(&repo_path, &["file", "list"]);
    // The file is no longer tracked
    assert!(!files_after.contains("file1.bak"));
    // Other files that match the ignore pattern are not untracked
    assert!(files_after.contains("file2.bak"));
    // The files still exist on disk
    assert!(repo_path.join("file1.bak").exists());
    assert!(repo_path.join("file2.bak").exists());

    // Errors out when multiple specified files are not ignored
    let stderr = test_env.jj_cmd_failure(&repo_path, &["file", "untrack", "target"]);
    assert_eq!(
        stderr,
        format!(
            "Error: '{}' and 1 other files are not ignored.\nHint: Files that are not ignored \
             will be added back by the next command.\nMake sure they're ignored, then try again.\n",
            PathBuf::from("target").join("file2").display()
        )
    );

    // Can untrack after adding to ignore patterns
    std::fs::write(repo_path.join(".gitignore"), ".bak\ntarget/\n").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["file", "untrack", "target"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let files_after = test_env.jj_cmd_success(&repo_path, &["file", "list"]);
    assert!(!files_after.contains("target"));
}

#[test]
fn test_track_untrack_sparse() {
    let test_env = TestEnvironment::default();
    test_env.add_config(r#"ui.allow-init-native = true"#);
    test_env.jj_cmd_ok(test_env.env_root(), &["init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1"), "contents").unwrap();
    std::fs::write(repo_path.join("file2"), "contents").unwrap();

    // When untracking a file that's not included in the sparse working copy, it
    // doesn't need to be ignored (because it won't be automatically added
    // back).
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    file1
    file2
    "###);
    test_env.jj_cmd_ok(&repo_path, &["sparse", "set", "--clear", "--add", "file1"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["file", "untrack", "file2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    file1
    "###);
    // Trying to manually track a file that's not included in the sparse working has
    // no effect. TODO: At least a warning would be useful
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["file", "track", "file2"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    file1
    "###);
}

#[test]
fn test_auto_track() {
    let test_env = TestEnvironment::default();
    test_env.add_config(r#"snapshot.auto-track = 'glob:*.rs'"#);
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join("file1.rs"), "initial").unwrap();
    std::fs::write(repo_path.join("file2.md"), "initial").unwrap();
    std::fs::write(repo_path.join("file3.md"), "initial").unwrap();

    // Only configured paths get auto-tracked
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    file1.rs
    "###);

    // Can manually track paths
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "track", "file3.md"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    file1.rs
    file3.md
    "###);

    // Can manually untrack paths
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "untrack", "file3.md"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    file1.rs
    "###);

    // CWD-relative paths in `snapshot.auto-track` are evaluated from the repo root
    let subdir = repo_path.join("sub");
    std::fs::create_dir(&subdir).unwrap();
    std::fs::write(subdir.join("file1.rs"), "initial").unwrap();
    let stdout = test_env.jj_cmd_success(&subdir, &["file", "list"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r###"
    ../file1.rs
    "###);

    // But `jj file track` wants CWD-relative paths
    let stdout = test_env.jj_cmd_success(&subdir, &["file", "track", "file1.rs"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&subdir, &["file", "list"]);
    insta::assert_snapshot!(stdout.replace('\\', "/"), @r###"
    ../file1.rs
    file1.rs
    "###);
}

#[test]
fn test_track_ignored() {
    let test_env = TestEnvironment::default();
    test_env.add_config(r#"snapshot.auto-track = 'none()'"#);
    test_env.jj_cmd_ok(test_env.env_root(), &["git", "init", "repo"]);
    let repo_path = test_env.env_root().join("repo");

    std::fs::write(repo_path.join(".gitignore"), "*.bak\n").unwrap();
    std::fs::write(repo_path.join("file1"), "initial").unwrap();
    std::fs::write(repo_path.join("file1.bak"), "initial").unwrap();

    // Track an unignored path
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "track", "file1"]);
    insta::assert_snapshot!(stdout, @"");
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    file1
    "###);
    // Track an ignored path
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "track", "file1.bak"]);
    insta::assert_snapshot!(stdout, @"");
    // TODO: We should teach `jj file track` to track ignored paths (possibly
    // requiring a flag)
    let stdout = test_env.jj_cmd_success(&repo_path, &["file", "list"]);
    insta::assert_snapshot!(stdout, @r###"
    file1
    "###);
}
