use std::path::PathBuf;

#[test]
fn test_no_forgotten_test_files() {
    let test_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    testutils::assert_no_forgotten_test_files(&test_dir);
}

mod test_bad_locking;
mod test_commit_builder;
mod test_commit_concurrent;
mod test_conflicts;
mod test_default_revset_graph_iterator;
mod test_diff_summary;
mod test_git;
mod test_git_backend;
mod test_gpg;
mod test_id_prefix;
mod test_index;
mod test_init;
mod test_load_repo;
mod test_local_working_copy;
mod test_local_working_copy_concurrent;
mod test_local_working_copy_sparse;
mod test_merge_trees;
mod test_merged_tree;
mod test_mut_repo;
mod test_operations;
mod test_refs;
mod test_revset;
mod test_rewrite;
mod test_signing;
mod test_ssh_signing;
mod test_view;
mod test_workspace;
