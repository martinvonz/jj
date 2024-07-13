// Copyright 2021 The Jujutsu Authors
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

use indoc::indoc;
use jj_lib::backend::FileId;
use jj_lib::conflicts::{
    extract_as_single_hunk, materialize_merge_result, parse_merge_result, update_from_content,
};
use jj_lib::merge::Merge;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;
use jj_lib::store::Store;
use pollster::FutureExt;
use testutils::TestRepo;

#[test]
fn test_materialize_conflict_basic() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store();

    let path = RepoPath::from_internal_string("file");
    let base_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1
            line 2
            line 3
            line 4
            line 5
        "},
    );
    let left_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1
            line 2
            left 3.1
            left 3.2
            left 3.3
            line 4
            line 5
        "},
    );
    let right_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1
            line 2
            right 3.1
            line 4
            line 5
        "},
    );

    // The left side should come first. The diff should be use the smaller (right)
    // side, and the left side should be a snapshot.
    let conflict = Merge::from_removes_adds(
        vec![Some(base_id.clone())],
        vec![Some(left_id.clone()), Some(right_id.clone())],
    );
    insta::assert_snapshot!(
        &materialize_conflict_string(store, path, &conflict),
        @r###"
    line 1
    line 2
    <<<<<<< Conflict 1 of 1
    +++++++ Contents of side #1
    left 3.1
    left 3.2
    left 3.3
    %%%%%%% Changes from base to side #2
    -line 3
    +right 3.1
    >>>>>>> Conflict 1 of 1 ends
    line 4
    line 5
    "###
    );
    // Swap the positive terms in the conflict. The diff should still use the right
    // side, but now the right side should come first.
    let conflict = Merge::from_removes_adds(
        vec![Some(base_id.clone())],
        vec![Some(right_id.clone()), Some(left_id.clone())],
    );
    insta::assert_snapshot!(
        &materialize_conflict_string(store, path, &conflict),
        @r###"
    line 1
    line 2
    <<<<<<< Conflict 1 of 1
    %%%%%%% Changes from base to side #1
    -line 3
    +right 3.1
    +++++++ Contents of side #2
    left 3.1
    left 3.2
    left 3.3
    >>>>>>> Conflict 1 of 1 ends
    line 4
    line 5
    "###
    );
}

#[test]
fn test_materialize_conflict_multi_rebase_conflicts() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store();

    // Create changes (a, b, c) on top of the base, and linearize them.
    let path = RepoPath::from_internal_string("file");
    let base_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1
            line 2 base
            line 3
        "},
    );
    let a_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1
            line 2 a.1
            line 2 a.2
            line 2 a.3
            line 3
        "},
    );
    let b_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1
            line 2 b.1
            line 2 b.2
            line 3
        "},
    );
    let c_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1
            line 2 c.1
            line 3
        "},
    );

    // The order of (a, b, c) should be preserved. For all cases, the "a" side
    // should be a snapshot.
    let conflict = Merge::from_removes_adds(
        vec![Some(base_id.clone()), Some(base_id.clone())],
        vec![Some(a_id.clone()), Some(b_id.clone()), Some(c_id.clone())],
    );
    insta::assert_snapshot!(
        &materialize_conflict_string(store, path, &conflict),
        @r###"
    line 1
    <<<<<<< Conflict 1 of 1
    +++++++ Contents of side #1
    line 2 a.1
    line 2 a.2
    line 2 a.3
    %%%%%%% Changes from base #1 to side #2
    -line 2 base
    +line 2 b.1
    +line 2 b.2
    %%%%%%% Changes from base #2 to side #3
    -line 2 base
    +line 2 c.1
    >>>>>>> Conflict 1 of 1 ends
    line 3
    "###
    );
    let conflict = Merge::from_removes_adds(
        vec![Some(base_id.clone()), Some(base_id.clone())],
        vec![Some(c_id.clone()), Some(b_id.clone()), Some(a_id.clone())],
    );
    insta::assert_snapshot!(
        &materialize_conflict_string(store, path, &conflict),
        @r###"
    line 1
    <<<<<<< Conflict 1 of 1
    %%%%%%% Changes from base #1 to side #1
    -line 2 base
    +line 2 c.1
    %%%%%%% Changes from base #2 to side #2
    -line 2 base
    +line 2 b.1
    +line 2 b.2
    +++++++ Contents of side #3
    line 2 a.1
    line 2 a.2
    line 2 a.3
    >>>>>>> Conflict 1 of 1 ends
    line 3
    "###
    );
    let conflict = Merge::from_removes_adds(
        vec![Some(base_id.clone()), Some(base_id.clone())],
        vec![Some(c_id.clone()), Some(a_id.clone()), Some(b_id.clone())],
    );
    insta::assert_snapshot!(
        &materialize_conflict_string(store, path, &conflict),
        @r###"
    line 1
    <<<<<<< Conflict 1 of 1
    %%%%%%% Changes from base #1 to side #1
    -line 2 base
    +line 2 c.1
    +++++++ Contents of side #2
    line 2 a.1
    line 2 a.2
    line 2 a.3
    %%%%%%% Changes from base #2 to side #3
    -line 2 base
    +line 2 b.1
    +line 2 b.2
    >>>>>>> Conflict 1 of 1 ends
    line 3
    "###
    );
}

