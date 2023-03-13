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
use jujutsu_lib::default_revset_engine::revset_for_commits;
use jujutsu_lib::repo::Repo;
use jujutsu_lib::revset::RevsetGraphEdge;
use jujutsu_lib::revset_graph_iterator::{ReverseRevsetGraphIterator, RevsetGraphIterator};
use test_case::test_case;
use testutils::{CommitGraphBuilder, TestRepo};

#[test_case(false ; "keep transitive edges")]
#[test_case(true ; "skip transitive edges")]
fn test_graph_iterator_linearized(skip_transitive_edges: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;

    // Tests that a fork and a merge becomes a single edge:
    // D
    // |\        D
    // b c    => :
    // |/        A
    // A         ~
    // |
    // root
    let mut tx = repo.start_transaction(&settings, "test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let repo = tx.commit();

    let pos_root = repo
        .index()
        .commit_id_to_pos(repo.store().root_commit_id())
        .unwrap();
    let pos_a = repo.index().commit_id_to_pos(commit_a.id()).unwrap();

    let revset = revset_for_commits(&repo, &[&commit_a, &commit_d]);
    let commits = RevsetGraphIterator::new(revset.as_ref())
        .set_skip_transitive_edges(skip_transitive_edges)
        .collect_vec();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].0.commit_id(), *commit_d.id());
    assert_eq!(commits[1].0.commit_id(), *commit_a.id());
    assert_eq!(commits[0].1, vec![RevsetGraphEdge::indirect(pos_a)]);
    assert_eq!(commits[1].1, vec![RevsetGraphEdge::missing(pos_root)]);
}

