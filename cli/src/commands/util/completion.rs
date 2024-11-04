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

use std::io::Write as _;

use clap::Command;

use crate::cli_util::CommandHelper;
use crate::command_error::user_error;
use crate::command_error::CommandError;
use crate::ui::Ui;

// Using an explicit `doc` attribute prevents rustfmt from mangling the list
// formatting without disabling rustfmt for the entire struct.
#[doc = r#"Print a command-line-completion script

Apply it by running one of these:

- Bash: `source <(jj util completion bash)`
- Fish: `jj util completion fish | source`
- Nushell:
     ```nu
     jj util completion nushell | save "completions-jj.nu"
     use "completions-jj.nu" *  # Or `source "completions-jj.nu"`
     ```
- Zsh:
     ```shell
     autoload -U compinit
     compinit
     source <(jj util completion zsh)
     ```
"#]
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub struct UtilCompletionArgs {
    shell: Option<ShellCompletion>,
    /// Deprecated. Use the SHELL positional argument instead.
    #[arg(long, hide = true)]
    bash: bool,
    /// Deprecated. Use the SHELL positional argument instead.
    #[arg(long, hide = true)]
    fish: bool,
    /// Deprecated. Use the SHELL positional argument instead.
    #[arg(long, hide = true)]
    zsh: bool,
}

pub fn cmd_util_completion(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &UtilCompletionArgs,
) -> Result<(), CommandError> {
    let mut app = command.app().clone();
    let warn = |shell| -> std::io::Result<()> {
        writeln!(
            ui.warning_default(),
            "`jj util completion --{shell}` will be removed in a future version, and this will be \
             a hard error"
        )?;
        writeln!(
            ui.hint_default(),
            "Use `jj util completion {shell}` instead"
        )?;
        Ok(())
    };
    let shell = match (args.shell, args.fish, args.zsh, args.bash) {
        (Some(s), false, false, false) => s,
        // allow `--fish` and `--zsh` for back-compat, but don't allow them to be combined
        (None, true, false, false) => {
            warn("fish")?;
            ShellCompletion::Fish
        }
        (None, false, true, false) => {
            warn("zsh")?;
            ShellCompletion::Zsh
        }
        // default to bash for back-compat. TODO: consider making `shell` a required argument
        (None, false, false, _) => {
            warn("bash")?;
            ShellCompletion::Bash
        }
        _ => {
            return Err(user_error(
                "cannot generate completion for multiple shells at once",
            ))
        }
    };

    let buf = shell.generate(&mut app);
    ui.stdout().write_all(&buf)?;
    Ok(())
}

/// Available shell completions
#[derive(clap::ValueEnum, Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ShellCompletion {
    Bash,
    Elvish,
    Fish,
    Nushell,
    PowerShell,
    Zsh,
}

impl ShellCompletion {
    fn generate(&self, cmd: &mut Command) -> Vec<u8> {
        use clap_complete::generate;
        use clap_complete::Shell;
        use clap_complete_nushell::Nushell;

        let mut buf = Vec::new();

        let bin_name = "jj";

        match self {
            Self::Bash => generate(Shell::Bash, cmd, bin_name, &mut buf),
            Self::Elvish => generate(Shell::Elvish, cmd, bin_name, &mut buf),
            Self::Fish => generate(Shell::Fish, cmd, bin_name, &mut buf),
            Self::Nushell => generate(Nushell, cmd, bin_name, &mut buf),
            Self::PowerShell => generate(Shell::PowerShell, cmd, bin_name, &mut buf),
            Self::Zsh => generate(Shell::Zsh, cmd, bin_name, &mut buf),
        }

        buf
    }
}
