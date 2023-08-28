// Copyright 2022 The Jujutsu Authors
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

use std::collections::{HashSet, VecDeque};
use std::env::{self, ArgsOs, VarError};
use std::ffi::{OsStr, OsString};
use std::fmt::Debug;
use std::iter;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;
use std::time::SystemTime;

use clap::builder::{NonEmptyStringValueParser, TypedValueParser, ValueParserFactory};
use clap::{Arg, ArgAction, ArgMatches, Command, FromArgMatches};
use git2::{Oid, Repository};
use indexmap::IndexSet;
use itertools::Itertools;
use jj_lib::backend::{BackendError, ChangeId, CommitId, MergedTreeId, ObjectId};
use jj_lib::commit::Commit;
use jj_lib::git::{GitConfigParseError, GitExportError, GitImportError, GitRemoteManagementError};
use jj_lib::git_backend::GitBackend;
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::hex_util::to_reverse_hex;
use jj_lib::id_prefix::IdPrefixContext;
use jj_lib::matchers::{EverythingMatcher, Matcher, PrefixMatcher, Visit};
use jj_lib::merged_tree::{MergedTree, MergedTreeBuilder};
use jj_lib::op_heads_store::{self, OpHeadResolutionError, OpHeadsStore};
use jj_lib::op_store::{OpStore, OpStoreError, OperationId, RefTarget, WorkspaceId};
use jj_lib::operation::Operation;
use jj_lib::repo::{
    CheckOutCommitError, EditCommitError, MutableRepo, ReadonlyRepo, Repo, RepoLoader,
    RepoLoaderError, RewriteRootCommit, StoreFactories, StoreLoadError,
};
use jj_lib::repo_path::{FsPathParseError, RepoPath};
use jj_lib::revset::{
    DefaultSymbolResolver, Revset, RevsetAliasesMap, RevsetEvaluationError, RevsetExpression,
    RevsetIteratorExt, RevsetParseContext, RevsetParseError, RevsetParseErrorKind,
    RevsetResolutionError, RevsetWorkspaceContext,
};
use jj_lib::settings::{ConfigResultExt as _, UserSettings};
use jj_lib::transaction::Transaction;
use jj_lib::tree::TreeMergeError;
use jj_lib::view::RefName;
use jj_lib::working_copy::{
    CheckoutStats, LockedWorkingCopy, ResetError, SnapshotError, SnapshotOptions, TreeStateError,
    WorkingCopy,
};
use jj_lib::workspace::{Workspace, WorkspaceInitError, WorkspaceLoadError, WorkspaceLoader};
use jj_lib::{dag_walk, file_util, git, revset};
use once_cell::unsync::OnceCell;
use thiserror::Error;
use toml_edit;
use tracing::instrument;
use tracing_chrome::ChromeLayerBuilder;
use tracing_subscriber::prelude::*;

use crate::config::{
    new_config_path, AnnotatedValue, CommandNameAndArgs, ConfigSource, LayeredConfigs,
};
use crate::formatter::{FormatRecorder, Formatter, PlainTextFormatter};
use crate::merge_tools::{ConflictResolveError, DiffEditError, DiffGenerateError};
use crate::template_parser::{TemplateAliasesMap, TemplateParseError};
use crate::templater::Template;
use crate::ui::{ColorChoice, Ui};
use crate::{commit_templater, text_util};

#[derive(Clone, Debug)]
pub enum CommandError {
    UserError {
        message: String,
        hint: Option<String>,
    },
    ConfigError(String),
    /// Invalid command line
    CliError(String),
    /// Invalid command line detected by clap
    ClapCliError(Arc<clap::Error>),
    BrokenPipe,
    InternalError(String),
}

pub fn user_error(message: impl Into<String>) -> CommandError {
    CommandError::UserError {
        message: message.into(),
        hint: None,
    }
}
pub fn user_error_with_hint(message: impl Into<String>, hint: impl Into<String>) -> CommandError {
    CommandError::UserError {
        message: message.into(),
        hint: Some(hint.into()),
    }
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

impl From<std::io::Error> for CommandError {
    fn from(err: std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::BrokenPipe {
            CommandError::BrokenPipe
        } else {
            // TODO: Record the error as a chained cause
            CommandError::InternalError(format!("I/O error: {err}"))
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
        CommandError::InternalError(format!("Attempted to rewrite the root commit: {err}"))
    }
}

impl From<EditCommitError> for CommandError {
    fn from(err: EditCommitError) -> Self {
        CommandError::InternalError(format!("Failed to edit a commit: {err}"))
    }
}

impl From<CheckOutCommitError> for CommandError {
    fn from(err: CheckOutCommitError) -> Self {
        CommandError::InternalError(format!("Failed to check out a commit: {err}"))
    }
}

impl From<BackendError> for CommandError {
    fn from(err: BackendError) -> Self {
        user_error(format!("Unexpected error from backend: {err}"))
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
            WorkspaceInitError::CheckOutCommit(err) => CommandError::InternalError(format!(
                "Failed to check out the initial commit: {err}"
            )),
            WorkspaceInitError::Path(err) => {
                CommandError::InternalError(format!("Failed to access the repository: {err}"))
            }
            WorkspaceInitError::Backend(err) => {
                user_error(format!("Failed to access the repository: {err}"))
            }
            WorkspaceInitError::TreeState(err) => {
                CommandError::InternalError(format!("Failed to access the repository: {err}"))
            }
        }
    }
}

impl From<OpHeadResolutionError<CommandError>> for CommandError {
    fn from(err: OpHeadResolutionError<CommandError>) -> Self {
        match err {
            OpHeadResolutionError::NoHeads => CommandError::InternalError(
                "Corrupt repository: there are no operations".to_string(),
            ),
            OpHeadResolutionError::OpStore(err) => err.into(),
            OpHeadResolutionError::Err(e) => e,
        }
    }
}

impl From<SnapshotError> for CommandError {
    fn from(err: SnapshotError) -> Self {
        CommandError::InternalError(format!("Failed to snapshot the working copy: {err}"))
    }
}

impl From<TreeMergeError> for CommandError {
    fn from(err: TreeMergeError) -> Self {
        CommandError::InternalError(format!("Merge failed: {err}"))
    }
}

impl From<OpStoreError> for CommandError {
    fn from(err: OpStoreError) -> Self {
        CommandError::InternalError(format!("Failed to load an operation: {err}"))
    }
}

impl From<RepoLoaderError> for CommandError {
    fn from(err: RepoLoaderError) -> Self {
        CommandError::InternalError(format!("Failed to load the repo: {err}"))
    }
}

impl From<ResetError> for CommandError {
    fn from(_: ResetError) -> Self {
        CommandError::InternalError("Failed to reset the working copy".to_string())
    }
}

impl From<DiffEditError> for CommandError {
    fn from(err: DiffEditError) -> Self {
        user_error(format!("Failed to edit diff: {err}"))
    }
}

impl From<DiffGenerateError> for CommandError {
    fn from(err: DiffGenerateError) -> Self {
        user_error(format!("Failed to generate diff: {err}"))
    }
}

impl From<ConflictResolveError> for CommandError {
    fn from(err: ConflictResolveError) -> Self {
        user_error(format!("Failed to use external tool to resolve: {err}"))
    }
}

impl From<git2::Error> for CommandError {
    fn from(err: git2::Error) -> Self {
        user_error(format!("Git operation failed: {err}"))
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
            GitImportError::InternalGitError(_) => None,
        };
        CommandError::UserError { message, hint }
    }
}

impl From<GitExportError> for CommandError {
    fn from(err: GitExportError) -> Self {
        CommandError::InternalError(format!(
            "Failed to export refs to underlying Git repo: {err}"
        ))
    }
}

impl From<GitRemoteManagementError> for CommandError {
    fn from(err: GitRemoteManagementError) -> Self {
        user_error(format!("{err}"))
    }
}

impl From<RevsetEvaluationError> for CommandError {
    fn from(err: RevsetEvaluationError) -> Self {
        user_error(format!("{err}"))
    }
}

impl From<RevsetParseError> for CommandError {
    fn from(err: RevsetParseError) -> Self {
        let message = iter::successors(Some(&err), |e| e.origin()).join("\n");
        // Only for the top-level error as we can't attach hint to inner errors
        let hint = match err.kind() {
            RevsetParseErrorKind::NotPostfixOperator {
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
        CommandError::UserError {
            message: format!("Failed to parse revset: {message}"),
            hint,
        }
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

        CommandError::UserError {
            message: format!("{err}"),
            hint,
        }
    }
}

impl From<TemplateParseError> for CommandError {
    fn from(err: TemplateParseError) -> Self {
        let message = iter::successors(Some(&err), |e| e.origin()).join("\n");
        user_error(format!("Failed to parse template: {message}"))
    }
}

impl From<FsPathParseError> for CommandError {
    fn from(err: FsPathParseError) -> Self {
        user_error(format!("{err}"))
    }
}

impl From<glob::PatternError> for CommandError {
    fn from(err: glob::PatternError) -> Self {
        user_error(format!("Failed to compile glob: {err}"))
    }
}

impl From<clap::Error> for CommandError {
    fn from(err: clap::Error) -> Self {
        CommandError::ClapCliError(Arc::new(err))
    }
}

impl From<GitConfigParseError> for CommandError {
    fn from(err: GitConfigParseError) -> Self {
        CommandError::InternalError(format!("Failed to parse Git config: {err} "))
    }
}

impl From<TreeStateError> for CommandError {
    fn from(err: TreeStateError) -> Self {
        CommandError::InternalError(format!("Failed to access tree state: {err}"))
    }
}

#[derive(Clone)]
struct ChromeTracingFlushGuard {
    _inner: Option<Rc<tracing_chrome::FlushGuard>>,
}

impl Debug for ChromeTracingFlushGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { _inner } = self;
        f.debug_struct("ChromeTracingFlushGuard")
            .finish_non_exhaustive()
    }
}

/// Handle to initialize or change tracing subscription.
#[derive(Clone, Debug)]
pub struct TracingSubscription {
    reload_log_filter: tracing_subscriber::reload::Handle<
        tracing_subscriber::EnvFilter,
        tracing_subscriber::Registry,
    >,
    _chrome_tracing_flush_guard: ChromeTracingFlushGuard,
}