#[test_case(false ; "keep transitive edges")]
#[test_case(true ; "skip transitive edges")]
fn test_graph_iterator_virtual_octopus(skip_transitive_edges: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(true);
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
    let mut tx = repo.start_transaction(&settings, "test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.initial_commit();
    let commit_c = graph_builder.initial_commit();
    let commit_d = graph_builder.commit_with_parents(&[&commit_a, &commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d, &commit_e]);
    let repo = tx.commit();

    let pos_root = repo
        .index()
        .commit_id_to_pos(repo.store().root_commit_id())
        .unwrap();
    let pos_a = repo.index().commit_id_to_pos(commit_a.id()).unwrap();
    let pos_b = repo.index().commit_id_to_pos(commit_b.id()).unwrap();
    let pos_c = repo.index().commit_id_to_pos(commit_c.id()).unwrap();

    let revset = revset_for_commits(&repo, &[&commit_a, &commit_b, &commit_c, &commit_f]);
    let commits = RevsetGraphIterator::new(revset.as_ref())
        .set_skip_transitive_edges(skip_transitive_edges)
        .collect_vec();
    assert_eq!(commits.len(), 4);
    assert_eq!(commits[0].0.commit_id(), *commit_f.id());
    assert_eq!(commits[1].0.commit_id(), *commit_c.id());
    assert_eq!(commits[2].0.commit_id(), *commit_b.id());
    assert_eq!(commits[3].0.commit_id(), *commit_a.id());
    assert_eq!(
        commits[0].1,
        vec![
            RevsetGraphEdge::indirect(pos_c),
            RevsetGraphEdge::indirect(pos_b),
            RevsetGraphEdge::indirect(pos_a),
        ]
    );
    assert_eq!(commits[1].1, vec![RevsetGraphEdge::missing(pos_root)]);
    assert_eq!(commits[2].1, vec![RevsetGraphEdge::missing(pos_root)]);
    assert_eq!(commits[3].1, vec![RevsetGraphEdge::missing(pos_root)]);
}

#[test_case(false ; "keep transitive edges")]
#[test_case(true ; "skip transitive edges")]
fn test_graph_iterator_simple_fork(skip_transitive_edges: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(true);
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
    let mut tx = repo.start_transaction(&settings, "test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_d]);
    let repo = tx.commit();

    let pos_root = repo
        .index()
        .commit_id_to_pos(repo.store().root_commit_id())
        .unwrap();
    let pos_a = repo.index().commit_id_to_pos(commit_a.id()).unwrap();

    let revset = revset_for_commits(&repo, &[&commit_a, &commit_c, &commit_e]);
    let commits = RevsetGraphIterator::new(revset.as_ref())
        .set_skip_transitive_edges(skip_transitive_edges)
        .collect_vec();
    assert_eq!(commits.len(), 3);
    assert_eq!(commits[0].0.commit_id(), *commit_e.id());
    assert_eq!(commits[1].0.commit_id(), *commit_c.id());
    assert_eq!(commits[2].0.commit_id(), *commit_a.id());
    assert_eq!(commits[0].1, vec![RevsetGraphEdge::indirect(pos_a)]);
    assert_eq!(commits[1].1, vec![RevsetGraphEdge::indirect(pos_a)]);
    assert_eq!(commits[2].1, vec![RevsetGraphEdge::missing(pos_root)]);
}

#[test_case(false ; "keep transitive edges")]
#[test_case(true ; "skip transitive edges")]
fn test_graph_iterator_multiple_missing(skip_transitive_edges: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(true);
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
    let mut tx = repo.start_transaction(&settings, "test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.initial_commit();
    let commit_c = graph_builder.initial_commit();
    let commit_d = graph_builder.commit_with_parents(&[&commit_a, &commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d, &commit_e]);
    let repo = tx.commit();

    let pos_root = repo
        .index()
        .commit_id_to_pos(repo.store().root_commit_id())
        .unwrap();
    let pos_a = repo.index().commit_id_to_pos(commit_a.id()).unwrap();
    let pos_b = repo.index().commit_id_to_pos(commit_b.id()).unwrap();
    let pos_c = repo.index().commit_id_to_pos(commit_c.id()).unwrap();

    let revset = revset_for_commits(&repo, &[&commit_b, &commit_f]);
    let commits = RevsetGraphIterator::new(revset.as_ref())
        .set_skip_transitive_edges(skip_transitive_edges)
        .collect_vec();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].0.commit_id(), *commit_f.id());
    assert_eq!(commits[1].0.commit_id(), *commit_b.id());
    assert_eq!(
        commits[0].1,
        vec![
            RevsetGraphEdge::missing(pos_c),
            RevsetGraphEdge::indirect(pos_b),
            RevsetGraphEdge::missing(pos_a),
        ]
    );
    assert_eq!(commits[1].1, vec![RevsetGraphEdge::missing(pos_root)]);
}

#[test_case(false ; "keep transitive edges")]
#[test_case(true ; "skip transitive edges")]
fn test_graph_iterator_edge_to_ancestor(skip_transitive_edges: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(true);
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
    let mut tx = repo.start_transaction(&settings, "test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.initial_commit();
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d, &commit_e]);
    let repo = tx.commit();

    let pos_a = repo.index().commit_id_to_pos(commit_a.id()).unwrap();
    let pos_b = repo.index().commit_id_to_pos(commit_b.id()).unwrap();
    let pos_c = repo.index().commit_id_to_pos(commit_c.id()).unwrap();
    let pos_d = repo.index().commit_id_to_pos(commit_d.id()).unwrap();

    let revset = revset_for_commits(&repo, &[&commit_c, &commit_d, &commit_f]);
    let commits = RevsetGraphIterator::new(revset.as_ref())
        .set_skip_transitive_edges(skip_transitive_edges)
        .collect_vec();
    assert_eq!(commits.len(), 3);
    assert_eq!(commits[0].0.commit_id(), *commit_f.id());
    assert_eq!(commits[1].0.commit_id(), *commit_d.id());
    assert_eq!(commits[2].0.commit_id(), *commit_c.id());
    if skip_transitive_edges {
        assert_eq!(commits[0].1, vec![RevsetGraphEdge::direct(pos_d)]);
    } else {
        assert_eq!(
            commits[0].1,
            vec![
                RevsetGraphEdge::direct(pos_d),
                RevsetGraphEdge::indirect(pos_c),
            ]
        );
    }
    assert_eq!(
        commits[1].1,
        vec![
            RevsetGraphEdge::direct(pos_c),
            RevsetGraphEdge::missing(pos_b),
        ]
    );
    assert_eq!(commits[2].1, vec![RevsetGraphEdge::missing(pos_a)]);
}

