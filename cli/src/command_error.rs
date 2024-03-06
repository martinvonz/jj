// Copyright 2022-2024 The Jujutsu Authors
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
use std::process::ExitCode;
use std::sync::Arc;
use std::{error, io, iter, str};

use itertools::Itertools as _;
use jj_lib::backend::BackendError;
use jj_lib::git::{GitConfigParseError, GitExportError, GitImportError, GitRemoteManagementError};
use jj_lib::gitignore::GitIgnoreError;
use jj_lib::op_heads_store::OpHeadResolutionError;
use jj_lib::op_store::OpStoreError;
use jj_lib::op_walk::OpsetEvaluationError;
use jj_lib::repo::{CheckOutCommitError, EditCommitError, RepoLoaderError, RewriteRootCommit};
use jj_lib::repo_path::FsPathParseError;
use jj_lib::revset::{
    RevsetEvaluationError, RevsetParseError, RevsetParseErrorKind, RevsetResolutionError,
};
use jj_lib::signing::SignInitError;
use jj_lib::str_util::StringPatternParseError;
use jj_lib::working_copy::{ResetError, SnapshotError, WorkingCopyStateError};
use jj_lib::workspace::WorkspaceInitError;
use thiserror::Error;

use crate::formatter::{FormatRecorder, Formatter};
use crate::merge_tools::{
    ConflictResolveError, DiffEditError, DiffGenerateError, MergeToolConfigError,
};
use crate::revset_util::UserRevsetEvaluationError;
use crate::template_parser::{TemplateParseError, TemplateParseErrorKind};
use crate::ui::Ui;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandErrorKind {
    User,
    Config,
    /// Invalid command line. The inner error type may be `clap::Error`.
    Cli,
    BrokenPipe,
    Internal,
}

#[derive(Clone, Debug)]
pub struct CommandError {
    pub kind: CommandErrorKind,
    pub error: Arc<dyn error::Error + Send + Sync>,
    pub hints: Vec<ErrorHint>,
}

impl CommandError {
    pub fn new(
        kind: CommandErrorKind,
        err: impl Into<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        CommandError {
            kind,
            error: Arc::from(err.into()),
            hints: vec![],
        }
    }

    pub fn with_message(
        kind: CommandErrorKind,
        message: impl Into<String>,
        source: impl Into<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        Self::new(kind, ErrorWithMessage::new(message, source))
    }

    /// Returns error with the given plain-text `hint` attached.
    pub fn hinted(mut self, hint: impl Into<String>) -> Self {
        self.add_hint(hint);
        self
    }

    /// Appends plain-text `hint` to the error.
    pub fn add_hint(&mut self, hint: impl Into<String>) {
        self.hints.push(ErrorHint::PlainText(hint.into()));
    }

    /// Appends formatted `hint` to the error.
    pub fn add_formatted_hint(&mut self, hint: FormatRecorder) {
        self.hints.push(ErrorHint::Formatted(hint));
    }

    /// Constructs formatted hint and appends it to the error.
    pub fn add_formatted_hint_with(
        &mut self,
        write: impl FnOnce(&mut dyn Formatter) -> io::Result<()>,
    ) {
        let mut formatter = FormatRecorder::new();
        write(&mut formatter).expect("write() to FormatRecorder should never fail");
        self.add_formatted_hint(formatter);
    }

    /// Appends 0 or more plain-text `hints` to the error.
    pub fn extend_hints(&mut self, hints: impl IntoIterator<Item = String>) {
        self.hints
            .extend(hints.into_iter().map(ErrorHint::PlainText));
    }
}

#[derive(Clone, Debug)]
pub enum ErrorHint {
    PlainText(String),
    Formatted(FormatRecorder),
}

/// Wraps error with user-visible message.
#[derive(Debug, Error)]
#[error("{message}")]
struct ErrorWithMessage {
    message: String,
    source: Box<dyn error::Error + Send + Sync>,
}

impl ErrorWithMessage {
    fn new(
        message: impl Into<String>,
        source: impl Into<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        ErrorWithMessage {
            message: message.into(),
            source: source.into(),
        }
    }
}

pub fn user_error(err: impl Into<Box<dyn error::Error + Send + Sync>>) -> CommandError {
    CommandError::new(CommandErrorKind::User, err)
}