impl TracingSubscription {
    /// Initializes tracing with the default configuration. This should be
    /// called as early as possible.
    pub fn init() -> Self {
        let filter = tracing_subscriber::EnvFilter::builder()
            .with_default_directive(tracing::metadata::LevelFilter::ERROR.into())
            .from_env_lossy();
        let (filter, reload_log_filter) = tracing_subscriber::reload::Layer::new(filter);

        let (chrome_tracing_layer, chrome_tracing_flush_guard) = match std::env::var("JJ_TRACE") {
            Ok(filename) => {
                let filename = if filename.is_empty() {
                    format!(
                        "jj-trace-{}.json",
                        SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                    )
                } else {
                    filename
                };
                let include_args = std::env::var("JJ_TRACE_INCLUDE_ARGS").is_ok();
                let (layer, guard) = ChromeLayerBuilder::new()
                    .file(filename)
                    .include_args(include_args)
                    .build();
                (
                    Some(layer),
                    ChromeTracingFlushGuard {
                        _inner: Some(Rc::new(guard)),
                    },
                )
            }
            Err(_) => (None, ChromeTracingFlushGuard { _inner: None }),
        };

        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::Layer::default()
                    .with_writer(std::io::stderr)
                    .with_filter(filter),
            )
            .with(chrome_tracing_layer)
            .init();
        TracingSubscription {
            reload_log_filter,
            _chrome_tracing_flush_guard: chrome_tracing_flush_guard,
        }
    }

    pub fn enable_verbose_logging(&self) -> Result<(), CommandError> {
        self.reload_log_filter
            .modify(|filter| {
                *filter = tracing_subscriber::EnvFilter::builder()
                    .with_default_directive(tracing::metadata::LevelFilter::DEBUG.into())
                    .from_env_lossy()
            })
            .map_err(|err| {
                CommandError::InternalError(format!("failed to enable verbose logging: {err:?}"))
            })?;
        tracing::info!("verbose logging enabled");
        Ok(())
    }
}

pub struct CommandHelper {
    app: Command,
    cwd: PathBuf,
    string_args: Vec<String>,
    matches: ArgMatches,
    global_args: GlobalArgs,
    settings: UserSettings,
    layered_configs: LayeredConfigs,
    maybe_workspace_loader: Result<WorkspaceLoader, CommandError>,
    store_factories: StoreFactories,
}

impl CommandHelper {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app: Command,
        cwd: PathBuf,
        string_args: Vec<String>,
        matches: ArgMatches,
        global_args: GlobalArgs,
        settings: UserSettings,
        layered_configs: LayeredConfigs,
        maybe_workspace_loader: Result<WorkspaceLoader, CommandError>,
        store_factories: StoreFactories,
    ) -> Self {
        // `cwd` is canonicalized for consistency with `Workspace::workspace_root()` and
        // to easily compute relative paths between them.
        let cwd = cwd.canonicalize().unwrap_or(cwd);

        Self {
            app,
            cwd,
            string_args,
            matches,
            global_args,
            settings,
            layered_configs,
            maybe_workspace_loader,
            store_factories,
        }
    }

    pub fn app(&self) -> &Command {
        &self.app
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn string_args(&self) -> &Vec<String> {
        &self.string_args
    }

    pub fn matches(&self) -> &ArgMatches {
        &self.matches
    }

    pub fn global_args(&self) -> &GlobalArgs {
        &self.global_args
    }

    pub fn settings(&self) -> &UserSettings {
        &self.settings
    }

    pub fn resolved_config_values(
        &self,
        prefix: &[&str],
    ) -> Result<Vec<AnnotatedValue>, crate::config::ConfigError> {
        self.layered_configs.resolved_config_values(prefix)
    }

    pub fn workspace_loader(&self) -> Result<&WorkspaceLoader, CommandError> {
        self.maybe_workspace_loader.as_ref().map_err(Clone::clone)
    }

    #[instrument(skip(self, ui))]
    fn workspace_helper_internal(
        &self,
        ui: &mut Ui,
        snapshot: bool,
    ) -> Result<WorkspaceCommandHelper, CommandError> {
        let workspace = self.load_workspace()?;
        let op_head = self.resolve_operation(ui, workspace.repo_loader())?;
        let repo = workspace.repo_loader().load_at(&op_head)?;
        let mut workspace_command = self.for_loaded_repo(ui, workspace, repo)?;
        if snapshot {
            workspace_command.snapshot(ui)?;
        }
        Ok(workspace_command)
    }

    pub fn workspace_helper(&self, ui: &mut Ui) -> Result<WorkspaceCommandHelper, CommandError> {
        self.workspace_helper_internal(ui, true)
    }

    pub fn workspace_helper_no_snapshot(
        &self,
        ui: &mut Ui,
    ) -> Result<WorkspaceCommandHelper, CommandError> {
        self.workspace_helper_internal(ui, false)
    }

    #[instrument(skip_all)]
    pub fn load_workspace(&self) -> Result<Workspace, CommandError> {
        let loader = self.workspace_loader()?;
        loader
            .load(&self.settings, &self.store_factories)
            .map_err(|err| map_workspace_load_error(err, &self.global_args))
    }

    #[instrument(skip_all)]
    pub fn resolve_operation(
        &self,
        ui: &mut Ui,
        repo_loader: &RepoLoader,
    ) -> Result<Operation, OpHeadResolutionError<CommandError>> {
        if self.global_args.at_operation == "@" {
            op_heads_store::resolve_op_heads(
                repo_loader.op_heads_store().as_ref(),
                repo_loader.op_store(),
                |op_heads| {
                    writeln!(
                        ui,
                        "Concurrent modification detected, resolving automatically.",
                    )?;
                    let base_repo = repo_loader.load_at(&op_heads[0])?;
                    // TODO: It may be helpful to print each operation we're merging here
                    let mut tx = start_repo_transaction(
                        &base_repo,
                        &self.settings,
                        &self.string_args,
                        "resolve concurrent operations",
                    );
                    for other_op_head in op_heads.into_iter().skip(1) {
                        tx.merge_operation(other_op_head)?;
                        let num_rebased = tx.mut_repo().rebase_descendants(&self.settings)?;
                        if num_rebased > 0 {
                            writeln!(
                                ui,
                                "Rebased {num_rebased} descendant commits onto commits rewritten \
                                 by other operation"
                            )?;
                        }
                    }
                    Ok(tx.write().leave_unpublished().operation().clone())
                },
            )
        } else {
            resolve_op_for_load(
                repo_loader.op_store(),
                repo_loader.op_heads_store(),
                &self.global_args.at_operation,
            )
        }
    }

    #[instrument(skip_all)]
    pub fn for_loaded_repo(
        &self,
        ui: &mut Ui,
        workspace: Workspace,
        repo: Arc<ReadonlyRepo>,
    ) -> Result<WorkspaceCommandHelper, CommandError> {
        WorkspaceCommandHelper::new(ui, self, workspace, repo)
    }

    /// Loads workspace that will diverge from the last working-copy operation.
    pub fn for_stale_working_copy(
        &self,
        ui: &mut Ui,
    ) -> Result<WorkspaceCommandHelper, CommandError> {
        let workspace = self.load_workspace()?;
        let op_store = workspace.repo_loader().op_store();
        let op_id = workspace.working_copy().operation_id();
        let op_data = op_store
            .read_operation(op_id)
            .map_err(|e| CommandError::InternalError(format!("Failed to read operation: {e}")))?;
        let operation = Operation::new(op_store.clone(), op_id.clone(), op_data);
        let repo = workspace.repo_loader().load_at(&operation)?;
        self.for_loaded_repo(ui, workspace, repo)
    }
}

/// A ReadonlyRepo along with user-config-dependent derived data. The derived
/// data is lazily loaded.
struct ReadonlyUserRepo {
    repo: Arc<ReadonlyRepo>,
    id_prefix_context: OnceCell<IdPrefixContext>,
}

impl ReadonlyUserRepo {
    fn new(repo: Arc<ReadonlyRepo>) -> Self {
        Self {
            repo,
            id_prefix_context: OnceCell::new(),
        }
    }

    pub fn git_backend(&self) -> Option<&GitBackend> {
        self.repo.store().backend_impl().downcast_ref()
    }
}

// Provides utilities for writing a command that works on a workspace (like most
// commands do).
pub struct WorkspaceCommandHelper {
    cwd: PathBuf,
    string_args: Vec<String>,
    global_args: GlobalArgs,
    settings: UserSettings,
    workspace: Workspace,
    user_repo: ReadonlyUserRepo,
    revset_aliases_map: RevsetAliasesMap,
    template_aliases_map: TemplateAliasesMap,
    may_update_working_copy: bool,
    working_copy_shared_with_git: bool,
}

impl WorkspaceCommandHelper {
    #[instrument(skip_all)]
    pub fn new(
        ui: &mut Ui,
        command: &CommandHelper,
        workspace: Workspace,
        repo: Arc<ReadonlyRepo>,
    ) -> Result<Self, CommandError> {
        let revset_aliases_map = load_revset_aliases(ui, &command.layered_configs)?;
        let template_aliases_map = load_template_aliases(ui, &command.layered_configs)?;
        // Parse commit_summary template early to report error before starting mutable
        // operation.
        // TODO: Parsed template can be cached if it doesn't capture repo
        let id_prefix_context = IdPrefixContext::default();
        parse_commit_summary_template(
            repo.as_ref(),
            workspace.workspace_id(),
            &id_prefix_context,
            &template_aliases_map,
            &command.settings,
        )?;
        let loaded_at_head = command.global_args.at_operation == "@";
        let may_update_working_copy = loaded_at_head && !command.global_args.ignore_working_copy;
        let working_copy_shared_with_git = is_colocated_git_workspace(&workspace, &repo);
        Ok(Self {
            cwd: command.cwd.clone(),
            string_args: command.string_args.clone(),
            global_args: command.global_args.clone(),
            settings: command.settings.clone(),
            workspace,
            user_repo: ReadonlyUserRepo::new(repo),
            revset_aliases_map,
            template_aliases_map,
            may_update_working_copy,
            working_copy_shared_with_git,
        })
    }

    pub fn git_backend(&self) -> Option<&GitBackend> {
        self.user_repo.git_backend()
    }

    pub fn check_working_copy_writable(&self) -> Result<(), CommandError> {
        if self.may_update_working_copy {
            Ok(())
        } else {
            let hint = if self.global_args.ignore_working_copy {
                "Don't use --ignore-working-copy."
            } else {
                "Don't use --at-op."
            };
            Err(user_error_with_hint(
                "This command must be able to update the working copy.",
                hint,
            ))
        }
    }

    /// Snapshot the working copy if allowed, and import Git refs if the working
    /// copy is collocated with Git.
    #[instrument(skip_all)]
    pub fn snapshot(&mut self, ui: &mut Ui) -> Result<(), CommandError> {
        if self.may_update_working_copy {
            if self.working_copy_shared_with_git {
                let git_repo = self.git_backend().unwrap().git_repo_clone();
                self.import_git_refs_and_head(ui, &git_repo)?;
            }
            self.snapshot_working_copy(ui)?;
        }
        Ok(())
    }

