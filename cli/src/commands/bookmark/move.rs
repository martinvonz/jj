// Copyright 2020-2023 The Jujutsu Authors
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

use itertools::Itertools as _;
use jj_lib::backend::CommitId;
use jj_lib::object_id::ObjectId as _;
use jj_lib::op_store::RefTarget;
use jj_lib::str_util::StringPattern;

use super::find_bookmarks_with;
use super::is_fast_forward;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::user_error_with_hint;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Move existing bookmarks to target revision
///
/// If bookmark names are given, the specified bookmarks will be updated to
/// point to the target revision.
///
/// If `--from` options are given, bookmarks currently pointing to the
/// specified revisions will be updated. The bookmarks can also be filtered by
/// names.
///
/// Example: pull up the nearest bookmarks to the working-copy parent
///
/// $ jj bookmark move --from 'heads(::@- & bookmarks())' --to @-
#[derive(clap::Args, Clone, Debug)]
#[command(group(clap::ArgGroup::new("source").multiple(true).required(true)))]
pub struct BookmarkMoveArgs {
    /// Move bookmarks from the given revisions
    #[arg(long, group = "source", value_name = "REVISIONS")]
    from: Vec<RevisionArg>,

    /// Move bookmarks to this revision
    #[arg(long, default_value = "@", value_name = "REVISION")]
    to: RevisionArg,

    /// Allow moving bookmarks backwards or sideways
    #[arg(long, short = 'B')]
    allow_backwards: bool,

    /// Move bookmarks matching the given name patterns
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select bookmarks by wildcard pattern. For details, see
    /// https://martinvonz.github.io/jj/latest/revsets/#string-patterns.
    #[arg(group = "source", value_parser = StringPattern::parse)]
    names: Vec<StringPattern>,
}

pub fn cmd_bookmark_move(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BookmarkMoveArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo().clone();

    let target_commit = workspace_command.resolve_single_rev(&args.to)?;
    let matched_bookmarks = {
        let is_source_commit = if !args.from.is_empty() {
            workspace_command
                .parse_union_revsets(&args.from)?
                .evaluate()?
                .containing_fn()
        } else {
            Box::new(|_: &CommitId| true)
        };
        let mut bookmarks = if !args.names.is_empty() {
            find_bookmarks_with(&args.names, |pattern| {
                repo.view()
                    .local_bookmarks_matching(pattern)
                    .filter(|(_, target)| target.added_ids().any(&is_source_commit))
            })?
        } else {
            repo.view()
                .local_bookmarks()
                .filter(|(_, target)| target.added_ids().any(&is_source_commit))
                .collect()
        };
        // Noop matches aren't error, but should be excluded from stats.
        bookmarks.retain(|(_, old_target)| old_target.as_normal() != Some(target_commit.id()));
        bookmarks
    };

    if matched_bookmarks.is_empty() {
        writeln!(ui.status(), "No bookmarks to update.")?;
        return Ok(());
    }

    if !args.allow_backwards {
        if let Some((name, _)) = matched_bookmarks
            .iter()
            .find(|(_, old_target)| !is_fast_forward(repo.as_ref(), old_target, target_commit.id()))
        {
            return Err(user_error_with_hint(
                format!("Refusing to move bookmark backwards or sideways: {name}"),
                "Use --allow-backwards to allow it.",
            ));
        }
    }

    let mut tx = workspace_command.start_transaction();
    for (name, _) in &matched_bookmarks {
        tx.repo_mut()
            .set_local_bookmark_target(name, RefTarget::normal(target_commit.id().clone()));
    }

    if let Some(mut formatter) = ui.status_formatter() {
        write!(formatter, "Moved {} bookmarks to ", matched_bookmarks.len())?;
        tx.write_commit_summary(formatter.as_mut(), &target_commit)?;
        writeln!(formatter)?;
    }
    if matched_bookmarks.len() > 1 && args.names.is_empty() {
        writeln!(
            ui.hint_default(),
            "Specify bookmark by name to update just one of the bookmarks."
        )?;
    }

    tx.finish(
        ui,
        format!(
            "point bookmark {names} to commit {id}",
            names = matched_bookmarks.iter().map(|(name, _)| name).join(", "),
            id = target_commit.id().hex()
        ),
    )?;
    Ok(())
}