//  TODO: With options
#[test]
fn test_materialize_parse_roundtrip() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store();

    let path = RepoPath::from_internal_string("file");
    let base_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1
            line 2
            line 3
            line 4
            line 5
        "},
    );
    let left_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1 left
            line 2 left
            line 3
            line 4
            line 5 left
        "},
    );
    let right_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1 right
            line 2
            line 3
            line 4 right
            line 5 right
        "},
    );

    let conflict = Merge::from_removes_adds(
        vec![Some(base_id.clone())],
        vec![Some(left_id.clone()), Some(right_id.clone())],
    );
    let materialized = materialize_conflict_string(store, path, &conflict);
    insta::assert_snapshot!(
        materialized,
        @r###"
    <<<<<<< Conflict 1 of 2
    +++++++ Contents of side #1
    line 1 left
    line 2 left
    %%%%%%% Changes from base to side #2
    -line 1
    +line 1 right
     line 2
    >>>>>>> Conflict 1 of 2 ends
    line 3
    <<<<<<< Conflict 2 of 2
    %%%%%%% Changes from base to side #1
     line 4
    -line 5
    +line 5 left
    +++++++ Contents of side #2
    line 4 right
    line 5 right
    >>>>>>> Conflict 2 of 2 ends
    "###
    );

    // The first add should always be from the left side
    insta::assert_debug_snapshot!(
        parse_merge_result(materialized.as_bytes(), conflict.num_sides()),
        @r###"
    Some(
        Conflicted(
            [
                "line 1 left\nline 2 left\nline 3\nline 4\nline 5 left\n",
                "line 1\nline 2\nline 3\nline 4\nline 5\n",
                "line 1 right\nline 2\nline 3\nline 4 right\nline 5 right\n",
            ],
        ),
    )
    "###);
}

#[test]
fn test_materialize_conflict_no_newlines_at_eof() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store();

    let path = RepoPath::from_internal_string("file");
    let base_id = testutils::write_file(store, path, "base");
    let left_empty_id = testutils::write_file(store, path, "");
    let right_id = testutils::write_file(store, path, "right");

    let conflict = Merge::from_removes_adds(
        vec![Some(base_id.clone())],
        vec![Some(left_empty_id.clone()), Some(right_id.clone())],
    );
    let materialized = &materialize_conflict_string(store, path, &conflict);
    insta::assert_snapshot!(materialized,
        @r###"
    <<<<<<< Conflict 1 of 1
    %%%%%%% Changes from base to side #1
    -base+++++++ Contents of side #2
    right>>>>>>> Conflict 1 of 1 ends
    "###
    );
    // BUG(#3968): These conflict markers cannot be parsed
    insta::assert_debug_snapshot!(parse_merge_result(
        materialized.as_bytes(),
        conflict.num_sides()
    ),@"None");
}