    #[instrument(skip_all)]
    fn import_git_refs_and_head(
        &mut self,
        ui: &mut Ui,
        git_repo: &Repository,
    ) -> Result<(), CommandError> {
        let mut tx = self.start_transaction("import git refs").into_inner();
        // Automated import shouldn't fail because of reserved remote name.
        git::import_some_refs(
            tx.mut_repo(),
            git_repo,
            &self.settings.git_settings(),
            |ref_name| !git::is_reserved_git_remote_ref(ref_name),
        )?;
        if tx.mut_repo().has_changes() {
            let old_git_head = self.repo().view().git_head().clone();
            let new_git_head = tx.mut_repo().view().git_head().clone();
            // If the Git HEAD has changed, abandon our old checkout and check out the new
            // Git HEAD.
            match new_git_head.as_normal() {
                Some(new_git_head_id) if new_git_head != old_git_head => {
                    let workspace_id = self.workspace_id().to_owned();
                    let op_id = self.repo().op_id().clone();
                    if let Some(old_wc_commit_id) =
                        self.repo().view().get_wc_commit_id(&workspace_id)
                    {
                        tx.mut_repo()
                            .record_abandoned_commit(old_wc_commit_id.clone());
                    }
                    let new_git_head_commit = tx.mut_repo().store().get_commit(new_git_head_id)?;
                    tx.mut_repo()
                        .check_out(workspace_id, &self.settings, &new_git_head_commit)?;
                    let mut locked_working_copy =
                        self.workspace.working_copy_mut().start_mutation()?;
                    // The working copy was presumably updated by the git command that updated
                    // HEAD, so we just need to reset our working copy
                    // state to it without updating working copy files.
                    let new_git_head_tree = new_git_head_commit.merged_tree()?;
                    locked_working_copy.reset(&new_git_head_tree)?;
                    tx.mut_repo().rebase_descendants(&self.settings)?;
                    self.user_repo = ReadonlyUserRepo::new(tx.commit());
                    locked_working_copy.finish(op_id)?;
                }
                _ => {
                    let num_rebased = tx.mut_repo().rebase_descendants(&self.settings)?;
                    if num_rebased > 0 {
                        writeln!(
                            ui,
                            "Rebased {num_rebased} descendant commits off of commits rewritten \
                             from git"
                        )?;
                    }
                    self.finish_transaction(ui, tx)?;
                }
            }
        }
        Ok(())
    }

    fn export_head_to_git(&self, mut_repo: &mut MutableRepo) -> Result<(), CommandError> {
        let git_repo = mut_repo
            .store()
            .backend_impl()
            .downcast_ref::<GitBackend>()
            .unwrap()
            .git_repo_clone();
        let current_git_head_ref = git_repo.find_reference("HEAD").unwrap();
        let current_git_commit_id = current_git_head_ref
            .peel_to_commit()
            .ok()
            .map(|commit| commit.id());
        if let Some(wc_commit_id) = mut_repo.view().get_wc_commit_id(self.workspace_id()) {
            let wc_commit = mut_repo.store().get_commit(wc_commit_id)?;
            let first_parent_id = wc_commit.parent_ids()[0].clone();
            if first_parent_id != *mut_repo.store().root_commit_id() {
                if let Some(current_git_commit_id) = current_git_commit_id {
                    git_repo.set_head_detached(current_git_commit_id)?;
                }
                let new_git_commit_id = Oid::from_bytes(first_parent_id.as_bytes()).unwrap();
                let new_git_commit = git_repo.find_commit(new_git_commit_id)?;
                git_repo.reset(new_git_commit.as_object(), git2::ResetType::Mixed, None)?;
                mut_repo.set_git_head_target(RefTarget::normal(first_parent_id));
            }
        } else {
            // The workspace was removed (maybe the user undid the
            // initialization of the workspace?), which is weird,
            // but we should probably just not do anything else here.
            // Except maybe print a note about it?
        }
        Ok(())
    }

    pub fn repo(&self) -> &Arc<ReadonlyRepo> {
        &self.user_repo.repo
    }

    pub fn working_copy(&self) -> &WorkingCopy {
        self.workspace.working_copy()
    }

    pub fn unchecked_start_working_copy_mutation(
        &mut self,
    ) -> Result<(LockedWorkingCopy, Commit), CommandError> {
        self.check_working_copy_writable()?;
        let wc_commit = if let Some(wc_commit_id) = self.get_wc_commit_id() {
            self.repo().store().get_commit(wc_commit_id)?
        } else {
            return Err(user_error("Nothing checked out in this workspace"));
        };

        let locked_working_copy = self.workspace.working_copy_mut().start_mutation()?;

        Ok((locked_working_copy, wc_commit))
    }

    pub fn start_working_copy_mutation(
        &mut self,
    ) -> Result<(LockedWorkingCopy, Commit), CommandError> {
        let (locked_working_copy, wc_commit) = self.unchecked_start_working_copy_mutation()?;
        if wc_commit.merged_tree_id() != locked_working_copy.old_tree_id() {
            return Err(user_error("Concurrent working copy operation. Try again."));
        }
        Ok((locked_working_copy, wc_commit))
    }

    pub fn workspace_root(&self) -> &PathBuf {
        self.workspace.workspace_root()
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        self.workspace.workspace_id()
    }

    pub fn get_wc_commit_id(&self) -> Option<&CommitId> {
        self.repo().view().get_wc_commit_id(self.workspace_id())
    }

    pub fn working_copy_shared_with_git(&self) -> bool {
        self.working_copy_shared_with_git
    }

    pub fn format_file_path(&self, file: &RepoPath) -> String {
        file_util::relative_path(&self.cwd, &file.to_fs_path(self.workspace_root()))
            .to_str()
            .unwrap()
            .to_owned()
    }

    /// Parses a path relative to cwd into a RepoPath, which is relative to the
    /// workspace root.
    pub fn parse_file_path(&self, input: &str) -> Result<RepoPath, FsPathParseError> {
        RepoPath::parse_fs_path(&self.cwd, self.workspace_root(), input)
    }

    pub fn matcher_from_values(&self, values: &[String]) -> Result<Box<dyn Matcher>, CommandError> {
        if values.is_empty() {
            Ok(Box::new(EverythingMatcher))
        } else {
            // TODO: Add support for globs and other formats
            let paths: Vec<_> = values
                .iter()
                .map(|v| self.parse_file_path(v))
                .try_collect()?;
            Ok(Box::new(PrefixMatcher::new(&paths)))
        }
    }

    pub fn git_config(&self) -> Result<git2::Config, git2::Error> {
        if let Some(git_backend) = self.git_backend() {
            git_backend.git_repo().config()
        } else {
            git2::Config::open_default()
        }
    }

    #[instrument(skip_all)]
    pub fn base_ignores(&self) -> Arc<GitIgnoreFile> {
        fn xdg_config_home() -> Result<PathBuf, VarError> {
            if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
                if !x.is_empty() {
                    return Ok(PathBuf::from(x));
                }
            }
            std::env::var("HOME").map(|x| Path::new(&x).join(".config"))
        }

