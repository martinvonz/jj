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

use std::collections::HashMap;

use super::{find_remote_branches, make_branch_term};
use crate::cli_util::{CommandHelper, RemoteBranchNamePattern};
use crate::command_error::CommandError;
use crate::commit_templater::{CommitTemplateLanguage, RefName};
use crate::ui::Ui;

/// Start tracking given remote branches
///
/// A tracking remote branch will be imported as a local branch of the same
/// name. Changes to it will propagate to the existing local branch on future
/// pulls.
#[derive(clap::Args, Clone, Debug)]
pub struct BranchTrackArgs {
    /// Remote branches to track
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// select branches by wildcard pattern. For details, see
    /// https://github.com/martinvonz/jj/blob/main/docs/revsets.md#string-patterns.
    ///
    /// Examples: branch@remote, glob:main@*, glob:jjfan-*@upstream
    #[arg(required = true, value_name = "BRANCH@REMOTE")]
    names: Vec<RemoteBranchNamePattern>,
}

pub fn cmd_branch_track(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BranchTrackArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    let mut names = Vec::new();
    for (name, remote_ref) in find_remote_branches(view, &args.names)? {
        if remote_ref.is_tracking() {
            writeln!(
                ui.warning_default(),
                "Remote branch already tracked: {name}"
            )?;
        } else {
            names.push(name);
        }
    }
    let mut tx = workspace_command.start_transaction();
    for name in &names {
        tx.mut_repo()
            .track_remote_branch(&name.branch, &name.remote);
    }
    tx.finish(ui, format!("track remote {}", make_branch_term(&names)))?;
    if names.len() > 1 {
        writeln!(
            ui.status(),
            "Started tracking {} remote branches.",
            names.len()
        )?;
    }

    //show conflicted branches if there are some

    if let Some(mut formatter) = ui.status_formatter() {
        let template = {
            let language = workspace_command.commit_template_language()?;
            let text = command
                .settings()
                .config()
                .get::<String>("templates.branch_list")?;
            workspace_command
                .parse_template(&language, &text, CommitTemplateLanguage::wrap_ref_name)?
                .labeled("branch_list")
        };

        let mut remote_per_branch: HashMap<&str, Vec<&str>> = HashMap::new();
        for n in names.iter() {
            remote_per_branch
                .entry(&n.branch)
                .or_default()
                .push(&n.remote);
        }
        let branches_to_list =
            workspace_command
                .repo()
                .view()
                .branches()
                .filter(|(name, target)| {
                    remote_per_branch.contains_key(name) && target.local_target.has_conflict()
                });

        for (name, branch_target) in branches_to_list {
            let local_target = branch_target.local_target;
            let ref_name = RefName::local(
                name,
                local_target.clone(),
                branch_target.remote_refs.iter().map(|x| x.1),
            );
            template.format(&ref_name, formatter.as_mut())?;

            for (remote_name, remote_ref) in branch_target.remote_refs {
                if remote_per_branch[name].contains(&remote_name) {
                    let ref_name =
                        RefName::remote(name, remote_name, remote_ref.clone(), local_target);
                    template.format(&ref_name, formatter.as_mut())?;
                }
            }
        }
    }
    Ok(())
}