#[test]
fn test_materialize_conflict_modify_delete() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store();

    let path = RepoPath::from_internal_string("file");
    let base_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1
            line 2
            line 3
            line 4
            line 5
        "},
    );
    let modified_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1
            line 2
            modified
            line 4
            line 5
        "},
    );
    let deleted_id = testutils::write_file(
        store,
        path,
        indoc! {"
            line 1
            line 2
            line 4
            line 5
        "},
    );

    // left modifies a line, right deletes the same line.
    let conflict = Merge::from_removes_adds(
        vec![Some(base_id.clone())],
        vec![Some(modified_id.clone()), Some(deleted_id.clone())],
    );
    insta::assert_snapshot!(&materialize_conflict_string(store, path, &conflict), @r###"
    line 1
    line 2
    <<<<<<< Conflict 1 of 1
    +++++++ Contents of side #1
    modified
    %%%%%%% Changes from base to side #2
    -line 3
    >>>>>>> Conflict 1 of 1 ends
    line 4
    line 5
    "###
    );

    // right modifies a line, left deletes the same line.
    let conflict = Merge::from_removes_adds(
        vec![Some(base_id.clone())],
        vec![Some(deleted_id.clone()), Some(modified_id.clone())],
    );
    insta::assert_snapshot!(&materialize_conflict_string(store, path, &conflict), @r###"
    line 1
    line 2
    <<<<<<< Conflict 1 of 1
    %%%%%%% Changes from base to side #1
    -line 3
    +++++++ Contents of side #2
    modified
    >>>>>>> Conflict 1 of 1 ends
    line 4
    line 5
    "###
    );

    // modify/delete conflict at the file level
    let conflict = Merge::from_removes_adds(
        vec![Some(base_id.clone())],
        vec![Some(modified_id.clone()), None],
    );
    insta::assert_snapshot!(&materialize_conflict_string(store, path, &conflict), @r###"
    <<<<<<< Conflict 1 of 1
    %%%%%%% Changes from base to side #1
     line 1
     line 2
    -line 3
    +modified
     line 4
     line 5
    +++++++ Contents of side #2
    >>>>>>> Conflict 1 of 1 ends
    "###
    );
}

#[test]
fn test_materialize_conflict_two_forward_diffs() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store();

    // Create conflict A-B+B-C+D-E+C. This is designed to tempt the algorithm to
    // produce a negative snapshot at the end like this:
    // <<<<
    // ====
    // A
    // %%%%
    //  B
    // ++++
    // D
    // %%%%
    //  C
    // ----
    // E
    // >>>>
    // TODO: Maybe we should never have negative snapshots
    let path = RepoPath::from_internal_string("file");
    let a_id = testutils::write_file(store, path, "A\n");
    let b_id = testutils::write_file(store, path, "B\n");
    let c_id = testutils::write_file(store, path, "C\n");
    let d_id = testutils::write_file(store, path, "D\n");
    let e_id = testutils::write_file(store, path, "E\n");

    let conflict = Merge::from_removes_adds(
        vec![Some(b_id.clone()), Some(c_id.clone()), Some(e_id.clone())],
        vec![
            Some(a_id.clone()),
            Some(b_id.clone()),
            Some(d_id.clone()),
            Some(c_id.clone()),
        ],
    );
    insta::assert_snapshot!(
        &materialize_conflict_string(store, path, &conflict),
        @r###"
    <<<<<<< Conflict 1 of 1
    +++++++ Contents of side #1
    A
    %%%%%%% Changes from base #1 to side #2
     B
    +++++++ Contents of side #3
    D
    %%%%%%% Changes from base #2 to side #4
     C
    ------- Contents of base #3
    E
    >>>>>>> Conflict 1 of 1 ends
    "###
    );
}

