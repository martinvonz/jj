use std::path::PathBuf;

mod common;

#[test]
fn test_no_forgotten_test_files() {
    let test_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    testutils::assert_no_forgotten_test_files(&test_dir);
}

mod test_abandon_command;
mod test_alias;
mod test_branch_command;
mod test_builtin_aliases;
mod test_cat_command;
mod test_checkout;
mod test_chmod_command;
mod test_commit_command;
mod test_commit_template;
mod test_concurrent_operations;
mod test_config_command;
mod test_debug_command;
mod test_describe_command;
mod test_diff_command;
mod test_diffedit_command;
mod test_duplicate_command;
mod test_edit_command;
mod test_generate_md_cli_help;
mod test_git_clone;
mod test_git_colocated;
mod test_git_fetch;
mod test_git_import_export;
mod test_git_init;
mod test_git_push;
mod test_git_remotes;
mod test_git_submodule;
mod test_gitignores;
mod test_global_opts;
mod test_immutable_commits;
mod test_init_command;
mod test_interdiff_command;
mod test_log_command;
mod test_move_command;
mod test_new_command;
mod test_next_prev_commands;
mod test_obslog_command;
mod test_operations;
mod test_rebase_command;
mod test_repo_change_report;
mod test_resolve_command;
mod test_restore_command;
mod test_revset_output;
mod test_root;
mod test_shell_completion;
mod test_show_command;
mod test_sparse_command;
mod test_split_command;
mod test_squash_command;
mod test_status_command;
mod test_tag_command;
mod test_templater;
mod test_tree_level_conflicts;
mod test_undo;
mod test_unsquash_command;
mod test_untrack_command;
mod test_util_command;
mod test_working_copy;
mod test_workspaces;
