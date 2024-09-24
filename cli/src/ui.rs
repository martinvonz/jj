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

use std::env;
use std::error;
use std::fmt;
use std::io;
use std::io::IsTerminal as _;
use std::io::Stderr;
use std::io::StderrLock;
use std::io::Stdout;
use std::io::StdoutLock;
use std::io::Write;
use std::iter;
use std::mem;
use std::process::Child;
use std::process::ChildStdin;
use std::process::Stdio;
use std::str::FromStr;
use std::thread::JoinHandle;

use indoc::indoc;
use itertools::Itertools as _;
use jj_lib::settings::ignore_executable_bit;
use minus::MinusError;
use minus::Pager as MinusPager;
use tracing::instrument;

use crate::command_error::config_error_with_message;
use crate::command_error::CommandError;
use crate::config::CommandNameAndArgs;
use crate::formatter::Formatter;
use crate::formatter::FormatterFactory;
use crate::formatter::HeadingLabeledWriter;
use crate::formatter::LabeledWriter;
use crate::formatter::PlainTextFormatter;

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
    dynamic_pager_thread: JoinHandle<Result<(), MinusError>>,
}

impl Write for &BuiltinPager {
    fn flush(&mut self) -> io::Result<()> {
        // no-op since this is being run in a dynamic pager mode.
        Ok(())
    }

    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Since minus::Pager holds the rx end internally, push_str() doesn't
        // fail even after the paging thread gets terminated.
        if self.dynamic_pager_thread.is_finished() {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let string = std::str::from_utf8(buf).map_err(io::Error::other)?;
        self.pager.push_str(string).map_err(io::Error::other)?;
        Ok(buf.len())
    }
}

impl Default for BuiltinPager {
    fn default() -> Self {
        Self::new()
    }
}

impl BuiltinPager {
    pub fn finalize(self) -> Result<(), MinusError> {
        let dynamic_pager_thread = self.dynamic_pager_thread;
        dynamic_pager_thread.join().unwrap()
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
                minus::dynamic_paging(pager_handle)
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
        let mut cmd = pager_cmd.to_command();
        tracing::info!(?cmd, "spawning pager");
        let mut child = cmd.stdin(Stdio::piped()).spawn()?;
        let child_stdin = child.stdin.take().unwrap();
        Ok(UiOutput::Paged { child, child_stdin })
    }

