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

use core::fmt;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::env::{self, ArgsOs, VarError};
use std::ffi::OsString;
use std::fmt::Debug;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;
use std::time::SystemTime;
use std::{fs, str};

use clap::builder::{
    MapValueParser, NonEmptyStringValueParser, TypedValueParser, ValueParserFactory,
};
use clap::error::{ContextKind, ContextValue};
use clap::{ArgAction, ArgMatches, Command, FromArgMatches};
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use jj_lib::backend::{ChangeId, CommitId, MergedTreeId, TreeValue};
use jj_lib::commit::Commit;
use jj_lib::fileset::{FilesetExpression, FilesetParseContext};
use jj_lib::git_backend::GitBackend;
use jj_lib::gitignore::{GitIgnoreError, GitIgnoreFile};
use jj_lib::hex_util::to_reverse_hex;
use jj_lib::id_prefix::IdPrefixContext;
use jj_lib::matchers::Matcher;
use jj_lib::merge::MergedTreeValue;
use jj_lib::merged_tree::MergedTree;
use jj_lib::object_id::ObjectId;
use jj_lib::op_store::{OpStoreError, OperationId, RefTarget, WorkspaceId};
use jj_lib::op_walk::OpsetEvaluationError;
use jj_lib::operation::Operation;
use jj_lib::repo::{
    merge_factories_map, CheckOutCommitError, EditCommitError, MutableRepo, ReadonlyRepo, Repo,
    RepoLoader, StoreFactories, StoreLoadError,
};
use jj_lib::repo_path::{FsPathParseError, RepoPath, RepoPathBuf};
use jj_lib::revset::{
    RevsetAliasesMap, RevsetExpression, RevsetExtensions, RevsetFilterPredicate, RevsetFunction,
    RevsetIteratorExt, RevsetModifier, RevsetParseContext, RevsetWorkspaceContext,
    SymbolResolverExtension,
};
use jj_lib::rewrite::restore_tree;
use jj_lib::settings::{ConfigResultExt as _, UserSettings};
use jj_lib::signing::SignInitError;
use jj_lib::str_util::StringPattern;
use jj_lib::transaction::Transaction;
use jj_lib::view::View;
use jj_lib::working_copy::{
    CheckoutStats, LockedWorkingCopy, SnapshotOptions, WorkingCopy, WorkingCopyFactory,
};
use jj_lib::workspace::{
    default_working_copy_factories, LockedWorkspace, WorkingCopyFactories, Workspace,
    WorkspaceLoadError, WorkspaceLoader,
};
use jj_lib::{dag_walk, file_util, fileset, git, op_heads_store, op_walk, revset};
use once_cell::unsync::OnceCell;
use tracing::instrument;
use tracing_chrome::ChromeLayerBuilder;
use tracing_subscriber::prelude::*;

use crate::command_error::{
    cli_error, config_error_with_message, handle_command_result, internal_error,
    internal_error_with_message, user_error, user_error_with_hint, user_error_with_message,
    CommandError,
};
use crate::commit_templater::{CommitTemplateLanguage, CommitTemplateLanguageExtension};
use crate::config::{
    new_config_path, AnnotatedValue, CommandNameAndArgs, ConfigSource, LayeredConfigs,
};
use crate::formatter::{FormatRecorder, Formatter, PlainTextFormatter};
use crate::git_util::{
    is_colocated_git_workspace, print_failed_git_export, print_git_import_stats,
};
use crate::merge_tools::{DiffEditor, MergeEditor, MergeToolConfigError};
use crate::operation_templater::OperationTemplateLanguageExtension;
use crate::revset_util::RevsetExpressionEvaluator;
use crate::template_builder::TemplateLanguage;
use crate::template_parser::TemplateAliasesMap;
use crate::templater::{PropertyPlaceholder, TemplateRenderer};
use crate::ui::{ColorChoice, Ui};
use crate::{revset_util, template_builder, text_util};

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

    pub fn enable_debug_logging(&self) -> Result<(), CommandError> {
        self.reload_log_filter
            .modify(|filter| {
                *filter = tracing_subscriber::EnvFilter::builder()
                    .with_default_directive(tracing::metadata::LevelFilter::DEBUG.into())
                    .from_env_lossy()
            })
            .map_err(|err| internal_error_with_message("failed to enable debug logging", err))?;
        tracing::info!("debug logging enabled");
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
    revset_extensions: Arc<RevsetExtensions>,
    commit_template_extensions: Vec<Arc<dyn CommitTemplateLanguageExtension>>,
    operation_template_extensions: Vec<Arc<dyn OperationTemplateLanguageExtension>>,
    maybe_workspace_loader: Result<WorkspaceLoader, CommandError>,
    store_factories: StoreFactories,
    working_copy_factories: WorkingCopyFactories,
}

impl CommandHelper {
    pub fn app(&self) -> &Command {
        &self.app
    }

    /// Canonical form of the current working directory path.
    ///
    /// A loaded `Workspace::workspace_root()` also returns a canonical path, so
    /// relative paths can be easily computed from these paths.
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

    pub fn revset_extensions(&self) -> &Arc<RevsetExtensions> {
        &self.revset_extensions
    }

    /// Loads template aliases from the configs.
    ///
    /// For most commands that depend on a loaded repo, you should use
    /// `WorkspaceCommandHelper::template_aliases_map()` instead.
    fn load_template_aliases(&self, ui: &Ui) -> Result<TemplateAliasesMap, CommandError> {
        load_template_aliases(ui, &self.layered_configs)
    }