        let mut git_ignores = GitIgnoreFile::empty();
        if let Ok(excludes_file_path) = self
            .git_config()
            .and_then(|git_config| {
                git_config
                    .get_string("core.excludesFile")
                    .map(expand_git_path)
            })
            .or_else(|_| xdg_config_home().map(|x| x.join("git").join("ignore")))
        {
            git_ignores = git_ignores.chain_with_file("", excludes_file_path);
        }
        if let Some(git_backend) = self.git_backend() {
            git_ignores = git_ignores.chain_with_file(
                "",
                git_backend.git_repo().path().join("info").join("exclude"),
            );
        }
        git_ignores
    }

    pub fn resolve_single_op(&self, op_str: &str) -> Result<Operation, CommandError> {
        // When resolving the "@" operation in a `ReadonlyRepo`, we resolve it to the
        // operation the repo was loaded at.
        resolve_single_op(
            self.repo().op_store(),
            self.repo().op_heads_store(),
            || Ok(self.repo().operation().clone()),
            op_str,
        )
    }

    /// Resolve a revset to a single revision. Return an error if the revset is
    /// empty or has multiple revisions.
    pub fn resolve_single_rev(
        &self,
        revision_str: &str,
        ui: &mut Ui,
    ) -> Result<Commit, CommandError> {
        let revset_expression = self.parse_revset(revision_str, Some(ui))?;
        let revset = self.evaluate_revset(revset_expression)?;
        let mut iter = revset.iter().commits(self.repo().store()).fuse();
        match (iter.next(), iter.next()) {
            (Some(commit), None) => Ok(commit?),
            (None, _) => Err(user_error(format!(
                r#"Revset "{revision_str}" didn't resolve to any revisions"#
            ))),
            (Some(commit0), Some(commit1)) => {
                let mut iter = [commit0, commit1].into_iter().chain(iter);
                let commits: Vec<_> = iter.by_ref().take(5).try_collect()?;
                let elided = iter.next().is_some();
                let hint = format!(
                    r#"The revset "{revision_str}" resolved to these revisions:{eol}{commits}{ellipsis}"#,
                    eol = "\n",
                    commits = commits
                        .iter()
                        .map(|c| self.format_commit_summary(c))
                        .join("\n"),
                    ellipsis = elided.then_some("\n...").unwrap_or_default()
                );
                Err(user_error_with_hint(
                    format!(r#"Revset "{revision_str}" resolved to more than one revision"#),
                    hint,
                ))
            }
        }
    }

    /// Resolve a revset any number of revisions (including 0).
    pub fn resolve_revset(
        &self,
        revision_str: &str,
        ui: &mut Ui,
    ) -> Result<Vec<Commit>, CommandError> {
        let revset_expression = self.parse_revset(revision_str, Some(ui))?;
        let revset = self.evaluate_revset(revset_expression)?;
        Ok(revset.iter().commits(self.repo().store()).try_collect()?)
    }

    /// Resolve a revset any number of revisions (including 0), but require the
    /// user to indicate if they allow multiple revisions by prefixing the
    /// expression with `all:`.
    pub fn resolve_revset_default_single(
        &self,
        revision_str: &str,
        ui: &mut Ui,
    ) -> Result<Vec<Commit>, CommandError> {
        // TODO: Let pest parse the prefix too once we've dropped support for `:`
        if let Some(revision_str) = revision_str.strip_prefix("all:") {
            self.resolve_revset(revision_str, ui)
        } else {
            self.resolve_single_rev(revision_str, ui)
                .map_err(|err| match err {
                    CommandError::UserError { message, hint } => user_error_with_hint(
                        message,
                        format!(
                            "{old_hint}Prefix the expression with 'all' to allow any number of \
                             revisions (i.e. 'all:{}').",
                            revision_str,
                            old_hint = hint.map(|hint| format!("{hint}\n")).unwrap_or_default()
                        ),
                    ),
                    err => err,
                })
                .map(|commit| vec![commit])
        }
    }

    pub fn parse_revset(
        &self,
        revision_str: &str,
        ui: Option<&mut Ui>,
    ) -> Result<Rc<RevsetExpression>, RevsetParseError> {
        let expression = revset::parse(revision_str, &self.revset_parse_context())?;
        if let Some(ui) = ui {
            fn has_legacy_rule(expression: &Rc<RevsetExpression>) -> bool {
                match expression.as_ref() {
                    RevsetExpression::None => false,
                    RevsetExpression::All => false,
                    RevsetExpression::Commits(_) => false,
                    RevsetExpression::CommitRef(_) => false,
                    RevsetExpression::Ancestors {
                        heads,
                        generation: _,
                        is_legacy,
                    } => *is_legacy || has_legacy_rule(heads),
                    RevsetExpression::Descendants {
                        roots,
                        generation: _,
                        is_legacy,
                    } => *is_legacy || has_legacy_rule(roots),
                    RevsetExpression::Range {
                        roots,
                        heads,
                        generation: _,
                    } => has_legacy_rule(roots) || has_legacy_rule(heads),
                    RevsetExpression::DagRange {
                        roots,
                        heads,
                        is_legacy,
                    } => *is_legacy || has_legacy_rule(roots) || has_legacy_rule(heads),
                    RevsetExpression::Heads(expression) => has_legacy_rule(expression),
                    RevsetExpression::Roots(expression) => has_legacy_rule(expression),
                    RevsetExpression::Latest {
                        candidates,
                        count: _,
                    } => has_legacy_rule(candidates),
                    RevsetExpression::Filter(_) => false,
                    RevsetExpression::AsFilter(expression) => has_legacy_rule(expression),
                    RevsetExpression::Present(expression) => has_legacy_rule(expression),
                    RevsetExpression::NotIn(expression) => has_legacy_rule(expression),
                    RevsetExpression::Union(expression1, expression2) => {
                        has_legacy_rule(expression1) || has_legacy_rule(expression2)
                    }
                    RevsetExpression::Intersection(expression1, expression2) => {
                        has_legacy_rule(expression1) || has_legacy_rule(expression2)
                    }
                    RevsetExpression::Difference(expression1, expression2) => {
                        has_legacy_rule(expression1) || has_legacy_rule(expression2)
                    }
                }
            }
            if has_legacy_rule(&expression) {
                writeln!(
                    ui.warning(),
                    "The `:` revset operator is deprecated. Please switch to `::`."
                )
                .ok();
            }
        }
        Ok(revset::optimize(expression))
    }

    pub fn evaluate_revset<'repo>(
        &'repo self,
        revset_expression: Rc<RevsetExpression>,
    ) -> Result<Box<dyn Revset<'repo> + 'repo>, CommandError> {
        let revset_expression = revset_expression
            .resolve_user_expression(self.repo().as_ref(), &self.revset_symbol_resolver())?;
        Ok(revset_expression.evaluate(self.repo().as_ref())?)
    }

    pub(crate) fn revset_parse_context(&self) -> RevsetParseContext {
        let workspace_context = RevsetWorkspaceContext {
            cwd: &self.cwd,
            workspace_id: self.workspace_id(),
            workspace_root: self.workspace.workspace_root(),
        };
        RevsetParseContext {
            aliases_map: &self.revset_aliases_map,
            user_email: self.settings.user_email(),
            workspace: Some(workspace_context),
        }
    }

    pub(crate) fn revset_symbol_resolver(&self) -> DefaultSymbolResolver<'_> {
        let id_prefix_context = self.id_prefix_context();
        let commit_id_resolver: revset::PrefixResolver<CommitId> =
            Box::new(|repo, prefix| id_prefix_context.resolve_commit_prefix(repo, prefix));
        let change_id_resolver: revset::PrefixResolver<Vec<CommitId>> =
            Box::new(|repo, prefix| id_prefix_context.resolve_change_prefix(repo, prefix));
        DefaultSymbolResolver::new(self.repo().as_ref())
            .with_commit_id_resolver(commit_id_resolver)
            .with_change_id_resolver(change_id_resolver)
    }

    pub fn id_prefix_context(&self) -> &IdPrefixContext {
        self.user_repo.id_prefix_context.get_or_init(|| {
            let mut context: IdPrefixContext = IdPrefixContext::default();
            let revset_string: String = self
                .settings
                .config()
                .get_string("revsets.short-prefixes")
                .unwrap_or_else(|_| self.settings.default_revset());
            if !revset_string.is_empty() {
                let disambiguation_revset = self.parse_revset(&revset_string, None).unwrap();
                context = context.disambiguate_within(disambiguation_revset);
            }
            context
        })
    }

    pub fn template_aliases_map(&self) -> &TemplateAliasesMap {
        &self.template_aliases_map
    }

    pub fn parse_commit_template(
        &self,
        template_text: &str,
    ) -> Result<Box<dyn Template<Commit> + '_>, TemplateParseError> {
        commit_templater::parse(
            self.repo().as_ref(),
            self.workspace_id(),
            self.id_prefix_context(),
            template_text,
            &self.template_aliases_map,
        )
    }

    /// Returns one-line summary of the given `commit`.
    pub fn format_commit_summary(&self, commit: &Commit) -> String {
        let mut output = Vec::new();
        self.write_commit_summary(&mut PlainTextFormatter::new(&mut output), commit)
            .expect("write() to PlainTextFormatter should never fail");
        String::from_utf8(output).expect("template output should be utf-8 bytes")
    }

    /// Writes one-line summary of the given `commit`.
    #[instrument(skip_all)]
    pub fn write_commit_summary(
        &self,
        formatter: &mut dyn Formatter,
        commit: &Commit,
    ) -> std::io::Result<()> {
        let template = parse_commit_summary_template(
            self.repo().as_ref(),
            self.workspace_id(),
            self.id_prefix_context(),
            &self.template_aliases_map,
            &self.settings,
        )
        .expect("parse error should be confined by WorkspaceCommandHelper::new()");
        template.format(commit, formatter)
    }

    pub fn check_rewritable(&self, commit: &Commit) -> Result<(), CommandError> {
        if commit.id() == self.repo().store().root_commit_id() {
            return Err(user_error("Cannot rewrite the root commit"));
        }
        Ok(())
    }

    pub fn check_non_empty(&self, commits: &[Commit]) -> Result<(), CommandError> {
        if commits.is_empty() {
            return Err(user_error("Empty revision set"));
        }
        Ok(())
    }

    #[instrument(skip_all)]
    pub fn snapshot_working_copy(&mut self, ui: &mut Ui) -> Result<(), CommandError> {
        let workspace_id = self.workspace_id().to_owned();
        let get_wc_commit = |repo: &ReadonlyRepo| -> Result<Option<_>, _> {
            repo.view()
                .get_wc_commit_id(&workspace_id)
                .map(|id| repo.store().get_commit(id))
                .transpose()
        };
        let repo = self.repo().clone();
        let Some(wc_commit) = get_wc_commit(&repo)? else {
            // If the workspace has been deleted, it's unclear what to do, so we just skip
            // committing the working copy.
            return Ok(());
        };
        let base_ignores = self.base_ignores();

        // Compare working-copy tree and operation with repo's, and reload as needed.
        let mut locked_wc = self.workspace.working_copy_mut().start_mutation()?;
        let old_op_id = locked_wc.old_operation_id().clone();
        let (repo, wc_commit) = match check_stale_working_copy(&locked_wc, &wc_commit, &repo) {
            Ok(None) => (repo, wc_commit),
            Ok(Some(wc_operation)) => {
                let repo = repo.reload_at(&wc_operation)?;
                let wc_commit = if let Some(wc_commit) = get_wc_commit(&repo)? {
                    wc_commit
                } else {
                    return Ok(()); // The workspace has been deleted (see above)
                };
                (repo, wc_commit)
            }
            Err(StaleWorkingCopyError::WorkingCopyStale) => {
                locked_wc.discard();
                return Err(user_error_with_hint(
                    format!(
                        "The working copy is stale (not updated since operation {}).",
                        short_operation_hash(&old_op_id)
                    ),
                    "Run `jj workspace update-stale` to update it.
See https://github.com/martinvonz/jj/blob/main/docs/working-copy.md#stale-working-copy \
                     for more information.",
                ));
            }
            Err(StaleWorkingCopyError::SiblingOperation) => {
                locked_wc.discard();
                return Err(CommandError::InternalError(format!(
                    "The repo was loaded at operation {}, which seems to be a sibling of the \
                     working copy's operation {}",
                    short_operation_hash(repo.op_id()),
                    short_operation_hash(&old_op_id)
                )));
            }
            Err(StaleWorkingCopyError::UnrelatedOperation) => {
                locked_wc.discard();
                return Err(CommandError::InternalError(format!(
                    "The repo was loaded at operation {}, which seems unrelated to the working \
                     copy's operation {}",
                    short_operation_hash(repo.op_id()),
                    short_operation_hash(&old_op_id)
                )));
            }
        };
        self.user_repo = ReadonlyUserRepo::new(repo);
        let progress = crate::progress::snapshot_progress(ui);
        let new_tree_id = locked_wc.snapshot(SnapshotOptions {
            base_ignores,
            fsmonitor_kind: self.settings.fsmonitor_kind()?,
            progress: progress.as_ref().map(|x| x as _),
            max_new_file_size: self.settings.max_new_file_size()?,
        })?;
        drop(progress);
        if new_tree_id != *wc_commit.merged_tree_id() {
            let mut tx = start_repo_transaction(
                &self.user_repo.repo,
                &self.settings,
                &self.string_args,
                "snapshot working copy",
            );
            let mut_repo = tx.mut_repo();
            let commit = mut_repo
                .rewrite_commit(&self.settings, &wc_commit)
                .set_tree_id(new_tree_id)
                .write()?;
            mut_repo.set_wc_commit(workspace_id, commit.id().clone())?;

            // Rebase descendants
            let num_rebased = mut_repo.rebase_descendants(&self.settings)?;
            if num_rebased > 0 {
                writeln!(
                    ui,
                    "Rebased {num_rebased} descendant commits onto updated working copy"
                )?;
            }

            if self.working_copy_shared_with_git {
                let failed_branches =
                    git::export_refs(mut_repo, &self.user_repo.git_backend().unwrap().git_repo())?;
                print_failed_git_export(ui, &failed_branches)?;
            }

            self.user_repo = ReadonlyUserRepo::new(tx.commit());
        }
        locked_wc.finish(self.user_repo.repo.op_id().clone())?;
        Ok(())
    }

    fn update_working_copy(
        &mut self,
        ui: &mut Ui,
        maybe_old_commit: Option<&Commit>,
    ) -> Result<(), CommandError> {
        assert!(self.may_update_working_copy);
        let new_commit = match self.get_wc_commit_id() {
            Some(commit_id) => self.repo().store().get_commit(commit_id)?,
            None => {
                // It seems the workspace was deleted, so we shouldn't try to update it.
                return Ok(());
            }
        };
        let stats = update_working_copy(
            &self.user_repo.repo,
            self.workspace.working_copy_mut(),
            maybe_old_commit,
            &new_commit,
        )?;
        if Some(&new_commit) != maybe_old_commit {
            ui.write("Working copy now at: ")?;
            self.write_commit_summary(ui.stdout_formatter().as_mut(), &new_commit)?;
            ui.write("\n")?;
            for parent in new_commit.parents() {
                //       "Working copy now at: "
                ui.write("Parent commit      : ")?;
                self.write_commit_summary(ui.stdout_formatter().as_mut(), &parent)?;
                ui.write("\n")?;
            }
        }
        if let Some(stats) = stats {
            print_checkout_stats(ui, stats)?;
        }
        Ok(())
    }

    pub fn start_transaction(&mut self, description: &str) -> WorkspaceCommandTransaction {
        let tx =
            start_repo_transaction(self.repo(), &self.settings, &self.string_args, description);
        WorkspaceCommandTransaction { helper: self, tx }
    }

    fn finish_transaction(&mut self, ui: &mut Ui, mut tx: Transaction) -> Result<(), CommandError> {
        let mut_repo = tx.mut_repo();
        let store = mut_repo.store().clone();
        if !mut_repo.has_changes() {
            writeln!(ui, "Nothing changed.")?;
            return Ok(());
        }
        let num_rebased = mut_repo.rebase_descendants(&self.settings)?;
        if num_rebased > 0 {
            writeln!(ui, "Rebased {num_rebased} descendant commits")?;
        }
        if self.working_copy_shared_with_git {
            self.export_head_to_git(mut_repo)?;
            let failed_branches =
                git::export_refs(mut_repo, &self.git_backend().unwrap().git_repo())?;
            print_failed_git_export(ui, &failed_branches)?;
        }
        let maybe_old_commit = tx
            .base_repo()
            .view()
            .get_wc_commit_id(self.workspace_id())
            .map(|commit_id| store.get_commit(commit_id))
            .transpose()?;
        self.user_repo = ReadonlyUserRepo::new(tx.commit());
        if self.may_update_working_copy {
            self.update_working_copy(ui, maybe_old_commit.as_ref())?;
        }
        let settings = &self.settings;
        if settings.user_name().is_empty() || settings.user_email().is_empty() {
            writeln!(
                ui.warning(),
                r#"Name and email not configured. Until configured, your commits will be created with the empty identity, and can't be pushed to remotes. To configure, run:
  jj config set --user user.name "Some One"
  jj config set --user user.email "someone@example.com""#
            )?;
        }
        Ok(())
    }
}

