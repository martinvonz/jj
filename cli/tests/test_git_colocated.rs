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

use git2::Oid;

use crate::common::TestEnvironment;

#[test]
fn test_git_colocated() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&workspace_root).unwrap();

    // Create an initial commit in Git
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    git_repo
        .index()
        .unwrap()
        .add_path(Path::new("file"))
        .unwrap();
    let tree1_oid = git_repo.index().unwrap().write_tree().unwrap();
    let tree1 = git_repo.find_tree(tree1_oid).unwrap();
    let signature = git2::Signature::new(
        "Someone",
        "someone@example.com",
        &git2::Time::new(1234567890, 60),
    )
    .unwrap();
    git_repo
        .commit(
            Some("refs/heads/master"),
            &signature,
            &signature,
            "initial",
            &tree1,
            &[],
        )
        .unwrap();
    insta::assert_snapshot!(
        git_repo.head().unwrap().peel_to_commit().unwrap().id().to_string(),
        @"e61b6729ff4292870702f2f72b2a60165679ef37"
    );

    // Import the repo
    test_env.jj_cmd_ok(&workspace_root, &["init", "--git-repo", "."]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  3e9369cd54227eb88455e1834dbc08aad6a16ac4
    ◉  e61b6729ff4292870702f2f72b2a60165679ef37 master HEAD@git initial
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(
        git_repo.head().unwrap().peel_to_commit().unwrap().id().to_string(),
        @"e61b6729ff4292870702f2f72b2a60165679ef37"
    );

    // Modify the working copy. The working-copy commit should changed, but the Git
    // HEAD commit should not
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  b26951a9c6f5c270e4d039880208952fd5faae5e
    ◉  e61b6729ff4292870702f2f72b2a60165679ef37 master HEAD@git initial
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(
        git_repo.head().unwrap().peel_to_commit().unwrap().id().to_string(),
        @"e61b6729ff4292870702f2f72b2a60165679ef37"
    );

    // Create a new change from jj and check that it's reflected in Git
    test_env.jj_cmd_ok(&workspace_root, &["new"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  9dbb23ff2ff5e66c43880f1042369d704f7a321e
    ◉  b26951a9c6f5c270e4d039880208952fd5faae5e HEAD@git
    ◉  e61b6729ff4292870702f2f72b2a60165679ef37 master initial
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(
        git_repo.head().unwrap().target().unwrap().to_string(),
        @"b26951a9c6f5c270e4d039880208952fd5faae5e"
    );
}

#[test]
fn test_git_colocated_unborn_branch() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&workspace_root).unwrap();

    let add_file_to_index = |name: &str, data: &str| {
        std::fs::write(workspace_root.join(name), data).unwrap();
        let mut index = git_repo.index().unwrap();
        index.add_path(Path::new(name)).unwrap();
        index.write().unwrap();
    };
    let checkout_index = || {
        let mut index = git_repo.index().unwrap();
        index.read(true).unwrap(); // discard in-memory cache
        git_repo.checkout_index(Some(&mut index), None).unwrap();
    };

    // Initially, HEAD isn't set.
    test_env.jj_cmd_ok(&workspace_root, &["init", "--git-repo", "."]);
    assert!(git_repo.head().is_err());
    assert_eq!(
        git_repo.find_reference("HEAD").unwrap().symbolic_target(),
        Some("refs/heads/master")
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◉  0000000000000000000000000000000000000000
    "###);

    // Stage some change, and check out root. This shouldn't clobber the HEAD.
    add_file_to_index("file0", "");
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["new", "root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: kkmpptxz fcdbbd73 (empty) (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 0 files, removed 1 files
    "###);
    assert!(git_repo.head().is_err());
    assert_eq!(
        git_repo.find_reference("HEAD").unwrap().symbolic_target(),
        Some("refs/heads/master")
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  fcdbbd731496cae17161cd6be9b6cf1f759655a8
    │ ◉  1de814dbef9641cc6c5c80d2689b80778edcce09
    ├─╯
    ◉  0000000000000000000000000000000000000000
    "###);
    // Staged change shouldn't persist.
    checkout_index();
    insta::assert_snapshot!(test_env.jj_cmd_success(&workspace_root, &["status"]), @r###"
    The working copy is clean
    Working copy : kkmpptxz fcdbbd73 (empty) (no description set)
    Parent commit: zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // Stage some change, and create new HEAD. This shouldn't move the default
    // branch.
    add_file_to_index("file1", "");
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["new"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: royxmykx 76c60bf0 (empty) (no description set)
    Parent commit      : kkmpptxz f8d5bc77 (no description set)
    "###);
    assert!(git_repo.head().unwrap().symbolic_target().is_none());
    insta::assert_snapshot!(
        git_repo.head().unwrap().peel_to_commit().unwrap().id().to_string(),
        @"f8d5bc772d1147351fd6e8cea52a4f935d3b31e7"
    );
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  76c60bf0a66dcbe74d74d58c23848d96f9e86e84
    ◉  f8d5bc772d1147351fd6e8cea52a4f935d3b31e7 HEAD@git
    │ ◉  1de814dbef9641cc6c5c80d2689b80778edcce09
    ├─╯
    ◉  0000000000000000000000000000000000000000
    "###);
    // Staged change shouldn't persist.
    checkout_index();
    insta::assert_snapshot!(test_env.jj_cmd_success(&workspace_root, &["status"]), @r###"
    The working copy is clean
    Working copy : royxmykx 76c60bf0 (empty) (no description set)
    Parent commit: kkmpptxz f8d5bc77 (no description set)
    "###);

    // Assign the default branch. The branch is no longer "unborn".
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "-r@-", "master"]);

    // Stage some change, and check out root again. This should unset the HEAD.
    // https://github.com/martinvonz/jj/issues/1495
    add_file_to_index("file2", "");
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["new", "root()"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: znkkpsqq 10dd328b (empty) (no description set)
    Parent commit      : zzzzzzzz 00000000 (empty) (no description set)
    Added 0 files, modified 0 files, removed 2 files
    "###);
    assert!(git_repo.head().is_err());
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  10dd328bb906e15890e55047740eab2812a3b2f7
    │ ◉  2c576a57d2e6e8494616629cfdbb8fe5e3fea73b
    │ ◉  f8d5bc772d1147351fd6e8cea52a4f935d3b31e7 master
    ├─╯
    │ ◉  1de814dbef9641cc6c5c80d2689b80778edcce09
    ├─╯
    ◉  0000000000000000000000000000000000000000
    "###);
    // Staged change shouldn't persist.
    checkout_index();
    insta::assert_snapshot!(test_env.jj_cmd_success(&workspace_root, &["status"]), @r###"
    The working copy is clean
    Working copy : znkkpsqq 10dd328b (empty) (no description set)
    Parent commit: zzzzzzzz 00000000 (empty) (no description set)
    "###);

    // New snapshot and commit can be created after the HEAD got unset.
    std::fs::write(workspace_root.join("file3"), "").unwrap();
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["new"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: wqnwkozp cab23370 (empty) (no description set)
    Parent commit      : znkkpsqq 8f5b2638 (no description set)
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  cab233704a5c0b21bde070943055f22142fb2043
    ◉  8f5b263819457712a2937428b9c58a2a84afbb1c HEAD@git
    │ ◉  2c576a57d2e6e8494616629cfdbb8fe5e3fea73b
    │ ◉  f8d5bc772d1147351fd6e8cea52a4f935d3b31e7 master
    ├─╯
    │ ◉  1de814dbef9641cc6c5c80d2689b80778edcce09
    ├─╯
    ◉  0000000000000000000000000000000000000000
    "###);
}

#[test]
fn test_git_colocated_export_branches_on_snapshot() {
    // Checks that we export branches that were changed only because the working
    // copy was snapshotted

    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["init", "--git-repo", "."]);

    // Create branch pointing to the initial commit
    std::fs::write(workspace_root.join("file"), "initial").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "foo"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  438471f3fbf1004298d8fb01eeb13663a051a643 foo
    ◉  0000000000000000000000000000000000000000
    "###);

    // The branch gets updated when we modify the working copy, and it should get
    // exported to Git without requiring any other changes
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  fab22d1acf5bb9c5aa48cb2c3dd2132072a359ca foo
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(git_repo
        .find_reference("refs/heads/foo")
        .unwrap()
        .target()
        .unwrap()
        .to_string(), @"fab22d1acf5bb9c5aa48cb2c3dd2132072a359ca");
}

#[test]
fn test_git_colocated_rebase_on_import() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["init", "--git-repo", "."]);

    // Make some changes in jj and check that they're reflected in git
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["commit", "-m", "add a file"]);
    std::fs::write(workspace_root.join("file"), "modified").unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "master"]);
    test_env.jj_cmd_ok(&workspace_root, &["commit", "-m", "modify a file"]);
    // TODO: We shouldn't need this command here to trigger an import of the
    // refs/heads/master we just exported
    test_env.jj_cmd_ok(&workspace_root, &["st"]);

    // Move `master` and HEAD backwards, which should result in commit2 getting
    // hidden, and a new working-copy commit at the new position.
    let commit2_oid = git_repo
        .find_branch("master", git2::BranchType::Local)
        .unwrap()
        .get()
        .target()
        .unwrap();
    let commit2 = git_repo.find_commit(commit2_oid).unwrap();
    let commit1 = commit2.parents().next().unwrap();
    git_repo.branch("master", &commit1, true).unwrap();
    git_repo.set_head("refs/heads/master").unwrap();
    let (stdout, stderr) = get_log_output_with_stderr(&test_env, &workspace_root);
    insta::assert_snapshot!(stdout, @r###"
    @  7f96185cfbe36341d0f9a86ebfaeab67a5922c7e
    ◉  4bcbeaba9a4b309c5f45a8807fbf5499b9714315 master HEAD@git add a file
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Reset the working copy parent to the new Git HEAD.
    Abandoned 1 commits that are no longer reachable.
    Done importing changes from the underlying Git repo.
    "###);
}

#[test]
fn test_git_colocated_branches() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["init", "--git-repo", "."]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "-m", "foo"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "@-", "-m", "bar"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  3560559274ab431feea00b7b7e0b9250ecce951f bar
    │ ◉  1e6f0b403ed2ff9713b5d6b1dc601e4804250cda foo
    ├─╯
    ◉  230dd059e1b059aefc0da06a2e5a7dbf22362f22 HEAD@git
    ◉  0000000000000000000000000000000000000000
    "###);

    // Create a branch in jj. It should be exported to Git even though it points to
    // the working- copy commit.
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "master"]);
    insta::assert_snapshot!(
        git_repo.find_reference("refs/heads/master").unwrap().target().unwrap().to_string(),
        @"3560559274ab431feea00b7b7e0b9250ecce951f"
    );
    insta::assert_snapshot!(
        git_repo.head().unwrap().target().unwrap().to_string(),
        @"230dd059e1b059aefc0da06a2e5a7dbf22362f22"
    );

    // Update the branch in Git
    let target_id = test_env.jj_cmd_success(
        &workspace_root,
        &["log", "--no-graph", "-T=commit_id", "-r=description(foo)"],
    );
    git_repo
        .reference(
            "refs/heads/master",
            Oid::from_str(&target_id).unwrap(),
            true,
            "test",
        )
        .unwrap();
    let (stdout, stderr) = get_log_output_with_stderr(&test_env, &workspace_root);
    insta::assert_snapshot!(stdout, @r###"
    @  096dc80da67094fbaa6683e2a205dddffa31f9a8
    │ ◉  1e6f0b403ed2ff9713b5d6b1dc601e4804250cda master foo
    ├─╯
    ◉  230dd059e1b059aefc0da06a2e5a7dbf22362f22 HEAD@git
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Abandoned 1 commits that are no longer reachable.
    Working copy now at: yqosqzyt 096dc80d (empty) (no description set)
    Parent commit      : qpvuntsm 230dd059 (empty) (no description set)
    Done importing changes from the underlying Git repo.
    "###);
}