    /// Parses template of the given language into evaluation tree.
    ///
    /// This function also loads template aliases from the settings. Use
    /// `WorkspaceCommandHelper::parse_template()` if you've already
    /// instantiated the workspace helper.
    pub fn parse_template<'a, C: Clone + 'a, L: TemplateLanguage<'a> + ?Sized>(
        &self,
        ui: &Ui,
        language: &L,
        template_text: &str,
        wrap_self: impl Fn(PropertyPlaceholder<C>) -> L::Property,
    ) -> Result<TemplateRenderer<'a, C>, CommandError> {
        let aliases = self.load_template_aliases(ui)?;
        Ok(template_builder::parse(
            language,
            template_text,
            &aliases,
            wrap_self,
        )?)
    }

    pub fn operation_template_extensions(&self) -> &[Arc<dyn OperationTemplateLanguageExtension>] {
        &self.operation_template_extensions
    }

    pub fn workspace_loader(&self) -> Result<&WorkspaceLoader, CommandError> {
        self.maybe_workspace_loader.as_ref().map_err(Clone::clone)
    }

    /// Loads workspace and repo, then snapshots the working copy if allowed.
    #[instrument(skip(self, ui))]
    pub fn workspace_helper(&self, ui: &mut Ui) -> Result<WorkspaceCommandHelper, CommandError> {
        let mut workspace_command = self.workspace_helper_no_snapshot(ui)?;
        workspace_command.maybe_snapshot(ui)?;
        Ok(workspace_command)
    }

    /// Loads workspace and repo, but never snapshots the working copy. Most
    /// commands should use `workspace_helper()` instead.
    #[instrument(skip(self, ui))]
    pub fn workspace_helper_no_snapshot(
        &self,
        ui: &mut Ui,
    ) -> Result<WorkspaceCommandHelper, CommandError> {
        let workspace = self.load_workspace()?;
        let op_head = self.resolve_operation(ui, workspace.repo_loader())?;
        let repo = workspace.repo_loader().load_at(&op_head)?;
        self.for_loaded_repo(ui, workspace, repo)
    }

    pub fn get_working_copy_factory(&self) -> Result<&dyn WorkingCopyFactory, CommandError> {
        let loader = self.workspace_loader()?;

        // We convert StoreLoadError -> WorkspaceLoadError -> CommandError
        let factory: Result<_, WorkspaceLoadError> = loader
            .get_working_copy_factory(&self.working_copy_factories)
            .map_err(|e| e.into());
        let factory = factory
            .map_err(|err| map_workspace_load_error(err, self.global_args.repository.as_deref()))?;
        Ok(factory)
    }

    #[instrument(skip_all)]
    pub fn load_workspace(&self) -> Result<Workspace, CommandError> {
        let loader = self.workspace_loader()?;
        loader
            .load(
                &self.settings,
                &self.store_factories,
                &self.working_copy_factories,
            )
            .map_err(|err| map_workspace_load_error(err, self.global_args.repository.as_deref()))
    }

    #[instrument(skip_all)]
    pub fn resolve_operation(
        &self,
        ui: &mut Ui,
        repo_loader: &RepoLoader,
    ) -> Result<Operation, CommandError> {
        if self.global_args.at_operation == "@" {
            op_heads_store::resolve_op_heads(
                repo_loader.op_heads_store().as_ref(),
                repo_loader.op_store(),
                |op_heads| {
                    writeln!(
                        ui.status(),
                        "Concurrent modification detected, resolving automatically.",
                    )?;
                    let base_repo = repo_loader.load_at(&op_heads[0])?;
                    // TODO: It may be helpful to print each operation we're merging here
                    let mut tx =
                        start_repo_transaction(&base_repo, &self.settings, &self.string_args);
                    for other_op_head in op_heads.into_iter().skip(1) {
                        tx.merge_operation(other_op_head)?;
                        let num_rebased = tx.mut_repo().rebase_descendants(&self.settings)?;
                        if num_rebased > 0 {
                            writeln!(
                                ui.status(),
                                "Rebased {num_rebased} descendant commits onto commits rewritten \
                                 by other operation"
                            )?;
                        }
                    }
                    Ok(tx
                        .write("resolve concurrent operations")
                        .leave_unpublished()
                        .operation()
                        .clone())
                },
            )
        } else {
            let operation =
                op_walk::resolve_op_for_load(repo_loader, &self.global_args.at_operation)?;
            Ok(operation)
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

/// A branch that should be advanced to satisfy the "advance-branches" feature.
/// This is a helper for `WorkspaceCommandTransaction`. It provides a type-safe
/// way to separate the work of checking whether a branch can be advanced and
/// actually advancing it. Advancing the branch never fails, but can't be done
/// until the new `CommitId` is available. Splitting the work in this way also
/// allows us to identify eligible branches without actually moving them and
/// return config errors to the user early.
pub struct AdvanceableBranch {
    name: String,
    old_commit_id: CommitId,
}

/// Helper for parsing and evaluating settings for the advance-branches feature.
/// Settings are configured in the jj config.toml as lists of [`StringPattern`]s
/// for enabled and disabled branches. Example:
/// ```toml
/// [experimental-advance-branches]
/// # Enable the feature for all branches except "main".
/// enabled-branches = ["glob:*"]
/// disabled-branches = ["main"]
/// ```
struct AdvanceBranchesSettings {
    enabled_branches: Vec<StringPattern>,
    disabled_branches: Vec<StringPattern>,
}

impl AdvanceBranchesSettings {
    fn from_config(config: &config::Config) -> Result<Self, CommandError> {
        let get_setting = |setting_key| {
            let setting = format!("experimental-advance-branches.{setting_key}");
            match config.get::<Vec<String>>(&setting).optional()? {
                Some(patterns) => patterns
                    .into_iter()
                    .map(|s| {
                        StringPattern::parse(&s).map_err(|e| {
                            config_error_with_message(
                                format!("Error parsing '{s}' for {setting}"),
                                e,
                            )
                        })
                    })
                    .collect(),
                None => Ok(Vec::new()),
            }
        };
        Ok(Self {
            enabled_branches: get_setting("enabled-branches")?,
            disabled_branches: get_setting("disabled-branches")?,
        })
    }

    /// Returns true if the advance-branches feature is enabled for
    /// `branch_name`.
    fn branch_is_eligible(&self, branch_name: &str) -> bool {
        if self
            .disabled_branches
            .iter()
            .any(|d| d.matches(branch_name))
        {
            return false;
        }
        self.enabled_branches.iter().any(|e| e.matches(branch_name))
    }

    /// Returns true if the config includes at least one "enabled-branches"
    /// pattern.
    fn feature_enabled(&self) -> bool {
        !self.enabled_branches.is_empty()
    }
}

/// Provides utilities for writing a command that works on a [`Workspace`]
/// (which most commands do).
pub struct WorkspaceCommandHelper {
    cwd: PathBuf,
    string_args: Vec<String>,
    global_args: GlobalArgs,
    settings: UserSettings,
    workspace: Workspace,
    user_repo: ReadonlyUserRepo,
    revset_extensions: Arc<RevsetExtensions>,
    // TODO: Parsed template can be cached if it doesn't capture 'repo lifetime
    commit_summary_template_text: String,
    commit_template_extensions: Vec<Arc<dyn CommitTemplateLanguageExtension>>,
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
        let settings = command.settings.clone();
        let commit_summary_template_text =
            settings.config().get_string("templates.commit_summary")?;
        let revset_aliases_map = revset_util::load_revset_aliases(ui, &command.layered_configs)?;
        let template_aliases_map = command.load_template_aliases(ui)?;
        let loaded_at_head = command.global_args.at_operation == "@";
        let may_update_working_copy = loaded_at_head && !command.global_args.ignore_working_copy;
        let working_copy_shared_with_git = is_colocated_git_workspace(&workspace, &repo);
        let helper = Self {
            cwd: command.cwd.clone(),
            string_args: command.string_args.clone(),
            global_args: command.global_args.clone(),
            settings,
            workspace,
            user_repo: ReadonlyUserRepo::new(repo),
            revset_extensions: command.revset_extensions.clone(),
            commit_summary_template_text,
            commit_template_extensions: command.commit_template_extensions.clone(),
            revset_aliases_map,
            template_aliases_map,
            may_update_working_copy,
            working_copy_shared_with_git,
        };
        // Parse commit_summary template (and short-prefixes revset) early to
        // report error before starting mutable operation.
        helper.parse_commit_template(&helper.commit_summary_template_text)?;
        Ok(helper)
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
    pub fn maybe_snapshot(&mut self, ui: &mut Ui) -> Result<(), CommandError> {
        if self.may_update_working_copy {
            if self.working_copy_shared_with_git {
                self.import_git_head(ui)?;
            }
            // Because the Git refs (except HEAD) aren't imported yet, the ref
            // pointing to the new working-copy commit might not be exported.
            // In that situation, the ref would be conflicted anyway, so export
            // failure is okay.
            self.snapshot_working_copy(ui)?;
            // import_git_refs() can rebase the working-copy commit.
            if self.working_copy_shared_with_git {
                self.import_git_refs(ui)?;
            }
        }
        Ok(())
    }

    /// Imports new HEAD from the colocated Git repo.
    ///
    /// If the Git HEAD has changed, this function abandons our old checkout and
    /// checks out the new Git HEAD. The working-copy state will be reset to
    /// point to the new Git HEAD. The working-copy contents won't be updated.
    #[instrument(skip_all)]
    fn import_git_head(&mut self, ui: &mut Ui) -> Result<(), CommandError> {
        assert!(self.may_update_working_copy);
        let mut tx = self.start_transaction();
        git::import_head(tx.mut_repo())?;
        if !tx.mut_repo().has_changes() {
            return Ok(());
        }

        // TODO: There are various ways to get duplicated working-copy
        // commits. Some of them could be mitigated by checking the working-copy
        // operation id after acquiring the lock, but that isn't enough.
        //
        // - moved HEAD was observed by multiple jj processes, and new working-copy
        //   commits are created concurrently.
        // - new HEAD was exported by jj, but the operation isn't committed yet.
        // - new HEAD was exported by jj, but the new working-copy commit isn't checked
        //   out yet.

        let mut tx = tx.into_inner();
        let old_git_head = self.repo().view().git_head().clone();
        let new_git_head = tx.mut_repo().view().git_head().clone();
        if let Some(new_git_head_id) = new_git_head.as_normal() {
            let workspace_id = self.workspace_id().to_owned();
            if let Some(old_wc_commit_id) = self.repo().view().get_wc_commit_id(&workspace_id) {
                tx.mut_repo()
                    .record_abandoned_commit(old_wc_commit_id.clone());
            }
            let new_git_head_commit = tx.mut_repo().store().get_commit(new_git_head_id)?;
            tx.mut_repo()
                .check_out(workspace_id, &self.settings, &new_git_head_commit)?;
            let mut locked_ws = self.workspace.start_working_copy_mutation()?;
            // The working copy was presumably updated by the git command that updated
            // HEAD, so we just need to reset our working copy
            // state to it without updating working copy files.
            locked_ws.locked_wc().reset(&new_git_head_commit)?;
            tx.mut_repo().rebase_descendants(&self.settings)?;
            self.user_repo = ReadonlyUserRepo::new(tx.commit("import git head"));
            locked_ws.finish(self.user_repo.repo.op_id().clone())?;
            if old_git_head.is_present() {
                writeln!(
                    ui.status(),
                    "Reset the working copy parent to the new Git HEAD."
                )?;
            } else {
                // Don't print verbose message on initial checkout.
            }
        } else {
            // Unlikely, but the HEAD ref got deleted by git?
            self.finish_transaction(ui, tx, "import git head")?;
        }
        Ok(())
    }

    /// Imports branches and tags from the underlying Git repo, abandons old
    /// branches.
    ///
    /// If the working-copy branch is rebased, and if update is allowed, the new
    /// working-copy commit will be checked out.
    ///
    /// This function does not import the Git HEAD, but the HEAD may be reset to
    /// the working copy parent if the repository is colocated.
    #[instrument(skip_all)]
    fn import_git_refs(&mut self, ui: &mut Ui) -> Result<(), CommandError> {
        let git_settings = self.settings.git_settings();
        let mut tx = self.start_transaction();
        // Automated import shouldn't fail because of reserved remote name.
        let stats = git::import_some_refs(tx.mut_repo(), &git_settings, |ref_name| {
            !git::is_reserved_git_remote_ref(ref_name)
        })?;
        if !tx.mut_repo().has_changes() {
            return Ok(());
        }

        print_git_import_stats(ui, tx.repo(), &stats, false)?;
        let mut tx = tx.into_inner();
        // Rebase here to show slightly different status message.
        let num_rebased = tx.mut_repo().rebase_descendants(&self.settings)?;
        if num_rebased > 0 {
            writeln!(
                ui.status(),
                "Rebased {num_rebased} descendant commits off of commits rewritten from git"
            )?;
        }
        self.finish_transaction(ui, tx, "import git refs")?;
        writeln!(
            ui.status(),
            "Done importing changes from the underlying Git repo."
        )?;
        Ok(())
    }

    pub fn repo(&self) -> &Arc<ReadonlyRepo> {
        &self.user_repo.repo
    }

    pub fn working_copy(&self) -> &dyn WorkingCopy {
        self.workspace.working_copy()
    }

    pub fn unchecked_start_working_copy_mutation(
        &mut self,
    ) -> Result<(LockedWorkspace, Commit), CommandError> {
        self.check_working_copy_writable()?;
        let wc_commit = if let Some(wc_commit_id) = self.get_wc_commit_id() {
            self.repo().store().get_commit(wc_commit_id)?
        } else {
            return Err(user_error("Nothing checked out in this workspace"));
        };

        let locked_ws = self.workspace.start_working_copy_mutation()?;

        Ok((locked_ws, wc_commit))
    }

    pub fn start_working_copy_mutation(
        &mut self,
    ) -> Result<(LockedWorkspace, Commit), CommandError> {
        let (mut locked_ws, wc_commit) = self.unchecked_start_working_copy_mutation()?;
        if wc_commit.tree_id() != locked_ws.locked_wc().old_tree_id() {
            return Err(user_error("Concurrent working copy operation. Try again."));
        }
        Ok((locked_ws, wc_commit))
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
    pub fn parse_file_path(&self, input: &str) -> Result<RepoPathBuf, FsPathParseError> {
        RepoPathBuf::parse_fs_path(&self.cwd, self.workspace_root(), input)
    }

    /// Parses the given strings as file patterns.
    pub fn parse_file_patterns(
        &self,
        values: &[String],
    ) -> Result<FilesetExpression, CommandError> {
        // TODO: This function might be superseded by parse_union_filesets(),
        // but it would be weird if parse_union_*() had a special case for the
        // empty arguments.
        if values.is_empty() {
            Ok(FilesetExpression::all())
        } else if self.settings.config().get_bool("ui.allow-filesets")? {
            self.parse_union_filesets(values)
        } else {
            let expressions = values
                .iter()
                .map(|v| self.parse_file_path(v))
                .map_ok(FilesetExpression::prefix_path)
                .try_collect()?;
            Ok(FilesetExpression::union_all(expressions))
        }
    }

    /// Parses the given fileset expressions and concatenates them all.
    pub fn parse_union_filesets(
        &self,
        file_args: &[String], // TODO: introduce FileArg newtype?
    ) -> Result<FilesetExpression, CommandError> {
        let ctx = self.fileset_parse_context();
        let expressions: Vec<_> = file_args
            .iter()
            .map(|arg| fileset::parse_maybe_bare(arg, &ctx))
            .try_collect()?;
        Ok(FilesetExpression::union_all(expressions))
    }

    pub(crate) fn fileset_parse_context(&self) -> FilesetParseContext<'_> {
        FilesetParseContext {
            cwd: &self.cwd,
            workspace_root: self.workspace.workspace_root(),
        }
    }

    #[instrument(skip_all)]
    pub fn base_ignores(&self) -> Result<Arc<GitIgnoreFile>, GitIgnoreError> {
        fn get_excludes_file_path(config: &gix::config::File) -> Option<PathBuf> {
            // TODO: maybe use path_by_key() and interpolate(), which can process non-utf-8
            // path on Unix.
            if let Some(value) = config.string_by_key("core.excludesFile") {
                str::from_utf8(&value)
                    .ok()
                    .map(crate::git_util::expand_git_path)
            } else {
                xdg_config_home().ok().map(|x| x.join("git").join("ignore"))
            }
        }

        fn xdg_config_home() -> Result<PathBuf, VarError> {
            if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
                if !x.is_empty() {
                    return Ok(PathBuf::from(x));
                }
            }
            std::env::var("HOME").map(|x| Path::new(&x).join(".config"))
        }

        let mut git_ignores = GitIgnoreFile::empty();
        if let Some(git_backend) = self.git_backend() {
            let git_repo = git_backend.git_repo();
            if let Some(excludes_file_path) = get_excludes_file_path(&git_repo.config_snapshot()) {
                git_ignores = git_ignores.chain_with_file("", excludes_file_path)?;
            }
            git_ignores = git_ignores
                .chain_with_file("", git_backend.git_repo_path().join("info").join("exclude"))?;
        } else if let Ok(git_config) = gix::config::File::from_globals() {
            if let Some(excludes_file_path) = get_excludes_file_path(&git_config) {
                git_ignores = git_ignores.chain_with_file("", excludes_file_path)?;
            }
        }
        Ok(git_ignores)
    }

    /// Loads diff editor from the settings.
    ///
    /// If the `tool_name` isn't specified, the default editor will be returned.
    pub fn diff_editor(
        &self,
        ui: &Ui,
        tool_name: Option<&str>,
    ) -> Result<DiffEditor, CommandError> {
        let base_ignores = self.base_ignores()?;
        if let Some(name) = tool_name {
            Ok(DiffEditor::with_name(name, &self.settings, base_ignores)?)
        } else {
            Ok(DiffEditor::from_settings(ui, &self.settings, base_ignores)?)
        }
    }

    /// Conditionally loads diff editor from the settings.
    ///
    /// If the `tool_name` is specified, interactive session is implied.
    pub fn diff_selector(
        &self,
        ui: &Ui,
        tool_name: Option<&str>,
        force_interactive: bool,
    ) -> Result<DiffSelector, CommandError> {
        if tool_name.is_some() || force_interactive {
            Ok(DiffSelector::Interactive(self.diff_editor(ui, tool_name)?))
        } else {
            Ok(DiffSelector::NonInteractive)
        }
    }

    /// Loads 3-way merge editor from the settings.
    ///
    /// If the `tool_name` isn't specified, the default editor will be returned.
    pub fn merge_editor(
        &self,
        ui: &Ui,
        tool_name: Option<&str>,
    ) -> Result<MergeEditor, MergeToolConfigError> {
        if let Some(name) = tool_name {
            MergeEditor::with_name(name, &self.settings)
        } else {
            MergeEditor::from_settings(ui, &self.settings)
        }
    }

    pub fn resolve_single_op(&self, op_str: &str) -> Result<Operation, OpsetEvaluationError> {
        op_walk::resolve_op_with_repo(self.repo(), op_str)
    }

    /// Resolve a revset to a single revision. Return an error if the revset is
    /// empty or has multiple revisions.
    pub fn resolve_single_rev(&self, revision_arg: &RevisionArg) -> Result<Commit, CommandError> {
        let expression = self.parse_revset(revision_arg)?;
        let should_hint_about_all_prefix = false;
        revset_util::evaluate_revset_to_single_commit(
            revision_arg.as_ref(),
            &expression,
            || self.commit_summary_template(),
            should_hint_about_all_prefix,
        )
    }

    /// Evaluates revset expressions to non-empty set of commits. The returned
    /// set preserves the order of the input expressions.
    ///
    /// If an input expression is prefixed with `all:`, it may be evaluated to
    /// any number of revisions (including 0.)
    pub fn resolve_some_revsets_default_single(
        &self,
        revision_args: &[RevisionArg],
    ) -> Result<IndexSet<Commit>, CommandError> {
        let mut all_commits = IndexSet::new();
        for revision_arg in revision_args {
            let (expression, modifier) = self.parse_revset_with_modifier(revision_arg)?;
            let all = match modifier {
                Some(RevsetModifier::All) => true,
                None => self
                    .settings
                    .config()
                    .get_bool("ui.always-allow-large-revsets")?,
            };
            if all {
                for commit in expression.evaluate_to_commits()? {
                    all_commits.insert(commit?);
                }
            } else {
                let should_hint_about_all_prefix = true;
                let commit = revset_util::evaluate_revset_to_single_commit(
                    revision_arg.as_ref(),
                    &expression,
                    || self.commit_summary_template(),
                    should_hint_about_all_prefix,
                )?;
                let commit_hash = short_commit_hash(commit.id());
                if !all_commits.insert(commit) {
                    return Err(user_error(format!(
                        r#"More than one revset resolved to revision {commit_hash}"#,
                    )));
                }
            }
        }
        if all_commits.is_empty() {
            Err(user_error("Empty revision set"))
        } else {
            Ok(all_commits)
        }
    }

    pub fn parse_revset(
        &self,
        revision_arg: &RevisionArg,
    ) -> Result<RevsetExpressionEvaluator<'_>, CommandError> {
        let expression = revset::parse(revision_arg.as_ref(), &self.revset_parse_context())?;
        self.attach_revset_evaluator(expression)
    }

    // TODO: maybe better to parse all: prefix even if it is the default? It
    // shouldn't be allowed in aliases, though.
    fn parse_revset_with_modifier(
        &self,
        revision_arg: &RevisionArg,
    ) -> Result<(RevsetExpressionEvaluator<'_>, Option<RevsetModifier>), CommandError> {
        let context = self.revset_parse_context();
        let (expression, modifier) = revset::parse_with_modifier(revision_arg.as_ref(), &context)?;
        Ok((self.attach_revset_evaluator(expression)?, modifier))
    }

    /// Parses the given revset expressions and concatenates them all.
    pub fn parse_union_revsets(
        &self,
        revision_args: &[RevisionArg],
    ) -> Result<RevsetExpressionEvaluator<'_>, CommandError> {
        let context = self.revset_parse_context();
        let expressions: Vec<_> = revision_args
            .iter()
            .map(|arg| revset::parse(arg.as_ref(), &context))
            .try_collect()?;
        let expression = RevsetExpression::union_all(&expressions);
        self.attach_revset_evaluator(expression)
    }

    pub fn attach_revset_evaluator(
        &self,
        expression: Rc<RevsetExpression>,
    ) -> Result<RevsetExpressionEvaluator<'_>, CommandError> {
        Ok(RevsetExpressionEvaluator::new(
            self.repo().as_ref(),
            self.revset_extensions.clone(),
            self.id_prefix_context()?,
            expression,
        ))
    }

    pub(crate) fn revset_parse_context(&self) -> RevsetParseContext {
        let workspace_context = RevsetWorkspaceContext {
            cwd: &self.cwd,
            workspace_id: self.workspace_id(),
            workspace_root: self.workspace.workspace_root(),
        };
        RevsetParseContext::new(
            &self.revset_aliases_map,
            self.settings.user_email(),
            &self.revset_extensions,
            Some(workspace_context),
        )
    }

    pub fn id_prefix_context(&self) -> Result<&IdPrefixContext, CommandError> {
        self.user_repo.id_prefix_context.get_or_try_init(|| {
            let mut context: IdPrefixContext = IdPrefixContext::new(self.revset_extensions.clone());
            let revset_string: String = self
                .settings
                .config()
                .get_string("revsets.short-prefixes")
                .unwrap_or_else(|_| self.settings.default_revset());
            if !revset_string.is_empty() {
                let disambiguation_revset =
                    revset::parse(&revset_string, &self.revset_parse_context()).map_err(|err| {
                        config_error_with_message("Invalid `revsets.short-prefixes`", err)
                    })?;
                context = context.disambiguate_within(revset::optimize(disambiguation_revset));
            }
            Ok(context)
        })
    }

    pub fn template_aliases_map(&self) -> &TemplateAliasesMap {
        &self.template_aliases_map
    }

    /// Parses template of the given language into evaluation tree.
    ///
    /// `wrap_self` specifies the type of the top-level property, which should
    /// be one of the `L::wrap_*()` functions.
    pub fn parse_template<'a, C: Clone + 'a, L: TemplateLanguage<'a> + ?Sized>(
        &self,
        language: &L,
        template_text: &str,
        wrap_self: impl Fn(PropertyPlaceholder<C>) -> L::Property,
    ) -> Result<TemplateRenderer<'a, C>, CommandError> {
        let aliases = &self.template_aliases_map;
        Ok(template_builder::parse(
            language,
            template_text,
            aliases,
            wrap_self,
        )?)
    }

    /// Parses commit template into evaluation tree.
    pub fn parse_commit_template(
        &self,
        template_text: &str,
    ) -> Result<TemplateRenderer<'_, Commit>, CommandError> {
        let language = self.commit_template_language()?;
        self.parse_template(
            &language,
            template_text,
            CommitTemplateLanguage::wrap_commit,
        )
    }

    /// Creates commit template language environment for this workspace.
    pub fn commit_template_language(&self) -> Result<CommitTemplateLanguage<'_>, CommandError> {
        Ok(CommitTemplateLanguage::new(
            self.repo().as_ref(),
            self.workspace_id(),
            self.revset_parse_context(),
            self.id_prefix_context()?,
            &self.commit_template_extensions,
        ))
    }

    /// Template for one-line summary of a commit.
    pub fn commit_summary_template(&self) -> TemplateRenderer<'_, Commit> {
        self.parse_commit_template(&self.commit_summary_template_text)
            .expect("parse error should be confined by WorkspaceCommandHelper::new()")
    }

    /// Returns one-line summary of the given `commit`.
    ///
    /// Use `write_commit_summary()` to get colorized output. Use
    /// `commit_summary_template()` if you have many commits to process.
    pub fn format_commit_summary(&self, commit: &Commit) -> String {
        let mut output = Vec::new();
        self.write_commit_summary(&mut PlainTextFormatter::new(&mut output), commit)
            .expect("write() to PlainTextFormatter should never fail");
        String::from_utf8(output).expect("template output should be utf-8 bytes")
    }

    /// Writes one-line summary of the given `commit`.
    ///
    /// Use `commit_summary_template()` if you have many commits to process.
    #[instrument(skip_all)]
    pub fn write_commit_summary(
        &self,
        formatter: &mut dyn Formatter,
        commit: &Commit,
    ) -> std::io::Result<()> {
        self.commit_summary_template().format(commit, formatter)
    }

    pub fn check_rewritable<'a>(
        &self,
        commits: impl IntoIterator<Item = &'a CommitId>,
    ) -> Result<(), CommandError> {
        if self.global_args.ignore_immutable {
            let root_id = self.repo().store().root_commit_id();
            return if commits.into_iter().contains(root_id) {
                Err(user_error(format!(
                    "The root commit {} is immutable",
                    short_commit_hash(root_id),
                )))
            } else {
                Ok(())
            };
        }
        let to_rewrite_revset =
            RevsetExpression::commits(commits.into_iter().cloned().collect_vec());
        let immutable = revset_util::parse_immutable_expression(&self.revset_parse_context())
            .map_err(|e| {
                config_error_with_message("Invalid `revset-aliases.immutable_heads()`", e)
            })?;
        let mut expression = self.attach_revset_evaluator(immutable)?;
        expression.intersect_with(&to_rewrite_revset);

        let mut commit_id_iter = expression.evaluate_to_commit_ids().map_err(|e| {
            config_error_with_message("Invalid `revset-aliases.immutable_heads()`", e)
        })?;

        if let Some(commit_id) = commit_id_iter.next() {
            let error = if &commit_id == self.repo().store().root_commit_id() {
                user_error(format!(
                    "The root commit {} is immutable",
                    short_commit_hash(&commit_id),
                ))
            } else {
                user_error_with_hint(
                    format!("Commit {} is immutable", short_commit_hash(&commit_id)),
                    "Pass `--ignore-immutable` or configure the set of immutable commits via \
                     `revset-aliases.immutable_heads()`.",
                )
            };
            return Err(error);
        }

        Ok(())
    }

    #[instrument(skip_all)]
    fn snapshot_working_copy(&mut self, ui: &mut Ui) -> Result<(), CommandError> {
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
        let base_ignores = self.base_ignores()?;

        // Compare working-copy tree and operation with repo's, and reload as needed.
        let mut locked_ws = self.workspace.start_working_copy_mutation()?;
        let old_op_id = locked_ws.locked_wc().old_operation_id().clone();
        let (repo, wc_commit) =
            match check_stale_working_copy(locked_ws.locked_wc(), &wc_commit, &repo) {
                Ok(WorkingCopyFreshness::Fresh) => (repo, wc_commit),
                Ok(WorkingCopyFreshness::Updated(wc_operation)) => {
                    let repo = repo.reload_at(&wc_operation)?;
                    let wc_commit = if let Some(wc_commit) = get_wc_commit(&repo)? {
                        wc_commit
                    } else {
                        return Ok(()); // The workspace has been deleted (see
                                       // above)
                    };
                    (repo, wc_commit)
                }
                Ok(WorkingCopyFreshness::WorkingCopyStale) => {
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
                Ok(WorkingCopyFreshness::SiblingOperation) => {
                    return Err(internal_error(format!(
                        "The repo was loaded at operation {}, which seems to be a sibling of the \
                         working copy's operation {}",
                        short_operation_hash(repo.op_id()),
                        short_operation_hash(&old_op_id)
                    )));
                }
                Err(OpStoreError::ObjectNotFound { .. }) => {
                    return Err(user_error_with_hint(
                        "Could not read working copy's operation.",
                        "Run `jj workspace update-stale` to recover.
See https://github.com/martinvonz/jj/blob/main/docs/working-copy.md#stale-working-copy \
                         for more information.",
                    ))
                }
                Err(e) => return Err(e.into()),
            };
        self.user_repo = ReadonlyUserRepo::new(repo);
        let progress = crate::progress::snapshot_progress(ui);
        let new_tree_id = locked_ws.locked_wc().snapshot(SnapshotOptions {
            base_ignores,
            fsmonitor_kind: self.settings.fsmonitor_kind()?,
            progress: progress.as_ref().map(|x| x as _),
            max_new_file_size: self.settings.max_new_file_size()?,
        })?;
        drop(progress);
        if new_tree_id != *wc_commit.tree_id() {
            let mut tx =
                start_repo_transaction(&self.user_repo.repo, &self.settings, &self.string_args);
            tx.set_is_snapshot(true);
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
                    ui.status(),
                    "Rebased {num_rebased} descendant commits onto updated working copy"
                )?;
            }

            if self.working_copy_shared_with_git {
                let failed_branches = git::export_refs(mut_repo)?;
                print_failed_git_export(ui, &failed_branches)?;
            }

            self.user_repo = ReadonlyUserRepo::new(tx.commit("snapshot working copy"));
        }
        locked_ws.finish(self.user_repo.repo.op_id().clone())?;
        Ok(())
    }

    fn update_working_copy(
        &mut self,
        ui: &mut Ui,
        maybe_old_commit: Option<&Commit>,
        new_commit: &Commit,
    ) -> Result<(), CommandError> {
        assert!(self.may_update_working_copy);
        let stats = update_working_copy(
            &self.user_repo.repo,
            &mut self.workspace,
            maybe_old_commit,
            new_commit,
        )?;
        if Some(new_commit) != maybe_old_commit {
            if let Some(mut formatter) = ui.status_formatter() {
                let template = self.commit_summary_template();
                write!(formatter, "Working copy now at: ")?;
                formatter.with_label("working_copy", |fmt| template.format(new_commit, fmt))?;
                writeln!(formatter)?;
                for parent in new_commit.parents() {
                    //                "Working copy now at: "
                    write!(formatter, "Parent commit      : ")?;
                    template.format(&parent, formatter.as_mut())?;
                    writeln!(formatter)?;
                }
            }
        }
        if let Some(stats) = stats {
            print_checkout_stats(ui, stats, new_commit)?;
        }
        if Some(new_commit) != maybe_old_commit {
            if let Some(mut formatter) = ui.status_formatter() {
                let conflicts = new_commit.tree()?.conflicts().collect_vec();
                if !conflicts.is_empty() {
                    writeln!(formatter, "There are unresolved conflicts at these paths:")?;
                    print_conflicted_paths(&conflicts, formatter.as_mut(), self)?;
                }
            }
        }
        Ok(())
    }

    pub fn start_transaction(&mut self) -> WorkspaceCommandTransaction {
        let tx = start_repo_transaction(self.repo(), &self.settings, &self.string_args);
        WorkspaceCommandTransaction { helper: self, tx }
    }

    fn finish_transaction(
        &mut self,
        ui: &mut Ui,
        mut tx: Transaction,
        description: impl Into<String>,
    ) -> Result<(), CommandError> {
        if !tx.mut_repo().has_changes() {
            writeln!(ui.status(), "Nothing changed.")?;
            return Ok(());
        }
        let num_rebased = tx.mut_repo().rebase_descendants(&self.settings)?;
        if num_rebased > 0 {
            writeln!(ui.status(), "Rebased {num_rebased} descendant commits")?;
        }

        let old_repo = tx.base_repo().clone();

        let maybe_old_wc_commit = old_repo
            .view()
            .get_wc_commit_id(self.workspace_id())
            .map(|commit_id| tx.base_repo().store().get_commit(commit_id))
            .transpose()?;
        let maybe_new_wc_commit = tx
            .repo()
            .view()
            .get_wc_commit_id(self.workspace_id())
            .map(|commit_id| tx.repo().store().get_commit(commit_id))
            .transpose()?;
        if self.working_copy_shared_with_git {
            let git_repo = self.git_backend().unwrap().open_git_repo()?;
            if let Some(wc_commit) = &maybe_new_wc_commit {
                git::reset_head(tx.mut_repo(), &git_repo, wc_commit)?;
            }
            let failed_branches = git::export_refs(tx.mut_repo())?;
            print_failed_git_export(ui, &failed_branches)?;
        }
        self.user_repo = ReadonlyUserRepo::new(tx.commit(description));
        self.report_repo_changes(ui, &old_repo)?;

        if self.may_update_working_copy {
            if let Some(new_commit) = &maybe_new_wc_commit {
                self.update_working_copy(ui, maybe_old_wc_commit.as_ref(), new_commit)?;
            } else {
                // It seems the workspace was deleted, so we shouldn't try to
                // update it.
            }
        }
        let settings = &self.settings;
        if settings.user_name().is_empty() || settings.user_email().is_empty() {
            writeln!(
                ui.warning_default(),
                r#"Name and email not configured. Until configured, your commits will be created with the empty identity, and can't be pushed to remotes. To configure, run:
  jj config set --user user.name "Some One"
  jj config set --user user.email "someone@example.com""#
            )?;
        }
        Ok(())
    }

    /// Inform the user about important changes to the repo since the previous
    /// operation (when `old_repo` was loaded).
    fn report_repo_changes(
        &self,
        ui: &mut Ui,
        old_repo: &Arc<ReadonlyRepo>,
    ) -> Result<(), CommandError> {
        let Some(mut fmt) = ui.status_formatter() else {
            return Ok(());
        };
        let old_view = old_repo.view();
        let new_repo = self.repo().as_ref();
        let new_view = new_repo.view();
        let old_heads = RevsetExpression::commits(old_view.heads().iter().cloned().collect());
        let new_heads = RevsetExpression::commits(new_view.heads().iter().cloned().collect());
        // Filter the revsets by conflicts instead of reading all commits and doing the
        // filtering here. That way, we can afford to evaluate the revset even if there
        // are millions of commits added to the repo, assuming the revset engine can
        // efficiently skip non-conflicting commits. Filter out empty commits mostly so
        // `jj new <conflicted commit>` doesn't result in a message about new conflicts.
        let conflicts = RevsetExpression::filter(RevsetFilterPredicate::HasConflict).intersection(
            &RevsetExpression::filter(RevsetFilterPredicate::File(FilesetExpression::all())),
        );
        let removed_conflicts_expr = new_heads.range(&old_heads).intersection(&conflicts);
        let added_conflicts_expr = old_heads.range(&new_heads).intersection(&conflicts);

        let get_commits = |expr: Rc<RevsetExpression>| -> Result<Vec<Commit>, CommandError> {
            let commits = expr
                .evaluate_programmatic(new_repo)?
                .iter()
                .commits(new_repo.store())
                .try_collect()?;
            Ok(commits)
        };
        let removed_conflict_commits = get_commits(removed_conflicts_expr)?;
        let added_conflict_commits = get_commits(added_conflicts_expr)?;

        fn commits_by_change_id(commits: &[Commit]) -> IndexMap<&ChangeId, Vec<&Commit>> {
            let mut result: IndexMap<&ChangeId, Vec<&Commit>> = IndexMap::new();
            for commit in commits {
                result.entry(commit.change_id()).or_default().push(commit);
            }
            result
        }
        let removed_conflicts_by_change_id = commits_by_change_id(&removed_conflict_commits);
        let added_conflicts_by_change_id = commits_by_change_id(&added_conflict_commits);
        let mut resolved_conflicts_by_change_id = removed_conflicts_by_change_id.clone();
        resolved_conflicts_by_change_id
            .retain(|change_id, _commits| !added_conflicts_by_change_id.contains_key(change_id));
        let mut new_conflicts_by_change_id = added_conflicts_by_change_id.clone();
        new_conflicts_by_change_id
            .retain(|change_id, _commits| !removed_conflicts_by_change_id.contains_key(change_id));

        // TODO: Also report new divergence and maybe resolved divergence
        let template = self.commit_summary_template();
        if !resolved_conflicts_by_change_id.is_empty() {
            writeln!(
                fmt,
                "Existing conflicts were resolved or abandoned from these commits:"
            )?;
            for (_, old_commits) in &resolved_conflicts_by_change_id {
                // TODO: Report which ones were resolved and which ones were abandoned. However,
                // that involves resolving the change_id among the visible commits in the new
                // repo, which isn't currently supported by Google's revset engine.
                for commit in old_commits {
                    write!(fmt, "  ")?;
                    template.format(commit, fmt.as_mut())?;
                    writeln!(fmt)?;
                }
            }
        }
        if !new_conflicts_by_change_id.is_empty() {
            writeln!(fmt, "New conflicts appeared in these commits:")?;
            for (_, new_commits) in &new_conflicts_by_change_id {
                for commit in new_commits {
                    write!(fmt, "  ")?;
                    template.format(commit, fmt.as_mut())?;
                    writeln!(fmt)?;
                }
            }
        }

        // Hint that the user might want to `jj new` to the first conflict commit to
        // resolve conflicts. Only show the hints if there were any new or resolved
        // conflicts, and only if there are still some conflicts.
        if !(added_conflict_commits.is_empty()
            || resolved_conflicts_by_change_id.is_empty() && new_conflicts_by_change_id.is_empty())
        {
            // If the user just resolved some conflict and squashed them in, there won't be
            // any new conflicts. Clarify to them that there are still some other conflicts
            // to resolve. (We don't mention conflicts in commits that weren't affected by
            // the operation, however.)
            if new_conflicts_by_change_id.is_empty() {
                writeln!(
                    fmt,
                    "There are still unresolved conflicts in rebased descendants.",
                )?;
            }

            self.report_repo_conflicts(
                fmt.as_mut(),
                new_repo,
                added_conflict_commits
                    .iter()
                    .map(|commit| commit.id().clone())
                    .collect(),
            )?;
        }

        Ok(())
    }

    pub fn report_repo_conflicts(
        &self,
        fmt: &mut dyn Formatter,
        repo: &ReadonlyRepo,
        conflicted_commits: Vec<CommitId>,
    ) -> Result<(), CommandError> {
        let only_one_conflicted_commit = conflicted_commits.len() == 1;
        let root_conflicts_revset = RevsetExpression::commits(conflicted_commits)
            .roots()
            .evaluate_programmatic(repo)?;

        let root_conflict_change_ids: Vec<_> = root_conflicts_revset
            .iter()
            .commits(repo.store())
            .map(|maybe_commit| maybe_commit.map(|c| c.change_id().clone()))
            .try_collect()?;

        if !root_conflict_change_ids.is_empty() {
            fmt.push_label("hint")?;
            if only_one_conflicted_commit {
                writeln!(fmt, "To resolve the conflicts, start by updating to it:",)?;
            } else if root_conflict_change_ids.len() == 1 {
                writeln!(
                    fmt,
                    "To resolve the conflicts, start by updating to the first one:",
                )?;
            } else {
                writeln!(
                    fmt,
                    "To resolve the conflicts, start by updating to one of the first ones:",
                )?;
            }
            for change_id in root_conflict_change_ids {
                writeln!(fmt, "  jj new {}", short_change_hash(&change_id))?;
            }
            writeln!(
                fmt,
                r#"Then use `jj resolve`, or edit the conflict markers in the file directly.
Once the conflicts are resolved, you may want inspect the result with `jj diff`.
Then run `jj squash` to move the resolution into the conflicted commit."#,
            )?;
            fmt.pop_label()?;
        }
        Ok(())
    }

    /// Identifies branches which are eligible to be moved automatically during
    /// `jj commit` and `jj new`. Whether a branch is eligible is determined by
    /// its target and the user and repo config for "advance-branches".
    ///
    /// Returns a Vec of branches in `repo` that point to any of the `from`
    /// commits and that are eligible to advance. The `from` commits are
    /// typically the parents of the target commit of `jj commit` or `jj new`.
    ///
    /// Branches are not moved until
    /// `WorkspaceCommandTransaction::advance_branches()` is called with the
    /// `AdvanceableBranch`s returned by this function.
    ///
    /// Returns an empty `std::Vec` if no branches are eligible to advance.
    pub fn get_advanceable_branches<'a>(
        &self,
        from: impl IntoIterator<Item = &'a CommitId>,
    ) -> Result<Vec<AdvanceableBranch>, CommandError> {
        let ab_settings = AdvanceBranchesSettings::from_config(self.settings.config())?;
        if !ab_settings.feature_enabled() {
            // Return early if we know that there's no work to do.
            return Ok(Vec::new());
        }

        let mut advanceable_branches = Vec::new();
        for from_commit in from {
            for (name, _) in self.repo().view().local_branches_for_commit(from_commit) {
                if ab_settings.branch_is_eligible(name) {
                    advanceable_branches.push(AdvanceableBranch {
                        name: name.to_owned(),
                        old_commit_id: from_commit.clone(),
                    });
                }
            }
        }

        Ok(advanceable_branches)
    }
}

