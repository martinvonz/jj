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

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::rc::Rc;

use itertools::Itertools;
use jj_lib::backend::{BackendResult, CommitId};
use jj_lib::revset::{self, RevsetExpression};
use jj_lib::str_util::StringPattern;

use crate::cli_util::{CommandHelper, RevisionArg};
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

/// Manage branches.
///
/// For information about branches, see
/// https://github.com/martinvonz/jj/blob/main/docs/branches.md.
#[derive(clap::Subcommand, Clone, Debug)]
pub enum TopicCommand {
    #[command(visible_alias("d"))]
    Delete(TopicDeleteArgs),
    #[command(visible_alias("l"))]
    List(TopicListArgs),
    #[command(visible_alias("r"))]
    Rename(TopicRenameArgs),
    #[command(visible_alias("s"))]
    Set(TopicSetArgs),
    #[command(visible_alias("u"))]
    Unset(TopicUnsetArgs),
}

/// Delete an existing topic and unset it from all associated commits
#[derive(clap::Args, Clone, Debug)]
pub struct TopicDeleteArgs {
    /// The topic to delete
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select topics by wildcard pattern. For details, see
    /// https://github.com/martinvonz/jj/blob/main/docs/revsets.md#string-patterns.
    #[arg(required = true, value_parser = StringPattern::parse)]
    pub names: Vec<StringPattern>,
}

/// List topics
///
/// For information about topics, see
/// https://github.com/martinvonz/jj/blob/main/docs/topics.md.
#[derive(clap::Args, Clone, Debug)]
pub struct TopicListArgs {
    /// Show topics whose name matches
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select topics by wildcard pattern. For details, see
    /// https://github.com/martinvonz/jj/blob/main/docs/revsets.md#string-patterns.
    #[arg(value_parser = StringPattern::parse)]
    pub names: Vec<StringPattern>,

    /// Show topics associated with any of the given revisions.
    #[arg(long, short)]
    pub revisions: Vec<RevisionArg>,
}

/// Rename `old` topic name to `new` topic name.
///
/// The new topic name is associated with the same commits as the old
/// topic name.
#[derive(clap::Args, Clone, Debug)]
pub struct TopicRenameArgs {
    /// The old name of the topic.
    pub old: String,

    /// The new name of the topic.
    pub new: String,
}

/// Updates the given revision(s) to be associated with the given topic(s).
///
/// Unless `--keep` is passed, this will replace previously associated topics.
#[derive(clap::Args, Clone, Debug)]
pub struct TopicSetArgs {
    /// The revisions to associate with the topics, defaults to @.
    #[arg(long, short)]
    pub revisions: Vec<RevisionArg>,

    /// The topics to add.
    #[arg(required = true)]
    pub names: Vec<String>,

    /// Whether to keep other topics on the revisions
    #[arg(long)]
    pub exclusive_topics: bool,

    /// Whether to disassociate the topics from other revisions
    #[arg(long)]
    pub exclusive_commits: bool,
}

/// Updates the given revision(s) to no longer be associated with the given
/// topic(s).
#[derive(clap::Args, Clone, Debug)]
pub struct TopicUnsetArgs {
    /// The revisions to disassociate with the topics, defaults to @.
    #[arg(long, short)]
    pub revisions: Vec<RevisionArg>,

    /// The topic to unset
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select topics by wildcard pattern. For details, see
    /// https://github.com/martinvonz/jj/blob/main/docs/revsets.md#string-patterns.
    #[arg(value_parser = StringPattern::parse)]
    pub names: Vec<StringPattern>,
}

pub fn cmd_topic(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &TopicCommand,
) -> Result<(), CommandError> {
    match subcommand {
        TopicCommand::Rename(sub_args) => cmd_topic_rename(ui, command, sub_args),
        TopicCommand::Set(sub_args) => cmd_topic_set(ui, command, sub_args),
        TopicCommand::Unset(sub_args) => cmd_topic_unset(ui, command, sub_args),
        TopicCommand::Delete(sub_args) => cmd_topic_delete(ui, command, sub_args),
        TopicCommand::List(sub_args) => cmd_topic_list(ui, command, sub_args),
    }
}

fn cmd_topic_rename(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &TopicRenameArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let mut tx = workspace_command.start_transaction();
    let repo = tx.mut_repo();
    let base_repo = repo.base_repo().clone();

    let Some(commits) = base_repo.view().get_topic_commits(&args.old) else {
        return Err(user_error(format!("No such topic {}", args.old)));
    };

    let mut stats = repo.set_topic_commits(
        HashSet::from_iter([args.new.clone()]),
        commits,
        false,
        false,
    );
    stats += repo.remove_topics(vec![StringPattern::exact(args.old.to_string())]);

    tx.finish(
        ui,
        format!("rename topic {} to {} commits", args.old, args.new),
    )?;
    if !stats.is_empty() {
        let summary = stats
            .iter()
            .map(|(topic, stats)| {
                format!(
                    "{topic}: {} added, {} removed",
                    stats.added().len(),
                    stats.removed().len()
                )
            })
            .join("\n");
        writeln!(
            ui.stdout_formatter(),
            "The following topics were updated:\n{}",
            summary
        )?;
    }

    Ok(())
}