#[test]
fn test_git_colocated_branch_forget() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let _git_repo = git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["init", "--git-repo", "."]);
    test_env.jj_cmd_ok(&workspace_root, &["new"]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "foo"]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  65b6b74e08973b88d38404430f119c8c79465250 foo
    ◉  230dd059e1b059aefc0da06a2e5a7dbf22362f22 HEAD@git
    ◉  0000000000000000000000000000000000000000
    "###);
    let stdout = test_env.jj_cmd_success(&workspace_root, &["branch", "list", "--all"]);
    insta::assert_snapshot!(stdout, @r###"
    foo: rlvkpnrz 65b6b74e (empty) (no description set)
      @git: rlvkpnrz 65b6b74e (empty) (no description set)
    "###);

    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["branch", "forget", "foo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @"");
    // A forgotten branch is deleted in the git repo. For a detailed demo explaining
    // this, see `test_branch_forget_export` in `test_branch_command.rs`.
    let stdout = test_env.jj_cmd_success(&workspace_root, &["branch", "list", "--all"]);
    insta::assert_snapshot!(stdout, @"");
}

#[test]
fn test_git_colocated_conflicting_git_refs() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["init", "--git-repo", "."]);
    test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "main"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&workspace_root, &["branch", "create", "main/sub"]);
    insta::assert_snapshot!(stdout, @"");
    insta::with_settings!({filters => vec![(": The lock for resource.*", ": ...")]}, {
        insta::assert_snapshot!(stderr, @r###"
        Warning: Failed to export some branches:
          main/sub: Failed to set: A lock could not be obtained for reference "refs/heads/main/sub": ...
        Hint: Git doesn't allow a branch name that looks like a parent directory of
        another (e.g. `foo` and `foo/bar`). Try to rename the branches that failed to
        export or their "parent" branches.
        "###);
    });
}