/// A [`Transaction`] tied to a particular workspace.
/// `WorkspaceCommandTransaction`s are created with
/// [`WorkspaceCommandHelper::start_transaction`] and committed with
/// [`WorkspaceCommandTransaction::finish`]. The inner `Transaction` can also be
/// extracted using [`WorkspaceCommandTransaction::into_inner`] in situations
/// where finer-grained control over the `Transaction` is necessary.
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

    pub fn check_out(&mut self, commit: &Commit) -> Result<Commit, CheckOutCommitError> {
        let workspace_id = self.helper.workspace_id().to_owned();
        let settings = &self.helper.settings;
        self.tx.mut_repo().check_out(workspace_id, settings, commit)
    }

    pub fn edit(&mut self, commit: &Commit) -> Result<(), EditCommitError> {
        let workspace_id = self.helper.workspace_id().to_owned();
        self.tx.mut_repo().edit(workspace_id, commit)
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
        let id_prefix_context = IdPrefixContext::new(self.helper.revset_extensions.clone());
        let language = CommitTemplateLanguage::new(
            self.tx.repo(),
            self.helper.workspace_id(),
            self.helper.revset_parse_context(),
            &id_prefix_context,
            &self.helper.commit_template_extensions,
        );
        let template = self
            .helper
            .parse_template(
                &language,
                &self.helper.commit_summary_template_text,
                CommitTemplateLanguage::wrap_commit,
            )
            .expect("parse error should be confined by WorkspaceCommandHelper::new()");
        template.format(commit, formatter)
    }

    pub fn finish(self, ui: &mut Ui, description: impl Into<String>) -> Result<(), CommandError> {
        self.helper.finish_transaction(ui, self.tx, description)
    }

    /// Returns the wrapped [`Transaction`] for circumstances where
    /// finer-grained control is needed. The caller becomes responsible for
    /// finishing the `Transaction`, including rebasing descendants and updating
    /// the working copy, if applicable.
    pub fn into_inner(self) -> Transaction {
        self.tx
    }

    /// Moves each branch in `branches` from an old commit it's associated with
    /// (configured by `get_advanceable_branches`) to the `move_to` commit. If
    /// the branch is conflicted before the update, it will remain conflicted
    /// after the update, but the conflict will involve the `move_to` commit
    /// instead of the old commit.
    pub fn advance_branches(&mut self, branches: Vec<AdvanceableBranch>, move_to: &CommitId) {
        for branch in branches {
            // This removes the old commit ID from the branch's RefTarget and
            // replaces it with the `move_to` ID.
            self.mut_repo().merge_local_branch(
                &branch.name,
                &RefTarget::normal(branch.old_commit_id),
                &RefTarget::normal(move_to.clone()),
            );
        }
    }
}