#[must_use]
pub struct WorkspaceCommandTransaction<'a> {
    helper: &'a mut WorkspaceCommandHelper,
    tx: Transaction,
}

impl WorkspaceCommandTransaction<'_> {
    /// Workspace helper that may use the base repo.
    pub fn base_workspace_helper(&self) -> &WorkspaceCommandHelper {
        self.helper
    }

    pub fn base_repo(&self) -> &Arc<ReadonlyRepo> {
        self.tx.base_repo()
    }

    pub fn repo(&self) -> &MutableRepo {
        self.tx.repo()
    }

    pub fn mut_repo(&mut self) -> &mut MutableRepo {
        self.tx.mut_repo()
    }

    pub fn set_description(&mut self, description: &str) {
        self.tx.set_description(description)
    }

    pub fn check_out(&mut self, commit: &Commit) -> Result<Commit, CheckOutCommitError> {
        let workspace_id = self.helper.workspace_id().to_owned();
        let settings = &self.helper.settings;
        self.tx.mut_repo().check_out(workspace_id, settings, commit)
    }

    pub fn edit(&mut self, commit: &Commit) -> Result<(), EditCommitError> {
        let workspace_id = self.helper.workspace_id().to_owned();
        self.tx.mut_repo().edit(workspace_id, commit)
    }

    pub fn run_mergetool(
        &self,
        ui: &Ui,
        tree: &MergedTree,
        repo_path: &RepoPath,
    ) -> Result<MergedTreeId, CommandError> {
        let settings = &self.helper.settings;
        Ok(crate::merge_tools::run_mergetool(
            ui, tree, repo_path, settings,
        )?)
    }

    pub fn edit_diff(
        &self,
        ui: &Ui,
        left_tree: &MergedTree,
        right_tree: &MergedTree,
        instructions: &str,
    ) -> Result<MergedTreeId, CommandError> {
        let base_ignores = self.helper.base_ignores();
        let settings = &self.helper.settings;
        Ok(crate::merge_tools::edit_diff(
            ui,
            left_tree,
            right_tree,
            instructions,
            base_ignores,
            settings,
        )?)
    }

    pub fn select_diff(
        &self,
        ui: &Ui,
        left_tree: &MergedTree,
        right_tree: &MergedTree,
        instructions: &str,
        interactive: bool,
        matcher: &dyn Matcher,
    ) -> Result<MergedTreeId, CommandError> {
        if interactive {
            self.edit_diff(ui, left_tree, right_tree, instructions)
        } else if matcher.visit(&RepoPath::root()) == Visit::AllRecursively {
            // Optimization for a common case
            Ok(right_tree.id().clone())
        } else {
            let mut tree_builder = MergedTreeBuilder::new(left_tree.id().clone());
            for (repo_path, _left, right) in left_tree.diff(right_tree, matcher) {
                tree_builder.set_or_remove(repo_path, right);
            }
            Ok(tree_builder.write_tree(self.repo().store())?)
        }
    }

    pub fn format_commit_summary(&self, commit: &Commit) -> String {
        let mut output = Vec::new();
        self.write_commit_summary(&mut PlainTextFormatter::new(&mut output), commit)
            .expect("write() to PlainTextFormatter should never fail");
        String::from_utf8(output).expect("template output should be utf-8 bytes")
    }

    pub fn write_commit_summary(
        &self,
        formatter: &mut dyn Formatter,
        commit: &Commit,
    ) -> std::io::Result<()> {
        // TODO: Use the disambiguation revset
        let id_prefix_context = IdPrefixContext::default();
        let template = parse_commit_summary_template(
            self.tx.repo(),
            self.helper.workspace_id(),
            &id_prefix_context,
            &self.helper.template_aliases_map,
            &self.helper.settings,
        )
        .expect("parse error should be confined by WorkspaceCommandHelper::new()");
        template.format(commit, formatter)
    }

    pub fn finish(self, ui: &mut Ui) -> Result<(), CommandError> {
        self.helper.finish_transaction(ui, self.tx)
    }

    pub fn into_inner(self) -> Transaction {
        self.tx
    }
}

#[instrument]
fn init_workspace_loader(
    cwd: &Path,
    global_args: &GlobalArgs,
) -> Result<WorkspaceLoader, CommandError> {
    let workspace_root = if let Some(path) = global_args.repository.as_ref() {
        cwd.join(path)
    } else {
        cwd.ancestors()
            .find(|path| path.join(".jj").is_dir())
            .unwrap_or(cwd)
            .to_owned()
    };
    WorkspaceLoader::init(&workspace_root).map_err(|err| map_workspace_load_error(err, global_args))
}