#[test]
fn test_git_colocated_checkout_non_empty_working_copy() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&workspace_root).unwrap();
    test_env.jj_cmd_ok(&workspace_root, &["init", "--git-repo", "."]);

    // Create an initial commit in Git
    // We use this to set HEAD to master
    std::fs::write(workspace_root.join("file"), "contents").unwrap();
    git_repo
        .index()
        .unwrap()
        .add_path(Path::new("file"))
        .unwrap();
    let tree1_oid = git_repo.index().unwrap().write_tree().unwrap();
    let tree1 = git_repo.find_tree(tree1_oid).unwrap();
    let signature = git2::Signature::new(
        "Someone",
        "someone@example.com",
        &git2::Time::new(1234567890, 60),
    )
    .unwrap();
    git_repo
        .commit(
            Some("refs/heads/master"),
            &signature,
            &signature,
            "initial",
            &tree1,
            &[],
        )
        .unwrap();
    insta::assert_snapshot!(
        git_repo.head().unwrap().peel_to_commit().unwrap().id().to_string(),
        @"e61b6729ff4292870702f2f72b2a60165679ef37"
    );

    std::fs::write(workspace_root.join("two"), "y").unwrap();

    test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "two"]);
    test_env.jj_cmd_ok(&workspace_root, &["new", "@-"]);
    let (_, stderr) = test_env.jj_cmd_ok(&workspace_root, &["describe", "-m", "new"]);
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: kkmpptxz 4c049607 (empty) new
    Parent commit      : lnksqltp e61b6729 master | initial
    "###);

    let git_head = git_repo.find_reference("HEAD").unwrap();
    let git_head_target = git_head.symbolic_target().unwrap();

    assert_eq!(git_head_target, "refs/heads/master");

    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  4c04960765ca906d0cb25b15a946be4c0dd71b8e new
    │ ◉  4ec6f6506bd1903410f15b80058a7f0d8f62deea two
    ├─╯
    ◉  e61b6729ff4292870702f2f72b2a60165679ef37 master HEAD@git initial
    ◉  0000000000000000000000000000000000000000
    "###);
}

