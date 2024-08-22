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

use std::fmt::Debug;
use std::io::Write as _;

use jj_lib::backend::TreeId;
use jj_lib::merged_tree::MergedTree;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPathBuf;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// List the recursive entries of a tree.
#[derive(clap::Args, Clone, Debug)]
pub struct DebugTreeArgs {
    #[arg(long, short = 'r')]
    revision: Option<RevisionArg>,
    #[arg(long, conflicts_with = "revision")]
    id: Option<String>,
    #[arg(long, requires = "id")]
    dir: Option<String>,
    paths: Vec<String>,
    // TODO: Add an option to include trees that are ancestors of the matched paths
}

pub fn cmd_debug_tree(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DebugTreeArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let tree = if let Some(tree_id_hex) = &args.id {
        let tree_id =
            TreeId::try_from_hex(tree_id_hex).map_err(|_| user_error("Invalid tree id"))?;
        let dir = if let Some(dir_str) = &args.dir {
            workspace_command.parse_file_path(dir_str)?
        } else {
            RepoPathBuf::root()
        };
        let store = workspace_command.repo().store();
        let tree = store.get_tree(&dir, &tree_id)?;
        MergedTree::resolved(tree)
    } else {
        let commit = workspace_command
            .resolve_single_rev(args.revision.as_ref().unwrap_or(&RevisionArg::AT))?;
        commit.tree()?
    };
    let matcher = workspace_command
        .parse_file_patterns(&args.paths)?
        .to_matcher();
    for (path, value) in tree.entries_matching(matcher.as_ref()) {
        let ui_path = workspace_command.format_file_path(&path);
        writeln!(ui.stdout(), "{ui_path}: {value:?}")?;
    }

    Ok(())
}
