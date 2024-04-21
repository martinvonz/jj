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

use std::collections::HashMap;

use jj_lib::repo::Repo;
use maplit::hashset;
use testutils::{CommitGraphBuilder, TestRepo};

// Simulate some `jj sync` command that rebases B:: onto G while abandoning C
// (because it's presumably already in G).
//
// G
// | E
// | D F
// | |/
// | C
// | B
// |/
// A
#[test]
fn test_transform_descendants_sync() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_b]);
    let commit_d = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_e = graph_builder.commit_with_parents(&[&commit_d]);
    let commit_f = graph_builder.commit_with_parents(&[&commit_c]);
    let commit_g = graph_builder.commit_with_parents(&[&commit_a]);

    let mut rebased = HashMap::new();
    tx.mut_repo()
        .transform_descendants(&settings, vec![commit_b.id().clone()], |mut rewriter| {
            rewriter.replace_parent(commit_a.id(), [commit_g.id()]);
            if *rewriter.old_commit() == commit_c {
                let old_id = rewriter.old_commit().id().clone();
                let new_parent_ids = rewriter.new_parents().to_vec();
                rewriter
                    .mut_repo()
                    .record_abandoned_commit_with_parents(old_id, new_parent_ids);
            } else {
                let old_commit_id = rewriter.old_commit().id().clone();
                let new_commit = rewriter.rebase(&settings)?.write()?;
                rebased.insert(old_commit_id, new_commit);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(rebased.len(), 4);
    let new_commit_b = rebased.get(commit_b.id()).unwrap();
    let new_commit_d = rebased.get(commit_d.id()).unwrap();
    let new_commit_e = rebased.get(commit_e.id()).unwrap();
    let new_commit_f = rebased.get(commit_f.id()).unwrap();

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            new_commit_e.id().clone(),
            new_commit_f.id().clone(),
        }
    );

    assert_eq!(new_commit_b.parent_ids(), vec![commit_g.id().clone()]);
    assert_eq!(new_commit_d.parent_ids(), vec![new_commit_b.id().clone()]);
    assert_eq!(new_commit_e.parent_ids(), vec![new_commit_d.id().clone()]);
    assert_eq!(new_commit_f.parent_ids(), vec![new_commit_b.id().clone()]);
}

// Transform just commit C replacing parent A by parent B. The parents should be
// deduplicated.
//
//   C
//  /|
// B |
// |/
// A
#[test]
fn test_transform_descendants_sync_linearize_merge() {
    let settings = testutils::user_settings();
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let mut tx = repo.start_transaction(&settings);
    let mut graph_builder = CommitGraphBuilder::new(&settings, tx.mut_repo());
    let commit_a = graph_builder.initial_commit();
    let commit_b = graph_builder.commit_with_parents(&[&commit_a]);
    let commit_c = graph_builder.commit_with_parents(&[&commit_a, &commit_b]);

    let mut rebased = HashMap::new();
    tx.mut_repo()
        .transform_descendants(&settings, vec![commit_c.id().clone()], |mut rewriter| {
            rewriter.replace_parent(commit_a.id(), [commit_b.id()]);
            let old_commit_id = rewriter.old_commit().id().clone();
            let new_commit = rewriter.rebase(&settings)?.write()?;
            rebased.insert(old_commit_id, new_commit);
            Ok(())
        })
        .unwrap();
    assert_eq!(rebased.len(), 1);
    let new_commit_c = rebased.get(commit_c.id()).unwrap();

    assert_eq!(
        *tx.mut_repo().view().heads(),
        hashset! {
            new_commit_c.id().clone(),
        }
    );

    assert_eq!(new_commit_c.parent_ids(), vec![commit_b.id().clone()]);
}