    fn finalize(self, ui: &Ui) {
        match self {
            UiOutput::Terminal { .. } => { /* no-op */ }
            UiOutput::Paged {
                mut child,
                child_stdin,
            } => {
                drop(child_stdin);
                if let Err(err) = child.wait() {
                    // It's possible (though unlikely) that this write fails, but
                    // this function gets called so late that there's not much we
                    // can do about it.
                    writeln!(
                        ui.warning_default(),
                        "Failed to wait on pager: {err}",
                        err = format_error_with_sources(&err),
                    )
                    .ok();
                }
            }
            UiOutput::BuiltinPaged { pager } => {
                if let Err(minus_error) = pager.finalize() {
                    writeln!(
                        ui.warning_default(),
                        "Built-in pager failed to start: {err}",
                        err = format_error_with_sources(&minus_error),
                    )
                    .ok();
                    writeln!(ui.warning_default(), "The output of this command is lost.").ok();
                    if matches!(
                        minus_error,
                        MinusError::Setup(minus::error::SetupError::InvalidTerminal)
                    ) {
                        if cfg!(windows) {
                            writeln!(
                                ui.hint_default(),
                                indoc! {r#"
                                    jj's builtin pager is likely incompatible with this terminal

                                    This is known to happen with `mintty`, the default Git Bash terminal on Windows.

                                    Possible workarounds:
                                    - Use `jj --no-pager`
                                    - Configure a different pager, see https://martinvonz.github.io/jj/latest/windows/#pagination for Git Bash on Windows
                                    - Use a different terminal (e.g. Windows Terminal or the Command Prompt)
                                    - Use `winpty jj ...`; `winpty` comes with Git Bash or Cygwin."#
                                }
                            ).ok();
                        } else {
                            writeln!(
                                ui.hint_default(),
                                "Try `jj --no-pager`. You can also try `TERM=xterm` or setting up \
                                 a different pager."
                            )
                            .ok();
                        }
                    }
                    // We do not cause a panic or another error so that the exit
                    // code of the command is preserved.
                }
            }
        }
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
    quiet: bool,
    pager_cmd: CommandNameAndArgs,
    paginate: PaginationChoice,
    progress_indicator: bool,
    formatter_factory: FormatterFactory,
    output: UiOutput,
    pub exec_config: Option<bool>,
}

fn progress_indicator_setting(config: &config::Config) -> bool {
    config.get_bool("ui.progress-indicator").unwrap_or(true)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColorChoice {
    Always,
    Never,
    Debug,
    #[default]
    Auto,
}

impl FromStr for ColorChoice {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "always" => Ok(ColorChoice::Always),
            "never" => Ok(ColorChoice::Never),
            "debug" => Ok(ColorChoice::Debug),
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
            ColorChoice::Debug => "debug",
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

fn prepare_formatter_factory(
    config: &config::Config,
    stdout: &Stdout,
) -> Result<FormatterFactory, config::ConfigError> {
    let terminal = stdout.is_terminal();
    let (color, debug) = match color_setting(config) {
        ColorChoice::Always => (true, false),
        ColorChoice::Never => (false, false),
        ColorChoice::Debug => (true, true),
        ColorChoice::Auto => (terminal, false),
    };
    if color {
        FormatterFactory::color(config, debug)
    } else if terminal {
        // Sanitize ANSI escape codes if we're printing to a terminal. Doesn't
        // affect ANSI escape codes that originate from the formatter itself.
        Ok(FormatterFactory::sanitized())
    } else {
        Ok(FormatterFactory::plain_text())
    }
}

fn be_quiet(config: &config::Config) -> bool {
    config.get_bool("ui.quiet").unwrap_or_default()
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
        let quiet = be_quiet(config);
        let formatter_factory = prepare_formatter_factory(config, &io::stdout())?;
        let progress_indicator = progress_indicator_setting(config);
        Ok(Ui {
            quiet,
            formatter_factory,
            pager_cmd: pager_setting(config)?,
            paginate: pagination_setting(config)?,
            progress_indicator,
            output: UiOutput::new_terminal(),
            exec_config: ignore_executable_bit(config),
        })
    }

    pub fn reset(&mut self, config: &config::Config) -> Result<(), CommandError> {
        self.quiet = be_quiet(config);
        self.paginate = pagination_setting(config)?;
        self.pager_cmd = pager_setting(config)?;
        self.progress_indicator = progress_indicator_setting(config);
        self.formatter_factory = prepare_formatter_factory(config, &io::stdout())?;
        Ok(())
    }

    /// Switches the output to use the pager, if allowed.
    #[instrument(skip_all)]
    pub fn request_pager(&mut self) {
        match self.paginate {
            PaginationChoice::Never => return,
            PaginationChoice::Auto => {}
        }
        if !matches!(&self.output, UiOutput::Terminal { stdout, .. } if stdout.is_terminal()) {
            return;
        }

        let use_builtin_pager = matches!(
            &self.pager_cmd, CommandNameAndArgs::String(name) if name == BUILTIN_PAGER_NAME);
        let new_output = if use_builtin_pager {
            Some(UiOutput::new_builtin())
        } else {
            UiOutput::new_paged(&self.pager_cmd)
                .inspect_err(|err| {
                    // The pager executable couldn't be found or couldn't be run
                    writeln!(
                        self.warning_default(),
                        "Failed to spawn pager '{name}': {err}",
                        name = self.pager_cmd.split_name(),
                        err = format_error_with_sources(err),
                    )
                    .ok();
                    writeln!(self.hint_default(), "Consider using the `:builtin` pager.").ok();
                })
                .ok()
        };
        if let Some(output) = new_output {
            self.output = output;
        }
    }

    pub fn color(&self) -> bool {
        self.formatter_factory.is_color()
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

    /// Writer to print an update that's not part of the command's main output.
    pub fn status(&self) -> Box<dyn Write + '_> {
        if self.quiet {
            Box::new(io::sink())
        } else {
            Box::new(self.stderr())
        }
    }

    /// A formatter to print an update that's not part of the command's main
    /// output. Returns `None` if `--quiet` was requested.
    pub fn status_formatter(&self) -> Option<Box<dyn Formatter + '_>> {
        (!self.quiet).then(|| self.stderr_formatter())
    }

    /// Writer to print hint with the default "Hint: " heading.
    pub fn hint_default(
        &self,
    ) -> HeadingLabeledWriter<Box<dyn Formatter + '_>, &'static str, &'static str> {
        self.hint_with_heading("Hint: ")
    }

    /// Writer to print hint without the "Hint: " heading.
    pub fn hint_no_heading(&self) -> LabeledWriter<Box<dyn Formatter + '_>, &'static str> {
        let formatter = self
            .status_formatter()
            .unwrap_or_else(|| Box::new(PlainTextFormatter::new(io::sink())));
        LabeledWriter::new(formatter, "hint")
    }

    /// Writer to print hint with the given heading.
    pub fn hint_with_heading<H: fmt::Display>(
        &self,
        heading: H,
    ) -> HeadingLabeledWriter<Box<dyn Formatter + '_>, &'static str, H> {
        self.hint_no_heading().with_heading(heading)
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
        self.warning_no_heading().with_heading(heading)
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
        self.error_no_heading().with_heading(heading)
    }

    /// Waits for the pager exits.
    #[instrument(skip_all)]
    pub fn finalize_pager(&mut self) {
        let old_output = mem::replace(&mut self.output, UiOutput::new_terminal());
        old_output.finalize(self);
    }

    pub fn can_prompt() -> bool {
        io::stdout().is_terminal()
            || env::var("JJ_INTERACTIVE")
                .map(|v| v == "1")
                .unwrap_or(false)
    }

    #[allow(unknown_lints)] // XXX FIXME (aseipp): nightly bogons; re-test this occasionally
    #[allow(clippy::assigning_clones)]
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

    pub fn term_width(&self) -> usize {
        term_width().unwrap_or(80).into()
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

fn format_error_with_sources(err: &dyn error::Error) -> impl fmt::Display + '_ {
    iter::successors(Some(err), |&err| err.source()).format(": ")
}

fn term_width() -> Option<u16> {
    if let Some(cols) = env::var("COLUMNS").ok().and_then(|s| s.parse().ok()) {
        Some(cols)
    } else {
        crossterm::terminal::size().ok().map(|(cols, _)| cols)
    }
}