fn map_workspace_load_error(err: WorkspaceLoadError, global_args: &GlobalArgs) -> CommandError {
    match err {
        WorkspaceLoadError::NoWorkspaceHere(wc_path) => {
            // Prefer user-specified workspace_path_str instead of absolute wc_path.
            let workspace_path_str = global_args.repository.as_deref().unwrap_or(".");
            let message = format!(r#"There is no jj repo in "{workspace_path_str}""#);
            let git_dir = wc_path.join(".git");
            if git_dir.is_dir() {
                user_error_with_hint(
                    message,
                    "It looks like this is a git repo. You can create a jj repo backed by it by \
                     running this:
jj init --git-repo=.",
                )
            } else {
                user_error(message)
            }
        }
        WorkspaceLoadError::RepoDoesNotExist(repo_dir) => user_error(format!(
            "The repository directory at {} is missing. Was it moved?",
            repo_dir.display(),
        )),
        WorkspaceLoadError::Path(e) => user_error(format!("{}: {}", e, e.error)),
        WorkspaceLoadError::NonUnicodePath => user_error(err.to_string()),
        WorkspaceLoadError::StoreLoadError(err @ StoreLoadError::UnsupportedType { .. }) => {
            CommandError::InternalError(format!(
                "This version of the jj binary doesn't support this type of repo: {err}"
            ))
        }
        WorkspaceLoadError::StoreLoadError(
            err @ (StoreLoadError::ReadError { .. } | StoreLoadError::Backend(_)),
        ) => CommandError::InternalError(format!(
            "The repository appears broken or inaccessible: {err}"
        )),
    }
}

fn is_colocated_git_workspace(workspace: &Workspace, repo: &ReadonlyRepo) -> bool {
    let Some(git_backend) = repo.store().backend_impl().downcast_ref::<GitBackend>() else {
        return false;
    };
    let git_repo = git_backend.git_repo();
    let Some(git_workdir) = git_repo.workdir().and_then(|path| path.canonicalize().ok()) else {
        return false; // Bare repository
    };
    // Colocated workspace should have ".git" directory, file, or symlink. Since the
    // backend is loaded from the canonical path, its working directory should also
    // be resolved from the canonical ".git" path.
    let Ok(dot_git_path) = workspace.workspace_root().join(".git").canonicalize() else {
        return false;
    };
    Some(git_workdir.as_ref()) == dot_git_path.parent()
}

pub fn start_repo_transaction(
    repo: &Arc<ReadonlyRepo>,
    settings: &UserSettings,
    string_args: &[String],
    description: &str,
) -> Transaction {
    let mut tx = repo.start_transaction(settings, description);
    // TODO: Either do better shell-escaping here or store the values in some list
    // type (which we currently don't have).
    let shell_escape = |arg: &String| {
        if arg.as_bytes().iter().all(|b| {
            matches!(b,
                b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b','
                | b'-'
                | b'.'
                | b'/'
                | b':'
                | b'@'
                | b'_'
            )
        }) {
            arg.clone()
        } else {
            format!("'{}'", arg.replace('\'', "\\'"))
        }
    };
    let mut quoted_strings = vec!["jj".to_string()];
    quoted_strings.extend(string_args.iter().skip(1).map(shell_escape));
    tx.set_tag("args".to_string(), quoted_strings.join(" "));
    tx
}

#[derive(Debug, Error)]
pub enum StaleWorkingCopyError {
    #[error("The working copy is behind the latest operation")]
    WorkingCopyStale,
    #[error("The working copy is a sibling of the latest operation")]
    SiblingOperation,
    #[error("The working copy is unrelated to the latest operation")]
    UnrelatedOperation,
}

#[instrument(skip_all)]
pub fn check_stale_working_copy(
    locked_wc: &LockedWorkingCopy,
    wc_commit: &Commit,
    repo: &ReadonlyRepo,
) -> Result<Option<Operation>, StaleWorkingCopyError> {
    // Check if the working copy's tree matches the repo's view
    let wc_tree_id = locked_wc.old_tree_id();
    if wc_commit.merged_tree_id() == wc_tree_id {
        // The working copy isn't stale, and no need to reload the repo.
        Ok(None)
    } else {
        let wc_operation_data = repo
            .op_store()
            .read_operation(locked_wc.old_operation_id())
            .unwrap();
        let wc_operation = Operation::new(
            repo.op_store().clone(),
            locked_wc.old_operation_id().clone(),
            wc_operation_data,
        );
        let repo_operation = repo.operation();
        let maybe_ancestor_op = dag_walk::closest_common_node(
            [wc_operation.clone()],
            [repo_operation.clone()],
            |op: &Operation| op.id().clone(),
            |op: &Operation| op.parents(),
        );
        if let Some(ancestor_op) = maybe_ancestor_op {
            if ancestor_op.id() == repo_operation.id() {
                // The working copy was updated since we loaded the repo. The repo must be
                // reloaded at the working copy's operation.
                Ok(Some(wc_operation))
            } else if ancestor_op.id() == wc_operation.id() {
                // The working copy was not updated when some repo operation committed,
                // meaning that it's stale compared to the repo view.
                Err(StaleWorkingCopyError::WorkingCopyStale)
            } else {
                Err(StaleWorkingCopyError::SiblingOperation)
            }
        } else {
            Err(StaleWorkingCopyError::UnrelatedOperation)
        }
    }
}

pub fn print_checkout_stats(ui: &mut Ui, stats: CheckoutStats) -> Result<(), std::io::Error> {
    if stats.added_files > 0 || stats.updated_files > 0 || stats.removed_files > 0 {
        writeln!(
            ui,
            "Added {} files, modified {} files, removed {} files",
            stats.added_files, stats.updated_files, stats.removed_files
        )?;
    }
    Ok(())
}

pub fn print_failed_git_export(ui: &Ui, failed_branches: &[RefName]) -> Result<(), std::io::Error> {
    if !failed_branches.is_empty() {
        writeln!(ui.warning(), "Failed to export some branches:")?;
        let mut formatter = ui.stderr_formatter();
        for branch_ref in failed_branches {
            formatter.write_str("  ")?;
            write!(formatter.labeled("branch"), "{branch_ref}")?;
            formatter.write_str("\n")?;
        }
        drop(formatter);
        writeln!(
            ui.hint(),
            r#"Hint: Git doesn't allow a branch name that looks like a parent directory of
another (e.g. `foo` and `foo/bar`). Try to rename the branches that failed to
export or their "parent" branches."#,
        )?;
    }
    Ok(())
}

/// Expands "~/" to "$HOME/" as Git seems to do for e.g. core.excludesFile.
fn expand_git_path(path_str: String) -> PathBuf {
    if let Some(remainder) = path_str.strip_prefix("~/") {
        if let Ok(home_dir_str) = std::env::var("HOME") {
            return PathBuf::from(home_dir_str).join(remainder);
        }
    }
    PathBuf::from(path_str)
}

pub fn resolve_op_for_load(
    op_store: &Arc<dyn OpStore>,
    op_heads_store: &Arc<dyn OpHeadsStore>,
    op_str: &str,
) -> Result<Operation, OpHeadResolutionError<CommandError>> {
    let get_current_op = || {
        op_heads_store::resolve_op_heads(op_heads_store.as_ref(), op_store, |_| {
            Err(user_error(format!(
                r#"The "{op_str}" expression resolved to more than one operation"#
            )))
        })
    };
    let operation = resolve_single_op(op_store, op_heads_store, get_current_op, op_str)
        .map_err(OpHeadResolutionError::Err)?;
    Ok(operation)
}

fn resolve_single_op(
    op_store: &Arc<dyn OpStore>,
    op_heads_store: &Arc<dyn OpHeadsStore>,
    get_current_op: impl FnOnce() -> Result<Operation, OpHeadResolutionError<CommandError>>,
    op_str: &str,
) -> Result<Operation, CommandError> {
    let op_symbol = op_str.trim_end_matches('-');
    let op_postfix = &op_str[op_symbol.len()..];
    let mut operation = match op_symbol {
        "@" => get_current_op(),
        s => resolve_single_op_from_store(op_store, op_heads_store, s)
            .map_err(OpHeadResolutionError::Err),
    }?;
    for _ in op_postfix.chars() {
        operation = match operation.parents().as_slice() {
            [op] => Ok(op.clone()),
            [] => Err(user_error(format!(
                r#"The "{op_str}" expression resolved to no operations"#
            ))),
            [_, _, ..] => Err(user_error(format!(
                r#"The "{op_str}" expression resolved to more than one operation"#
            ))),
        }?;
    }
    Ok(operation)
}

fn find_all_operations(
    op_store: &Arc<dyn OpStore>,
    op_heads_store: &Arc<dyn OpHeadsStore>,
) -> Vec<Operation> {
    let mut visited = HashSet::new();
    let mut work: VecDeque<_> = op_heads_store.get_op_heads().into_iter().collect();
    let mut operations = vec![];
    while let Some(op_id) = work.pop_front() {
        if visited.insert(op_id.clone()) {
            let store_operation = op_store.read_operation(&op_id).unwrap();
            work.extend(store_operation.parents.iter().cloned());
            let operation = Operation::new(op_store.clone(), op_id, store_operation);
            operations.push(operation);
        }
    }
    operations
}

fn resolve_single_op_from_store(
    op_store: &Arc<dyn OpStore>,
    op_heads_store: &Arc<dyn OpHeadsStore>,
    op_str: &str,
) -> Result<Operation, CommandError> {
    if op_str.is_empty() || !op_str.as_bytes().iter().all(|b| b.is_ascii_hexdigit()) {
        return Err(user_error(format!(
            "Operation ID \"{op_str}\" is not a valid hexadecimal prefix"
        )));
    }
    if let Ok(binary_op_id) = hex::decode(op_str) {
        let op_id = OperationId::new(binary_op_id);
        match op_store.read_operation(&op_id) {
            Ok(operation) => {
                return Ok(Operation::new(op_store.clone(), op_id, operation));
            }
            Err(OpStoreError::NotFound) => {
                // Fall through
            }
            Err(err) => {
                return Err(CommandError::InternalError(format!(
                    "Failed to read operation: {err}"
                )));
            }
        }
    }
    let mut matches = vec![];
    for op in find_all_operations(op_store, op_heads_store) {
        if op.id().hex().starts_with(op_str) {
            matches.push(op);
        }
    }
    if matches.is_empty() {
        Err(user_error(format!("No operation ID matching \"{op_str}\"")))
    } else if matches.len() == 1 {
        Ok(matches.pop().unwrap())
    } else {
        Err(user_error(format!(
            "Operation ID prefix \"{op_str}\" is ambiguous"
        )))
    }
}

fn load_revset_aliases(
    ui: &Ui,
    layered_configs: &LayeredConfigs,
) -> Result<RevsetAliasesMap, CommandError> {
    const TABLE_KEY: &str = "revset-aliases";
    let mut aliases_map = RevsetAliasesMap::new();
    // Load from all config layers in order. 'f(x)' in default layer should be
    // overridden by 'f(a)' in user.
    for (_, config) in layered_configs.sources() {
        let table = if let Some(table) = config.get_table(TABLE_KEY).optional()? {
            table
        } else {
            continue;
        };
        for (decl, value) in table.into_iter().sorted_by(|a, b| a.0.cmp(&b.0)) {
            let r = value
                .into_string()
                .map_err(|e| e.to_string())
                .and_then(|v| aliases_map.insert(&decl, v).map_err(|e| e.to_string()));
            if let Err(s) = r {
                writeln!(ui.warning(), r#"Failed to load "{TABLE_KEY}.{decl}": {s}"#)?;
            }
        }
    }
    Ok(aliases_map)
}

pub fn resolve_multiple_nonempty_revsets(
    revision_args: &[RevisionArg],
    workspace_command: &WorkspaceCommandHelper,
    ui: &mut Ui,
) -> Result<IndexSet<Commit>, CommandError> {
    let mut acc = IndexSet::new();
    for revset in revision_args {
        let revisions = workspace_command.resolve_revset(revset, ui)?;
        workspace_command.check_non_empty(&revisions)?;
        acc.extend(revisions);
    }
    Ok(acc)
}

pub fn resolve_multiple_nonempty_revsets_default_single(
    workspace_command: &WorkspaceCommandHelper,
    ui: &mut Ui,
    revisions: &[RevisionArg],
) -> Result<IndexSet<Commit>, CommandError> {
    let mut all_commits = IndexSet::new();
    for revision_str in revisions {
        let commits = workspace_command.resolve_revset_default_single(revision_str, ui)?;
        workspace_command.check_non_empty(&commits)?;
        for commit in commits {
            let commit_hash = short_commit_hash(commit.id());
            if !all_commits.insert(commit) {
                return Err(user_error(format!(
                    r#"More than one revset resolved to revision {commit_hash}"#,
                )));
            }
        }
    }
    Ok(all_commits)
}

pub fn update_working_copy(
    repo: &Arc<ReadonlyRepo>,
    wc: &mut WorkingCopy,
    old_commit: Option<&Commit>,
    new_commit: &Commit,
) -> Result<Option<CheckoutStats>, CommandError> {
    let old_tree_id = old_commit.map(|commit| commit.merged_tree_id().clone());
    let stats = if Some(new_commit.merged_tree_id()) != old_tree_id.as_ref() {
        // TODO: CheckoutError::ConcurrentCheckout should probably just result in a
        // warning for most commands (but be an error for the checkout command)
        let new_tree = new_commit.merged_tree()?;
        let stats = wc
            .check_out(repo.op_id().clone(), old_tree_id.as_ref(), &new_tree)
            .map_err(|err| {
                CommandError::InternalError(format!(
                    "Failed to check out commit {}: {}",
                    new_commit.id().hex(),
                    err
                ))
            })?;
        Some(stats)
    } else {
        // Record new operation id which represents the latest working-copy state
        let locked_wc = wc.start_mutation()?;
        locked_wc.finish(repo.op_id().clone())?;
        None
    };
    Ok(stats)
}

fn load_template_aliases(
    ui: &Ui,
    layered_configs: &LayeredConfigs,
) -> Result<TemplateAliasesMap, CommandError> {
    const TABLE_KEY: &str = "template-aliases";
    let mut aliases_map = TemplateAliasesMap::new();
    // Load from all config layers in order. 'f(x)' in default layer should be
    // overridden by 'f(a)' in user.
    for (_, config) in layered_configs.sources() {
        let table = if let Some(table) = config.get_table(TABLE_KEY).optional()? {
            table
        } else {
            continue;
        };
        for (decl, value) in table.into_iter().sorted_by(|a, b| a.0.cmp(&b.0)) {
            let r = value
                .into_string()
                .map_err(|e| e.to_string())
                .and_then(|v| aliases_map.insert(&decl, v).map_err(|e| e.to_string()));
            if let Err(s) = r {
                writeln!(ui.warning(), r#"Failed to load "{TABLE_KEY}.{decl}": {s}"#)?;
            }
        }
    }
    Ok(aliases_map)
}

#[instrument(skip_all)]
fn parse_commit_summary_template<'a>(
    repo: &'a dyn Repo,
    workspace_id: &WorkspaceId,
    id_prefix_context: &'a IdPrefixContext,
    aliases_map: &TemplateAliasesMap,
    settings: &UserSettings,
) -> Result<Box<dyn Template<Commit> + 'a>, CommandError> {
    let template_text = settings.config().get_string("templates.commit_summary")?;
    Ok(commit_templater::parse(
        repo,
        workspace_id,
        id_prefix_context,
        &template_text,
        aliases_map,
    )?)
}

/// Helper to reformat content of log-like commands.
#[derive(Clone, Debug)]
pub enum LogContentFormat {
    NoWrap,
    Wrap { term_width: usize },
}

impl LogContentFormat {
    pub fn new(ui: &Ui, settings: &UserSettings) -> Result<Self, config::ConfigError> {
        if settings.config().get_bool("ui.log-word-wrap")? {
            let term_width = usize::from(ui.term_width().unwrap_or(80));
            Ok(LogContentFormat::Wrap { term_width })
        } else {
            Ok(LogContentFormat::NoWrap)
        }
    }

    pub fn write(
        &self,
        formatter: &mut dyn Formatter,
        content_fn: impl FnOnce(&mut dyn Formatter) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        self.write_graph_text(formatter, content_fn, || 0)
    }

    pub fn write_graph_text(
        &self,
        formatter: &mut dyn Formatter,
        content_fn: impl FnOnce(&mut dyn Formatter) -> std::io::Result<()>,
        graph_width_fn: impl FnOnce() -> usize,
    ) -> std::io::Result<()> {
        match self {
            LogContentFormat::NoWrap => content_fn(formatter),
            LogContentFormat::Wrap { term_width } => {
                let mut recorder = FormatRecorder::new();
                content_fn(&mut recorder)?;
                text_util::write_wrapped(
                    formatter,
                    &recorder,
                    term_width.saturating_sub(graph_width_fn()),
                )?;
                Ok(())
            }
        }
    }
}

// TODO: Use a proper TOML library to serialize instead.
pub fn serialize_config_value(value: &config::Value) -> String {
    match &value.kind {
        config::ValueKind::Table(table) => format!(
            "{{{}}}",
            // TODO: Remove sorting when config crate maintains deterministic ordering.
            table
                .iter()
                .sorted_by_key(|(k, _)| *k)
                .map(|(k, v)| format!("{k}={}", serialize_config_value(v)))
                .join(", ")
        ),
        config::ValueKind::Array(vals) => {
            format!("[{}]", vals.iter().map(serialize_config_value).join(", "))
        }
        config::ValueKind::String(val) => format!("{val:?}"),
        _ => value.to_string(),
    }
}

pub fn write_config_value_to_file(
    key: &str,
    value_str: &str,
    path: &Path,
) -> Result<(), CommandError> {
    // Read config
    let config_toml = std::fs::read_to_string(path).or_else(|err| {
        match err.kind() {
            // If config doesn't exist yet, read as empty and we'll write one.
            std::io::ErrorKind::NotFound => Ok("".to_string()),
            _ => Err(user_error(format!(
                "Failed to read file {path}: {err:?}",
                path = path.display()
            ))),
        }
    })?;
    let mut doc = toml_edit::Document::from_str(&config_toml).map_err(|err| {
        user_error(format!(
            "Failed to parse file {path}: {err:?}",
            path = path.display()
        ))
    })?;

    // Apply config value
    // Iterpret value as string unless it's another simple scalar type.
    // TODO(#531): Infer types based on schema (w/ --type arg to override).
    let item = match toml_edit::Value::from_str(value_str) {
        Ok(value @ toml_edit::Value::Boolean(..))
        | Ok(value @ toml_edit::Value::Integer(..))
        | Ok(value @ toml_edit::Value::Float(..))
        | Ok(value @ toml_edit::Value::String(..)) => toml_edit::value(value),
        _ => toml_edit::value(value_str),
    };
    let mut target_table = doc.as_table_mut();
    let mut key_parts_iter = key.split('.');
    // Note: split guarantees at least one item.
    let last_key_part = key_parts_iter.next_back().unwrap();
    for key_part in key_parts_iter {
        target_table = target_table
            .entry(key_part)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                user_error(format!(
                    "Failed to set {key}: would overwrite non-table value with parent table"
                ))
            })?;
    }
    // Error out if overwriting non-scalar value for key (table or array).
    match target_table.get(last_key_part) {
        None | Some(toml_edit::Item::None) => {}
        Some(toml_edit::Item::Value(val)) if !val.is_array() && !val.is_inline_table() => {}
        _ => {
            return Err(user_error(format!(
                "Failed to set {key}: would overwrite entire non-scalar value with scalar"
            )))
        }
    }
    target_table[last_key_part] = item;

    // Write config back
    std::fs::write(path, doc.to_string()).map_err(|err| {
        user_error(format!(
            "Failed to write file {path}: {err:?}",
            path = path.display()
        ))
    })
}