pub fn user_error_with_hint(
    err: impl Into<Box<dyn error::Error + Send + Sync>>,
    hint: impl Into<String>,
) -> CommandError {
    user_error(err).hinted(hint)
}

pub fn user_error_with_message(
    message: impl Into<String>,
    source: impl Into<Box<dyn error::Error + Send + Sync>>,
) -> CommandError {
    CommandError::with_message(CommandErrorKind::User, message, source)
}

pub fn config_error(err: impl Into<Box<dyn error::Error + Send + Sync>>) -> CommandError {
    CommandError::new(CommandErrorKind::Config, err)
}

pub fn config_error_with_message(
    message: impl Into<String>,
    source: impl Into<Box<dyn error::Error + Send + Sync>>,
) -> CommandError {
    CommandError::with_message(CommandErrorKind::Config, message, source)
}

pub fn cli_error(err: impl Into<Box<dyn error::Error + Send + Sync>>) -> CommandError {
    CommandError::new(CommandErrorKind::Cli, err)
}

pub fn internal_error(err: impl Into<Box<dyn error::Error + Send + Sync>>) -> CommandError {
    CommandError::new(CommandErrorKind::Internal, err)
}

pub fn internal_error_with_message(
    message: impl Into<String>,
    source: impl Into<Box<dyn error::Error + Send + Sync>>,
) -> CommandError {
    CommandError::with_message(CommandErrorKind::Internal, message, source)
}