#[test]
fn test_parse_conflict_resolved() {
    assert_eq!(
        parse_merge_result(
            indoc! {b"
            line 1
line 2
line 3
line 4
line 5
"},
            2
        ),
        None
    )
}

#[test]
fn test_parse_conflict_simple() {
    insta::assert_debug_snapshot!(
        parse_merge_result(indoc! {b"
            line 1
            <<<<<<<
            %%%%%%%
             line 2
            -line 3
            +left
             line 4
            +++++++
            right
            >>>>>>>
            line 5
            "},
            2
        ),
        @r###"
    Some(
        Conflicted(
            [
                "line 1\nline 2\nleft\nline 4\nline 5\n",
                "line 1\nline 2\nline 3\nline 4\nline 5\n",
                "line 1\nright\nline 5\n",
            ],
        ),
    )
    "###
    );
    insta::assert_debug_snapshot!(
        parse_merge_result(indoc! {b"
            line 1
            <<<<<<< Text
            %%%%%%% Different text 
             line 2
            -line 3
            +left
             line 4
            +++++++ Yet <><>< more text
            right
            >>>>>>> More and more text
            line 5
            "},
            2
        ),
        @r###"
    Some(
        Conflicted(
            [
                "line 1\nline 2\nleft\nline 4\nline 5\n",
                "line 1\nline 2\nline 3\nline 4\nline 5\n",
                "line 1\nright\nline 5\n",
            ],
        ),
    )
    "###
    );
    // The conflict markers are too long and shouldn't parse (though we may
    // decide to change this in the future)
    insta::assert_debug_snapshot!(
        parse_merge_result(indoc! {b"
            line 1
            <<<<<<<<<<<
            %%%%%%%%%%%
             line 2
            -line 3
            +left
             line 4
            +++++++++++
            right
            >>>>>>>>>>>
            line 5
            "},
            2
        ),
        @"None"
    )
}

#[test]
fn test_parse_conflict_multi_way() {
    insta::assert_debug_snapshot!(
        parse_merge_result(
            indoc! {b"
                line 1
                <<<<<<<
                %%%%%%%
                 line 2
                -line 3
                +left
                 line 4
                +++++++
                right
                %%%%%%%
                 line 2
                +forward
                 line 3
                 line 4
                >>>>>>>
                line 5
                "},
            3
        ),
        @r###"
    Some(
        Conflicted(
            [
                "line 1\nline 2\nleft\nline 4\nline 5\n",
                "line 1\nline 2\nline 3\nline 4\nline 5\n",
                "line 1\nright\nline 5\n",
                "line 1\nline 2\nline 3\nline 4\nline 5\n",
                "line 1\nline 2\nforward\nline 3\nline 4\nline 5\n",
            ],
        ),
    )
    "###
    );
    insta::assert_debug_snapshot!(
        parse_merge_result(indoc! {b"
            line 1
            <<<<<<< Random text
            %%%%%%% Random text
             line 2
            -line 3
            +left
             line 4
            +++++++ Random text
            right
            %%%%%%% Random text
             line 2
            +forward
             line 3
             line 4
            >>>>>>> Random text
            line 5
            "},
            3
        ),
        @r###"
    Some(
        Conflicted(
            [
                "line 1\nline 2\nleft\nline 4\nline 5\n",
                "line 1\nline 2\nline 3\nline 4\nline 5\n",
                "line 1\nright\nline 5\n",
                "line 1\nline 2\nline 3\nline 4\nline 5\n",
                "line 1\nline 2\nforward\nline 3\nline 4\nline 5\n",
            ],
        ),
    )
    "###
    );
}

#[test]
fn test_parse_conflict_different_wrong_arity() {
    assert_eq!(
        parse_merge_result(
            indoc! {b"
            line 1
            <<<<<<<
            %%%%%%%
             line 2
            -line 3
            +left
             line 4
            +++++++
            right
            >>>>>>>
            line 5
            "},
            3
        ),
        None
    )
}

#[test]
fn test_parse_conflict_malformed_marker() {
    // The conflict marker is missing `%%%%%%%`
    assert_eq!(
        parse_merge_result(
            indoc! {b"
            line 1
            <<<<<<<
             line 2
            -line 3
            +left
             line 4
            +++++++
            right
            >>>>>>>
            line 5
            "},
            2
        ),
        None
    )
}

#[test]
fn test_parse_conflict_malformed_diff() {
    // The diff part is invalid (missing space before "line 4")
    assert_eq!(
        parse_merge_result(
            indoc! {b"
            line 1
            <<<<<<<
            %%%%%%%
             line 2
            -line 3
            +left
            line 4
            +++++++
            right
            >>>>>>>
            line 5
            "},
            2
        ),
        None
    )
}

#[test]
fn test_update_conflict_from_content() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store();

    let path = RepoPath::from_internal_string("dir/file");
    let base_file_id = testutils::write_file(store, path, "line 1\nline 2\nline 3\n");
    let left_file_id = testutils::write_file(store, path, "left 1\nline 2\nleft 3\n");
    let right_file_id = testutils::write_file(store, path, "right 1\nline 2\nright 3\n");
    let conflict = Merge::from_removes_adds(
        vec![Some(base_file_id.clone())],
        vec![Some(left_file_id.clone()), Some(right_file_id.clone())],
    );

    // If the content is unchanged compared to the materialized value, we get the
    // old conflict id back.
    let materialized = materialize_conflict_string(store, path, &conflict);
    let parse = |content| {
        update_from_content(&conflict, store, path, content)
            .block_on()
            .unwrap()
    };
    assert_eq!(parse(materialized.as_bytes()), conflict);

    // If the conflict is resolved, we get None back to indicate that.
    let expected_file_id = testutils::write_file(store, path, "resolved 1\nline 2\nresolved 3\n");
    assert_eq!(
        parse(b"resolved 1\nline 2\nresolved 3\n"),
        Merge::normal(expected_file_id)
    );

    // If the conflict is partially resolved, we get a new conflict back.
    let new_conflict = parse(
        b"resolved 1\nline 2\n<<<<<<<\n%%%%%%%\n-line 3\n+left 3\n+++++++\nright 3\n>>>>>>>\n",
    );
    assert_ne!(new_conflict, conflict);
    // Calculate expected new FileIds
    let new_base_file_id = testutils::write_file(store, path, "resolved 1\nline 2\nline 3\n");
    let new_left_file_id = testutils::write_file(store, path, "resolved 1\nline 2\nleft 3\n");
    let new_right_file_id = testutils::write_file(store, path, "resolved 1\nline 2\nright 3\n");
    assert_eq!(
        new_conflict,
        Merge::from_removes_adds(
            vec![Some(new_base_file_id.clone())],
            vec![
                Some(new_left_file_id.clone()),
                Some(new_right_file_id.clone())
            ]
        )
    );
}

#[test]
fn test_update_conflict_from_content_modify_delete() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store();

    let path = RepoPath::from_internal_string("dir/file");
    let before_file_id = testutils::write_file(store, path, "line 1\nline 2 before\nline 3\n");
    let after_file_id = testutils::write_file(store, path, "line 1\nline 2 after\nline 3\n");
    let conflict =
        Merge::from_removes_adds(vec![Some(before_file_id)], vec![Some(after_file_id), None]);

    // If the content is unchanged compared to the materialized value, we get the
    // old conflict id back.
    let materialized = materialize_conflict_string(store, path, &conflict);
    let parse = |content| {
        update_from_content(&conflict, store, path, content)
            .block_on()
            .unwrap()
    };
    assert_eq!(parse(materialized.as_bytes()), conflict);

    // If the conflict is resolved, we get None back to indicate that.
    let expected_file_id = testutils::write_file(store, path, "resolved\n");
    assert_eq!(parse(b"resolved\n"), Merge::normal(expected_file_id));

    // If the conflict is modified, we get a new conflict back.
    let new_conflict = parse(
        b"<<<<<<<\n%%%%%%%\n line 1\n-line 2 before\n+line 2 modified after\n line 3\n+++++++\n>>>>>>>\n",
    );
    // Calculate expected new FileIds
    let new_base_file_id = testutils::write_file(store, path, "line 1\nline 2 before\nline 3\n");
    let new_left_file_id =
        testutils::write_file(store, path, "line 1\nline 2 modified after\nline 3\n");

    assert_eq!(
        new_conflict,
        Merge::from_removes_adds(
            vec![Some(new_base_file_id.clone())],
            vec![Some(new_left_file_id.clone()), None]
        )
    );
}

#[test]
fn test_update_conflict_from_content_simplified_conflict() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store();

    let path = RepoPath::from_internal_string("dir/file");
    let base_file_id = testutils::write_file(store, path, "line 1\nline 2\nline 3\n");
    let left_file_id = testutils::write_file(store, path, "left 1\nline 2\nleft 3\n");
    let right_file_id = testutils::write_file(store, path, "right 1\nline 2\nright 3\n");
    // Conflict: left - base + base - base + right
    let conflict = Merge::from_removes_adds(
        vec![Some(base_file_id.clone()), Some(base_file_id.clone())],
        vec![
            Some(left_file_id.clone()),
            Some(base_file_id.clone()),
            Some(right_file_id.clone()),
        ],
    );
    let simplified_conflict = conflict.clone().simplify();

    // If the content is unchanged compared to the materialized value, we get the
    // old conflict id back. Both the simplified and unsimplified materialized
    // conflicts should return the old conflict id.
    let materialized = materialize_conflict_string(store, path, &conflict);
    let materialized_simplified = materialize_conflict_string(store, path, &simplified_conflict);
    let parse = |content| {
        update_from_content(&conflict, store, path, content)
            .block_on()
            .unwrap()
    };
    insta::assert_snapshot!(
        materialized,
        @r###"
    <<<<<<< Conflict 1 of 2
    +++++++ Contents of side #1
    left 1
    %%%%%%% Changes from base #1 to side #2
     line 1
    %%%%%%% Changes from base #2 to side #3
    -line 1
    +right 1
    >>>>>>> Conflict 1 of 2 ends
    line 2
    <<<<<<< Conflict 2 of 2
    +++++++ Contents of side #1
    left 3
    %%%%%%% Changes from base #1 to side #2
     line 3
    %%%%%%% Changes from base #2 to side #3
    -line 3
    +right 3
    >>>>>>> Conflict 2 of 2 ends
    "###
    );
    insta::assert_snapshot!(
        materialized_simplified,
        @r###"
    <<<<<<< Conflict 1 of 2
    %%%%%%% Changes from base to side #1
    -line 1
    +left 1
    +++++++ Contents of side #2
    right 1
    >>>>>>> Conflict 1 of 2 ends
    line 2
    <<<<<<< Conflict 2 of 2
    %%%%%%% Changes from base to side #1
    -line 3
    +left 3
    +++++++ Contents of side #2
    right 3
    >>>>>>> Conflict 2 of 2 ends
    "###
    );
    assert_eq!(parse(materialized.as_bytes()), conflict);
    assert_eq!(parse(materialized_simplified.as_bytes()), conflict);

    // If the conflict is resolved, we get a normal merge back to indicate that.
    let expected_file_id = testutils::write_file(store, path, "resolved 1\nline 2\nresolved 3\n");
    assert_eq!(
        parse(b"resolved 1\nline 2\nresolved 3\n"),
        Merge::normal(expected_file_id)
    );

    // If the conflict is partially resolved, we get a new conflict back.
    // This should work with both the simplified and unsimplified conflict.
    let new_conflict = parse(indoc! {b"
        resolved 1
        line 2
        <<<<<<< Conflict 2 of 2
        +++++++ Contents of side #1
        edited left 3
        %%%%%%% Changes from base #1 to side #2
         edited line 3
        %%%%%%% Changes from base #2 to side #3
        -edited line 3
        +edited right 3
        >>>>>>> Conflict 2 of 2 ends
    "});
    let new_simplified_conflict = parse(indoc! {b"
        resolved 1
        line 2
        <<<<<<< Conflict 2 of 2
        %%%%%%% Changes from base to side #1
        -edited line 3
        +edited left 3
        +++++++ Contents of side #2
        edited right 3
        >>>>>>> Conflict 2 of 2 ends
    "});
    assert_ne!(new_conflict, conflict);
    assert_ne!(new_simplified_conflict, conflict);
    // Calculate expected new FileIds
    let new_base_file_id =
        testutils::write_file(store, path, "resolved 1\nline 2\nedited line 3\n");
    let new_left_file_id =
        testutils::write_file(store, path, "resolved 1\nline 2\nedited left 3\n");
    let new_right_file_id =
        testutils::write_file(store, path, "resolved 1\nline 2\nedited right 3\n");
    assert_eq!(
        new_conflict,
        Merge::from_removes_adds(
            vec![
                Some(new_base_file_id.clone()),
                Some(new_base_file_id.clone())
            ],
            vec![
                Some(new_left_file_id.clone()),
                Some(new_base_file_id.clone()),
                Some(new_right_file_id.clone())
            ]
        )
    );
    assert_eq!(
        new_simplified_conflict,
        Merge::from_removes_adds(
            vec![Some(base_file_id.clone()), Some(new_base_file_id.clone())],
            vec![
                Some(new_left_file_id.clone()),
                Some(base_file_id.clone()),
                Some(new_right_file_id.clone())
            ]
        )
    );
}

fn materialize_conflict_string(
    store: &Store,
    path: &RepoPath,
    conflict: &Merge<Option<FileId>>,
) -> String {
    let mut result: Vec<u8> = vec![];
    let contents = extract_as_single_hunk(conflict, store, path)
        .block_on()
        .unwrap();
    materialize_merge_result(&contents, &mut result).unwrap();
    String::from_utf8(result).unwrap()
}