pub fn get_new_config_file_path(
    config_source: &ConfigSource,
    command: &CommandHelper,
) -> Result<PathBuf, CommandError> {
    let edit_path = match config_source {
        // TODO(#531): Special-case for editors that can't handle viewing directories?
        ConfigSource::User => {
            new_config_path()?.ok_or_else(|| user_error("No repo config path found to edit"))?
        }
        ConfigSource::Repo => command.workspace_loader()?.repo_path().join("config.toml"),
        _ => {
            return Err(user_error(format!(
                "Can't get path for config source {config_source:?}"
            )));
        }
    };
    Ok(edit_path)
}

pub fn run_ui_editor(settings: &UserSettings, edit_path: &PathBuf) -> Result<(), CommandError> {
    let editor: CommandNameAndArgs = settings
        .config()
        .get("ui.editor")
        .map_err(|err| CommandError::ConfigError(format!("ui.editor: {err}")))?;
    let exit_status = editor
        .to_command()
        .arg(edit_path)
        .status()
        .map_err(|_| user_error(format!("Failed to run editor '{editor}'")))?;
    if !exit_status.success() {
        return Err(user_error(format!(
            "Editor '{editor}' exited with an error"
        )));
    }

    Ok(())
}

pub fn short_commit_hash(commit_id: &CommitId) -> String {
    commit_id.hex()[0..12].to_string()
}

pub fn short_change_hash(change_id: &ChangeId) -> String {
    // TODO: We could avoid the unwrap() and make this more efficient by converting
    // straight from binary.
    to_reverse_hex(&change_id.hex()[0..12]).unwrap()
}

pub fn short_operation_hash(operation_id: &OperationId) -> String {
    operation_id.hex()[0..12].to_string()
}

/// Jujutsu (An experimental VCS)
///
/// To get started, see the tutorial at https://github.com/martinvonz/jj/blob/main/docs/tutorial.md.
#[derive(clap::Parser, Clone, Debug)]
#[command(name = "jj")]
pub struct Args {
    #[command(flatten)]
    pub global_args: GlobalArgs,
}

#[derive(clap::Args, Clone, Debug)]
pub struct GlobalArgs {
    /// Path to repository to operate on
    ///
    /// By default, Jujutsu searches for the closest .jj/ directory in an
    /// ancestor of the current working directory.
    #[arg(
    long,
    short = 'R',
    global = true,
    help_heading = "Global Options",
    value_hint = clap::ValueHint::DirPath,
    )]
    pub repository: Option<String>,
    /// Don't snapshot the working copy, and don't update it
    ///
    /// By default, Jujutsu snapshots the working copy at the beginning of every
    /// command. The working copy is also updated at the end of the command,
    /// if the command modified the working-copy commit (`@`). If you want
    /// to avoid snapshotting the working and instead see a possibly
    /// stale working copy commit, you can use `--ignore-working-copy`.
    /// This may be useful e.g. in a command prompt, especially if you have
    /// another process that commits the working copy.
    ///
    /// Loading the repository is at a specific operation with `--at-operation`
    /// implies `--ignore-working-copy`.
    #[arg(long, global = true, help_heading = "Global Options")]
    pub ignore_working_copy: bool,
    /// Operation to load the repo at
    ///
    /// Operation to load the repo at. By default, Jujutsu loads the repo at the
    /// most recent operation. You can use `--at-op=<operation ID>` to see what
    /// the repo looked like at an earlier operation. For example `jj
    /// --at-op=<operation ID> st` will show you what `jj st` would have
    /// shown you when the given operation had just finished.
    ///
    /// Use `jj op log` to find the operation ID you want. Any unambiguous
    /// prefix of the operation ID is enough.
    ///
    /// When loading the repo at an earlier operation, the working copy will be
    /// ignored, as if `--ignore-working-copy` had been specified.
    ///
    /// It is possible to run mutating commands when loading the repo at an
    /// earlier operation. Doing that is equivalent to having run concurrent
    /// commands starting at the earlier operation. There's rarely a reason to
    /// do that, but it is possible.
    #[arg(
        long,
        visible_alias = "at-op",
        global = true,
        help_heading = "Global Options",
        default_value = "@"
    )]
    pub at_operation: String,
    /// Enable verbose logging
    #[arg(long, short = 'v', global = true, help_heading = "Global Options")]
    pub verbose: bool,

    #[command(flatten)]
    pub early_args: EarlyArgs,
}

#[derive(clap::Args, Clone, Debug)]
pub struct EarlyArgs {
    /// When to colorize output (always, never, auto)
    #[arg(
        long,
        value_name = "WHEN",
        global = true,
        help_heading = "Global Options"
    )]
    pub color: Option<ColorChoice>,
    /// Disable the pager
    #[arg(
        long,
        value_name = "WHEN",
        global = true,
        help_heading = "Global Options",
        action = ArgAction::SetTrue
    )]
    // Parsing with ignore_errors will crash if this is bool, so use
    // Option<bool>.
    pub no_pager: Option<bool>,
    /// Additional configuration options
    //  TODO: Introduce a `--config` option with simpler syntax for simple
    //  cases, designed so that `--config ui.color=auto` works
    #[arg(
        long,
        value_name = "TOML",
        global = true,
        help_heading = "Global Options"
    )]
    pub config_toml: Vec<String>,
}

/// Create a description from a list of paragraphs.
///
/// Based on the Git CLI behavior. See `opt_parse_m()` and `cleanup_mode` in
/// `git/builtin/commit.c`.
pub fn join_message_paragraphs(paragraphs: &[String]) -> String {
    // Ensure each paragraph ends with a newline, then add another newline between
    // paragraphs.
    paragraphs
        .iter()
        .map(|p| text_util::complete_newline(p.as_str()))
        .join("\n")
}

#[derive(Clone, Debug)]
pub struct RevisionArg(String);

impl Deref for RevisionArg {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.0.as_str()
    }
}

#[derive(Clone)]
pub struct RevisionArgValueParser;

impl TypedValueParser for RevisionArgValueParser {
    type Value = RevisionArg;

    fn parse_ref(
        &self,
        cmd: &Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let string = NonEmptyStringValueParser::new().parse(cmd, arg, value.to_os_string())?;
        Ok(RevisionArg(string))
    }
}

impl ValueParserFactory for RevisionArg {
    type Parser = RevisionArgValueParser;

    fn value_parser() -> RevisionArgValueParser {
        RevisionArgValueParser
    }
}

fn resolve_default_command(
    ui: &Ui,
    config: &config::Config,
    app: &Command,
    mut string_args: Vec<String>,
) -> Result<Vec<String>, CommandError> {
    const PRIORITY_FLAGS: &[&str] = &["help", "--help", "-h", "--version", "-V"];

    let has_priority_flag = string_args
        .iter()
        .any(|arg| PRIORITY_FLAGS.contains(&arg.as_str()));
    if has_priority_flag {
        return Ok(string_args);
    }

    let app_clone = app
        .clone()
        .allow_external_subcommands(true)
        .ignore_errors(true);
    let matches = app_clone.try_get_matches_from(&string_args).ok();

    if let Some(matches) = matches {
        if matches.subcommand_name().is_none() {
            if config.get_string("ui.default-command").is_err() {
                writeln!(
                    ui.hint(),
                    "Hint: Use `jj -h` for a list of available commands."
                )?;
                writeln!(
                    ui.hint(),
                    "Set the config `ui.default-command = \"log\"` to disable this message."
                )?;
            }
            let default_command = config
                .get_string("ui.default-command")
                .unwrap_or_else(|_| "log".to_string());
            // Insert the default command directly after the path to the binary.
            string_args.insert(1, default_command);
        }
    }
    Ok(string_args)
}

