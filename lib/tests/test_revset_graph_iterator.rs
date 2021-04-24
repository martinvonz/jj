// Copyright 2021 Google LLC
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

use jujube_lib::revset::revset_for_commits;
use jujube_lib::revset_graph_iterator::RevsetGraphEdge;
use jujube_lib::testutils;
use jujube_lib::testutils::CommitGraphBuilder;
use maplit::hashset;

#[test]
fn test_graph_iterator_linearized() {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, true);
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // Tests that a fork and a merge becomes a single edge:
    // D
    // |\        D
    // b c    => :
    // |/        A
    // A         ~
    // |
    // root
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let pos_root = mut_repo
        .index()
        .commit_id_to_pos(repo.store().root_commit_id())
        .unwrap();
    let pos_a = mut_repo.index().commit_id_to_pos(commit_a.id()).unwrap();

    let revset = revset_for_commits(mut_repo.as_repo_ref(), &[&commit_a, &commit_d]);
    let commits: Vec<_> = revset.iter().graph().collect();
    drop(revset);
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].0.commit_id(), *commit_d.id());
    assert_eq!(commits[1].0.commit_id(), *commit_a.id());
    assert_eq!(commits[0].1, hashset![RevsetGraphEdge::indirect(pos_a)]);
    assert_eq!(commits[1].1, hashset![RevsetGraphEdge::missing(pos_root)]);

    tx.discard();
}

#[test]
fn test_graph_iterator_virtual_octopus() {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, true);
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // Tests that merges outside the set can result in more parent edges than there
    // was in the input: F
    // |\
    // d e            F
    // |\|\      =>  /|\
    // A B C        A B C
    //  \|/         ~ ~ ~
    //  root
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.initial_commit();
    let commit_c = graph_builder.initial_commit();
    let commit_d = graph_builder.commit_with_parents(&[&commit_a, &commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d, &commit_e]);
    let pos_root = mut_repo
        .index()
        .commit_id_to_pos(repo.store().root_commit_id())
        .unwrap();
    let pos_a = mut_repo.index().commit_id_to_pos(commit_a.id()).unwrap();
    let pos_b = mut_repo.index().commit_id_to_pos(commit_b.id()).unwrap();
    let pos_c = mut_repo.index().commit_id_to_pos(commit_c.id()).unwrap();

    let revset = revset_for_commits(
        mut_repo.as_repo_ref(),
        &[&commit_a, &commit_b, &commit_c, &commit_f],
    );
    let commits: Vec<_> = revset.iter().graph().collect();
    drop(revset);
    assert_eq!(commits.len(), 4);
    assert_eq!(commits[0].0.commit_id(), *commit_f.id());
    assert_eq!(commits[1].0.commit_id(), *commit_c.id());
    assert_eq!(commits[2].0.commit_id(), *commit_b.id());
    assert_eq!(commits[3].0.commit_id(), *commit_a.id());
    assert_eq!(
        commits[0].1,
        hashset![
            RevsetGraphEdge::indirect(pos_a),
            RevsetGraphEdge::indirect(pos_b),
            RevsetGraphEdge::indirect(pos_c),
        ]
    );
    assert_eq!(commits[1].1, hashset![RevsetGraphEdge::missing(pos_root)]);
    assert_eq!(commits[2].1, hashset![RevsetGraphEdge::missing(pos_root)]);
    assert_eq!(commits[3].1, hashset![RevsetGraphEdge::missing(pos_root)]);

    tx.discard();
}

#[test]
fn test_graph_iterator_simple_fork() {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, true);
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

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
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_d]);
    let pos_root = mut_repo
        .index()
        .commit_id_to_pos(repo.store().root_commit_id())
        .unwrap();
    let pos_a = mut_repo.index().commit_id_to_pos(commit_a.id()).unwrap();

    let revset = revset_for_commits(mut_repo.as_repo_ref(), &[&commit_a, &commit_c, &commit_e]);
    let commits: Vec<_> = revset.iter().graph().collect();
    drop(revset);
    assert_eq!(commits.len(), 3);
    assert_eq!(commits[0].0.commit_id(), *commit_e.id());
    assert_eq!(commits[1].0.commit_id(), *commit_c.id());
    assert_eq!(commits[2].0.commit_id(), *commit_a.id());
    assert_eq!(commits[0].1, hashset![RevsetGraphEdge::indirect(pos_a)]);
    assert_eq!(commits[1].1, hashset![RevsetGraphEdge::indirect(pos_a)]);
    assert_eq!(commits[2].1, hashset![RevsetGraphEdge::missing(pos_root)]);

    tx.discard();
}

