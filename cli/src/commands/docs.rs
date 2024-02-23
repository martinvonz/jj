// Copyright 2024 The Jujutsu Authors
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

use std::fmt::Debug;
use std::io::Write;

use jj_docs::DocAssets;

use crate::cli_util::CommandHelper;
use crate::command_error::{user_error, CommandError};
use crate::ui::Ui;

#[derive(clap::Args, Clone, Debug)]
pub(crate) struct DocsArgs {
    /// The command to show documentation for
    command: Option<String>,
}

pub fn cmd_docs(
    ui: &mut Ui,
    _command: &CommandHelper,
    args: &DocsArgs,
) -> Result<(), CommandError> {
    // just print all the documents in the static map, and the size of the contents

    let cmd = args.command.clone();
    match cmd {
        None => {
            for name in DocAssets::iter() {
                // XXX FIXME (aseipp): show a real document index
                let content = DocAssets::get(&name).unwrap();
                writeln!(ui.stdout(), "{}: {} bytes", name, content.len())?;
            }
        }
        Some(command) => {
            // print the document for the command, if it exists
            if let Some(name) = DocAssets::iter().find(|name| name == &command) {
                let skin = termimad::MadSkin::default();
                let content = DocAssets::get(&name).unwrap();
                writeln!(
                    ui.stdout(),
                    "{}",
                    skin.term_text(&String::from_utf8_lossy(&content))
                )?;
                return Ok(());
            } else {
                return Err(user_error(format!(
                    "No documentation found for item: {}",
                    command
                )));
            }
        }
    }

    Ok(())
}
