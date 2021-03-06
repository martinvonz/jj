// Copyright 2020 Google LLC
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

use crate::commit::Commit;
use crate::commit_builder::CommitBuilder;
use crate::repo::RepoRef;
use crate::repo_path::DirRepoPath;
use crate::settings::UserSettings;
use crate::transaction::Transaction;
use crate::tree::Tree;
use crate::trees::merge_trees;

pub fn merge_commit_trees(repo: RepoRef, commits: &[Commit]) -> Tree {
    let store = repo.store();
    if commits.is_empty() {
        store
            .get_tree(&DirRepoPath::root(), store.empty_tree_id())
            .unwrap()
    } else {
        let index = repo.index();
        let mut new_tree = commits[0].tree();
        let commit_ids: Vec<_> = commits.iter().map(|commit| commit.id().clone()).collect();
        for (i, other_commit) in commits.iter().enumerate().skip(1) {
            let ancestors = index.common_ancestors(&commit_ids[0..i], &[commit_ids[i].clone()]);
            // TODO: Do recursive merge here instead of using just the first ancestor.
            let ancestor = store.get_commit(&ancestors[0]).unwrap();
            let new_tree_id =
                merge_trees(&new_tree, &ancestor.tree(), &other_commit.tree()).unwrap();
            new_tree = store.get_tree(&DirRepoPath::root(), &new_tree_id).unwrap();
        }
        new_tree
    }
}

pub fn rebase_commit(
    settings: &UserSettings,
    tx: &mut Transaction,
    old_commit: &Commit,
    new_parents: &[Commit],
) -> Commit {
    let store = tx.store();
    let old_base_tree = merge_commit_trees(tx.as_repo_ref(), &old_commit.parents());
    let new_base_tree = merge_commit_trees(tx.as_repo_ref(), &new_parents);
    // TODO: pass in labels for the merge parts
    let new_tree_id = merge_trees(&new_base_tree, &old_base_tree, &old_commit.tree()).unwrap();
    let new_parent_ids = new_parents
        .iter()
        .map(|commit| commit.id().clone())
        .collect();
    CommitBuilder::for_rewrite_from(settings, store, &old_commit)
        .set_parents(new_parent_ids)
        .set_tree(new_tree_id)
        .write_to_transaction(tx)
}

pub fn back_out_commit(
    settings: &UserSettings,
    tx: &mut Transaction,
    old_commit: &Commit,
    new_parents: &[Commit],
) -> Commit {
    let store = tx.store();
    let old_base_tree = merge_commit_trees(tx.as_repo_ref(), &old_commit.parents());
    let new_base_tree = merge_commit_trees(tx.as_repo_ref(), &new_parents);
    // TODO: pass in labels for the merge parts
    let new_tree_id = merge_trees(&new_base_tree, &old_commit.tree(), &old_base_tree).unwrap();
    let new_parent_ids = new_parents
        .iter()
        .map(|commit| commit.id().clone())
        .collect();
    // TODO: i18n the description based on repo language
    CommitBuilder::for_new_commit(settings, store, new_tree_id)
        .set_parents(new_parent_ids)
        .set_description(format!(
            "backout of commit {}",
            hex::encode(&old_commit.id().0)
        ))
        .write_to_transaction(tx)
}