fn cmd_topic_set(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &TopicSetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let commit_ids = if args.revisions.is_empty() {
        workspace_command.attach_revset_evaluator(RevsetExpression::working_copy(
            workspace_command.workspace_id().clone(),
        ))?
    } else {
        workspace_command.parse_union_revsets(&args.revisions)?
    }
    .evaluate_to_commit_ids()?
    .collect();

    let mut tx = workspace_command.start_transaction();
    let repo = tx.mut_repo();

    let stats = repo.set_topic_commits(
        HashSet::from_iter(args.names.clone()),
        &commit_ids,
        args.exclusive_topics,
        args.exclusive_commits,
    );

    tx.finish(
        ui,
        format!(
            "update topics {} on {} commits",
            stats.keys().join(", "),
            stats.affected().len()
        ),
    )?;
    if !stats.is_empty() {
        let summary = stats
            .iter()
            .map(|(topic, stats)| {
                format!(
                    "{topic}: {} added, {} removed",
                    stats.added().len(),
                    stats.removed().len()
                )
            })
            .join("\n");
        writeln!(
            ui.stdout_formatter(),
            "The following topics were updated:\n{}",
            summary
        )?;
    }

    Ok(())
}

fn cmd_topic_unset(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &TopicUnsetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let expression = if args.revisions.is_empty() {
        workspace_command.attach_revset_evaluator(RevsetExpression::working_copy(
            workspace_command.workspace_id().clone(),
        ))?
    } else {
        workspace_command.parse_union_revsets(&args.revisions)?
    };
    let commit_ids = expression.evaluate_to_commit_ids()?.collect();

    let mut tx = workspace_command.start_transaction();
    let repo = tx.mut_repo();

    let stats = repo.disassociate_topics_from_commits(
        if args.names.is_empty() {
            vec![StringPattern::everything()]
        } else {
            args.names.clone()
        },
        &commit_ids,
    );

    tx.finish(
        ui,
        format!(
            "disassociate topics {} from {} commits",
            stats.keys().join(", "),
            stats.affected().len()
        ),
    )?;
    if !stats.is_empty() {
        let summary = stats
            .iter()
            .map(|(topic, stats)| format!("{topic}: {} removed", stats.removed().len()))
            .join("\n");
        writeln!(
            ui.stdout_formatter(),
            "The following topics were updated:\n{}",
            summary
        )?;
    }

    Ok(())
}

fn cmd_topic_delete(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &TopicDeleteArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let mut tx = workspace_command.start_transaction();
    let repo = tx.mut_repo();

    let stats = repo.remove_topics(args.names.clone());

    tx.finish(ui, format!("delete topics {}", stats.keys().join(", "),))?;
    if !stats.is_empty() {
        let summary = stats
            .iter()
            .map(|(topic, stats)| format!("{topic}: {} commits", stats.removed().len()))
            .join("\n");
        writeln!(
            ui.stdout_formatter(),
            "The following topics were removed:\n{}",
            summary
        )?;
    }

    Ok(())
}

fn cmd_topic_list(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &TopicListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();

    let topic_names_to_list = {
        let mut expression = if args.revisions.is_empty() {
            workspace_command.attach_revset_evaluator(RevsetExpression::all())?
        } else {
            workspace_command.parse_union_revsets(&args.revisions)?
        };
        for pattern in args.names.clone() {
            expression.intersect_with(&Rc::new(RevsetExpression::CommitRef(
                revset::RevsetCommitRef::Topics(pattern),
            )))
        }
        let res = expression.evaluate()?;
        let mut topics = res.iter().try_fold(
            HashMap::new(),
            |mut topics, id| -> BackendResult<HashMap<String, Vec<CommitId>>> {
                for topic in repo.view().topics_containing_commit(&id) {
                    match topics.entry(topic.to_string()) {
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            entry.get_mut().push(id.clone());
                        }
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(vec![id.clone()]);
                        }
                    }
                }
                Ok(topics)
            },
        )?;

        if !args.names.is_empty() {
            topics.retain(|name, _| args.names.iter().any(|pattern| pattern.matches(name)))
        }
        topics
    };

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();

    for (topic, commits) in topic_names_to_list {
        write!(formatter.labeled("topics"), "{topic}")?;
        writeln!(formatter, ": {}", commits.len())?;
    }

    Ok(())
}
