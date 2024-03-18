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

use std::collections::BTreeMap;
use std::io::Write;

use itertools::Itertools;
use jj_lib::backend::TreeValue;
use jj_lib::merge::MergedTreeValue;
use jj_lib::object_id::ObjectId;
use jj_lib::repo_path::RepoPathBuf;
use tracing::instrument;

use crate::cli_util::{CommandHelper, WorkspaceCommandHelper};
use crate::command_error::{cli_error, CommandError};
use crate::formatter::Formatter;
use crate::ui::Ui;

/// Resolve a conflicted file with an external merge tool
///
/// Only conflicts that can be resolved with a 3-way merge are supported. See
/// docs for merge tool configuration instructions.
///
/// Note that conflicts can also be resolved without using this command. You may
/// edit the conflict markers in the conflicted file directly with a text
/// editor.
//  TODOs:
//   - `jj resolve --editor` to resolve a conflict in the default text editor. Should work for
//     conflicts with 3+ adds. Useful to resolve conflicts in a commit other than the current one.
//   - A way to help split commits with conflicts that are too complicated (more than two sides)
//     into commits with simpler conflicts. In case of a tree with many merges, we could for example
//     point to existing commits with simpler conflicts where resolving those conflicts would help
//     simplify the present one.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct ResolveArgs {
    #[arg(long, short, default_value = "@")]
    revision: String,
    /// Instead of resolving one conflict, list all the conflicts
    // TODO: Also have a `--summary` option. `--list` currently acts like
    // `diff --summary`, but should be more verbose.
    #[arg(long, short)]
    list: bool,
    /// Do not print the list of remaining conflicts (if any) after resolving a
    /// conflict
    #[arg(long, short, conflicts_with = "list")]
    quiet: bool,
    /// Specify 3-way merge tool to be used
    #[arg(long, conflicts_with = "list", value_name = "NAME")]
    tool: Option<String>,
    /// Restrict to these paths when searching for a conflict to resolve. We
    /// will attempt to resolve the first conflict we can find. You can use
    /// the `--list` argument to find paths to use here.
    // TODO: Find the conflict we can resolve even if it's not the first one.
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) fn cmd_resolve(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ResolveArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    let tree = commit.tree()?;
    let conflicts = tree
        .conflicts()
        .filter(|path| matcher.matches(&path.0))
        .collect_vec();
    if conflicts.is_empty() {
        return Err(cli_error(if args.paths.is_empty() {
            "No conflicts found at this revision"
        } else {
            "No conflicts found at the given path(s)"
        }));
    }
    if args.list {
        return print_conflicted_paths(
            &conflicts,
            ui.stdout_formatter().as_mut(),
            &workspace_command,
        );
    };

    let (repo_path, _) = conflicts.first().unwrap();
    workspace_command.check_rewritable([&commit])?;
    let merge_editor = workspace_command.merge_editor(ui, args.tool.as_deref())?;
    writeln!(
        ui.stderr(),
        "Resolving conflicts in: {}",
        workspace_command.format_file_path(repo_path)
    )?;
    let mut tx = workspace_command.start_transaction();
    let new_tree_id = merge_editor.edit_file(&tree, repo_path)?;
    let new_commit = tx
        .mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .set_tree_id(new_tree_id)
        .write()?;
    tx.finish(
        ui,
        format!("Resolve conflicts in commit {}", commit.id().hex()),
    )?;

    if !args.quiet {
        let new_tree = new_commit.tree()?;
        let new_conflicts = new_tree.conflicts().collect_vec();
        if !new_conflicts.is_empty() {
            writeln!(
                ui.stderr(),
                "After this operation, some files at this revision still have conflicts:"
            )?;
            print_conflicted_paths(
                &new_conflicts,
                ui.stderr_formatter().as_mut(),
                &workspace_command,
            )?;
        }
    };
    Ok(())
}

#[instrument(skip_all)]
pub(crate) fn print_conflicted_paths(
    conflicts: &[(RepoPathBuf, MergedTreeValue)],
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
) -> Result<(), CommandError> {
    let formatted_paths = conflicts
        .iter()
        .map(|(path, _conflict)| workspace_command.format_file_path(path))
        .collect_vec();
    let max_path_len = formatted_paths.iter().map(|p| p.len()).max().unwrap_or(0);
    let formatted_paths = formatted_paths
        .into_iter()
        .map(|p| format!("{:width$}", p, width = max_path_len.min(32) + 3));

    for ((_, conflict), formatted_path) in std::iter::zip(conflicts.iter(), formatted_paths) {
        let sides = conflict.num_sides();
        let n_adds = conflict.adds().flatten().count();
        let deletions = sides - n_adds;

        let mut seen_objects = BTreeMap::new(); // Sort for consistency and easier testing
        if deletions > 0 {
            seen_objects.insert(
                format!(
                    // Starting with a number sorts this first
                    "{deletions} deletion{}",
                    if deletions > 1 { "s" } else { "" }
                ),
                "normal", // Deletions don't interfere with `jj resolve` or diff display
            );
        }
        // TODO: We might decide it's OK for `jj resolve` to ignore special files in the
        // `removes` of a conflict (see e.g. https://github.com/martinvonz/jj/pull/978). In
        // that case, `conflict.removes` should be removed below.
        for term in itertools::chain(conflict.removes(), conflict.adds()).flatten() {
            seen_objects.insert(
                match term {
                    TreeValue::File {
                        executable: false, ..
                    } => continue,
                    TreeValue::File {
                        executable: true, ..
                    } => "an executable",
                    TreeValue::Symlink(_) => "a symlink",
                    TreeValue::Tree(_) => "a directory",
                    TreeValue::GitSubmodule(_) => "a git submodule",
                    TreeValue::Conflict(_) => "another conflict (you found a bug!)",
                }
                .to_string(),
                "difficult",
            );
        }

        write!(formatter, "{formatted_path} ",)?;
        formatter.with_label("conflict_description", |formatter| {
            let print_pair = |formatter: &mut dyn Formatter, (text, label): &(String, &str)| {
                write!(formatter.labeled(label), "{text}")
            };
            print_pair(
                formatter,
                &(
                    format!("{sides}-sided"),
                    if sides > 2 { "difficult" } else { "normal" },
                ),
            )?;
            write!(formatter, " conflict")?;

            if !seen_objects.is_empty() {
                write!(formatter, " including ")?;
                let seen_objects = seen_objects.into_iter().collect_vec();
                match &seen_objects[..] {
                    [] => unreachable!(),
                    [only] => print_pair(formatter, only)?,
                    [first, middle @ .., last] => {
                        print_pair(formatter, first)?;
                        for pair in middle {
                            write!(formatter, ", ")?;
                            print_pair(formatter, pair)?;
                        }
                        write!(formatter, " and ")?;
                        print_pair(formatter, last)?;
                    }
                };
            }
            Ok(())
        })?;
        writeln!(formatter)?;
    }
    Ok(())
}
