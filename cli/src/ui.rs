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

use std::io::{IsTerminal as _, Stderr, StderrLock, Stdout, StdoutLock, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::str::FromStr;
use std::thread::JoinHandle;
use std::{env, fmt, io, mem};

use minus::Pager as MinusPager;
use tracing::instrument;

use crate::command_error::{config_error_with_message, CommandError};
use crate::config::CommandNameAndArgs;
use crate::formatter::{Formatter, FormatterFactory, HeadingLabeledWriter, LabeledWriter};

const BUILTIN_PAGER_NAME: &str = ":builtin";

enum UiOutput {
    Terminal {
        stdout: Stdout,
        stderr: Stderr,
    },
    Paged {
        child: Child,
        child_stdin: ChildStdin,
    },
    BuiltinPaged {
        pager: BuiltinPager,
    },
}

/// A builtin pager
pub struct BuiltinPager {
    pager: MinusPager,
    dynamic_pager_thread: JoinHandle<()>,
}

impl std::io::Write for &BuiltinPager {
    fn flush(&mut self) -> io::Result<()> {
        // no-op since this is being run in a dynamic pager mode.
        Ok(())
    }

    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let string = std::str::from_utf8(buf).map_err(std::io::Error::other)?;
        self.pager.push_str(string).map_err(std::io::Error::other)?;
        Ok(buf.len())
    }
}

impl Default for BuiltinPager {
    fn default() -> Self {
        Self::new()
    }
}

impl BuiltinPager {
    pub fn finalize(self) {
        let dynamic_pager_thread = self.dynamic_pager_thread;
        dynamic_pager_thread.join().unwrap();
    }

    pub fn new() -> Self {
        let pager = MinusPager::new();
        // Prefer to be cautious and only kill the pager instead of the whole process
        // like minus does by default.
        pager
            .set_exit_strategy(minus::ExitStrategy::PagerQuit)
            .expect("Able to set the exit strategy");
        let pager_handle = pager.clone();

        BuiltinPager {
            pager,
            dynamic_pager_thread: std::thread::spawn(move || {
                // This thread handles the actual paging.
                minus::dynamic_paging(pager_handle).unwrap();
            }),
        }
    }
}

impl UiOutput {
    fn new_builtin() -> UiOutput {
        UiOutput::BuiltinPaged {
            pager: BuiltinPager::new(),
        }
    }
    fn new_terminal() -> UiOutput {
        UiOutput::Terminal {
            stdout: io::stdout(),
            stderr: io::stderr(),
        }
    }

    fn new_paged(pager_cmd: &CommandNameAndArgs) -> io::Result<UiOutput> {
        let mut child = pager_cmd.to_command().stdin(Stdio::piped()).spawn()?;
        let child_stdin = child.stdin.take().unwrap();
        Ok(UiOutput::Paged { child, child_stdin })
    }
}

pub enum UiStdout<'a> {
    Terminal(StdoutLock<'static>),
    Paged(&'a ChildStdin),
    Builtin(&'a BuiltinPager),
}

pub enum UiStderr<'a> {
    Terminal(StderrLock<'static>),
    Paged(&'a ChildStdin),
    Builtin(&'a BuiltinPager),
}

macro_rules! for_outputs {
    ($ty:ident, $output:expr, $pat:pat => $expr:expr) => {
        match $output {
            $ty::Terminal($pat) => $expr,
            $ty::Paged($pat) => $expr,
            $ty::Builtin($pat) => $expr,
        }
    };
}

impl Write for UiStdout<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for_outputs!(Self, self, w => w.write(buf))
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        for_outputs!(Self, self, w => w.write_all(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        for_outputs!(Self, self, w => w.flush())
    }
}

impl Write for UiStderr<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for_outputs!(Self, self, w => w.write(buf))
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        for_outputs!(Self, self, w => w.write_all(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        for_outputs!(Self, self, w => w.flush())
    }
}

pub struct Ui {
    color: bool,
    pager_cmd: CommandNameAndArgs,
    paginate: PaginationChoice,
    progress_indicator: bool,
    formatter_factory: FormatterFactory,
    output: UiOutput,
}

fn progress_indicator_setting(config: &config::Config) -> bool {
    config.get_bool("ui.progress-indicator").unwrap_or(true)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColorChoice {
    Always,
    Never,
    #[default]
    Auto,
}

impl FromStr for ColorChoice {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "always" => Ok(ColorChoice::Always),
            "never" => Ok(ColorChoice::Never),
            "auto" => Ok(ColorChoice::Auto),
            _ => Err("must be one of always, never, or auto"),
        }
    }
}

impl fmt::Display for ColorChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ColorChoice::Always => "always",
            ColorChoice::Never => "never",
            ColorChoice::Auto => "auto",
        };
        write!(f, "{s}")
    }
}