fn find_workspace_dir(cwd: &Path) -> &Path {
    cwd.ancestors()
        .find(|path| path.join(".jj").is_dir())
        .unwrap_or(cwd)
}

fn map_workspace_load_error(err: WorkspaceLoadError, workspace_path: Option<&str>) -> CommandError {
    match err {
        WorkspaceLoadError::NoWorkspaceHere(wc_path) => {
            // Prefer user-specified workspace_path_str instead of absolute wc_path.
            let workspace_path_str = workspace_path.unwrap_or(".");
            let message = format!(r#"There is no jj repo in "{workspace_path_str}""#);
            let git_dir = wc_path.join(".git");
            if git_dir.is_dir() {
                user_error_with_hint(
                    message,
                    "It looks like this is a git repo. You can create a jj repo backed by it by \
                     running this:
jj git init --colocate",
                )
            } else {
                user_error(message)
            }
        }
        WorkspaceLoadError::RepoDoesNotExist(repo_dir) => user_error(format!(
            "The repository directory at {} is missing. Was it moved?",
            repo_dir.display(),
        )),
        WorkspaceLoadError::StoreLoadError(err @ StoreLoadError::UnsupportedType { .. }) => {
            internal_error_with_message(
                "This version of the jj binary doesn't support this type of repo",
                err,
            )
        }
        WorkspaceLoadError::StoreLoadError(
            err @ (StoreLoadError::ReadError { .. } | StoreLoadError::Backend(_)),
        ) => internal_error_with_message("The repository appears broken or inaccessible", err),
        WorkspaceLoadError::StoreLoadError(StoreLoadError::Signing(
            err @ SignInitError::UnknownBackend(_),
        )) => user_error(err),
        WorkspaceLoadError::StoreLoadError(err) => internal_error(err),
        WorkspaceLoadError::WorkingCopyState(err) => internal_error(err),
        WorkspaceLoadError::NonUnicodePath | WorkspaceLoadError::Path(_) => user_error(err),
    }
}

pub fn start_repo_transaction(
    repo: &Arc<ReadonlyRepo>,
    settings: &UserSettings,
    string_args: &[String],
) -> Transaction {
    let mut tx = repo.start_transaction(settings);
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

/// Whether the working copy is stale or not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkingCopyFreshness {
    /// The working copy isn't stale, and no need to reload the repo.
    Fresh,
    /// The working copy was updated since we loaded the repo. The repo must be
    /// reloaded at the working copy's operation.
    Updated(Box<Operation>),
    /// The working copy is behind the latest operation.
    WorkingCopyStale,
    /// The working copy is a sibling of the latest operation.
    SiblingOperation,
}

#[instrument(skip_all)]
pub fn check_stale_working_copy(
    locked_wc: &dyn LockedWorkingCopy,
    wc_commit: &Commit,
    repo: &ReadonlyRepo,
) -> Result<WorkingCopyFreshness, OpStoreError> {
    // Check if the working copy's tree matches the repo's view
    let wc_tree_id = locked_wc.old_tree_id();
    if wc_commit.tree_id() == wc_tree_id {
        // The working copy isn't stale, and no need to reload the repo.
        Ok(WorkingCopyFreshness::Fresh)
    } else {
        let wc_operation_data = repo
            .op_store()
            .read_operation(locked_wc.old_operation_id())?;
        let wc_operation = Operation::new(
            repo.op_store().clone(),
            locked_wc.old_operation_id().clone(),
            wc_operation_data,
        );
        let repo_operation = repo.operation();
        let ancestor_op = dag_walk::closest_common_node_ok(
            [Ok(wc_operation.clone())],
            [Ok(repo_operation.clone())],
            |op: &Operation| op.id().clone(),
            |op: &Operation| op.parents().collect_vec(),
        )?
        .expect("unrelated operations");
        if ancestor_op.id() == repo_operation.id() {
            // The working copy was updated since we loaded the repo. The repo must be
            // reloaded at the working copy's operation.
            Ok(WorkingCopyFreshness::Updated(Box::new(wc_operation)))
        } else if ancestor_op.id() == wc_operation.id() {
            // The working copy was not updated when some repo operation committed,
            // meaning that it's stale compared to the repo view.
            Ok(WorkingCopyFreshness::WorkingCopyStale)
        } else {
            Ok(WorkingCopyFreshness::SiblingOperation)
        }
    }
}

#[instrument(skip_all)]
pub fn print_conflicted_paths(
    conflicts: &[(RepoPathBuf, MergedTreeValue)],
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
) -> Result<(), CommandError> {
    let formatted_paths = conflicts
        .iter()
        .map(|(path, _conflict)| workspace_command.format_file_path(path))
        .collect_vec();
    let max_path_len = formatted_paths.iter().map(|p| p.len()).max().unwrap_or(0);
    let formatted_paths = formatted_paths
        .into_iter()
        .map(|p| format!("{:width$}", p, width = max_path_len.min(32) + 3));

    for ((_, conflict), formatted_path) in std::iter::zip(conflicts.iter(), formatted_paths) {
        let sides = conflict.num_sides();
        let n_adds = conflict.adds().flatten().count();
        let deletions = sides - n_adds;

        let mut seen_objects = BTreeMap::new(); // Sort for consistency and easier testing
        if deletions > 0 {
            seen_objects.insert(
                format!(
                    // Starting with a number sorts this first
                    "{deletions} deletion{}",
                    if deletions > 1 { "s" } else { "" }
                ),
                "normal", // Deletions don't interfere with `jj resolve` or diff display
            );
        }
        // TODO: We might decide it's OK for `jj resolve` to ignore special files in the
        // `removes` of a conflict (see e.g. https://github.com/martinvonz/jj/pull/978). In
        // that case, `conflict.removes` should be removed below.
        for term in itertools::chain(conflict.removes(), conflict.adds()).flatten() {
            seen_objects.insert(
                match term {
                    TreeValue::File {
                        executable: false, ..
                    } => continue,
                    TreeValue::File {
                        executable: true, ..
                    } => "an executable",
                    TreeValue::Symlink(_) => "a symlink",
                    TreeValue::Tree(_) => "a directory",
                    TreeValue::GitSubmodule(_) => "a git submodule",
                    TreeValue::Conflict(_) => "another conflict (you found a bug!)",
                }
                .to_string(),
                "difficult",
            );
        }

        write!(formatter, "{formatted_path} ")?;
        formatter.with_label("conflict_description", |formatter| {
            let print_pair = |formatter: &mut dyn Formatter, (text, label): &(String, &str)| {
                write!(formatter.labeled(label), "{text}")
            };
            print_pair(
                formatter,
                &(
                    format!("{sides}-sided"),
                    if sides > 2 { "difficult" } else { "normal" },
                ),
            )?;
            write!(formatter, " conflict")?;

            if !seen_objects.is_empty() {
                write!(formatter, " including ")?;
                let seen_objects = seen_objects.into_iter().collect_vec();
                match &seen_objects[..] {
                    [] => unreachable!(),
                    [only] => print_pair(formatter, only)?,
                    [first, middle @ .., last] => {
                        print_pair(formatter, first)?;
                        for pair in middle {
                            write!(formatter, ", ")?;
                            print_pair(formatter, pair)?;
                        }
                        write!(formatter, " and ")?;
                        print_pair(formatter, last)?;
                    }
                };
            }
            Ok(())
        })?;
        writeln!(formatter)?;
    }
    Ok(())
}

pub fn print_checkout_stats(
    ui: &mut Ui,
    stats: CheckoutStats,
    new_commit: &Commit,
) -> Result<(), std::io::Error> {
    if stats.added_files > 0 || stats.updated_files > 0 || stats.removed_files > 0 {
        writeln!(
            ui.status(),
            "Added {} files, modified {} files, removed {} files",
            stats.added_files,
            stats.updated_files,
            stats.removed_files
        )?;
    }
    if stats.skipped_files != 0 {
        writeln!(
            ui.warning_default(),
            "{} of those updates were skipped because there were conflicting changes in the \
             working copy.",
            stats.skipped_files
        )?;
        if let Some(mut writer) = ui.hint_default() {
            writeln!(
                writer,
                "Inspect the changes compared to the intended target with `jj diff --from {}`.
Discard the conflicting changes with `jj restore --from {}`.",
                short_commit_hash(new_commit.id()),
                short_commit_hash(new_commit.id())
            )?;
        }
    }
    Ok(())
}

/// Prints warning about explicit paths that don't match any of the tree
/// entries.
pub fn print_unmatched_explicit_paths<'a>(
    ui: &Ui,
    workspace_command: &WorkspaceCommandHelper,
    expression: &FilesetExpression,
    trees: impl IntoIterator<Item = &'a MergedTree>,
) -> io::Result<()> {
    let mut explicit_paths = expression.explicit_paths().collect_vec();
    for tree in trees {
        explicit_paths.retain(|&path| tree.path_value(path).is_absent());
        if explicit_paths.is_empty() {
            return Ok(());
        }
    }
    let ui_paths = explicit_paths
        .iter()
        .map(|&path| workspace_command.format_file_path(path))
        .join(", ");
    writeln!(
        ui.warning_default(),
        "No matching entries for paths: {ui_paths}"
    )?;
    Ok(())
}