#[test_case(false ; "keep transitive edges")]
#[test_case(true ; "skip transitive edges")]
fn test_graph_iterator_edge_escapes_from_(skip_transitive_edges: bool) {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(true);
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
    let mut tx = repo.start_transaction(&settings, "test");
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
    let repo = tx.commit();

    let pos_root = repo
        .index()
        .commit_id_to_pos(repo.store().root_commit_id())
        .unwrap();
    let pos_a = repo.index().commit_id_to_pos(commit_a.id()).unwrap();
    let pos_d = repo.index().commit_id_to_pos(commit_d.id()).unwrap();
    let pos_g = repo.index().commit_id_to_pos(commit_g.id()).unwrap();
    let pos_h = repo.index().commit_id_to_pos(commit_h.id()).unwrap();

    let revset = revset_for_commits(
        &repo,
        &[&commit_a, &commit_d, &commit_g, &commit_h, &commit_j],
    );
    let commits = RevsetGraphIterator::new(revset.as_ref())
        .set_skip_transitive_edges(skip_transitive_edges)
        .collect_vec();
    assert_eq!(commits.len(), 5);
    assert_eq!(commits[0].0.commit_id(), *commit_j.id());
    assert_eq!(commits[1].0.commit_id(), *commit_h.id());
    assert_eq!(commits[2].0.commit_id(), *commit_g.id());
    assert_eq!(commits[3].0.commit_id(), *commit_d.id());
    assert_eq!(commits[4].0.commit_id(), *commit_a.id());
    if skip_transitive_edges {
        assert_eq!(
            commits[0].1,
            vec![
                RevsetGraphEdge::indirect(pos_h),
                RevsetGraphEdge::direct(pos_g)
            ]
        );
        assert_eq!(commits[1].1, vec![RevsetGraphEdge::indirect(pos_d)]);
    } else {
        assert_eq!(
            commits[0].1,
            vec![
                RevsetGraphEdge::indirect(pos_h),
                RevsetGraphEdge::direct(pos_g),
                RevsetGraphEdge::indirect(pos_d),
            ]
        );
        assert_eq!(
            commits[1].1,
            vec![
                RevsetGraphEdge::indirect(pos_d),
                RevsetGraphEdge::indirect(pos_a)
            ]
        );
    }
    assert_eq!(commits[2].1, vec![RevsetGraphEdge::indirect(pos_a)]);
    assert_eq!(commits[3].1, vec![RevsetGraphEdge::indirect(pos_a)]);
    assert_eq!(commits[4].1, vec![RevsetGraphEdge::missing(pos_root)]);
}

#[test]
fn test_reverse_graph_iterator() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init(true);
    let repo = &test_repo.repo;

    // Tests that merges, forks, direct edges, indirect edges, and "missing" edges
    // are correct in reversed graph. "Missing" edges (i.e. edges to commits not
    // in the input set) won't be part of the reversed graph. Conversely, there
    // won't be missing edges to children not in the input.
    //
    //  F
    //  |\
    //  D E
    //  |/
    //  C
    //  |
    //  b
    //  |
    //  A
    //  |
    // root
    let mut tx = repo.start_transaction(&settings, "test");
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d, &commit_e]);
    let repo = tx.commit();

    let pos_c = repo.index().commit_id_to_pos(commit_c.id()).unwrap();
    let pos_d = repo.index().commit_id_to_pos(commit_d.id()).unwrap();
    let pos_e = repo.index().commit_id_to_pos(commit_e.id()).unwrap();
    let pos_f = repo.index().commit_id_to_pos(commit_f.id()).unwrap();

    let revset = revset_for_commits(
        &repo,
        &[&commit_a, &commit_c, &commit_d, &commit_e, &commit_f],
    );
    let commits = ReverseRevsetGraphIterator::new(revset.iter_graph()).collect_vec();
    assert_eq!(commits.len(), 5);
    assert_eq!(commits[0].0.commit_id(), *commit_a.id());
    assert_eq!(commits[1].0.commit_id(), *commit_c.id());
    assert_eq!(commits[2].0.commit_id(), *commit_d.id());
    assert_eq!(commits[3].0.commit_id(), *commit_e.id());
    assert_eq!(commits[4].0.commit_id(), *commit_f.id());
    assert_eq!(commits[0].1, vec![RevsetGraphEdge::indirect(pos_c)]);
    assert_eq!(
        commits[1].1,
        vec![
            RevsetGraphEdge::direct(pos_e),
            RevsetGraphEdge::direct(pos_d),
        ]
    );
    assert_eq!(commits[2].1, vec![RevsetGraphEdge::direct(pos_f)]);
    assert_eq!(commits[3].1, vec![RevsetGraphEdge::direct(pos_f)]);
    assert_eq!(commits[4].1, vec![]);
}
