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

use itertools::Itertools;
use jj_lib::commit::Commit;
use jj_lib::default_index::revset_engine::{evaluate, RevsetImpl};
use jj_lib::default_index::DefaultReadonlyIndex;
use jj_lib::repo::{ReadonlyRepo, Repo as _};
use jj_lib::revset::ResolvedExpression;
use jj_lib::revset_graph::RevsetGraphEdge;
use test_case::test_case;
use testutils::{CommitGraphBuilder, TestRepo};

fn revset_for_commits(
    repo: &ReadonlyRepo,
    commits: &[&Commit],
) -> RevsetImpl<DefaultReadonlyIndex> {
    let index = repo
        .readonly_index()
        .as_any()
        .downcast_ref::<DefaultReadonlyIndex>()
        .unwrap();
    let expression =
        ResolvedExpression::Commits(commits.iter().map(|commit| commit.id().clone()).collect());
    evaluate(&expression, repo.store(), index.clone()).unwrap()
}

fn direct(commit: &Commit) -> RevsetGraphEdge {
    RevsetGraphEdge::direct(commit.id().clone())
}

fn indirect(commit: &Commit) -> RevsetGraphEdge {
    RevsetGraphEdge::indirect(commit.id().clone())
}

fn missing(commit: &Commit) -> RevsetGraphEdge {
    RevsetGraphEdge::missing(commit.id().clone())
}

#[test_case(false ; "keep transitive edges")]
#[test_case(true ; "skip transitive edges")]
fn test_graph_iterator_linearized(skip_transitive_edges: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Tests that a fork and a merge becomes a single edge:
    // D
    // |\        D
    // b c    => :
    // |/        A
    // A         ~
    // |
    // root
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let repo = tx.commit("test");
    let root_commit = repo.store().root_commit();

    let revset = revset_for_commits(repo.as_ref(), &[&commit_a, &commit_d]);
    let commits = revset.iter_graph_impl(skip_transitive_edges).collect_vec();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].0, *commit_d.id());
    assert_eq!(commits[1].0, *commit_a.id());
    assert_eq!(commits[0].1, vec![indirect(&commit_a)]);
    assert_eq!(commits[1].1, vec![missing(&root_commit)]);
}

#[test_case(false ; "keep transitive edges")]
#[test_case(true ; "skip transitive edges")]
fn test_graph_iterator_virtual_octopus(skip_transitive_edges: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Tests that merges outside the set can result in more parent edges than there
    // was in the input:
    //
    // F
    // |\
    // d e            F
    // |\|\      =>  /|\
    // A B C        A B C
    //  \|/         ~ ~ ~
    //  root
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.initial_commit();
    let commit_c = graph_builder.initial_commit();
    let commit_d = graph_builder.commit_with_parents(&[&commit_a, &commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d, &commit_e]);
    let repo = tx.commit("test");
    let root_commit = repo.store().root_commit();

    let revset = revset_for_commits(repo.as_ref(), &[&commit_a, &commit_b, &commit_c, &commit_f]);
    let commits = revset.iter_graph_impl(skip_transitive_edges).collect_vec();
    assert_eq!(commits.len(), 4);
    assert_eq!(commits[0].0, *commit_f.id());
    assert_eq!(commits[1].0, *commit_c.id());
    assert_eq!(commits[2].0, *commit_b.id());
    assert_eq!(commits[3].0, *commit_a.id());
    assert_eq!(
        commits[0].1,
        vec![
            indirect(&commit_a),
            indirect(&commit_b),
            indirect(&commit_c),
        ]
    );
    assert_eq!(commits[1].1, vec![missing(&root_commit)]);
    assert_eq!(commits[2].1, vec![missing(&root_commit)]);
    assert_eq!(commits[3].1, vec![missing(&root_commit)]);
}

#[test_case(false ; "keep transitive edges")]
#[test_case(true ; "skip transitive edges")]
fn test_graph_iterator_simple_fork(skip_transitive_edges: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Tests that the branch with "C" gets emitted correctly:
    // E
    // |
    // d
    // | C       E C
    // |/        |/
    // b     =>  A
    // |         ~
    // A
    // |
    // root
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_d]);
    let repo = tx.commit("test");
    let root_commit = repo.store().root_commit();

    let revset = revset_for_commits(repo.as_ref(), &[&commit_a, &commit_c, &commit_e]);
    let commits = revset.iter_graph_impl(skip_transitive_edges).collect_vec();
    assert_eq!(commits.len(), 3);
    assert_eq!(commits[0].0, *commit_e.id());
    assert_eq!(commits[1].0, *commit_c.id());
    assert_eq!(commits[2].0, *commit_a.id());
    assert_eq!(commits[0].1, vec![indirect(&commit_a)]);
    assert_eq!(commits[1].1, vec![indirect(&commit_a)]);
    assert_eq!(commits[2].1, vec![missing(&root_commit)]);
}

