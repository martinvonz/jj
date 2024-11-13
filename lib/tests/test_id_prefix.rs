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

use itertools::Itertools;
use jj_lib::backend::ChangeId;
use jj_lib::backend::CommitId;
use jj_lib::backend::MillisSinceEpoch;
use jj_lib::backend::Signature;
use jj_lib::backend::Timestamp;
use jj_lib::id_prefix::IdPrefixContext;
use jj_lib::object_id::HexPrefix;
use jj_lib::object_id::ObjectId;
use jj_lib::object_id::PrefixResolution::AmbiguousMatch;
use jj_lib::object_id::PrefixResolution::NoMatch;
use jj_lib::object_id::PrefixResolution::SingleMatch;
use jj_lib::repo::Repo;
use jj_lib::revset::RevsetExpression;
use testutils::TestRepo;
use testutils::TestRepoBackend;

#[test]
fn test_id_prefix() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init_with_backend(TestRepoBackend::Git);
    let repo = &test_repo.repo;
    let root_commit_id = repo.store().root_commit_id();
    let root_change_id = repo.store().root_change_id();

    let mut tx = repo.start_transaction(&settings);
    let mut create_commit = |parent_id: &CommitId| {
        let signature = Signature {
            name: "Some One".to_string(),
            email: "some.one@example.com".to_string(),
            timestamp: Timestamp {
                timestamp: MillisSinceEpoch(0),
                tz_offset: 0,
            },
        };
        tx.repo_mut()
            .new_commit(
                &settings,
                vec![parent_id.clone()],
                repo.store().empty_merged_tree_id(),
            )
            .set_author(signature.clone())
            .set_committer(signature)
            .write()
            .unwrap()
    };
    let mut commits = vec![create_commit(root_commit_id)];
    for _ in 0..25 {
        commits.push(create_commit(commits.last().unwrap().id()));
    }
    let repo = tx.commit("test").unwrap();

    // Print the commit IDs and change IDs for reference
    let commit_prefixes = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| format!("{} {}", &commit.id().hex()[..3], i))
        .sorted()
        .join("\n");
    insta::assert_snapshot!(commit_prefixes, @r###"
    11a 5
    214 24
    2a6 2
    33e 14
    3be 16
    3ea 18
    593 20
    5d3 1
    5f6 13
    676 3
    7b6 25
    7da 9
    81c 10
    87e 12
    997 21
    9f7 22
    a0e 4
    a55 19
    ac4 23
    c18 17
    ce9 0
    d42 6
    d9d 8
    eec 15
    efe 7
    fa3 11
    "###);
    let change_prefixes = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| format!("{} {}", &commit.change_id().hex()[..3], i))
        .sorted()
        .join("\n");
    insta::assert_snapshot!(change_prefixes, @r###"
    026 9
    030 13
    1b5 6
    26b 3
    26c 8
    271 10
    439 2
    44a 17
    4e9 16
    5b2 23
    6c2 19
    781 0
    79f 14
    7d5 24
    86b 20
    871 7
    896 5
    9e4 18
    a2c 1
    a63 22
    b19 11
    b93 4
    bf5 21
    c24 15
    d64 12
    fee 25
    "###);

    let prefix = |x| HexPrefix::new(x).unwrap();

    // Without a disambiguation revset
    // ---------------------------------------------------------------------------------------------
    let context = IdPrefixContext::default();
    let index = context.populate(repo.as_ref()).unwrap();
    assert_eq!(
        index.shortest_commit_prefix_len(repo.as_ref(), commits[2].id()),
        2
    );
    assert_eq!(
        index.shortest_commit_prefix_len(repo.as_ref(), commits[5].id()),
        1
    );
    assert_eq!(
        index.resolve_commit_prefix(repo.as_ref(), &prefix("2")),
        AmbiguousMatch
    );
    assert_eq!(
        index.resolve_commit_prefix(repo.as_ref(), &prefix("2a")),
        SingleMatch(commits[2].id().clone())
    );
    assert_eq!(
        index.resolve_commit_prefix(repo.as_ref(), &prefix("20")),
        NoMatch
    );
    assert_eq!(
        index.resolve_commit_prefix(repo.as_ref(), &prefix("2a0")),
        NoMatch
    );
    assert_eq!(
        index.shortest_change_prefix_len(repo.as_ref(), commits[0].change_id()),
        2
    );
    assert_eq!(
        index.shortest_change_prefix_len(repo.as_ref(), commits[6].change_id()),
        1
    );
    assert_eq!(
        index.resolve_change_prefix(repo.as_ref(), &prefix("7")),
        AmbiguousMatch
    );
    assert_eq!(
        index.resolve_change_prefix(repo.as_ref(), &prefix("78")),
        SingleMatch(vec![commits[0].id().clone()])
    );
    assert_eq!(
        index.resolve_change_prefix(repo.as_ref(), &prefix("70")),
        NoMatch
    );
    assert_eq!(
        index.resolve_change_prefix(repo.as_ref(), &prefix("780")),
        NoMatch
    );

    // Disambiguate within a revset
    // ---------------------------------------------------------------------------------------------
    let expression =
        RevsetExpression::commits(vec![commits[0].id().clone(), commits[2].id().clone()]);
    let context = context.disambiguate_within(expression);
    let index = context.populate(repo.as_ref()).unwrap();
    // The prefix is now shorter
    assert_eq!(
        index.shortest_commit_prefix_len(repo.as_ref(), commits[2].id()),
        1
    );
    // Shorter prefix within the set can be used
    assert_eq!(
        index.resolve_commit_prefix(repo.as_ref(), &prefix("2")),
        SingleMatch(commits[2].id().clone())
    );
    // Can still resolve commits outside the set
    assert_eq!(
        index.resolve_commit_prefix(repo.as_ref(), &prefix("21")),
        SingleMatch(commits[24].id().clone())
    );
    assert_eq!(
        index.shortest_change_prefix_len(repo.as_ref(), commits[0].change_id()),
        1
    );
    assert_eq!(
        index.resolve_change_prefix(repo.as_ref(), &prefix("7")),
        SingleMatch(vec![commits[0].id().clone()])
    );

    // Single commit in revset. Length 0 is unambiguous, but we pretend 1 digit is
    // needed.
    // ---------------------------------------------------------------------------------------------
    let expression = RevsetExpression::commit(root_commit_id.clone());
    let context = context.disambiguate_within(expression);
    let index = context.populate(repo.as_ref()).unwrap();
    assert_eq!(
        index.shortest_commit_prefix_len(repo.as_ref(), root_commit_id),
        1
    );
    assert_eq!(
        index.resolve_commit_prefix(repo.as_ref(), &prefix("")),
        AmbiguousMatch
    );
    assert_eq!(
        index.resolve_commit_prefix(repo.as_ref(), &prefix("0")),
        SingleMatch(root_commit_id.clone())
    );
    assert_eq!(
        index.shortest_change_prefix_len(repo.as_ref(), root_change_id),
        1
    );
    assert_eq!(
        index.resolve_change_prefix(repo.as_ref(), &prefix("")),
        AmbiguousMatch
    );
    assert_eq!(
        index.resolve_change_prefix(repo.as_ref(), &prefix("0")),
        SingleMatch(vec![root_commit_id.clone()])
    );

    // Disambiguate within revset that fails to evaluate
    // ---------------------------------------------------------------------------------------------
    let expression = RevsetExpression::symbol("nonexistent".to_string());
    let context = context.disambiguate_within(expression);
    assert!(context.populate(repo.as_ref()).is_err());
}