pub fn print_trackable_remote_branches(ui: &Ui, view: &View) -> io::Result<()> {
    let remote_branch_names = view
        .branches()
        .filter(|(_, branch_target)| branch_target.local_target.is_present())
        .flat_map(|(name, branch_target)| {
            branch_target
                .remote_refs
                .into_iter()
                .filter(|&(_, remote_ref)| !remote_ref.is_tracking())
                .map(move |(remote, _)| format!("{name}@{remote}"))
        })
        .collect_vec();
    if remote_branch_names.is_empty() {
        return Ok(());
    }

    if let Some(mut writer) = ui.hint_default() {
        writeln!(
            writer,
            "The following remote branches aren't associated with the existing local branches:"
        )?;
    }
    if let Some(mut formatter) = ui.status_formatter() {
        for full_name in &remote_branch_names {
            write!(formatter, "  ")?;
            writeln!(formatter.labeled("branch"), "{full_name}")?;
        }
    }
    if let Some(mut writer) = ui.hint_default() {
        writeln!(
            writer,
            "Run `jj branch track {names}` to keep local branches updated on future pulls.",
            names = remote_branch_names.join(" "),
        )?;
    }
    Ok(())
}

pub fn update_working_copy(
    repo: &Arc<ReadonlyRepo>,
    workspace: &mut Workspace,
    old_commit: Option<&Commit>,
    new_commit: &Commit,
) -> Result<Option<CheckoutStats>, CommandError> {
    let old_tree_id = old_commit.map(|commit| commit.tree_id().clone());
    let stats = if Some(new_commit.tree_id()) != old_tree_id.as_ref() {
        // TODO: CheckoutError::ConcurrentCheckout should probably just result in a
        // warning for most commands (but be an error for the checkout command)
        let stats = workspace
            .check_out(repo.op_id().clone(), old_tree_id.as_ref(), new_commit)
            .map_err(|err| {
                internal_error_with_message(
                    format!("Failed to check out commit {}", new_commit.id().hex()),
                    err,
                )
            })?;
        Some(stats)
    } else {
        // Record new operation id which represents the latest working-copy state
        let locked_ws = workspace.start_working_copy_mutation()?;
        locked_ws.finish(repo.op_id().clone())?;
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
                writeln!(
                    ui.warning_default(),
                    r#"Failed to load "{TABLE_KEY}.{decl}": {s}"#
                )?;
            }
        }
    }
    Ok(aliases_map)
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
            _ => Err(user_error_with_message(
                format!("Failed to read file {path}", path = path.display()),
                err,
            )),
        }
    })?;
    let mut doc = toml_edit::Document::from_str(&config_toml).map_err(|err| {
        user_error_with_message(
            format!("Failed to parse file {path}", path = path.display()),
            err,
        )
    })?;

    // Apply config value
    // Interpret value as string if it can't be parsed as a TOML value.
    // TODO(#531): Infer types based on schema (w/ --type arg to override).
    let item = match toml_edit::Value::from_str(value_str) {
        Ok(value) => toml_edit::value(value),
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
    // Error out if overwriting non-scalar value for key (table or array) with
    // scalar.
    match target_table.get(last_key_part) {
        None | Some(toml_edit::Item::None | toml_edit::Item::Value(_)) => {}
        Some(toml_edit::Item::Table(_) | toml_edit::Item::ArrayOfTables(_)) => {
            return Err(user_error(format!(
                "Failed to set {key}: would overwrite entire table"
            )));
        }
    }
    target_table[last_key_part] = item;

    // Write config back
    std::fs::write(path, doc.to_string()).map_err(|err| {
        user_error_with_message(
            format!("Failed to write file {path}", path = path.display()),
            err,
        )
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
        .map_err(|err| config_error_with_message("Invalid `ui.editor`", err))?;
    let exit_status = editor.to_command().arg(edit_path).status().map_err(|err| {
        user_error_with_message(
            format!(
                // The executable couldn't be found or run; command-line arguments are not relevant
                "Failed to run editor '{name}'",
                name = editor.split_name(),
            ),
            err,
        )
    })?;
    if !exit_status.success() {
        return Err(user_error(format!(
            "Editor '{editor}' exited with an error"
        )));
    }

    Ok(())
}

pub fn edit_temp_file(
    error_name: &str,
    tempfile_suffix: &str,
    dir: &Path,
    content: &str,
    settings: &UserSettings,
) -> Result<String, CommandError> {
    let path = (|| -> Result<_, io::Error> {
        let mut file = tempfile::Builder::new()
            .prefix("editor-")
            .suffix(tempfile_suffix)
            .tempfile_in(dir)?;
        file.write_all(content.as_bytes())?;
        let (_, path) = file.keep().map_err(|e| e.error)?;
        Ok(path)
    })()
    .map_err(|e| {
        user_error_with_message(
            format!(
                r#"Failed to create {} file in "{}""#,
                error_name,
                dir.display(),
            ),
            e,
        )
    })?;

    run_ui_editor(settings, &path)?;

    let edited = fs::read_to_string(&path).map_err(|e| {
        user_error_with_message(
            format!(r#"Failed to read {} file "{}""#, error_name, path.display()),
            e,
        )
    })?;

    // Delete the file only if everything went well.
    // TODO: Tell the user the name of the file we left behind.
    std::fs::remove_file(path).ok();

    Ok(edited)
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

/// Wrapper around a `DiffEditor` to conditionally start interactive session.
#[derive(Clone, Debug)]
pub enum DiffSelector {
    NonInteractive,
    Interactive(DiffEditor),
}

impl DiffSelector {
    pub fn is_interactive(&self) -> bool {
        matches!(self, DiffSelector::Interactive(_))
    }

    /// Restores diffs from the `right_tree` to the `left_tree` by using an
    /// interactive editor if enabled.
    pub fn select(
        &self,
        left_tree: &MergedTree,
        right_tree: &MergedTree,
        matcher: &dyn Matcher,
        instructions: Option<&str>,
    ) -> Result<MergedTreeId, CommandError> {
        match self {
            DiffSelector::NonInteractive => Ok(restore_tree(right_tree, left_tree, matcher)?),
            DiffSelector::Interactive(editor) => {
                Ok(editor.edit(left_tree, right_tree, matcher, instructions)?)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RemoteBranchName {
    pub branch: String,
    pub remote: String,
}

impl fmt::Display for RemoteBranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let RemoteBranchName { branch, remote } = self;
        write!(f, "{branch}@{remote}")
    }
}

#[derive(Clone, Debug)]
pub struct RemoteBranchNamePattern {
    pub branch: StringPattern,
    pub remote: StringPattern,
}

impl FromStr for RemoteBranchNamePattern {
    type Err = String;

    fn from_str(src: &str) -> Result<Self, Self::Err> {
        // The kind prefix applies to both branch and remote fragments. It's
        // weird that unanchored patterns like substring:branch@remote is split
        // into two, but I can't think of a better syntax.
        // TODO: should we disable substring pattern? what if we added regex?
        let (maybe_kind, pat) = src
            .split_once(':')
            .map_or((None, src), |(kind, pat)| (Some(kind), pat));
        let to_pattern = |pat: &str| {
            if let Some(kind) = maybe_kind {
                StringPattern::from_str_kind(pat, kind).map_err(|err| err.to_string())
            } else {
                Ok(StringPattern::exact(pat))
            }
        };
        // TODO: maybe reuse revset parser to handle branch/remote name containing @
        let (branch, remote) = pat
            .rsplit_once('@')
            .ok_or_else(|| "remote branch must be specified in branch@remote form".to_owned())?;
        Ok(RemoteBranchNamePattern {
            branch: to_pattern(branch)?,
            remote: to_pattern(remote)?,
        })
    }
}

impl RemoteBranchNamePattern {
    pub fn is_exact(&self) -> bool {
        self.branch.is_exact() && self.remote.is_exact()
    }
}

impl fmt::Display for RemoteBranchNamePattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let RemoteBranchNamePattern { branch, remote } = self;
        write!(f, "{branch}@{remote}")
    }
}

/// Jujutsu (An experimental VCS)
///
/// To get started, see the tutorial at https://github.com/martinvonz/jj/blob/main/docs/tutorial.md.
#[allow(rustdoc::bare_urls)]
#[derive(clap::Parser, Clone, Debug)]
#[command(name = "jj")]
pub struct Args {
    #[command(flatten)]
    pub global_args: GlobalArgs,
}

#[derive(clap::Args, Clone, Debug)]
#[command(next_help_heading = "Global Options")]
pub struct GlobalArgs {
    /// Path to repository to operate on
    ///
    /// By default, Jujutsu searches for the closest .jj/ directory in an
    /// ancestor of the current working directory.
    #[arg(long, short = 'R', global = true, value_hint = clap::ValueHint::DirPath)]
    pub repository: Option<String>,
    /// Don't snapshot the working copy, and don't update it
    ///
    /// By default, Jujutsu snapshots the working copy at the beginning of every
    /// command. The working copy is also updated at the end of the command,
    /// if the command modified the working-copy commit (`@`). If you want
    /// to avoid snapshotting the working copy and instead see a possibly
    /// stale working copy commit, you can use `--ignore-working-copy`.
    /// This may be useful e.g. in a command prompt, especially if you have
    /// another process that commits the working copy.
    ///
    /// Loading the repository at a specific operation with `--at-operation`
    /// implies `--ignore-working-copy`.
    #[arg(long, global = true)]
    pub ignore_working_copy: bool,
    /// Allow rewriting immutable commits
    ///
    /// By default, Jujutsu prevents rewriting commits in the configured set of
    /// immutable commits. This option disables that check and lets you rewrite
    /// any commit but the root commit.
    ///
    /// This option only affects the check. It does not affect the
    /// `immutable_heads()` revset or the `immutable` template keyword.
    #[arg(long, global = true)]
    pub ignore_immutable: bool,
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
    #[arg(long, visible_alias = "at-op", global = true, default_value = "@")]
    pub at_operation: String,
    /// Enable debug logging
    #[arg(long, global = true)]
    pub debug: bool,

    #[command(flatten)]
    pub early_args: EarlyArgs,
}

#[derive(clap::Args, Clone, Debug)]
pub struct EarlyArgs {
    /// When to colorize output (always, never, auto)
    #[arg(long, value_name = "WHEN", global = true)]
    pub color: Option<ColorChoice>,
    /// Silence non-primary command output
    ///
    /// For example, `jj files` will still list files, but it won't tell you if
    /// the working copy was snapshotted or if descendants were rebased.
    ///
    /// Warnings and errors will still be printed.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    // Parsing with ignore_errors will crash if this is bool, so use
    // Option<bool>.
    pub quiet: Option<bool>,
    /// Disable the pager
    #[arg(long, value_name = "WHEN", global = true, action = ArgAction::SetTrue)]
    // Parsing with ignore_errors will crash if this is bool, so use
    // Option<bool>.
    pub no_pager: Option<bool>,
    /// Additional configuration options (can be repeated)
    //  TODO: Introduce a `--config` option with simpler syntax for simple
    //  cases, designed so that `--config ui.color=auto` works
    #[arg(long, value_name = "TOML", global = true)]
    pub config_toml: Vec<String>,
}

/// Wrapper around revset expression argument.
///
/// An empty string is rejected early by the CLI value parser, but it's still
/// allowed to construct an empty `RevisionArg` from a config value for
/// example. An empty expression will be rejected by the revset parser.
#[derive(Clone, Debug)]
pub struct RevisionArg(Cow<'static, str>);

impl RevisionArg {
    /// The working-copy symbol, which is the default of the most commands.
    pub const AT: Self = RevisionArg(Cow::Borrowed("@"));
}

impl From<String> for RevisionArg {
    fn from(s: String) -> Self {
        RevisionArg(s.into())
    }
}

impl AsRef<str> for RevisionArg {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RevisionArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ValueParserFactory for RevisionArg {
    type Parser = MapValueParser<NonEmptyStringValueParser, fn(String) -> RevisionArg>;

    fn value_parser() -> Self::Parser {
        NonEmptyStringValueParser::new().map(RevisionArg::from)
    }
}

fn get_string_or_array(
    config: &config::Config,
    key: &str,
) -> Result<Vec<String>, config::ConfigError> {
    config
        .get(key)
        .map(|string| vec![string])
        .or_else(|_| config.get::<Vec<String>>(key))
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
            let args = get_string_or_array(config, "ui.default-command").optional()?;
            if args.is_none() {
                if let Some(mut writer) = ui.hint_default() {
                    writeln!(writer, "Use `jj -h` for a list of available commands.")?;
                    writeln!(
                        writer,
                        "Run `jj config set --user ui.default-command log` to disable this \
                         message."
                    )?;
                }
            }
            let default_command = args.unwrap_or_else(|| vec!["log".to_string()]);

            // Insert the default command directly after the path to the binary.
            string_args.splice(1..1, default_command);
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
    if args.quiet.unwrap_or_default() {
        args.config_toml.push(r#"ui.quiet=true"#.to_string());
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
            return Err(cli_error("Non-utf8 argument"));
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
    let matches = app
        .clone()
        .arg_required_else_help(true)
        .subcommand_required(true)
        .try_get_matches_from(string_args)?;

    let args: Args = Args::from_arg_matches(&matches).unwrap();
    if args.global_args.debug {
        // TODO: set up debug logging as early as possible
        tracing_subscription.enable_debug_logging()?;
    }

    Ok((matches, args))
}

pub fn format_template<C: Clone>(ui: &Ui, arg: &C, template: &TemplateRenderer<C>) -> String {
    let mut output = vec![];
    template
        .format(arg, ui.new_formatter(&mut output).as_mut())
        .expect("write() to vec backed formatter should never fail");
    String::from_utf8(output).expect("template output should be utf-8 bytes")
}

/// CLI command builder and runner.
#[must_use]
pub struct CliRunner {
    tracing_subscription: TracingSubscription,
    app: Command,
    extra_configs: Vec<config::Config>,
    store_factories: StoreFactories,
    working_copy_factories: WorkingCopyFactories,
    revset_extensions: RevsetExtensions,
    commit_template_extensions: Vec<Arc<dyn CommitTemplateLanguageExtension>>,
    operation_template_extensions: Vec<Arc<dyn OperationTemplateLanguageExtension>>,
    dispatch_fn: CliDispatchFn,
    start_hook_fns: Vec<CliDispatchFn>,
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
            extra_configs: vec![],
            store_factories: StoreFactories::default(),
            working_copy_factories: default_working_copy_factories(),
            revset_extensions: Default::default(),
            commit_template_extensions: vec![],
            operation_template_extensions: vec![],
            dispatch_fn: Box::new(crate::commands::run_command),
            start_hook_fns: vec![],
            process_global_args_fns: vec![],
        }
    }

    /// Set the version to be displayed by `jj version`.
    pub fn version(mut self, version: &str) -> Self {
        self.app = self.app.version(version.to_string());
        self
    }

    /// Adds default configs in addition to the normal defaults.
    pub fn add_extra_config(mut self, extra_configs: config::Config) -> Self {
        self.extra_configs.push(extra_configs);
        self
    }

    /// Adds `StoreFactories` to be used.
    pub fn add_store_factories(mut self, store_factories: StoreFactories) -> Self {
        self.store_factories.merge(store_factories);
        self
    }

    /// Adds working copy factories to be used.
    pub fn add_working_copy_factories(
        mut self,
        working_copy_factories: WorkingCopyFactories,
    ) -> Self {
        merge_factories_map(&mut self.working_copy_factories, working_copy_factories);
        self
    }

    pub fn add_symbol_resolver_extension(
        mut self,
        symbol_resolver: Box<dyn SymbolResolverExtension>,
    ) -> Self {
        self.revset_extensions.add_symbol_resolver(symbol_resolver);
        self
    }

    pub fn add_revset_function_extension(
        mut self,
        name: &'static str,
        func: RevsetFunction,
    ) -> Self {
        self.revset_extensions.add_custom_function(name, func);
        self
    }

    pub fn add_commit_template_extension(
        mut self,
        commit_template_extension: Box<dyn CommitTemplateLanguageExtension>,
    ) -> Self {
        self.commit_template_extensions
            .push(commit_template_extension.into());
        self
    }

    pub fn add_operation_template_extension(
        mut self,
        operation_template_extension: Box<dyn OperationTemplateLanguageExtension>,
    ) -> Self {
        self.operation_template_extensions
            .push(operation_template_extension.into());
        self
    }

    pub fn add_start_hook(mut self, start_hook_fn: CliDispatchFn) -> Self {
        self.start_hook_fns.push(start_hook_fn);
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
        // `cwd` is canonicalized for consistency with `Workspace::workspace_root()` and
        // to easily compute relative paths between them.
        let cwd = env::current_dir()
            .and_then(|cwd| cwd.canonicalize())
            .map_err(|_| {
                user_error_with_hint(
                    "Could not determine current directory",
                    "Did you update to a commit where the directory doesn't exist?",
                )
            })?;
        // Use cwd-relative workspace configs to resolve default command and
        // aliases. WorkspaceLoader::init() won't do any heavy lifting other
        // than the path resolution.
        let maybe_cwd_workspace_loader = WorkspaceLoader::init(find_workspace_dir(&cwd))
            .map_err(|err| map_workspace_load_error(err, None));
        layered_configs.read_user_config()?;
        if let Ok(loader) = &maybe_cwd_workspace_loader {
            layered_configs.read_repo_config(loader.repo_path())?;
        }
        let config = layered_configs.merge();
        ui.reset(&config)?;

        let string_args = expand_args(ui, &self.app, env::args_os(), &config)?;
        let (matches, args) = parse_args(
            ui,
            &self.app,
            &self.tracing_subscription,
            &string_args,
            &mut layered_configs,
        )
        .map_err(|err| map_clap_cli_error(err, ui, &layered_configs))?;
        for process_global_args_fn in self.process_global_args_fns {
            process_global_args_fn(ui, &matches)?;
        }

        let maybe_workspace_loader = if let Some(path) = &args.global_args.repository {
            // Invalid -R path is an error. No need to proceed.
            let loader = WorkspaceLoader::init(&cwd.join(path))
                .map_err(|err| map_workspace_load_error(err, Some(path)))?;
            layered_configs.read_repo_config(loader.repo_path())?;
            Ok(loader)
        } else {
            maybe_cwd_workspace_loader
        };

        // Apply workspace configs and --config-toml arguments.
        let config = layered_configs.merge();
        ui.reset(&config)?;

        // If -R is specified, check if the expanded arguments differ. Aliases
        // can also be injected by --config-toml, but that's obviously wrong.
        if args.global_args.repository.is_some() {
            let new_string_args = expand_args(ui, &self.app, env::args_os(), &config).ok();
            if new_string_args.as_ref() != Some(&string_args) {
                writeln!(
                    ui.warning_default(),
                    "Command aliases cannot be loaded from -R/--repository path"
                )?;
            }
        }

        let settings = UserSettings::from_config(config);
        let command_helper = CommandHelper {
            app: self.app,
            cwd,
            string_args,
            matches,
            global_args: args.global_args,
            settings,
            layered_configs,
            revset_extensions: self.revset_extensions.into(),
            commit_template_extensions: self.commit_template_extensions,
            operation_template_extensions: self.operation_template_extensions,
            maybe_workspace_loader,
            store_factories: self.store_factories,
            working_copy_factories: self.working_copy_factories,
        };
        for start_hook_fn in self.start_hook_fns {
            start_hook_fn(ui, &command_helper)?;
        }
        (self.dispatch_fn)(ui, &command_helper)
    }

    #[must_use]
    #[instrument(skip(self))]
    pub fn run(mut self) -> ExitCode {
        let builder = config::Config::builder().add_source(crate::config::default_config());
        let config = self
            .extra_configs
            .drain(..)
            .fold(builder, |builder, config| builder.add_source(config))
            .build()
            .unwrap();
        let layered_configs = LayeredConfigs::from_environment(config);
        let mut ui = Ui::with_config(&layered_configs.merge())
            .expect("default config should be valid, env vars are stringly typed");
        let result = self.run_internal(&mut ui, layered_configs);
        let exit_code = handle_command_result(&mut ui, result);
        ui.finalize_pager();
        exit_code
    }
}

fn map_clap_cli_error(
    mut cmd_err: CommandError,
    ui: &Ui,
    layered_configs: &LayeredConfigs,
) -> CommandError {
    let Some(err) = cmd_err.error.downcast_ref::<clap::Error>() else {
        return cmd_err;
    };
    if let (Some(ContextValue::String(arg)), Some(ContextValue::String(value))) = (
        err.get(ContextKind::InvalidArg),
        err.get(ContextKind::InvalidValue),
    ) {
        if arg.as_str() == "--template <TEMPLATE>" && value.is_empty() {
            // Suppress the error, it's less important than the original error.
            if let Ok(template_aliases) = load_template_aliases(ui, layered_configs) {
                cmd_err.add_hint(format_template_aliases_hint(&template_aliases));
            }
        }
    }
    cmd_err
}

fn format_template_aliases_hint(template_aliases: &TemplateAliasesMap) -> String {
    let mut hint = String::from("The following template aliases are defined:\n");
    hint.push_str(
        &template_aliases
            .symbol_names()
            .sorted_unstable()
            .map(|name| format!("- {name}"))
            .join("\n"),
    );
    hint
}
