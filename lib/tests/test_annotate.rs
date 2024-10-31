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

use std::fmt::Write as _;
use std::rc::Rc;

use jj_lib::annotate::get_annotation_for_file;
use jj_lib::annotate::get_annotation_with_file_content;
use jj_lib::annotate::FileAnnotation;
use jj_lib::backend::CommitId;
use jj_lib::backend::MergedTreeId;
use jj_lib::backend::MillisSinceEpoch;
use jj_lib::backend::Signature;
use jj_lib::backend::Timestamp;
use jj_lib::backend::TreeValue;
use jj_lib::commit::Commit;
use jj_lib::repo::MutableRepo;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;
use jj_lib::revset::ResolvedRevsetExpression;
use jj_lib::revset::RevsetExpression;
use jj_lib::settings::UserSettings;
use testutils::create_tree;
use testutils::TestRepo;

fn create_commit_fn<'a>(
    mut_repo: &'a mut MutableRepo,
    settings: &'a UserSettings,
) -> impl FnMut(&str, &[&CommitId], MergedTreeId) -> Commit + 'a {
    // stabilize commit IDs for ease of debugging
    let signature = Signature {
        name: "Some One".to_owned(),
        email: "some.one@example.com".to_owned(),
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(0),
            tz_offset: 0,
        },
    };
    move |description, parent_ids, tree_id| {
        let parent_ids = parent_ids.iter().map(|&id| id.clone()).collect();
        mut_repo
            .new_commit(settings, parent_ids, tree_id)
            .set_author(signature.clone())
            .set_committer(signature.clone())
            .set_description(description)
            .write()
            .unwrap()
    }
}

fn annotate(repo: &dyn Repo, commit: &Commit, file_path: &RepoPath) -> String {
    let domain = RevsetExpression::all();
    annotate_within(repo, commit, &domain, file_path)
}

fn annotate_within(
    repo: &dyn Repo,
    commit: &Commit,
    domain: &Rc<ResolvedRevsetExpression>,
    file_path: &RepoPath,
) -> String {
    let annotation = get_annotation_for_file(repo, commit, domain, file_path).unwrap();
    format_annotation(repo, &annotation)
}

fn annotate_parent_tree(repo: &dyn Repo, commit: &Commit, file_path: &RepoPath) -> String {
    let tree = commit.parent_tree(repo).unwrap();
    let text = match tree.path_value(file_path).unwrap().into_resolved().unwrap() {
        Some(TreeValue::File { id, .. }) => {
            let mut reader = repo.store().read_file(file_path, &id).unwrap();
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf).unwrap();
            buf
        }
        value => panic!("unexpected path value: {value:?}"),
    };
    let domain = RevsetExpression::all();
    let annotation =
        get_annotation_with_file_content(repo, commit.id(), &domain, file_path, text).unwrap();
    format_annotation(repo, &annotation)
}

fn format_annotation(repo: &dyn Repo, annotation: &FileAnnotation) -> String {
    let mut output = String::new();
    for (commit_id, line) in annotation.lines() {
        let commit = commit_id.map(|id| repo.store().get_commit(id).unwrap());
        let desc = commit
            .as_ref()
            .map_or("*******", |commit| commit.description().trim_end());
        write!(output, "{desc}: {line}").unwrap();
    }
    output
}