fn color_setting(config: &config::Config) -> ColorChoice {
    config
        .get_string("ui.color")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_default()
}

fn use_color(choice: ColorChoice) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => io::stdout().is_terminal(),
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all(deserialize = "kebab-case"))]
pub enum PaginationChoice {
    Never,
    #[default]
    Auto,
}

fn pagination_setting(config: &config::Config) -> Result<PaginationChoice, CommandError> {
    config
        .get::<PaginationChoice>("ui.paginate")
        .map_err(|err| config_error_with_message("Invalid `ui.paginate`", err))
}

fn pager_setting(config: &config::Config) -> Result<CommandNameAndArgs, CommandError> {
    config
        .get::<CommandNameAndArgs>("ui.pager")
        .map_err(|err| config_error_with_message("Invalid `ui.pager`", err))
}

impl Ui {
    pub fn with_config(config: &config::Config) -> Result<Ui, CommandError> {
        let color = use_color(color_setting(config));
        // Sanitize ANSI escape codes if we're printing to a terminal. Doesn't affect
        // ANSI escape codes that originate from the formatter itself.
        let sanitize = io::stdout().is_terminal();
        let formatter_factory = FormatterFactory::prepare(config, color, sanitize)?;
        let progress_indicator = progress_indicator_setting(config);
        Ok(Ui {
            color,
            formatter_factory,
            pager_cmd: pager_setting(config)?,
            paginate: pagination_setting(config)?,
            progress_indicator,
            output: UiOutput::new_terminal(),
        })
    }

    pub fn reset(&mut self, config: &config::Config) -> Result<(), CommandError> {
        self.color = use_color(color_setting(config));
        self.paginate = pagination_setting(config)?;
        self.pager_cmd = pager_setting(config)?;
        self.progress_indicator = progress_indicator_setting(config);
        let sanitize = io::stdout().is_terminal();
        self.formatter_factory = FormatterFactory::prepare(config, self.color, sanitize)?;
        Ok(())
    }

    /// Switches the output to use the pager, if allowed.
    #[instrument(skip_all)]
    pub fn request_pager(&mut self) {
        match self.paginate {
            PaginationChoice::Never => return,
            PaginationChoice::Auto => {}
        }

        match self.output {
            UiOutput::Terminal { .. } if io::stdout().is_terminal() => {
                if self.pager_cmd == CommandNameAndArgs::String(BUILTIN_PAGER_NAME.into()) {
                    self.output = UiOutput::new_builtin();
                    return;
                }

                match UiOutput::new_paged(&self.pager_cmd) {
                    Ok(pager_output) => {
                        self.output = pager_output;
                    }
                    Err(e) => {
                        // The pager executable couldn't be found or couldn't be run
                        writeln!(
                            self.warning_default(),
                            "Failed to spawn pager '{name}': {e}. Consider using the `:builtin` \
                             pager.",
                            name = self.pager_cmd.split_name(),
                        )
                        .ok();
                    }
                }
            }
            UiOutput::Terminal { .. } | UiOutput::BuiltinPaged { .. } | UiOutput::Paged { .. } => {}
        }
    }

    pub fn color(&self) -> bool {
        self.color
    }

