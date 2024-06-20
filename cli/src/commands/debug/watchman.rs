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

use std::any::Any;
use std::fmt::Debug;
use std::io::Write as _;

use clap::Subcommand;
use jj_lib::fsmonitor::{FsmonitorSettings, WatchmanConfig};
use jj_lib::local_working_copy::LocalWorkingCopy;

use crate::cli_util::CommandHelper;
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

#[derive(Subcommand, Clone, Debug)]
pub enum WatchmanCommand {
    /// Check whether `watchman` is enabled and whether it's correctly installed
    Status,
    QueryClock,
    QueryChangedFiles,
    ResetClock,
}

#[cfg(feature = "watchman")]
pub fn cmd_debug_watchman(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &WatchmanCommand,
) -> Result<(), CommandError> {
    use jj_lib::local_working_copy::LockedLocalWorkingCopy;

    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo().clone();
    match subcommand {
        WatchmanCommand::Status => {
            // TODO(ilyagr): It would be nice to add colors here
            let config = match command.settings().fsmonitor_settings()? {
                FsmonitorSettings::Watchman(config) => {
                    writeln!(ui.stdout(), "Watchman is enabled via `core.fsmonitor`.")?;
                    writeln!(
                        ui.stdout(),
                        r"Background snapshotting is {}. Use `core.watchman.register_snapshot_trigger` to control it.",
                        if config.register_trigger {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    )?;
                    config
                }
                FsmonitorSettings::None => {
                    writeln!(
                        ui.stdout(),
                        r#"Watchman is disabled. Set `core.fsmonitor="watchman"` to enable."#
                    )?;
                    writeln!(
                        ui.stdout(),
                        "Attempting to contact the `watchman` CLI regardless..."
                    )?;
                    WatchmanConfig::default()
                }
                other_fsmonitor => {
                    return Err(user_error(format!(
                        r"This command does not support the currently enabled filesystem monitor: {other_fsmonitor:?}."
                    )))
                }
            };
            let wc = check_local_disk_wc(workspace_command.working_copy().as_any())?;
            let _ = wc.query_watchman(&config)?;
            writeln!(
                ui.stdout(),
                "The watchman server seems to be installed and working correctly."
            )?;
            writeln!(
                ui.stdout(),
                "Background snapshotting is currently {}.",
                if wc.is_watchman_trigger_registered(&config)? {
                    "active"
                } else {
                    "inactive"
                }
            )?;
        }
        WatchmanCommand::QueryClock => {
            let wc = check_local_disk_wc(workspace_command.working_copy().as_any())?;
            let (clock, _changed_files) = wc.query_watchman(&WatchmanConfig::default())?;
            writeln!(ui.stdout(), "Clock: {clock:?}")?;
        }
        WatchmanCommand::QueryChangedFiles => {
            let wc = check_local_disk_wc(workspace_command.working_copy().as_any())?;
            let (_clock, changed_files) = wc.query_watchman(&WatchmanConfig::default())?;
            writeln!(ui.stdout(), "Changed files: {changed_files:?}")?;
        }
        WatchmanCommand::ResetClock => {
            let (mut locked_ws, _commit) = workspace_command.start_working_copy_mutation()?;
            let Some(locked_local_wc): Option<&mut LockedLocalWorkingCopy> =
                locked_ws.locked_wc().as_any_mut().downcast_mut()
            else {
                return Err(user_error(
                    "This command requires a standard local-disk working copy",
                ));
            };
            locked_local_wc.reset_watchman()?;
            locked_ws.finish(repo.op_id().clone())?;
            writeln!(ui.status(), "Reset Watchman clock")?;
        }
    }
    Ok(())
}

#[cfg(not(feature = "watchman"))]
pub fn cmd_debug_watchman(
    _ui: &mut Ui,
    _command: &CommandHelper,
    _subcommand: &WatchmanCommand,
) -> Result<(), CommandError> {
    Err(user_error(
        "Cannot query Watchman because jj was not compiled with the `watchman` feature",
    ))
}

fn check_local_disk_wc(x: &dyn Any) -> Result<&LocalWorkingCopy, CommandError> {
    x.downcast_ref()
        .ok_or_else(|| user_error("This command requires a standard local-disk working copy"))
}