#[test]
fn test_git_colocated_fetch_deleted_or_moved_branch() {
    let test_env = TestEnvironment::default();
    test_env.add_config("git.auto-local-branch = true");
    let origin_path = test_env.env_root().join("origin");
    git2::Repository::init(&origin_path).unwrap();
    test_env.jj_cmd_ok(&origin_path, &["init", "--git-repo=."]);
    test_env.jj_cmd_ok(&origin_path, &["describe", "-m=A"]);
    test_env.jj_cmd_ok(&origin_path, &["branch", "create", "A"]);
    test_env.jj_cmd_ok(&origin_path, &["new", "-m=B_to_delete"]);
    test_env.jj_cmd_ok(&origin_path, &["branch", "create", "B_to_delete"]);
    test_env.jj_cmd_ok(&origin_path, &["new", "-m=original C", "@-"]);
    test_env.jj_cmd_ok(&origin_path, &["branch", "create", "C_to_move"]);

    let clone_path = test_env.env_root().join("clone");
    git2::Repository::clone(origin_path.to_str().unwrap(), &clone_path).unwrap();
    test_env.jj_cmd_ok(&clone_path, &["init", "--git-repo=."]);
    test_env.jj_cmd_ok(&clone_path, &["new", "A"]);
    insta::assert_snapshot!(get_log_output(&test_env, &clone_path), @r###"
    @  0335878796213c3a701f1c9c34dcae242bee4131
    │ ◉  8d4e006fd63547965fbc3a26556a9aa531076d32 C_to_move original C
    ├─╯
    │ ◉  929e298ae9edf969b405a304c75c10457c47d52c B_to_delete B_to_delete
    ├─╯
    ◉  a86754f975f953fa25da4265764adc0c62e9ce6b A HEAD@git A
    ◉  0000000000000000000000000000000000000000
    "###);

    test_env.jj_cmd_ok(&origin_path, &["branch", "delete", "B_to_delete"]);
    // Move branch C sideways
    test_env.jj_cmd_ok(&origin_path, &["describe", "C_to_move", "-m", "moved C"]);
    let (stdout, stderr) = test_env.jj_cmd_ok(&clone_path, &["git", "fetch"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    branch: B_to_delete@origin [deleted] untracked
    branch: C_to_move@origin   [updated] tracked
    Abandoned 2 commits that are no longer reachable.
    "###);
    // "original C" and "B_to_delete" are abandoned, as the corresponding branches
    // were deleted or moved on the remote (#864)
    insta::assert_snapshot!(get_log_output(&test_env, &clone_path), @r###"
    ◉  04fd29df05638156b20044b3b6136b42abcb09ab C_to_move moved C
    │ @  0335878796213c3a701f1c9c34dcae242bee4131
    ├─╯
    ◉  a86754f975f953fa25da4265764adc0c62e9ce6b A HEAD@git A
    ◉  0000000000000000000000000000000000000000
    "###);
}

#[test]
fn test_git_colocated_rebase_dirty_working_copy() {
    let test_env = TestEnvironment::default();
    let repo_path = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&repo_path).unwrap();
    test_env.jj_cmd_ok(&repo_path, &["init", "--git-repo=."]);

    std::fs::write(repo_path.join("file"), "base").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    std::fs::write(repo_path.join("file"), "old").unwrap();
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "feature"]);

    // Make the working-copy dirty, delete the checked out branch.
    std::fs::write(repo_path.join("file"), "new").unwrap();
    git_repo
        .find_reference("refs/heads/feature")
        .unwrap()
        .delete()
        .unwrap();

    // Because the working copy is dirty, the new working-copy commit will be
    // diverged. Therefore, the feature branch has change-delete conflict.
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["status"]);
    insta::assert_snapshot!(stdout, @r###"
    Working copy changes:
    M file
    Working copy : rlvkpnrz d6c5e664 feature?? | (no description set)
    Parent commit: qpvuntsm 5973d373 (no description set)
    These branches have conflicts:
      feature
      Use `jj branch list` to see details. Use `jj branch set <name> -r <rev>` to resolve.
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Warning: Failed to export some branches:
      feature: Modified ref had been deleted in Git
    Done importing changes from the underlying Git repo.
    "###);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  d6c5e66473426f5ed3a24ecce8ce8b44ff23cf81 feature??
    ◉  5973d3731aba9dd86c00b4a765fbc4cc13f1e14b HEAD@git
    ◉  0000000000000000000000000000000000000000
    "###);

    // The working-copy content shouldn't be lost.
    insta::assert_snapshot!(
        std::fs::read_to_string(repo_path.join("file")).unwrap(), @"new");
}

#[test]
fn test_git_colocated_external_checkout() {
    let test_env = TestEnvironment::default();
    let repo_path = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&repo_path).unwrap();
    test_env.jj_cmd_ok(&repo_path, &["init", "--git-repo=."]);
    test_env.jj_cmd_ok(&repo_path, &["ci", "-m=A"]);
    test_env.jj_cmd_ok(&repo_path, &["branch", "create", "-r@-", "master"]);
    test_env.jj_cmd_ok(&repo_path, &["new", "-m=B", "root()"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);

    // Checked out anonymous branch
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  f8a23336e41840ed1757ef323402a770427dc89a
    ◉  eccedddfa5152d99fc8ddd1081b375387a8a382a HEAD@git B
    │ ◉  a86754f975f953fa25da4265764adc0c62e9ce6b master A
    ├─╯
    ◉  0000000000000000000000000000000000000000
    "###);

    // Check out another branch by external command
    git_repo
        .set_head_detached(
            git_repo
                .find_reference("refs/heads/master")
                .unwrap()
                .target()
                .unwrap(),
        )
        .unwrap();

    // The old working-copy commit gets abandoned, but the whole branch should not
    // be abandoned. (#1042)
    let (stdout, stderr) = get_log_output_with_stderr(&test_env, &repo_path);
    insta::assert_snapshot!(stdout, @r###"
    @  adadbd65a794e2294962b3c3da9aada09fe1b472
    ◉  a86754f975f953fa25da4265764adc0c62e9ce6b master HEAD@git A
    │ ◉  eccedddfa5152d99fc8ddd1081b375387a8a382a B
    ├─╯
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(stderr, @r###"
    Reset the working copy parent to the new Git HEAD.
    "###);
}

#[test]
fn test_git_colocated_squash_undo() {
    let test_env = TestEnvironment::default();
    let repo_path = test_env.env_root().join("repo");
    git2::Repository::init(&repo_path).unwrap();
    test_env.jj_cmd_ok(&repo_path, &["init", "--git-repo=."]);
    test_env.jj_cmd_ok(&repo_path, &["ci", "-m=A"]);
    // Test the setup
    insta::assert_snapshot!(get_log_output_divergence(&test_env, &repo_path), @r###"
    @  rlvkpnrzqnoo 8f71e3b6a3be
    ◉  qpvuntsmwlqt a86754f975f9 A HEAD@git
    ◉  zzzzzzzzzzzz 000000000000
    "###);

    test_env.jj_cmd_ok(&repo_path, &["squash"]);
    insta::assert_snapshot!(get_log_output_divergence(&test_env, &repo_path), @r###"
    @  zsuskulnrvyr f0c12b0396d9
    ◉  qpvuntsmwlqt 2f376ea1478c A HEAD@git
    ◉  zzzzzzzzzzzz 000000000000
    "###);
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    // TODO: There should be no divergence here; 2f376ea1478c should be hidden
    // (#922)
    insta::assert_snapshot!(get_log_output_divergence(&test_env, &repo_path), @r###"
    @  rlvkpnrzqnoo 8f71e3b6a3be
    ◉  qpvuntsmwlqt a86754f975f9 A HEAD@git
    ◉  zzzzzzzzzzzz 000000000000
    "###);
}

#[test]
fn test_git_colocated_undo_head_move() {
    let test_env = TestEnvironment::default();
    let repo_path = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&repo_path).unwrap();
    test_env.jj_cmd_ok(&repo_path, &["init", "--git-repo=."]);

    // Create new HEAD
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    insta::assert_snapshot!(
        git_repo.head().unwrap().target().unwrap().to_string(),
        @"230dd059e1b059aefc0da06a2e5a7dbf22362f22");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  65b6b74e08973b88d38404430f119c8c79465250
    ◉  230dd059e1b059aefc0da06a2e5a7dbf22362f22 HEAD@git
    ◉  0000000000000000000000000000000000000000
    "###);

    // HEAD should be unset
    test_env.jj_cmd_ok(&repo_path, &["undo"]);
    assert!(git_repo.head().is_err());
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◉  0000000000000000000000000000000000000000
    "###);

    // Create commit on non-root commit
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    test_env.jj_cmd_ok(&repo_path, &["new"]);
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  69b19f73cf584f162f078fb0d91c55ca39d10bc7
    ◉  eb08b363bb5ef8ee549314260488980d7bbe8f63 HEAD@git
    ◉  230dd059e1b059aefc0da06a2e5a7dbf22362f22
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(
        git_repo.head().unwrap().target().unwrap().to_string(),
        @"eb08b363bb5ef8ee549314260488980d7bbe8f63");

    // HEAD should be moved back
    let (stdout, stderr) = test_env.jj_cmd_ok(&repo_path, &["undo"]);
    insta::assert_snapshot!(stdout, @"");
    insta::assert_snapshot!(stderr, @r###"
    Working copy now at: royxmykx eb08b363 (empty) (no description set)
    Parent commit      : qpvuntsm 230dd059 (empty) (no description set)
    "###);
    insta::assert_snapshot!(
        git_repo.head().unwrap().target().unwrap().to_string(),
        @"230dd059e1b059aefc0da06a2e5a7dbf22362f22");
    insta::assert_snapshot!(get_log_output(&test_env, &repo_path), @r###"
    @  eb08b363bb5ef8ee549314260488980d7bbe8f63
    ◉  230dd059e1b059aefc0da06a2e5a7dbf22362f22 HEAD@git
    ◉  0000000000000000000000000000000000000000
    "###);
}

fn get_log_output_divergence(test_env: &TestEnvironment, repo_path: &Path) -> String {
    let template = r#"
    separate(" ",
      change_id.short(),
      commit_id.short(),
      description.first_line(),
      branches,
      git_head,
      if(divergent, "!divergence!"),
    )
    "#;
    test_env.jj_cmd_success(repo_path, &["log", "-T", template])
}

fn get_log_output(test_env: &TestEnvironment, workspace_root: &Path) -> String {
    let template = r#"separate(" ", commit_id, branches, git_head, description)"#;
    test_env.jj_cmd_success(workspace_root, &["log", "-T", template, "-r=all()"])
}

fn get_log_output_with_stderr(
    test_env: &TestEnvironment,
    workspace_root: &Path,
) -> (String, String) {
    let template = r#"separate(" ", commit_id, branches, git_head, description)"#;
    test_env.jj_cmd_ok(workspace_root, &["log", "-T", template, "-r=all()"])
}

#[test]
fn test_git_colocated_unreachable_commits() {
    let test_env = TestEnvironment::default();
    let workspace_root = test_env.env_root().join("repo");
    let git_repo = git2::Repository::init(&workspace_root).unwrap();

    // Create an initial commit in Git
    let empty_tree_oid = git_repo.treebuilder(None).unwrap().write().unwrap();
    let tree1 = git_repo.find_tree(empty_tree_oid).unwrap();
    let signature = git2::Signature::new(
        "Someone",
        "someone@example.com",
        &git2::Time::new(1234567890, 60),
    )
    .unwrap();
    let oid1 = git_repo
        .commit(
            Some("refs/heads/master"),
            &signature,
            &signature,
            "initial",
            &tree1,
            &[],
        )
        .unwrap();
    insta::assert_snapshot!(
        git_repo.head().unwrap().peel_to_commit().unwrap().id().to_string(),
        @"2ee37513d2b5e549f7478c671a780053614bff19"
    );

    // Add a second commit in Git
    let tree2 = git_repo.find_tree(empty_tree_oid).unwrap();
    let signature = git2::Signature::new(
        "Someone",
        "someone@example.com",
        &git2::Time::new(1234567890, 62),
    )
    .unwrap();
    let oid2 = git_repo
        .commit(
            None,
            &signature,
            &signature,
            "next",
            &tree2,
            &[&git_repo.find_commit(oid1).unwrap()],
        )
        .unwrap();
    insta::assert_snapshot!(
        git_repo.head().unwrap().peel_to_commit().unwrap().id().to_string(),
        @"2ee37513d2b5e549f7478c671a780053614bff19"
    );

    // Import the repo while there is no path to the second commit
    test_env.jj_cmd_ok(&workspace_root, &["init", "--git-repo", "."]);
    insta::assert_snapshot!(get_log_output(&test_env, &workspace_root), @r###"
    @  66ae47cee4f8c28ee8d7e4f5d9401b03c07e22f2
    ◉  2ee37513d2b5e549f7478c671a780053614bff19 master HEAD@git initial
    ◉  0000000000000000000000000000000000000000
    "###);
    insta::assert_snapshot!(
        git_repo.head().unwrap().peel_to_commit().unwrap().id().to_string(),
        @"2ee37513d2b5e549f7478c671a780053614bff19"
    );

    // Check that trying to look up the second commit fails gracefully
    let stderr = test_env.jj_cmd_failure(&workspace_root, &["show", &oid2.to_string()]);
    insta::assert_snapshot!(stderr, @r###"
    Error: Revision "8e713ff77b54928dd4a82aaabeca44b1ae91722c" doesn't exist
    "###);
}