#[test]
fn test_id_prefix_divergent() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init_with_backend(TestRepoBackend::Git);
    let repo = &test_repo.repo;
    let root_commit_id = repo.store().root_commit_id();

    let mut tx = repo.start_transaction(&settings);
    let mut create_commit_with_change_id =
        |parent_id: &CommitId, description: &str, change_id: ChangeId| {
            let signature = Signature {
                name: "Some One".to_string(),
                email: "some.one@example.com".to_string(),
                timestamp: Timestamp {
                    timestamp: MillisSinceEpoch(0),
                    tz_offset: 0,
                },
            };
            tx.repo_mut()
                .new_commit(
                    &settings,
                    vec![parent_id.clone()],
                    repo.store().empty_merged_tree_id(),
                )
                .set_description(description)
                .set_author(signature.clone())
                .set_committer(signature)
                .set_change_id(change_id)
                .write()
                .unwrap()
        };

    let first_change_id = ChangeId::from_hex("a5333333333333333333333333333333");
    let second_change_id = ChangeId::from_hex("a5000000000000000000000000000000");

    let first_commit = create_commit_with_change_id(root_commit_id, "first", first_change_id);
    let second_commit =
        create_commit_with_change_id(first_commit.id(), "second", second_change_id.clone());
    let third_commit_divergent_with_second =
        create_commit_with_change_id(first_commit.id(), "third", second_change_id);
    let commits = [
        first_commit.clone(),
        second_commit.clone(),
        third_commit_divergent_with_second.clone(),
    ];
    let repo = tx.commit("test").unwrap();

    // Print the commit IDs and change IDs for reference
    let change_prefixes = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| format!("{} {}", &commit.change_id().hex()[..4], i))
        .join("\n");
    insta::assert_snapshot!(change_prefixes, @r###"
    a533 0
    a500 1
    a500 2
    "###);
    let commit_prefixes = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| format!("{} {}", &commit.id().hex()[..4], i))
        .join("\n");
    insta::assert_snapshot!(commit_prefixes, @r###"
    eafa 0
    d48d 1
    2fbb 2
    "###);

    let prefix = |x| HexPrefix::new(x).unwrap();

    // Without a disambiguation revset
    // --------------------------------
    let context = IdPrefixContext::default();
    let index = context.populate(repo.as_ref()).unwrap();
    assert_eq!(
        index.shortest_change_prefix_len(repo.as_ref(), commits[0].change_id()),
        3
    );
    assert_eq!(
        index.shortest_change_prefix_len(repo.as_ref(), commits[1].change_id()),
        3
    );
    assert_eq!(
        index.shortest_change_prefix_len(repo.as_ref(), commits[2].change_id()),
        3
    );
    assert_eq!(
        index.resolve_change_prefix(repo.as_ref(), &prefix("a5")),
        AmbiguousMatch
    );
    assert_eq!(
        index.resolve_change_prefix(repo.as_ref(), &prefix("a53")),
        SingleMatch(vec![first_commit.id().clone()])
    );
    assert_eq!(
        index.resolve_change_prefix(repo.as_ref(), &prefix("a50")),
        SingleMatch(vec![
            second_commit.id().clone(),
            third_commit_divergent_with_second.id().clone()
        ])
    );

    // Now, disambiguate within the revset containing only the second commit
    // ----------------------------------------------------------------------
    let expression = RevsetExpression::commits(vec![second_commit.id().clone()]);
    let context = context.disambiguate_within(expression);
    let index = context.populate(repo.as_ref()).unwrap();
    // The prefix is now shorter
    assert_eq!(
        index.shortest_change_prefix_len(repo.as_ref(), second_commit.change_id()),
        1
    );
    // This tests two issues, both important:
    // - We find both commits with the same change id, even though
    // `third_commit_divergent_with_second` is not in the short prefix set
    // (#2476).
    // - The short prefix set still works: we do *not* find the first commit and the
    //   match is not ambiguous, even though the first commit's change id would also
    //   match the prefix.
    assert_eq!(
        index.resolve_change_prefix(repo.as_ref(), &prefix("a")),
        SingleMatch(vec![
            second_commit.id().clone(),
            third_commit_divergent_with_second.id().clone()
        ])
    );

    // We can still resolve commits outside the set
    assert_eq!(
        index.resolve_change_prefix(repo.as_ref(), &prefix("a53")),
        SingleMatch(vec![first_commit.id().clone()])
    );
    assert_eq!(
        index.shortest_change_prefix_len(repo.as_ref(), first_commit.change_id()),
        3
    );
}

