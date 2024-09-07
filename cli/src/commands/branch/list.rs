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

use std::collections::HashSet;

use jj_lib::git;
use jj_lib::revset::RevsetExpression;
use jj_lib::str_util::StringPattern;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::commit_templater::CommitTemplateLanguage;
use crate::commit_templater::RefName;
use crate::ui::Ui;

/// List branches and their targets
///
/// By default, a tracking remote branch will be included only if its target is
/// different from the local target. A non-tracking remote branch won't be
/// listed. For a conflicted branch (both local and remote), old target
/// revisions are preceded by a "-" and new target revisions are preceded by a
/// "+".
///
/// For information about branches, see
/// https://martinvonz.github.io/jj/latest/branches/.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchListArgs {
    /// Show all tracking and non-tracking remote branches including the ones
    /// whose targets are synchronized with the local branches
    #[arg(long, short, alias = "all")]
    all_remotes: bool,

    /// Show remote tracked branches only. Omits local Git-tracking branches by
    /// default
    #[arg(long, short, conflicts_with_all = ["all_remotes"])]
    tracked: bool,

    /// Show conflicted branches only
    #[arg(long, short, conflicts_with_all = ["all_remotes"])]
    conflicted: bool,

    /// Show branches whose local name matches
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select branches by wildcard pattern. For details, see
    /// https://martinvonz.github.io/jj/latest/revsets/#string-patterns.
    #[arg(value_parser = StringPattern::parse)]
    names: Vec<StringPattern>,

    /// Show branches whose local targets are in the given revisions
    ///
    /// Note that `-r deleted_branch` will not work since `deleted_branch`
    /// wouldn't have a local target.
    #[arg(long, short)]
    revisions: Vec<RevisionArg>,

    /// Render each branch using the given template
    ///
    /// All 0-argument methods of the `RefName` type are available as keywords.
    ///
    /// For the syntax, see https://martinvonz.github.io/jj/latest/templates/
    #[arg(long, short = 'T')]
    template: Option<String>,
}

pub fn cmd_branch_list(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let view = repo.view();

    // Like cmd_git_push(), names and revisions are OR-ed.
    let branch_names_to_list = if !args.names.is_empty() || !args.revisions.is_empty() {
        let mut branch_names: HashSet<&str> = HashSet::new();
        if !args.names.is_empty() {
            branch_names.extend(
                view.branches()
                    .filter(|&(name, _)| args.names.iter().any(|pattern| pattern.matches(name)))
                    .map(|(name, _)| name),
            );
        }
        if !args.revisions.is_empty() {
            // Match against local targets only, which is consistent with "jj git push".
            let mut expression = workspace_command.parse_union_revsets(&args.revisions)?;
            // Intersects with the set of local branch targets to minimize the lookup space.
            expression.intersect_with(&RevsetExpression::branches(StringPattern::everything()));
            let filtered_targets: HashSet<_> = expression.evaluate_to_commit_ids()?.collect();
            branch_names.extend(
                view.local_branches()
                    .filter(|(_, target)| {
                        target.added_ids().any(|id| filtered_targets.contains(id))
                    })
                    .map(|(name, _)| name),
            );
        }
        Some(branch_names)
    } else {
        None
    };

    let template = {
        let language = workspace_command.commit_template_language();
        let text = match &args.template {
            Some(value) => value.to_owned(),
            None => command.settings().config().get("templates.branch_list")?,
        };
        workspace_command
            .parse_template(&language, &text, CommitTemplateLanguage::wrap_ref_name)?
            .labeled("branch_list")
    };

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();

    let mut found_deleted_local_branch = false;
    let mut found_deleted_tracking_local_branch = false;
    let branches_to_list = view.branches().filter(|(name, target)| {
        branch_names_to_list
            .as_ref()
            .map_or(true, |branch_names| branch_names.contains(name))
            && (!args.conflicted || target.local_target.has_conflict())
    });
    for (name, branch_target) in branches_to_list {
        let local_target = branch_target.local_target;
        let remote_refs = branch_target.remote_refs;
        let (mut tracking_remote_refs, untracked_remote_refs) = remote_refs
            .iter()
            .copied()
            .partition::<Vec<_>, _>(|&(_, remote_ref)| remote_ref.is_tracking());

        if args.tracked {
            tracking_remote_refs
                .retain(|&(remote, _)| remote != git::REMOTE_NAME_FOR_LOCAL_GIT_REPO);
        } else if !args.all_remotes {
            tracking_remote_refs.retain(|&(_, remote_ref)| remote_ref.target != *local_target);
        }

        if !args.tracked && local_target.is_present() || !tracking_remote_refs.is_empty() {
            let ref_name = RefName::local(
                name,
                local_target.clone(),
                remote_refs.iter().map(|&(_, remote_ref)| remote_ref),
            );
            template.format(&ref_name, formatter.as_mut())?;
        }

        for &(remote, remote_ref) in &tracking_remote_refs {
            let ref_name = RefName::remote(name, remote, remote_ref.clone(), local_target);
            template.format(&ref_name, formatter.as_mut())?;
        }

        if local_target.is_absent() && !tracking_remote_refs.is_empty() {
            found_deleted_local_branch = true;
            found_deleted_tracking_local_branch |= tracking_remote_refs
                .iter()
                .any(|&(remote, _)| remote != git::REMOTE_NAME_FOR_LOCAL_GIT_REPO);
        }

        if args.all_remotes {
            for &(remote, remote_ref) in &untracked_remote_refs {
                let ref_name = RefName::remote_only(name, remote, remote_ref.target.clone());
                template.format(&ref_name, formatter.as_mut())?;
            }
        }
    }

    drop(formatter);

    // Print only one of these hints. It's not important to mention unexported
    // branches, but user might wonder why deleted branches are still listed.
    if found_deleted_tracking_local_branch {
        writeln!(
            ui.hint_default(),
            "Branches marked as deleted will be *deleted permanently* on the remote on the next \
             `jj git push`. Use `jj branch forget` to prevent this."
        )?;
    } else if found_deleted_local_branch {
        writeln!(
            ui.hint_default(),
            "Branches marked as deleted will be deleted from the underlying Git repo on the next \
             `jj git export`."
        )?;
    }

    Ok(())
}