fn format_similarity_hint<S: AsRef<str>>(candidates: &[S]) -> Option<String> {
    match candidates {
        [] => None,
        names => {
            let quoted_names = names
                .iter()
                .map(|s| format!(r#""{}""#, s.as_ref()))
                .join(", ");
            Some(format!("Did you mean {quoted_names}?"))
        }
    }
}

impl From<io::Error> for CommandError {
    fn from(err: io::Error) -> Self {
        let kind = match err.kind() {
            io::ErrorKind::BrokenPipe => CommandErrorKind::BrokenPipe,
            _ => CommandErrorKind::User,
        };
        CommandError::new(kind, err)
    }
}

impl From<config::ConfigError> for CommandError {
    fn from(err: config::ConfigError) -> Self {
        config_error(err)
    }
}

impl From<crate::config::ConfigError> for CommandError {
    fn from(err: crate::config::ConfigError) -> Self {
        config_error(err)
    }
}

impl From<RewriteRootCommit> for CommandError {
    fn from(err: RewriteRootCommit) -> Self {
        internal_error_with_message("Attempted to rewrite the root commit", err)
    }
}

impl From<EditCommitError> for CommandError {
    fn from(err: EditCommitError) -> Self {
        internal_error_with_message("Failed to edit a commit", err)
    }
}

impl From<CheckOutCommitError> for CommandError {
    fn from(err: CheckOutCommitError) -> Self {
        internal_error_with_message("Failed to check out a commit", err)
    }
}

impl From<BackendError> for CommandError {
    fn from(err: BackendError) -> Self {
        match &err {
            BackendError::Unsupported(_) => user_error(err),
            _ => internal_error_with_message("Unexpected error from backend", err),
        }
    }
}

impl From<WorkspaceInitError> for CommandError {
    fn from(err: WorkspaceInitError) -> Self {
        match err {
            WorkspaceInitError::DestinationExists(_) => {
                user_error("The target repo already exists")
            }
            WorkspaceInitError::NonUnicodePath => {
                user_error("The target repo path contains non-unicode characters")
            }
            WorkspaceInitError::CheckOutCommit(err) => {
                internal_error_with_message("Failed to check out the initial commit", err)
            }
            WorkspaceInitError::Path(err) => {
                internal_error_with_message("Failed to access the repository", err)
            }
            WorkspaceInitError::Backend(err) => {
                user_error_with_message("Failed to access the repository", err)
            }
            WorkspaceInitError::WorkingCopyState(err) => {
                internal_error_with_message("Failed to access the repository", err)
            }
            WorkspaceInitError::SignInit(err @ SignInitError::UnknownBackend(_)) => user_error(err),
            WorkspaceInitError::SignInit(err) => internal_error(err),
        }
    }
}

impl From<OpHeadResolutionError> for CommandError {
    fn from(err: OpHeadResolutionError) -> Self {
        match err {
            OpHeadResolutionError::NoHeads => {
                internal_error_with_message("Corrupt repository", err)
            }
        }
    }
}

impl From<OpsetEvaluationError> for CommandError {
    fn from(err: OpsetEvaluationError) -> Self {
        match err {
            OpsetEvaluationError::OpsetResolution(err) => user_error(err),
            OpsetEvaluationError::OpHeadResolution(err) => err.into(),
            OpsetEvaluationError::OpStore(err) => err.into(),
        }
    }
}

impl From<SnapshotError> for CommandError {
    fn from(err: SnapshotError) -> Self {
        match err {
            SnapshotError::NewFileTooLarge { .. } => {
                user_error_with_message("Failed to snapshot the working copy", err).hinted(
                    r#"Increase the value of the `snapshot.max-new-file-size` config option if you
want this file to be snapshotted. Otherwise add it to your `.gitignore` file."#,
                )
            }
            err => internal_error_with_message("Failed to snapshot the working copy", err),
        }
    }
}

impl From<OpStoreError> for CommandError {
    fn from(err: OpStoreError) -> Self {
        internal_error_with_message("Failed to load an operation", err)
    }
}

impl From<RepoLoaderError> for CommandError {
    fn from(err: RepoLoaderError) -> Self {
        internal_error_with_message("Failed to load the repo", err)
    }
}

impl From<ResetError> for CommandError {
    fn from(err: ResetError) -> Self {
        internal_error_with_message("Failed to reset the working copy", err)
    }
}

impl From<DiffEditError> for CommandError {
    fn from(err: DiffEditError) -> Self {
        user_error_with_message("Failed to edit diff", err)
    }
}

impl From<DiffGenerateError> for CommandError {
    fn from(err: DiffGenerateError) -> Self {
        user_error_with_message("Failed to generate diff", err)
    }
}

impl From<ConflictResolveError> for CommandError {
    fn from(err: ConflictResolveError) -> Self {
        user_error_with_message("Failed to resolve conflicts", err)
    }
}

impl From<MergeToolConfigError> for CommandError {
    fn from(err: MergeToolConfigError) -> Self {
        user_error_with_message("Failed to load tool configuration", err)
    }
}

impl From<git2::Error> for CommandError {
    fn from(err: git2::Error) -> Self {
        user_error_with_message("Git operation failed", err)
    }
}

impl From<GitImportError> for CommandError {
    fn from(err: GitImportError) -> Self {
        let hint = match &err {
            GitImportError::MissingHeadTarget { .. }
            | GitImportError::MissingRefAncestor { .. } => Some(
                "\
Is this Git repository a shallow or partial clone (cloned with the --depth or --filter \
                 argument)?
jj currently does not support shallow/partial clones. To use jj with this \
                 repository, try
unshallowing the repository (https://stackoverflow.com/q/6802145) or re-cloning with the full
repository contents."
                    .to_string(),
            ),
            GitImportError::RemoteReservedForLocalGitRepo => {
                Some("Run `jj git remote rename` to give different name.".to_string())
            }
            GitImportError::InternalBackend(_) => None,
            GitImportError::InternalGitError(_) => None,
            GitImportError::UnexpectedBackend => None,
        };
        let mut cmd_err =
            user_error_with_message("Failed to import refs from underlying Git repo", err);
        cmd_err.extend_hints(hint);
        cmd_err
    }
}

impl From<GitExportError> for CommandError {
    fn from(err: GitExportError) -> Self {
        internal_error_with_message("Failed to export refs to underlying Git repo", err)
    }
}

impl From<GitRemoteManagementError> for CommandError {
    fn from(err: GitRemoteManagementError) -> Self {
        user_error(err)
    }
}

impl From<RevsetEvaluationError> for CommandError {
    fn from(err: RevsetEvaluationError) -> Self {
        user_error(err)
    }
}

impl From<RevsetParseError> for CommandError {
    fn from(err: RevsetParseError) -> Self {
        // Only for the bottom error, which is usually the root cause
        let bottom_err = iter::successors(Some(&err), |e| e.origin()).last().unwrap();
        let hint = match bottom_err.kind() {
            RevsetParseErrorKind::NotPrefixOperator {
                op: _,
                similar_op,
                description,
            }
            | RevsetParseErrorKind::NotPostfixOperator {
                op: _,
                similar_op,
                description,
            }
            | RevsetParseErrorKind::NotInfixOperator {
                op: _,
                similar_op,
                description,
            } => Some(format!("Did you mean '{similar_op}' for {description}?")),
            RevsetParseErrorKind::NoSuchFunction {
                name: _,
                candidates,
            } => format_similarity_hint(candidates),
            RevsetParseErrorKind::InvalidFunctionArguments { .. } => {
                find_source_parse_error_hint(bottom_err)
            }
            _ => None,
        };
        let mut cmd_err =
            user_error_with_message(format!("Failed to parse revset: {}", err.kind()), err);
        cmd_err.extend_hints(hint);
        cmd_err
    }
}

impl From<RevsetResolutionError> for CommandError {
    fn from(err: RevsetResolutionError) -> Self {
        let hint = match &err {
            RevsetResolutionError::NoSuchRevision {
                name: _,
                candidates,
            } => format_similarity_hint(candidates),
            RevsetResolutionError::EmptyString
            | RevsetResolutionError::WorkspaceMissingWorkingCopy { .. }
            | RevsetResolutionError::AmbiguousCommitIdPrefix(_)
            | RevsetResolutionError::AmbiguousChangeIdPrefix(_)
            | RevsetResolutionError::StoreError(_) => None,
        };
        let mut cmd_err = user_error(err);
        cmd_err.extend_hints(hint);
        cmd_err
    }
}

impl From<UserRevsetEvaluationError> for CommandError {
    fn from(err: UserRevsetEvaluationError) -> Self {
        match err {
            UserRevsetEvaluationError::Resolution(err) => err.into(),
            UserRevsetEvaluationError::Evaluation(err) => err.into(),
        }
    }
}

impl From<TemplateParseError> for CommandError {
    fn from(err: TemplateParseError) -> Self {
        // Only for the bottom error, which is usually the root cause
        let bottom_err = iter::successors(Some(&err), |e| e.origin()).last().unwrap();
        let hint = match bottom_err.kind() {
            TemplateParseErrorKind::NoSuchKeyword { candidates, .. }
            | TemplateParseErrorKind::NoSuchFunction { candidates, .. }
            | TemplateParseErrorKind::NoSuchMethod { candidates, .. } => {
                format_similarity_hint(candidates)
            }
            TemplateParseErrorKind::InvalidArguments { .. }
            | TemplateParseErrorKind::Expression(_) => find_source_parse_error_hint(bottom_err),
            _ => None,
        };
        let mut cmd_err =
            user_error_with_message(format!("Failed to parse template: {}", err.kind()), err);
        cmd_err.extend_hints(hint);
        cmd_err
    }
}

impl From<FsPathParseError> for CommandError {
    fn from(err: FsPathParseError) -> Self {
        // TODO: implement pattern prefix like "root:<path>" or "--cwd" option,
        // and suggest it if the user input looks like repo-relative path #3216.
        user_error(err)
    }
}

impl From<clap::Error> for CommandError {
    fn from(err: clap::Error) -> Self {
        let hint = find_source_parse_error_hint(&err);
        let mut cmd_err = cli_error(err);
        cmd_err.extend_hints(hint);
        cmd_err
    }
}

impl From<GitConfigParseError> for CommandError {
    fn from(err: GitConfigParseError) -> Self {
        internal_error_with_message("Failed to parse Git config", err)
    }
}

impl From<WorkingCopyStateError> for CommandError {
    fn from(err: WorkingCopyStateError) -> Self {
        internal_error_with_message("Failed to access working copy state", err)
    }
}

impl From<GitIgnoreError> for CommandError {
    fn from(err: GitIgnoreError) -> Self {
        user_error_with_message("Failed to process .gitignore.", err)
    }
}

fn find_source_parse_error_hint(err: &dyn error::Error) -> Option<String> {
    let source = err.source()?;
    if let Some(source) = source.downcast_ref() {
        string_pattern_parse_error_hint(source)
    } else {
        None
    }
}

fn string_pattern_parse_error_hint(err: &StringPatternParseError) -> Option<String> {
    match err {
        StringPatternParseError::InvalidKind(_) => {
            Some("Try prefixing with one of `exact:`, `glob:` or `substring:`".into())
        }
        StringPatternParseError::GlobPattern(_) => None,
    }
}

const BROKEN_PIPE_EXIT_CODE: u8 = 3;

pub(crate) fn handle_command_result(ui: &mut Ui, result: Result<(), CommandError>) -> ExitCode {
    try_handle_command_result(ui, result).unwrap_or_else(|_| ExitCode::from(BROKEN_PIPE_EXIT_CODE))
}

fn try_handle_command_result(
    ui: &mut Ui,
    result: Result<(), CommandError>,
) -> io::Result<ExitCode> {
    let Err(cmd_err) = &result else {
        return Ok(ExitCode::SUCCESS);
    };
    let err = &cmd_err.error;
    let hints = &cmd_err.hints;
    match cmd_err.kind {
        CommandErrorKind::User => {
            print_error(ui, "Error: ", err, hints)?;
            Ok(ExitCode::from(1))
        }
        CommandErrorKind::Config => {
            print_error(ui, "Config error: ", err, hints)?;
            writeln!(
                ui.hint_no_heading(),
                "For help, see https://github.com/martinvonz/jj/blob/main/docs/config.md."
            )?;
            Ok(ExitCode::from(1))
        }
        CommandErrorKind::Cli => {
            if let Some(err) = err.downcast_ref::<clap::Error>() {
                handle_clap_error(ui, err, hints)
            } else {
                print_error(ui, "Error: ", err, hints)?;
                Ok(ExitCode::from(2))
            }
        }
        CommandErrorKind::BrokenPipe => {
            // A broken pipe is not an error, but a signal to exit gracefully.
            Ok(ExitCode::from(BROKEN_PIPE_EXIT_CODE))
        }
        CommandErrorKind::Internal => {
            print_error(ui, "Internal error: ", err, hints)?;
            Ok(ExitCode::from(255))
        }
    }
}

fn print_error(
    ui: &Ui,
    heading: &str,
    err: &dyn error::Error,
    hints: &[ErrorHint],
) -> io::Result<()> {
    writeln!(ui.error_with_heading(heading), "{err}")?;
    print_error_sources(ui, err.source())?;
    print_error_hints(ui, hints)?;
    Ok(())
}

fn print_error_sources(ui: &Ui, source: Option<&dyn error::Error>) -> io::Result<()> {
    let Some(err) = source else {
        return Ok(());
    };
    ui.stderr_formatter()
        .with_label("error_source", |formatter| {
            if err.source().is_none() {
                write!(formatter.labeled("heading"), "Caused by: ")?;
                writeln!(formatter, "{err}")?;
            } else {
                writeln!(formatter.labeled("heading"), "Caused by:")?;
                for (i, err) in iter::successors(Some(err), |err| err.source()).enumerate() {
                    write!(formatter.labeled("heading"), "{}: ", i + 1)?;
                    writeln!(formatter, "{err}")?;
                }
            }
            Ok(())
        })
}

fn print_error_hints(ui: &Ui, hints: &[ErrorHint]) -> io::Result<()> {
    for hint in hints {
        match hint {
            ErrorHint::PlainText(message) => {
                writeln!(ui.hint_default(), "{message}")?;
            }
            ErrorHint::Formatted(recorded) => {
                ui.stderr_formatter().with_label("hint", |formatter| {
                    write!(formatter.labeled("heading"), "Hint: ")?;
                    recorded.replay(formatter)?;
                    // Formatted hint is usually multi-line text, and it's
                    // convenient if trailing "\n" doesn't have to be omitted.
                    if !recorded.data().ends_with(b"\n") {
                        writeln!(formatter)?;
                    }
                    Ok(())
                })?;
            }
        }
    }
    Ok(())
}

fn handle_clap_error(ui: &mut Ui, err: &clap::Error, hints: &[ErrorHint]) -> io::Result<ExitCode> {
    let clap_str = if ui.color() {
        err.render().ansi().to_string()
    } else {
        err.render().to_string()
    };

    match err.kind() {
        clap::error::ErrorKind::DisplayHelp
        | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => ui.request_pager(),
        _ => {}
    };
    // Definitions for exit codes and streams come from
    // https://github.com/clap-rs/clap/blob/master/src/error/mod.rs
    match err.kind() {
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
            write!(ui.stdout(), "{clap_str}")?;
            return Ok(ExitCode::SUCCESS);
        }
        _ => {}
    }
    write!(ui.stderr(), "{clap_str}")?;
    print_error_hints(ui, hints)?;
    Ok(ExitCode::from(2))
}
