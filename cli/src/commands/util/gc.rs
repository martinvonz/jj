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

use std::slice;
use std::time::Duration;
use std::time::SystemTime;

use jj_lib::repo::Repo as _;

use crate::cli_util::CommandHelper;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Run backend-dependent garbage collection.
///
/// To garbage-collect old operations and the commits/objects referenced by
/// then, run `jj op abandon ..<some old operation>` before `jj util gc`.
///
/// Previous versions of a change that are reachable via the evolution log are
/// not garbage-collected.
#[derive(clap::Args, Clone, Debug)]
pub struct UtilGcArgs {
    /// Time threshold
    ///
    /// By default, only obsolete objects and operations older than 2 weeks are
    /// pruned.
    ///
    /// Only the string "now" can be passed to this parameter. Support for
    /// arbitrary absolute and relative timestamps will come in a subsequent
    /// release.
    #[arg(long)]
    expire: Option<String>,
}

pub fn cmd_util_gc(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &UtilGcArgs,
) -> Result<(), CommandError> {
    if !command.is_at_head_operation() {
        return Err(user_error(
            "Cannot garbage collect from a non-head operation",
        ));
    }
    let keep_newer = match args.expire.as_deref() {
        None => SystemTime::now() - Duration::from_secs(14 * 86400),
        Some("now") => SystemTime::now() - Duration::ZERO,
        _ => return Err(user_error("--expire only accepts 'now'")),
    };
    let workspace_command = command.workspace_helper(ui)?;

    let repo = workspace_command.repo();
    repo.op_store()
        .gc(slice::from_ref(repo.op_id()), keep_newer)?;
    repo.store().gc(repo.index(), keep_newer)?;
    Ok(())
}