#[test]
fn test_annotate_linear() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let root_commit_id = repo.store().root_commit_id();
    let file_path = RepoPath::from_internal_string("file");

    let mut tx = repo.start_transaction(&settings);
    let mut create_commit = create_commit_fn(tx.repo_mut(), &settings);
    let content1 = "";
    let content2 = "2a\n2b\n";
    let content3 = "2b\n3\n";
    let tree1 = create_tree(repo, &[(file_path, content1)]);
    let tree2 = create_tree(repo, &[(file_path, content2)]);
    let tree3 = create_tree(repo, &[(file_path, content3)]);
    let commit1 = create_commit("commit1", &[root_commit_id], tree1.id());
    let commit2 = create_commit("commit2", &[commit1.id()], tree2.id());
    let commit3 = create_commit("commit3", &[commit2.id()], tree3.id());
    let commit4 = create_commit("commit4", &[commit3.id()], tree3.id()); // empty commit
    drop(create_commit);

    insta::assert_snapshot!(annotate(tx.repo(), &commit1, file_path), @"");
    insta::assert_snapshot!(annotate(tx.repo(), &commit2, file_path), @r#"
    commit2: 2a
    commit2: 2b
    "#);
    insta::assert_snapshot!(annotate(tx.repo(), &commit3, file_path), @r#"
    commit2: 2b
    commit3: 3
    "#);
    insta::assert_snapshot!(annotate(tx.repo(), &commit4, file_path), @r#"
    commit2: 2b
    commit3: 3
    "#);
}

#[test]
fn test_annotate_merge_simple() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let root_commit_id = repo.store().root_commit_id();
    let file_path = RepoPath::from_internal_string("file");

    // 4    "2 1 3"
    // |\
    // | 3  "1 3"
    // | |
    // 2 |  "2 1"
    // |/
    // 1    "1"
    let mut tx = repo.start_transaction(&settings);
    let mut create_commit = create_commit_fn(tx.repo_mut(), &settings);
    let content1 = "1\n";
    let content2 = "2\n1\n";
    let content3 = "1\n3\n";
    let content4 = "2\n1\n3\n";
    let tree1 = create_tree(repo, &[(file_path, content1)]);
    let tree2 = create_tree(repo, &[(file_path, content2)]);
    let tree3 = create_tree(repo, &[(file_path, content3)]);
    let tree4 = create_tree(repo, &[(file_path, content4)]);
    let commit1 = create_commit("commit1", &[root_commit_id], tree1.id());
    let commit2 = create_commit("commit2", &[commit1.id()], tree2.id());
    let commit3 = create_commit("commit3", &[commit1.id()], tree3.id());
    let commit4 = create_commit("commit4", &[commit2.id(), commit3.id()], tree4.id());
    drop(create_commit);

    insta::assert_snapshot!(annotate(tx.repo(), &commit4, file_path), @r#"
    commit2: 2
    commit1: 1
    commit3: 3
    "#);

    // Exclude the fork commit and its ancestors.
    let domain = RevsetExpression::commit(commit1.id().clone())
        .ancestors()
        .negated();
    insta::assert_snapshot!(annotate_within(tx.repo(), &commit4, &domain, file_path), @r"
    commit2: 2
    *******: 1
    commit3: 3
    ");

    // Exclude one side of the merge and its ancestors.
    let domain = RevsetExpression::commit(commit2.id().clone())
        .ancestors()
        .negated();
    insta::assert_snapshot!(annotate_within(tx.repo(), &commit4, &domain, file_path), @r"
    *******: 2
    *******: 1
    commit3: 3
    ");

    // Exclude both sides of the merge and their ancestors.
    let domain = RevsetExpression::commit(commit4.id().clone());
    insta::assert_snapshot!(annotate_within(tx.repo(), &commit4, &domain, file_path), @r"
    *******: 2
    *******: 1
    *******: 3
    ");

    // Exclude intermediate commit, which is useless but works.
    let domain = RevsetExpression::commit(commit3.id().clone()).negated();
    insta::assert_snapshot!(annotate_within(tx.repo(), &commit4, &domain, file_path), @r"
    commit2: 2
    commit1: 1
    commit4: 3
    ");
}

#[test]
fn test_annotate_merge_split() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let root_commit_id = repo.store().root_commit_id();
    let file_path = RepoPath::from_internal_string("file");

    // 4    "2 1a 1b 3 4"
    // |\
    // | 3  "1b 3"
    // | |
    // 2 |  "2 1a"
    // |/
    // 1    "1a 1b"
    let mut tx = repo.start_transaction(&settings);
    let mut create_commit = create_commit_fn(tx.repo_mut(), &settings);
    let content1 = "1a\n1b\n";
    let content2 = "2\n1a\n";
    let content3 = "1b\n3\n";
    let content4 = "2\n1a\n1b\n3\n4\n";
    let tree1 = create_tree(repo, &[(file_path, content1)]);
    let tree2 = create_tree(repo, &[(file_path, content2)]);
    let tree3 = create_tree(repo, &[(file_path, content3)]);
    let tree4 = create_tree(repo, &[(file_path, content4)]);
    let commit1 = create_commit("commit1", &[root_commit_id], tree1.id());
    let commit2 = create_commit("commit2", &[commit1.id()], tree2.id());
    let commit3 = create_commit("commit3", &[commit1.id()], tree3.id());
    let commit4 = create_commit("commit4", &[commit2.id(), commit3.id()], tree4.id());
    drop(create_commit);

    insta::assert_snapshot!(annotate(tx.repo(), &commit4, file_path), @r#"
    commit2: 2
    commit1: 1a
    commit1: 1b
    commit3: 3
    commit4: 4
    "#);
}

#[test]
fn test_annotate_merge_split_interleaved() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let root_commit_id = repo.store().root_commit_id();
    let file_path = RepoPath::from_internal_string("file");

    // 6    "1a 4 1b 6 2a 5 2b"
    // |\
    // | 5  "1b 5 2b"
    // | |
    // 4 |  "1a 4 2a"
    // |/
    // 3    "1a 1b 2a 2b"
    // |\
    // | 2  "2a 2b"
    // |
    // 1    "1a 1b"
    let mut tx = repo.start_transaction(&settings);
    let mut create_commit = create_commit_fn(tx.repo_mut(), &settings);
    let content1 = "1a\n1b\n";
    let content2 = "2a\n2b\n";
    let content3 = "1a\n1b\n2a\n2b\n";
    let content4 = "1a\n4\n2a\n";
    let content5 = "1b\n5\n2b\n";
    let content6 = "1a\n4\n1b\n6\n2a\n5\n2b\n";
    let tree1 = create_tree(repo, &[(file_path, content1)]);
    let tree2 = create_tree(repo, &[(file_path, content2)]);
    let tree3 = create_tree(repo, &[(file_path, content3)]);
    let tree4 = create_tree(repo, &[(file_path, content4)]);
    let tree5 = create_tree(repo, &[(file_path, content5)]);
    let tree6 = create_tree(repo, &[(file_path, content6)]);
    let commit1 = create_commit("commit1", &[root_commit_id], tree1.id());
    let commit2 = create_commit("commit2", &[root_commit_id], tree2.id());
    let commit3 = create_commit("commit3", &[commit1.id(), commit2.id()], tree3.id());
    let commit4 = create_commit("commit4", &[commit3.id()], tree4.id());
    let commit5 = create_commit("commit5", &[commit3.id()], tree5.id());
    let commit6 = create_commit("commit6", &[commit4.id(), commit5.id()], tree6.id());
    drop(create_commit);

    insta::assert_snapshot!(annotate(tx.repo(), &commit6, file_path), @r#"
    commit1: 1a
    commit4: 4
    commit1: 1b
    commit6: 6
    commit2: 2a
    commit5: 5
    commit2: 2b
    "#);
}

#[test]
fn test_annotate_merge_dup() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let root_commit_id = repo.store().root_commit_id();
    let file_path = RepoPath::from_internal_string("file");

    // 4    "2 1 1 3 4"
    // |\
    // | 3  "1 3"
    // | |
    // 2 |  "2 1"
    // |/
    // 1    "1"
    let mut tx = repo.start_transaction(&settings);
    let mut create_commit = create_commit_fn(tx.repo_mut(), &settings);
    let content1 = "1\n";
    let content2 = "2\n1\n";
    let content3 = "1\n3\n";
    let content4 = "2\n1\n1\n3\n4\n";
    let tree1 = create_tree(repo, &[(file_path, content1)]);
    let tree2 = create_tree(repo, &[(file_path, content2)]);
    let tree3 = create_tree(repo, &[(file_path, content3)]);
    let tree4 = create_tree(repo, &[(file_path, content4)]);
    let commit1 = create_commit("commit1", &[root_commit_id], tree1.id());
    let commit2 = create_commit("commit2", &[commit1.id()], tree2.id());
    let commit3 = create_commit("commit3", &[commit1.id()], tree3.id());
    let commit4 = create_commit("commit4", &[commit2.id(), commit3.id()], tree4.id());
    drop(create_commit);

    // Both "1"s can be propagated to commit1 through commit2 and commit3.
    // Alternatively, it's also good to interpret that one of the "1"s was
    // produced at commit2, commit3, or commit4.
    insta::assert_snapshot!(annotate(tx.repo(), &commit4, file_path), @r#"
    commit2: 2
    commit1: 1
    commit1: 1
    commit3: 3
    commit4: 4
    "#);

    // For example, the parent tree of commit4 doesn't contain multiple "1"s.
    // If annotation were computed compared to the parent tree, not trees of the
    // parent commits, "1" would be inserted at commit4.
    insta::assert_snapshot!(annotate_parent_tree(tx.repo(), &commit4, file_path), @r"
    commit2: 2
    commit1: 1
    commit3: 3
    ");
}

#[test]
fn test_annotate_file_directory_transition() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let root_commit_id = repo.store().root_commit_id();
    let file_path1 = RepoPath::from_internal_string("file/was_dir");
    let file_path2 = RepoPath::from_internal_string("file");

    let mut tx = repo.start_transaction(&settings);
    let mut create_commit = create_commit_fn(tx.repo_mut(), &settings);
    let tree1 = create_tree(repo, &[(file_path1, "1\n")]);
    let tree2 = create_tree(repo, &[(file_path2, "2\n")]);
    let commit1 = create_commit("commit1", &[root_commit_id], tree1.id());
    let commit2 = create_commit("commit2", &[commit1.id()], tree2.id());
    drop(create_commit);

    insta::assert_snapshot!(annotate(tx.repo(), &commit2, file_path2), @r#"
    commit2: 2
    "#);
}
