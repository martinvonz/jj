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

use clap_complete::ArgValueCandidates;
use jj_lib::str_util::StringPattern;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Sync the local `jj` repo to remote Git branch(es).
///
/// The sync command will first fetch from the Git remote(s), then
/// rebase all local changes onto the appropriate updated
/// heads that were fetched.
///
/// Changes that are made empty by the rebase are dropped.
#[derive(clap::Args, Clone, Debug)]
pub struct GitSyncArgs {
    /// Rebase the specified branches only.
    ///
    /// Note that this affects only the rebase behaviour, as
    /// the fetch behaviour always fetches all branches.
    ///
    /// By default, the specified name matches exactly. Use `glob:` prefix to
    /// expand `*` as a glob. The other wildcard characters aren't supported.
    #[arg(long, short,
          alias="bookmark",
          default_value = "glob:*",
          value_parser = StringPattern::parse,
          add = ArgValueCandidates::new(complete::bookmarks),
    )]
    pub branch: Vec<StringPattern>,
    /// Fetch from all remotes
    ///
    /// By default, the fetch will only use remotes configured in the
    /// `git.fetch` section of the config.
    ///
    /// When specified, --all-remotes causes the fetch to use all remotes known
    /// to the underlying git repo.
    #[arg(long, default_value = "false")]
    pub all_remotes: bool,
}

pub fn cmd_git_sync(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &GitSyncArgs,
) -> Result<(), CommandError> {
    let _workspace_command = command.workspace_helper(ui)?;

    let guard = tracing::debug_span!("git.sync.pre-fetch").entered();
    drop(guard);

    let guard = tracing::debug_span!("git.sync.fetch").entered();
    drop(guard);

    let guard = tracing::debug_span!("git.sync.post-fetch").entered();
    drop(guard);

    let guard = tracing::debug_span!("git.sync.rebase").entered();
    drop(guard);

    Ok(())
}