#[test]
fn test_id_prefix_hidden() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init_with_backend(TestRepoBackend::Git);
    let repo = &test_repo.repo;
    let root_commit_id = repo.store().root_commit_id();

    let mut tx = repo.start_transaction(&settings);
    let mut commits = vec![];
    for i in 0..10 {
        let signature = Signature {
            name: "Some One".to_string(),
            email: "some.one@example.com".to_string(),
            timestamp: Timestamp {
                timestamp: MillisSinceEpoch(i),
                tz_offset: 0,
            },
        };
        let commit = tx
            .repo_mut()
            .new_commit(
                &settings,
                vec![root_commit_id.clone()],
                repo.store().empty_merged_tree_id(),
            )
            .set_author(signature.clone())
            .set_committer(signature)
            .write()
            .unwrap();
        commits.push(commit);
    }

    // Print the commit IDs and change IDs for reference
    let commit_prefixes = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| format!("{} {}", &commit.id().hex()[..3], i))
        .sorted()
        .join("\n");
    insta::assert_snapshot!(commit_prefixes, @r#"
    15e 9
    397 6
    53c 7
    62e 2
    648 8
    7c7 3
    853 4
    c0a 5
    ce9 0
    f10 1
    "#);
    let change_prefixes = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| format!("{} {}", &commit.change_id().hex()[..3], i))
        .sorted()
        .join("\n");
    insta::assert_snapshot!(change_prefixes, @r#"
    026 9
    1b5 6
    26b 3
    26c 8
    439 2
    781 0
    871 7
    896 5
    a2c 1
    b93 4
    "#);

    let hidden_commit = &commits[8];
    tx.repo_mut()
        .record_abandoned_commit(hidden_commit.id().clone());
    tx.repo_mut().rebase_descendants(&settings).unwrap();
    let repo = tx.commit("test").unwrap();

    let prefix = |x: &str| HexPrefix::new(x).unwrap();

    // Without a disambiguation revset
    // --------------------------------
    let context = IdPrefixContext::default();
    let index = context.populate(repo.as_ref()).unwrap();
    assert_eq!(
        index.shortest_commit_prefix_len(repo.as_ref(), hidden_commit.id()),
        2
    );
    assert_eq!(
        index.shortest_change_prefix_len(repo.as_ref(), hidden_commit.change_id()),
        3
    );
    assert_eq!(
        index.resolve_commit_prefix(repo.as_ref(), &prefix(&hidden_commit.id().hex()[..1])),
        AmbiguousMatch
    );
    assert_eq!(
        index.resolve_commit_prefix(repo.as_ref(), &prefix(&hidden_commit.id().hex()[..2])),
        SingleMatch(hidden_commit.id().clone())
    );
    assert_eq!(
        index.resolve_change_prefix(
            repo.as_ref(),
            &prefix(&hidden_commit.change_id().hex()[..2])
        ),
        AmbiguousMatch
    );
    assert_eq!(
        index.resolve_change_prefix(
            repo.as_ref(),
            &prefix(&hidden_commit.change_id().hex()[..3])
        ),
        NoMatch
    );

    // Disambiguate within hidden
    // --------------------------
    let expression = RevsetExpression::commit(hidden_commit.id().clone());
    let context = context.disambiguate_within(expression);
    let index = context.populate(repo.as_ref()).unwrap();
    assert_eq!(
        index.shortest_commit_prefix_len(repo.as_ref(), hidden_commit.id()),
        1
    );
    assert_eq!(
        index.shortest_change_prefix_len(repo.as_ref(), hidden_commit.change_id()),
        1
    );
    // Short commit id can be resolved even if it's hidden.
    assert_eq!(
        index.resolve_commit_prefix(repo.as_ref(), &prefix(&hidden_commit.id().hex()[..1])),
        SingleMatch(hidden_commit.id().clone())
    );
    // OTOH, hidden change id should never be found. The resolution might be
    // ambiguous if hidden commits were excluded from the disambiguation set.
    // In that case, shortest_change_prefix_len() shouldn't be 1.
    assert_eq!(
        index.resolve_change_prefix(
            repo.as_ref(),
            &prefix(&hidden_commit.change_id().hex()[..1])
        ),
        NoMatch
    );
    assert_eq!(
        index.resolve_change_prefix(
            repo.as_ref(),
            &prefix(&hidden_commit.change_id().hex()[..2])
        ),
        NoMatch
    );
}
