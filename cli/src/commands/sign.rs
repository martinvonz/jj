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

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Cryptographically sign a revision
#[derive(clap::Args, Clone, Debug)]
pub struct SignArgs {
    /// What key to use, depends on the configured signing backend.
    #[arg()]
    key: Option<String>,
    /// What revision to sign
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Sign a commit that is not authored by you or was already signed.
    #[arg(long, short)]
    force: bool,
    /// Drop the signature, explicitly "un-signing" the commit.
    #[arg(long, short = 'D', conflicts_with = "force")]
    drop: bool,
}

pub fn cmd_sign(
    _ui: &mut Ui,
    _command: &CommandHelper,
    _args: &SignArgs,
) -> Result<(), CommandError> {
    todo!()
}