    pub fn new_formatter<'output, W: Write + 'output>(
        &self,
        output: W,
    ) -> Box<dyn Formatter + 'output> {
        self.formatter_factory.new_formatter(output)
    }

    /// Locked stdout stream.
    pub fn stdout(&self) -> UiStdout<'_> {
        match &self.output {
            UiOutput::Terminal { stdout, .. } => UiStdout::Terminal(stdout.lock()),
            UiOutput::Paged { child_stdin, .. } => UiStdout::Paged(child_stdin),
            UiOutput::BuiltinPaged { pager } => UiStdout::Builtin(pager),
        }
    }

    /// Creates a formatter for the locked stdout stream.
    ///
    /// Labels added to the returned formatter should be removed by caller.
    /// Otherwise the last color would persist.
    pub fn stdout_formatter(&self) -> Box<dyn Formatter + '_> {
        for_outputs!(UiStdout, self.stdout(), w => self.new_formatter(w))
    }

    /// Locked stderr stream.
    pub fn stderr(&self) -> UiStderr<'_> {
        match &self.output {
            UiOutput::Terminal { stderr, .. } => UiStderr::Terminal(stderr.lock()),
            UiOutput::Paged { child_stdin, .. } => UiStderr::Paged(child_stdin),
            UiOutput::BuiltinPaged { pager } => UiStderr::Builtin(pager),
        }
    }

    /// Creates a formatter for the locked stderr stream.
    pub fn stderr_formatter(&self) -> Box<dyn Formatter + '_> {
        for_outputs!(UiStderr, self.stderr(), w => self.new_formatter(w))
    }

    /// Stderr stream to be attached to a child process.
    pub fn stderr_for_child(&self) -> io::Result<Stdio> {
        match &self.output {
            UiOutput::Terminal { .. } => Ok(Stdio::inherit()),
            UiOutput::Paged { child_stdin, .. } => Ok(duplicate_child_stdin(child_stdin)?.into()),
            // Stderr does not get redirected through the built-in pager.
            UiOutput::BuiltinPaged { .. } => Ok(Stdio::inherit()),
        }
    }

    /// Whether continuous feedback should be displayed for long-running
    /// operations
    pub fn use_progress_indicator(&self) -> bool {
        match &self.output {
            UiOutput::Terminal { stderr, .. } => self.progress_indicator && stderr.is_terminal(),
            UiOutput::Paged { .. } => false,
            UiOutput::BuiltinPaged { .. } => false,
        }
    }

    pub fn progress_output(&self) -> Option<ProgressOutput> {
        self.use_progress_indicator().then(|| ProgressOutput {
            output: io::stderr(),
        })
    }

    /// Writer to print hint with the default "Hint: " heading.
    pub fn hint_default(
        &self,
    ) -> HeadingLabeledWriter<Box<dyn Formatter + '_>, &'static str, &'static str> {
        self.hint_with_heading("Hint: ")
    }

    /// Writer to print hint without the "Hint: " heading.
    pub fn hint_no_heading(&self) -> LabeledWriter<Box<dyn Formatter + '_>, &'static str> {
        LabeledWriter::new(self.stderr_formatter(), "hint")
    }

    /// Writer to print hint with the given heading.
    pub fn hint_with_heading<H: fmt::Display>(
        &self,
        heading: H,
    ) -> HeadingLabeledWriter<Box<dyn Formatter + '_>, &'static str, H> {
        HeadingLabeledWriter::new(self.stderr_formatter(), "hint", heading)
    }

    /// Writer to print warning with the default "Warning: " heading.
    pub fn warning_default(
        &self,
    ) -> HeadingLabeledWriter<Box<dyn Formatter + '_>, &'static str, &'static str> {
        self.warning_with_heading("Warning: ")
    }

    /// Writer to print warning without the "Warning: " heading.
    pub fn warning_no_heading(&self) -> LabeledWriter<Box<dyn Formatter + '_>, &'static str> {
        LabeledWriter::new(self.stderr_formatter(), "warning")
    }

    /// Writer to print warning with the given heading.
    pub fn warning_with_heading<H: fmt::Display>(
        &self,
        heading: H,
    ) -> HeadingLabeledWriter<Box<dyn Formatter + '_>, &'static str, H> {
        HeadingLabeledWriter::new(self.stderr_formatter(), "warning", heading)
    }

    /// Writer to print error without the "Error: " heading.
    pub fn error_no_heading(&self) -> LabeledWriter<Box<dyn Formatter + '_>, &'static str> {
        LabeledWriter::new(self.stderr_formatter(), "error")
    }

    /// Writer to print error with the given heading.
    pub fn error_with_heading<H: fmt::Display>(
        &self,
        heading: H,
    ) -> HeadingLabeledWriter<Box<dyn Formatter + '_>, &'static str, H> {
        HeadingLabeledWriter::new(self.stderr_formatter(), "error", heading)
    }

    /// Waits for the pager exits.
    #[instrument(skip_all)]
    pub fn finalize_pager(&mut self) {
        match mem::replace(&mut self.output, UiOutput::new_terminal()) {
            UiOutput::Paged {
                mut child,
                child_stdin,
            } => {
                drop(child_stdin);
                if let Err(e) = child.wait() {
                    // It's possible (though unlikely) that this write fails, but
                    // this function gets called so late that there's not much we
                    // can do about it.
                    writeln!(self.warning_default(), "Failed to wait on pager: {e}").ok();
                }
            }
            UiOutput::BuiltinPaged { pager } => {
                pager.finalize();
            }
            _ => { /* no-op */ }
        }
    }

    pub fn can_prompt() -> bool {
        io::stdout().is_terminal()
            || env::var("JJ_INTERACTIVE")
                .map(|v| v == "1")
                .unwrap_or(false)
    }

    pub fn prompt(&self, prompt: &str) -> io::Result<String> {
        if !Self::can_prompt() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Cannot prompt for input since the output is not connected to a terminal",
            ));
        }
        write!(self.stdout(), "{prompt}: ")?;
        self.stdout().flush()?;
        let mut buf = String::new();
        io::stdin().read_line(&mut buf)?;

        if let Some(trimmed) = buf.strip_suffix('\n') {
            buf = trimmed.to_owned();
        } else if buf.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Prompt cancelled by EOF",
            ));
        }

        Ok(buf)
    }

    /// Repeat the given prompt until the input is one of the specified choices.
    pub fn prompt_choice(
        &self,
        prompt: &str,
        choices: &[impl AsRef<str>],
        default: Option<&str>,
    ) -> io::Result<String> {
        if !Self::can_prompt() {
            if let Some(default) = default {
                // Choose the default automatically without waiting.
                writeln!(self.stdout(), "{prompt}: {default}")?;
                return Ok(default.to_owned());
            }
        }

        loop {
            let choice = self.prompt(prompt)?.trim().to_owned();
            if choice.is_empty() {
                if let Some(default) = default {
                    return Ok(default.to_owned());
                }
            }
            if choices.iter().any(|c| choice == c.as_ref()) {
                return Ok(choice);
            }

            writeln!(self.warning_no_heading(), "unrecognized response")?;
        }
    }

    /// Prompts for a yes-or-no response, with yes = true and no = false.
    pub fn prompt_yes_no(&self, prompt: &str, default: Option<bool>) -> io::Result<bool> {
        let default_str = match &default {
            Some(true) => "(Yn)",
            Some(false) => "(yN)",
            None => "(yn)",
        };
        let default_choice = default.map(|c| if c { "Y" } else { "N" });

        let choice = self.prompt_choice(
            &format!("{} {}", prompt, default_str),
            &["y", "n", "yes", "no", "Yes", "No", "YES", "NO"],
            default_choice,
        )?;
        Ok(choice.starts_with(['y', 'Y']))
    }

    pub fn prompt_password(&self, prompt: &str) -> io::Result<String> {
        if !io::stdout().is_terminal() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Cannot prompt for input since the output is not connected to a terminal",
            ));
        }
        rpassword::prompt_password(format!("{prompt}: "))
    }

    pub fn term_width(&self) -> Option<u16> {
        term_width()
    }
}