fn resolve_aliases(
    config: &config::Config,
    app: &Command,
    mut string_args: Vec<String>,
) -> Result<Vec<String>, CommandError> {
    let mut aliases_map = config.get_table("aliases")?;
    if let Ok(alias_map) = config.get_table("alias") {
        for (alias, definition) in alias_map {
            if aliases_map.insert(alias.clone(), definition).is_some() {
                return Err(user_error_with_hint(
                    format!(r#"Alias "{alias}" is defined in both [aliases] and [alias]"#),
                    "[aliases] is the preferred section for aliases. Please remove the alias from \
                     [alias].",
                ));
            }
        }
    }
    let mut resolved_aliases = HashSet::new();
    let mut real_commands = HashSet::new();
    for command in app.get_subcommands() {
        real_commands.insert(command.get_name().to_string());
        for alias in command.get_all_aliases() {
            real_commands.insert(alias.to_string());
        }
    }
    loop {
        let app_clone = app.clone().allow_external_subcommands(true);
        let matches = app_clone.try_get_matches_from(&string_args).ok();
        if let Some((command_name, submatches)) = matches.as_ref().and_then(|m| m.subcommand()) {
            if !real_commands.contains(command_name) {
                let alias_name = command_name.to_string();
                let alias_args = submatches
                    .get_many::<OsString>("")
                    .unwrap_or_default()
                    .map(|arg| arg.to_str().unwrap().to_string())
                    .collect_vec();
                if resolved_aliases.contains(&alias_name) {
                    return Err(user_error(format!(
                        r#"Recursive alias definition involving "{alias_name}""#
                    )));
                }
                if let Some(value) = aliases_map.remove(&alias_name) {
                    if let Ok(alias_definition) = value.try_deserialize::<Vec<String>>() {
                        assert!(string_args.ends_with(&alias_args));
                        string_args.truncate(string_args.len() - 1 - alias_args.len());
                        string_args.extend(alias_definition);
                        string_args.extend_from_slice(&alias_args);
                        resolved_aliases.insert(alias_name.clone());
                        continue;
                    } else {
                        return Err(user_error(format!(
                            r#"Alias definition for "{alias_name}" must be a string list"#
                        )));
                    }
                } else {
                    // Not a real command and not an alias, so return what we've resolved so far
                    return Ok(string_args);
                }
            }
        }
        // No more alias commands, or hit unknown option
        return Ok(string_args);
    }
}

/// Parse args that must be interpreted early, e.g. before printing help.
fn handle_early_args(
    ui: &mut Ui,
    app: &Command,
    args: &[String],
    layered_configs: &mut LayeredConfigs,
) -> Result<(), CommandError> {
    // ignore_errors() bypasses errors like missing subcommand
    let early_matches = app
        .clone()
        .disable_version_flag(true)
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .ignore_errors(true)
        .try_get_matches_from(args)?;
    let mut args: EarlyArgs = EarlyArgs::from_arg_matches(&early_matches).unwrap();

    if let Some(choice) = args.color {
        args.config_toml.push(format!(r#"ui.color="{choice}""#));
    }
    if args.no_pager.unwrap_or_default() {
        args.config_toml.push(r#"ui.paginate="never""#.to_owned());
    }
    if !args.config_toml.is_empty() {
        layered_configs.parse_config_args(&args.config_toml)?;
        ui.reset(&layered_configs.merge())?;
    }
    Ok(())
}

pub fn expand_args(
    ui: &Ui,
    app: &Command,
    args_os: ArgsOs,
    config: &config::Config,
) -> Result<Vec<String>, CommandError> {
    let mut string_args: Vec<String> = vec![];
    for arg_os in args_os {
        if let Some(string_arg) = arg_os.to_str() {
            string_args.push(string_arg.to_owned());
        } else {
            return Err(CommandError::CliError("Non-utf8 argument".to_string()));
        }
    }

    let string_args = resolve_default_command(ui, config, app, string_args)?;
    resolve_aliases(config, app, string_args)
}

pub fn parse_args(
    ui: &mut Ui,
    app: &Command,
    tracing_subscription: &TracingSubscription,
    string_args: &[String],
    layered_configs: &mut LayeredConfigs,
) -> Result<(ArgMatches, Args), CommandError> {
    handle_early_args(ui, app, string_args, layered_configs)?;
    let matches = app.clone().try_get_matches_from(string_args)?;

    let args: Args = Args::from_arg_matches(&matches).unwrap();
    if args.global_args.verbose {
        // TODO: set up verbose logging as early as possible
        tracing_subscription.enable_verbose_logging()?;
    }

    Ok((matches, args))
}

const BROKEN_PIPE_EXIT_CODE: u8 = 3;

pub fn handle_command_result(
    ui: &mut Ui,
    result: Result<(), CommandError>,
) -> std::io::Result<ExitCode> {
    match result {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(CommandError::UserError { message, hint }) => {
            writeln!(ui.error(), "Error: {message}")?;
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
        Err(CommandError::ClapCliError(inner)) => {
            let clap_str = if ui.color() {
                inner.render().ansi().to_string()
            } else {
                inner.render().to_string()
            };

            match inner.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    ui.request_pager()
                }
                _ => {}
            };
            // Definitions for exit codes and streams come from
            // https://github.com/clap-rs/clap/blob/master/src/error/mod.rs
            match inner.kind() {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                    ui.write(&clap_str)?;
                    Ok(ExitCode::SUCCESS)
                }
                _ => {
                    ui.write_stderr(&clap_str)?;
                    Ok(ExitCode::from(2))
                }
            }
        }
        Err(CommandError::BrokenPipe) => {
            // A broken pipe is not an error, but a signal to exit gracefully.
            Ok(ExitCode::from(BROKEN_PIPE_EXIT_CODE))
        }
        Err(CommandError::InternalError(message)) => {
            writeln!(ui.error(), "Internal error: {message}")?;
            Ok(ExitCode::from(255))
        }
    }
}

/// CLI command builder and runner.
#[must_use]
pub struct CliRunner {
    tracing_subscription: TracingSubscription,
    app: Command,
    extra_configs: Option<config::Config>,
    store_factories: Option<StoreFactories>,
    dispatch_fn: CliDispatchFn,
    process_global_args_fns: Vec<ProcessGlobalArgsFn>,
}

type CliDispatchFn = Box<dyn FnOnce(&mut Ui, &CommandHelper) -> Result<(), CommandError>>;

type ProcessGlobalArgsFn = Box<dyn FnOnce(&mut Ui, &ArgMatches) -> Result<(), CommandError>>;

impl CliRunner {
    /// Initializes CLI environment and returns a builder. This should be called
    /// as early as possible.
    pub fn init() -> Self {
        let tracing_subscription = TracingSubscription::init();
        crate::cleanup_guard::init();
        CliRunner {
            tracing_subscription,
            app: crate::commands::default_app(),
            extra_configs: None,
            store_factories: None,
            dispatch_fn: Box::new(crate::commands::run_command),
            process_global_args_fns: vec![],
        }
    }

    /// Set the version to be displayed by `jj version`.
    pub fn version(mut self, version: &'static str) -> Self {
        self.app = self.app.version(version);
        self
    }

    /// Adds default configs in addition to the normal defaults.
    pub fn set_extra_config(mut self, extra_configs: config::Config) -> Self {
        self.extra_configs = Some(extra_configs);
        self
    }

    /// Replaces `StoreFactories` to be used.
    pub fn set_store_factories(mut self, store_factories: StoreFactories) -> Self {
        self.store_factories = Some(store_factories);
        self
    }

    /// Registers new subcommands in addition to the default ones.
    pub fn add_subcommand<C, F>(mut self, custom_dispatch_fn: F) -> Self
    where
        C: clap::Subcommand,
        F: FnOnce(&mut Ui, &CommandHelper, C) -> Result<(), CommandError> + 'static,
    {
        let old_dispatch_fn = self.dispatch_fn;
        let new_dispatch_fn =
            move |ui: &mut Ui, command_helper: &CommandHelper| match C::from_arg_matches(
                command_helper.matches(),
            ) {
                Ok(command) => custom_dispatch_fn(ui, command_helper, command),
                Err(_) => old_dispatch_fn(ui, command_helper),
            };
        self.app = C::augment_subcommands(self.app);
        self.dispatch_fn = Box::new(new_dispatch_fn);
        self
    }

    /// Registers new global arguments in addition to the default ones.
    pub fn add_global_args<A, F>(mut self, process_before: F) -> Self
    where
        A: clap::Args,
        F: FnOnce(&mut Ui, A) -> Result<(), CommandError> + 'static,
    {
        let process_global_args_fn = move |ui: &mut Ui, matches: &ArgMatches| {
            let custom_args = A::from_arg_matches(matches).unwrap();
            process_before(ui, custom_args)
        };
        self.app = A::augment_args(self.app);
        self.process_global_args_fns
            .push(Box::new(process_global_args_fn));
        self
    }

    #[instrument(skip_all)]
    fn run_internal(
        self,
        ui: &mut Ui,
        mut layered_configs: LayeredConfigs,
    ) -> Result<(), CommandError> {
        let cwd = env::current_dir().map_err(|_| {
            user_error_with_hint(
                "Could not determine current directory",
                "Did you check-out a commit where the directory doesn't exist?",
            )
        })?;
        layered_configs.read_user_config()?;
        let config = layered_configs.merge();
        ui.reset(&config)?;
        let string_args = expand_args(ui, &self.app, std::env::args_os(), &config)?;
        let (matches, args) = parse_args(
            ui,
            &self.app,
            &self.tracing_subscription,
            &string_args,
            &mut layered_configs,
        )?;
        for process_global_args_fn in self.process_global_args_fns {
            process_global_args_fn(ui, &matches)?;
        }

        let maybe_workspace_loader = init_workspace_loader(&cwd, &args.global_args);
        if let Ok(loader) = &maybe_workspace_loader {
            // TODO: maybe show error/warning if repo config contained command alias
            layered_configs.read_repo_config(loader.repo_path())?;
        }
        let config = layered_configs.merge();
        ui.reset(&config)?;
        let settings = UserSettings::from_config(config);
        let command_helper = CommandHelper::new(
            self.app,
            cwd,
            string_args,
            matches,
            args.global_args,
            settings,
            layered_configs,
            maybe_workspace_loader,
            self.store_factories.unwrap_or_default(),
        );
        (self.dispatch_fn)(ui, &command_helper)
    }

    #[must_use]
    #[instrument(skip(self))]
    pub fn run(mut self) -> ExitCode {
        let mut default_config = crate::config::default_config();
        if let Some(extra_configs) = self.extra_configs.take() {
            default_config = config::Config::builder()
                .add_source(default_config)
                .add_source(extra_configs)
                .build()
                .unwrap();
        }
        let layered_configs = LayeredConfigs::from_environment(default_config);
        let mut ui = Ui::with_config(&layered_configs.merge())
            .expect("default config should be valid, env vars are stringly typed");
        let result = self.run_internal(&mut ui, layered_configs);
        let exit_code = handle_command_result(&mut ui, result)
            .unwrap_or_else(|_| ExitCode::from(BROKEN_PIPE_EXIT_CODE));
        ui.finalize_pager();
        exit_code
    }
}
