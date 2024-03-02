// Copyright 2020 The Jujutsu Authors
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
use jj_lib::backend::TreeValue;
use jj_lib::merged_tree::MergedTreeBuilder;
use jj_lib::object_id::ObjectId;
use tracing::instrument;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
enum ChmodMode {
    /// Make a path non-executable (alias: normal)
    // We use short names for enum values so that errors say that the possible values are `n, x`.
    #[value(name = "n", alias("normal"))]
    Normal,
    /// Make a path executable (alias: executable)
    #[value(name = "x", alias("executable"))]
    Executable,
}

/// Sets or removes the executable bit for paths in the repo
///
/// Unlike the POSIX `chmod`, `jj chmod` also works on Windows, on conflicted
/// files, and on arbitrary revisions.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct ChmodArgs {
    mode: ChmodMode,
    /// The revision to update
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Paths to change the executable bit for
    #[arg(required = true, value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_chmod(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ChmodArgs,
) -> Result<(), CommandError> {
    let executable_bit = match args.mode {
        ChmodMode::Executable => true,
        ChmodMode::Normal => false,
    };

    let mut workspace_command = command.workspace_helper(ui)?;
    let repo_paths: Vec<_> = args
        .paths
        .iter()
        .map(|path| workspace_command.parse_file_path(path))
        .try_collect()?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewritable([&commit])?;

    let mut tx = workspace_command.start_transaction();
    let tree = commit.tree()?;
    let store = tree.store();
    let mut tree_builder = MergedTreeBuilder::new(commit.tree_id().clone());
    for repo_path in repo_paths {
        let user_error_with_path = |msg: &str| {
            user_error(format!(
                "{msg} at '{}'.",
                tx.base_workspace_helper().format_file_path(&repo_path)
            ))
        };
        let tree_value = tree.path_value(&repo_path);
        if tree_value.is_absent() {
            return Err(user_error_with_path("No such path"));
        }
        let all_files = tree_value
            .adds()
            .flatten()
            .all(|tree_value| matches!(tree_value, TreeValue::File { .. }));
        if !all_files {
            let message = if tree_value.is_resolved() {
                "Found neither a file nor a conflict"
            } else {
                "Some of the sides of the conflict are not files"
            };
            return Err(user_error_with_path(message));
        }
        let new_tree_value = tree_value.map(|value| match value {
            Some(TreeValue::File { id, executable: _ }) => Some(TreeValue::File {
                id: id.clone(),
                executable: executable_bit,
            }),
            Some(TreeValue::Conflict(_)) => {
                panic!("Conflict sides must not themselves be conflicts")
            }
            value => value.clone(),
        });
        tree_builder.set_or_remove(repo_path, new_tree_value);
    }

    let new_tree_id = tree_builder.write_tree(store)?;
    tx.mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .set_tree_id(new_tree_id)
        .write()?;
    tx.finish(
        ui,
        format!(
            "make paths {} in commit {}",
            if executable_bit {
                "executable"
            } else {
                "non-executable"
            },
            commit.id().hex(),
        ),
    )
}
