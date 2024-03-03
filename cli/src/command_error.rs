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
use jj_lib::tree::TreeMergeError;
use jj_lib::working_copy::{ResetError, SnapshotError, WorkingCopyStateError};
use jj_lib::workspace::WorkspaceInitError;
use thiserror::Error;

use crate::merge_tools::{
    ConflictResolveError, DiffEditError, DiffGenerateError, MergeToolConfigError,
};
use crate::template_parser::{TemplateParseError, TemplateParseErrorKind};
use crate::ui::Ui;

#[derive(Clone, Debug)]
pub enum CommandError {
    UserError {
        err: Arc<dyn error::Error + Send + Sync>,
        hint: Option<String>,
    },
    ConfigError(String),
    /// Invalid command line
    CliError(String),
    /// Invalid command line detected by clap
    ClapCliError {
        err: Arc<clap::Error>,
        hint: Option<String>,
    },
    BrokenPipe,
    InternalError(Arc<dyn error::Error + Send + Sync>),
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
    user_error_with_hint_opt(err, None)
}

pub fn user_error_with_hint(
    err: impl Into<Box<dyn error::Error + Send + Sync>>,
    hint: impl Into<String>,
) -> CommandError {
    user_error_with_hint_opt(err, Some(hint.into()))
}

pub fn user_error_with_message(
    message: impl Into<String>,
    source: impl Into<Box<dyn error::Error + Send + Sync>>,
) -> CommandError {
    user_error_with_hint_opt(ErrorWithMessage::new(message, source), None)
}

pub fn user_error_with_message_and_hint(
    message: impl Into<String>,
    hint: impl Into<String>,
    source: impl Into<Box<dyn error::Error + Send + Sync>>,
) -> CommandError {
    user_error_with_hint_opt(ErrorWithMessage::new(message, source), Some(hint.into()))
}

pub fn user_error_with_hint_opt(
    err: impl Into<Box<dyn error::Error + Send + Sync>>,
    hint: Option<String>,
) -> CommandError {
    CommandError::UserError {
        err: Arc::from(err.into()),
        hint,
    }
}

pub fn internal_error(err: impl Into<Box<dyn error::Error + Send + Sync>>) -> CommandError {
    CommandError::InternalError(Arc::from(err.into()))
}

pub fn internal_error_with_message(
    message: impl Into<String>,
    source: impl Into<Box<dyn error::Error + Send + Sync>>,
) -> CommandError {
    CommandError::InternalError(Arc::new(ErrorWithMessage::new(message, source)))
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
        if err.kind() == io::ErrorKind::BrokenPipe {
            CommandError::BrokenPipe
        } else {
            user_error(err)
        }
    }
}

impl From<config::ConfigError> for CommandError {
    fn from(err: config::ConfigError) -> Self {
        CommandError::ConfigError(err.to_string())
    }
}

impl From<crate::config::ConfigError> for CommandError {
    fn from(err: crate::config::ConfigError) -> Self {
        CommandError::ConfigError(err.to_string())
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
        internal_error_with_message("Unexpected error from backend", err)
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
            SnapshotError::NewFileTooLarge { .. } => user_error_with_message_and_hint(
                "Failed to snapshot the working copy",
                r#"Increase the value of the `snapshot.max-new-file-size` config option if you
want this file to be snapshotted. Otherwise add it to your `.gitignore` file."#,
                err,
            ),
            err => internal_error_with_message("Failed to snapshot the working copy", err),
        }
    }
}

impl From<TreeMergeError> for CommandError {
    fn from(err: TreeMergeError) -> Self {
        internal_error_with_message("Merge failed", err)
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
        let message = format!("Failed to import refs from underlying Git repo: {err}");
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
        user_error_with_hint_opt(message, hint)
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
        let err_chain = iter::successors(Some(&err), |e| e.origin());
        let message = err_chain.clone().join("\n");
        // Only for the bottom error, which is usually the root cause
        let hint = match err_chain.last().unwrap().kind() {
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
            _ => None,
        };
        user_error_with_hint_opt(format!("Failed to parse revset: {message}"), hint)
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
        user_error_with_hint_opt(err, hint)
    }
}