#[test_case(false ; "keep transitive edges")]
#[test_case(true ; "skip transitive edges")]
fn test_graph_iterator_multiple_missing(skip_transitive_edges: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Tests that we get missing edges to "a" and "c" and not just one missing edge
    // to the root.
    //   F
    //  / \        F
    // d   e   => /|\
    // |\ /|     ~ B ~
    // a B c       ~
    //  \|/
    //  root
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.initial_commit();
    let commit_c = graph_builder.initial_commit();
    let commit_d = graph_builder.commit_with_parents(&[&commit_a, &commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d, &commit_e]);
    let repo = tx.commit("test");
    let root_commit = repo.store().root_commit();

    let revset = revset_for_commits(repo.as_ref(), &[&commit_b, &commit_f]);
    let commits = revset.iter_graph_impl(skip_transitive_edges).collect_vec();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].0, *commit_f.id());
    assert_eq!(commits[1].0, *commit_b.id());
    assert_eq!(
        commits[0].1,
        vec![missing(&commit_a), indirect(&commit_b), missing(&commit_c)]
    );
    assert_eq!(commits[1].1, vec![missing(&root_commit)]);
}

#[test_case(false ; "keep transitive edges")]
#[test_case(true ; "skip transitive edges")]
fn test_graph_iterator_edge_to_ancestor(skip_transitive_edges: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Tests that we get both an edge from F to D and to D's ancestor C if we keep
    // transitive edges and only the edge from F to D if we skip transitive
    // edges:
    // F          F
    // |\         |\
    // D e        D :
    // |\|     => |\:
    // b C        ~ C
    //   |          ~
    //   a
    //   |
    //  root
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.initial_commit();
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d, &commit_e]);
    let repo = tx.commit("test");

    let revset = revset_for_commits(repo.as_ref(), &[&commit_c, &commit_d, &commit_f]);
    let commits = revset.iter_graph_impl(skip_transitive_edges).collect_vec();
    assert_eq!(commits.len(), 3);
    assert_eq!(commits[0].0, *commit_f.id());
    assert_eq!(commits[1].0, *commit_d.id());
    assert_eq!(commits[2].0, *commit_c.id());
    if skip_transitive_edges {
        assert_eq!(commits[0].1, vec![direct(&commit_d)]);
    } else {
        assert_eq!(commits[0].1, vec![direct(&commit_d), indirect(&commit_c),]);
    }
    assert_eq!(commits[1].1, vec![missing(&commit_b), direct(&commit_c)]);
    assert_eq!(commits[2].1, vec![missing(&commit_a)]);
}

#[test_case(false ; "keep transitive edges")]
#[test_case(true ; "skip transitive edges")]
fn test_graph_iterator_edge_escapes_from_(skip_transitive_edges: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Tests a more complex case for skipping transitive edges.
    //   J
    //  /|
    // | i                J
    // | |\              /:
    // | | H            | H
    // G | |            G :
    // | e f        =>  : D
    // |  \|\           :/
    // |   D |          A
    //  \ /  c          |
    //   b  /          root
    //   |/
    //   A
    //   |
    //  root
    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_d]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d, &commit_c]);
    let commit_g = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_h = graph_builder.commit_with_parents(&[&commit_f]);
    let commit_i = graph_builder.commit_with_parents(&[&commit_e, &commit_h]);
    let commit_j = graph_builder.commit_with_parents(&[&commit_g, &commit_i]);
    let repo = tx.commit("test");
    let root_commit = repo.store().root_commit();

    let revset = revset_for_commits(
        repo.as_ref(),
        &[&commit_a, &commit_d, &commit_g, &commit_h, &commit_j],
    );
    let commits = revset.iter_graph_impl(skip_transitive_edges).collect_vec();
    assert_eq!(commits.len(), 5);
    assert_eq!(commits[0].0, *commit_j.id());
    assert_eq!(commits[1].0, *commit_h.id());
    assert_eq!(commits[2].0, *commit_g.id());
    assert_eq!(commits[3].0, *commit_d.id());
    assert_eq!(commits[4].0, *commit_a.id());
    if skip_transitive_edges {
        assert_eq!(commits[0].1, vec![direct(&commit_g), indirect(&commit_h)]);
        assert_eq!(commits[1].1, vec![indirect(&commit_d)]);
    } else {
        assert_eq!(
            commits[0].1,
            vec![direct(&commit_g), indirect(&commit_d), indirect(&commit_h)]
        );
        assert_eq!(commits[1].1, vec![indirect(&commit_d), indirect(&commit_a)]);
    }
    assert_eq!(commits[2].1, vec![indirect(&commit_a)]);
    assert_eq!(commits[3].1, vec![indirect(&commit_a)]);
    assert_eq!(commits[4].1, vec![missing(&root_commit)]);
}