#[test]
fn test_graph_iterator_multiple_missing() {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, true);
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // Tests that we get missing edges to "a" and "c" and not just one missing edge
    // to the root.   F
    //  / \        F
    // d   e   => /|\
    // |\ /|     ~ B ~
    // a B c       ~
    //  \|/
    //  root
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.initial_commit();
    let commit_c = graph_builder.initial_commit();
    let commit_d = graph_builder.commit_with_parents(&[&commit_a, &commit_b]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d, &commit_e]);
    let pos_root = mut_repo
        .index()
        .commit_id_to_pos(repo.store().root_commit_id())
        .unwrap();
    let pos_a = mut_repo.index().commit_id_to_pos(commit_a.id()).unwrap();
    let pos_b = mut_repo.index().commit_id_to_pos(commit_b.id()).unwrap();
    let pos_c = mut_repo.index().commit_id_to_pos(commit_c.id()).unwrap();

    let revset = revset_for_commits(mut_repo.as_repo_ref(), &[&commit_b, &commit_f]);
    let commits: Vec<_> = revset.iter().graph().collect();
    drop(revset);
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].0.commit_id(), *commit_f.id());
    assert_eq!(commits[1].0.commit_id(), *commit_b.id());
    assert_eq!(
        commits[0].1,
        hashset![
            RevsetGraphEdge::missing(pos_a),
            RevsetGraphEdge::indirect(pos_b),
            RevsetGraphEdge::missing(pos_c),
        ]
    );
    assert_eq!(commits[1].1, hashset![RevsetGraphEdge::missing(pos_root)]);

    tx.discard();
}

#[test]
fn test_graph_iterator_edge_to_ancestor() {
    let settings = testutils::user_settings();
    let (_temp_dir, repo) = testutils::init_repo(&settings, true);
    let mut tx = repo.start_transaction("test");
    let mut_repo = tx.mut_repo();

    // Tests that we get both an edge from F to D and to D's ancestor C:
    // F          F
    // |\         |\
    // D e        D :
    // |\|     => |\:
    // b C        ~ C
    //   |          ~
    //   a
    //   |
    //  root
    let mut graph_builder = CommitGraphBuilder::new(&settings, mut_repo);
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.initial_commit();
    let commit_c = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_b, &commit_c]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_d, &commit_e]);
    let pos_a = mut_repo.index().commit_id_to_pos(commit_a.id()).unwrap();
    let pos_b = mut_repo.index().commit_id_to_pos(commit_b.id()).unwrap();
    let pos_c = mut_repo.index().commit_id_to_pos(commit_c.id()).unwrap();
    let pos_d = mut_repo.index().commit_id_to_pos(commit_d.id()).unwrap();

    let revset = revset_for_commits(mut_repo.as_repo_ref(), &[&commit_c, &commit_d, &commit_f]);
    let commits: Vec<_> = revset.iter().graph().collect();
    drop(revset);
    assert_eq!(commits.len(), 3);
    assert_eq!(commits[0].0.commit_id(), *commit_f.id());
    assert_eq!(commits[1].0.commit_id(), *commit_d.id());
    assert_eq!(commits[2].0.commit_id(), *commit_c.id());
    assert_eq!(
        commits[0].1,
        hashset![
            RevsetGraphEdge::indirect(pos_c),
            RevsetGraphEdge::direct(pos_d)
        ]
    );
    assert_eq!(
        commits[1].1,
        hashset![
            RevsetGraphEdge::missing(pos_b),
            RevsetGraphEdge::direct(pos_c)
        ]
    );
    assert_eq!(commits[2].1, hashset![RevsetGraphEdge::missing(pos_a)]);

    tx.discard();
}
