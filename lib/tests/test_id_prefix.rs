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
use jj_lib::backend::{CommitId, MillisSinceEpoch, Signature, Timestamp};
use jj_lib::id_prefix::IdPrefixContext;
use jj_lib::object_id::PrefixResolution::{AmbiguousMatch, NoMatch, SingleMatch};
use jj_lib::object_id::{HexPrefix, ObjectId};
use jj_lib::repo::Repo;
use jj_lib::revset::RevsetExpression;
use testutils::{TestRepo, TestRepoBackend};

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
        tx.mut_repo()
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
    let repo = tx.commit("test");

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
    let c = IdPrefixContext::default();
    assert_eq!(
        c.shortest_commit_prefix_len(repo.as_ref(), commits[2].id()),
        2
    );
    assert_eq!(
        c.shortest_commit_prefix_len(repo.as_ref(), commits[5].id()),
        1
    );
    assert_eq!(
        c.resolve_commit_prefix(repo.as_ref(), &prefix("2")),
        AmbiguousMatch
    );
    assert_eq!(
        c.resolve_commit_prefix(repo.as_ref(), &prefix("2a")),
        SingleMatch(commits[2].id().clone())
    );
    assert_eq!(
        c.resolve_commit_prefix(repo.as_ref(), &prefix("20")),
        NoMatch
    );
    assert_eq!(
        c.resolve_commit_prefix(repo.as_ref(), &prefix("2a0")),
        NoMatch
    );
    assert_eq!(
        c.shortest_change_prefix_len(repo.as_ref(), commits[0].change_id()),
        2
    );
    assert_eq!(
        c.shortest_change_prefix_len(repo.as_ref(), commits[6].change_id()),
        1
    );
    assert_eq!(
        c.resolve_change_prefix(repo.as_ref(), &prefix("7")),
        AmbiguousMatch
    );
    assert_eq!(
        c.resolve_change_prefix(repo.as_ref(), &prefix("78")),
        SingleMatch(vec![commits[0].id().clone()])
    );
    assert_eq!(
        c.resolve_change_prefix(repo.as_ref(), &prefix("70")),
        NoMatch
    );
    assert_eq!(
        c.resolve_change_prefix(repo.as_ref(), &prefix("780")),
        NoMatch
    );

    // Disambiguate within a revset
    // ---------------------------------------------------------------------------------------------
    let expression =
        RevsetExpression::commits(vec![commits[0].id().clone(), commits[2].id().clone()]);
    let c = c.disambiguate_within(expression);
    // The prefix is now shorter
    assert_eq!(
        c.shortest_commit_prefix_len(repo.as_ref(), commits[2].id()),
        1
    );
    // Shorter prefix within the set can be used
    assert_eq!(
        c.resolve_commit_prefix(repo.as_ref(), &prefix("2")),
        SingleMatch(commits[2].id().clone())
    );
    // Can still resolve commits outside the set
    assert_eq!(
        c.resolve_commit_prefix(repo.as_ref(), &prefix("21")),
        SingleMatch(commits[24].id().clone())
    );
    assert_eq!(
        c.shortest_change_prefix_len(repo.as_ref(), commits[0].change_id()),
        1
    );
    assert_eq!(
        c.resolve_change_prefix(repo.as_ref(), &prefix("7")),
        SingleMatch(vec![commits[0].id().clone()])
    );

    // Single commit in revset. Length 0 is unambiguous, but we pretend 1 digit is
    // needed.
    // ---------------------------------------------------------------------------------------------
    let expression = RevsetExpression::commit(root_commit_id.clone());
    let c = c.disambiguate_within(expression);
    assert_eq!(
        c.shortest_commit_prefix_len(repo.as_ref(), root_commit_id),
        1
    );
    assert_eq!(
        c.resolve_commit_prefix(repo.as_ref(), &prefix("")),
        AmbiguousMatch
    );
    assert_eq!(
        c.resolve_commit_prefix(repo.as_ref(), &prefix("0")),
        SingleMatch(root_commit_id.clone())
    );
    assert_eq!(
        c.shortest_change_prefix_len(repo.as_ref(), root_change_id),
        1
    );
    assert_eq!(
        c.resolve_change_prefix(repo.as_ref(), &prefix("")),
        AmbiguousMatch
    );
    assert_eq!(
        c.resolve_change_prefix(repo.as_ref(), &prefix("0")),
        SingleMatch(vec![root_commit_id.clone()])
    );

    // Disambiguate within revset that fails to evaluate
    // ---------------------------------------------------------------------------------------------
    // TODO: Should be an error
    let expression = RevsetExpression::symbol("nonexistent".to_string());
    let context = c.disambiguate_within(expression);
    assert_eq!(
        context.shortest_commit_prefix_len(repo.as_ref(), commits[2].id()),
        2
    );
    assert_eq!(
        context.resolve_commit_prefix(repo.as_ref(), &prefix("2")),
        AmbiguousMatch
    );
}
