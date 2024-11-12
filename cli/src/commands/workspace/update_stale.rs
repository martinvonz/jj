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

use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Update a workspace that has become stale
///
/// For information about stale working copies, see
/// https://martinvonz.github.io/jj/latest/working-copy/.
#[derive(clap::Args, Clone, Debug)]
pub struct WorkspaceUpdateStaleArgs {}

#[instrument(skip_all)]
pub fn cmd_workspace_update_stale(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &WorkspaceUpdateStaleArgs,
) -> Result<(), CommandError> {
    command.recover_stale_working_copy(ui)?;

    Ok(())
}