impl From<TemplateParseError> for CommandError {
    fn from(err: TemplateParseError) -> Self {
        let err_chain = iter::successors(Some(&err), |e| e.origin());
        let message = err_chain.clone().join("\n");
        // Only for the bottom error, which is usually the root cause
        let hint = match err_chain.last().unwrap().kind() {
            TemplateParseErrorKind::NoSuchKeyword { candidates, .. }
            | TemplateParseErrorKind::NoSuchFunction { candidates, .. }
            | TemplateParseErrorKind::NoSuchMethod { candidates, .. } => {
                format_similarity_hint(candidates)
            }
            _ => None,
        };
        user_error_with_hint_opt(format!("Failed to parse template: {message}"), hint)
    }
}

impl From<FsPathParseError> for CommandError {
    fn from(err: FsPathParseError) -> Self {
        user_error(err)
    }
}

impl From<clap::Error> for CommandError {
    fn from(err: clap::Error) -> Self {
        CommandError::ClapCliError {
            err: Arc::new(err),
            hint: None,
        }
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

const BROKEN_PIPE_EXIT_CODE: u8 = 3;

pub(crate) fn handle_command_result(ui: &mut Ui, result: Result<(), CommandError>) -> ExitCode {
    try_handle_command_result(ui, result).unwrap_or_else(|_| ExitCode::from(BROKEN_PIPE_EXIT_CODE))
}

fn try_handle_command_result(
    ui: &mut Ui,
    result: Result<(), CommandError>,
) -> io::Result<ExitCode> {
    match &result {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(CommandError::UserError { err, hint }) => {
            writeln!(ui.error(), "Error: {err}")?;
            print_error_sources(ui, err.source())?;
            if let Some(hint) = hint {
                writeln!(ui.hint(), "Hint: {hint}")?;
            }
            Ok(ExitCode::from(1))
        }
        Err(CommandError::ConfigError(message)) => {
            writeln!(ui.error(), "Config error: {message}")?;
            writeln!(
                ui.hint(),
                "For help, see https://github.com/martinvonz/jj/blob/main/docs/config.md."
            )?;
            Ok(ExitCode::from(1))
        }
        Err(CommandError::CliError(message)) => {
            writeln!(ui.error(), "Error: {message}")?;
            Ok(ExitCode::from(2))
        }
        Err(CommandError::ClapCliError { err, hint }) => {
            let clap_str = if ui.color() {
                err.render().ansi().to_string()
            } else {
                err.render().to_string()
            };

            match err.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    ui.request_pager()
                }
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
            if let Some(hint) = hint {
                writeln!(ui.hint(), "Hint: {hint}")?;
            }
            Ok(ExitCode::from(2))
        }
        Err(CommandError::BrokenPipe) => {
            // A broken pipe is not an error, but a signal to exit gracefully.
            Ok(ExitCode::from(BROKEN_PIPE_EXIT_CODE))
        }
        Err(CommandError::InternalError(err)) => {
            writeln!(ui.error(), "Internal error: {err}")?;
            print_error_sources(ui, err.source())?;
            Ok(ExitCode::from(255))
        }
    }
}

fn print_error_sources(ui: &Ui, source: Option<&dyn error::Error>) -> io::Result<()> {
    let Some(err) = source else {
        return Ok(());
    };
    if err.source().is_none() {
        writeln!(ui.stderr(), "Caused by: {err}")?;
    } else {
        writeln!(ui.stderr(), "Caused by:")?;
        for (i, err) in iter::successors(Some(err), |err| err.source()).enumerate() {
            writeln!(ui.stderr(), "{n}: {err}", n = i + 1)?;
        }
    }
    Ok(())
}