#[derive(Debug)]
pub struct ProgressOutput {
    output: Stderr,
}

impl ProgressOutput {
    pub fn write_fmt(&mut self, fmt: fmt::Arguments<'_>) -> io::Result<()> {
        self.output.write_fmt(fmt)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }

    pub fn term_width(&self) -> Option<u16> {
        // Terminal can be resized while progress is displayed, so don't cache it.
        term_width()
    }

    /// Construct a guard object which writes `text` when dropped. Useful for
    /// restoring terminal state.
    pub fn output_guard(&self, text: String) -> OutputGuard {
        OutputGuard {
            text,
            output: io::stderr(),
        }
    }
}

pub struct OutputGuard {
    text: String,
    output: Stderr,
}

impl Drop for OutputGuard {
    #[instrument(skip_all)]
    fn drop(&mut self) {
        _ = self.output.write_all(self.text.as_bytes());
        _ = self.output.flush();
    }
}

#[cfg(unix)]
fn duplicate_child_stdin(stdin: &ChildStdin) -> io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::AsFd as _;
    stdin.as_fd().try_clone_to_owned()
}

#[cfg(windows)]
fn duplicate_child_stdin(stdin: &ChildStdin) -> io::Result<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::AsHandle as _;
    stdin.as_handle().try_clone_to_owned()
}

fn term_width() -> Option<u16> {
    if let Some(cols) = env::var("COLUMNS").ok().and_then(|s| s.parse().ok()) {
        Some(cols)
    } else {
        crossterm::terminal::size().ok().map(|(cols, _)| cols)
    }
}
