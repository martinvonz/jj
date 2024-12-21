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

use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::env;
use std::env::VarError;
use std::ffi::OsString;
use std::fmt;
use std::fmt::Debug;
use std::fs;
use std::io;
use std::io::Write as _;
use std::iter;
use std::mem;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::str;
use std::str::FromStr;
use std::sync::Arc;
use std::time::SystemTime;

use bstr::ByteVec as _;
use chrono::TimeZone;
use clap::builder::MapValueParser;
use clap::builder::NonEmptyStringValueParser;
use clap::builder::TypedValueParser;
use clap::builder::ValueParserFactory;
use clap::error::ContextKind;
use clap::error::ContextValue;
use clap::ArgAction;
use clap::ArgMatches;
use clap::Command;
use clap::FromArgMatches;
use clap_complete::ArgValueCandidates;
use indexmap::IndexMap;
use indexmap::IndexSet;
use indoc::writedoc;
use itertools::Itertools;
use jj_lib::backend::BackendResult;
use jj_lib::backend::ChangeId;
use jj_lib::backend::CommitId;
use jj_lib::backend::MergedTreeId;
use jj_lib::backend::TreeValue;
use jj_lib::commit::Commit;
use jj_lib::config::ConfigGetError;
use jj_lib::config::ConfigGetResultExt as _;
use jj_lib::config::ConfigLayer;
use jj_lib::config::ConfigNamePathBuf;
use jj_lib::config::ConfigSource;
use jj_lib::config::StackedConfig;
use jj_lib::conflicts::ConflictMarkerStyle;
use jj_lib::file_util;
use jj_lib::fileset;
use jj_lib::fileset::FilesetDiagnostics;
use jj_lib::fileset::FilesetExpression;
use jj_lib::git;
use jj_lib::git_backend::GitBackend;
use jj_lib::gitignore::GitIgnoreError;
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::id_prefix::IdPrefixContext;
use jj_lib::matchers::Matcher;
use jj_lib::merge::MergedTreeValue;
use jj_lib::merged_tree::MergedTree;
use jj_lib::object_id::ObjectId;
use jj_lib::op_heads_store;
use jj_lib::op_store::OpStoreError;
use jj_lib::op_store::OperationId;
use jj_lib::op_store::RefTarget;
use jj_lib::op_store::WorkspaceId;
use jj_lib::op_walk;
use jj_lib::op_walk::OpsetEvaluationError;
use jj_lib::operation::Operation;
use jj_lib::repo::merge_factories_map;
use jj_lib::repo::CheckOutCommitError;
use jj_lib::repo::EditCommitError;
use jj_lib::repo::MutableRepo;
use jj_lib::repo::ReadonlyRepo;
use jj_lib::repo::Repo;
use jj_lib::repo::RepoLoader;
use jj_lib::repo::StoreFactories;
use jj_lib::repo::StoreLoadError;
use jj_lib::repo_path::RepoPath;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::repo_path::RepoPathUiConverter;
use jj_lib::repo_path::UiPathParseError;
use jj_lib::revset;
use jj_lib::revset::ResolvedRevsetExpression;
use jj_lib::revset::RevsetAliasesMap;
use jj_lib::revset::RevsetDiagnostics;
use jj_lib::revset::RevsetExpression;
use jj_lib::revset::RevsetExtensions;
use jj_lib::revset::RevsetFilterPredicate;
use jj_lib::revset::RevsetFunction;
use jj_lib::revset::RevsetIteratorExt;
use jj_lib::revset::RevsetModifier;
use jj_lib::revset::RevsetParseContext;
use jj_lib::revset::RevsetWorkspaceContext;
use jj_lib::revset::SymbolResolverExtension;
use jj_lib::revset::UserRevsetExpression;
use jj_lib::rewrite::restore_tree;
use jj_lib::settings::HumanByteSize;
use jj_lib::settings::UserSettings;
use jj_lib::str_util::StringPattern;
use jj_lib::transaction::Transaction;
use jj_lib::view::View;
use jj_lib::working_copy;
use jj_lib::working_copy::CheckoutOptions;
use jj_lib::working_copy::CheckoutStats;
use jj_lib::working_copy::SnapshotOptions;
use jj_lib::working_copy::SnapshotStats;
use jj_lib::working_copy::UntrackedReason;
use jj_lib::working_copy::WorkingCopy;
use jj_lib::working_copy::WorkingCopyFactory;
use jj_lib::working_copy::WorkingCopyFreshness;
use jj_lib::workspace::default_working_copy_factories;
use jj_lib::workspace::get_working_copy_factory;
use jj_lib::workspace::DefaultWorkspaceLoaderFactory;
use jj_lib::workspace::LockedWorkspace;
use jj_lib::workspace::WorkingCopyFactories;
use jj_lib::workspace::Workspace;
use jj_lib::workspace::WorkspaceLoadError;
use jj_lib::workspace::WorkspaceLoader;
use jj_lib::workspace::WorkspaceLoaderFactory;
use tracing::instrument;
use tracing_chrome::ChromeLayerBuilder;
use tracing_subscriber::prelude::*;

use crate::command_error::cli_error;
use crate::command_error::config_error_with_message;
use crate::command_error::handle_command_result;
use crate::command_error::internal_error;
use crate::command_error::internal_error_with_message;
use crate::command_error::print_parse_diagnostics;
use crate::command_error::user_error;
use crate::command_error::user_error_with_hint;
use crate::command_error::user_error_with_message;
use crate::command_error::CommandError;
use crate::commit_templater::CommitTemplateLanguage;
use crate::commit_templater::CommitTemplateLanguageExtension;
use crate::complete;
use crate::config::config_from_environment;
use crate::config::parse_config_args;
use crate::config::CommandNameAndArgs;
use crate::config::ConfigArgKind;
use crate::config::ConfigEnv;
use crate::config::RawConfig;
use crate::diff_util;
use crate::diff_util::DiffFormat;
use crate::diff_util::DiffFormatArgs;
use crate::diff_util::DiffRenderer;
use crate::formatter::FormatRecorder;
use crate::formatter::Formatter;
use crate::formatter::PlainTextFormatter;
use crate::git_util::is_colocated_git_workspace;
use crate::git_util::print_failed_git_export;
use crate::git_util::print_git_import_stats;
use crate::merge_tools::DiffEditor;
use crate::merge_tools::MergeEditor;
use crate::merge_tools::MergeToolConfigError;
use crate::operation_templater::OperationTemplateLanguage;
use crate::operation_templater::OperationTemplateLanguageExtension;
use crate::revset_util;
use crate::revset_util::RevsetExpressionEvaluator;
use crate::template_builder;
use crate::template_builder::TemplateLanguage;
use crate::template_parser::TemplateAliasesMap;
use crate::template_parser::TemplateDiagnostics;
use crate::templater::PropertyPlaceholder;
use crate::templater::TemplateRenderer;
use crate::text_util;
use crate::ui::ColorChoice;
use crate::ui::Ui;

const SHORT_CHANGE_ID_TEMPLATE_TEXT: &str = "format_short_change_id(self.change_id())";

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
    const ENV_VAR_NAME: &'static str = "JJ_LOG";

    /// Initializes tracing with the default configuration. This should be
    /// called as early as possible.
    pub fn init() -> Self {
        let filter = tracing_subscriber::EnvFilter::builder()
            .with_default_directive(tracing::metadata::LevelFilter::ERROR.into())
            .with_env_var(Self::ENV_VAR_NAME)
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
                    .with_env_var(Self::ENV_VAR_NAME)
                    .from_env_lossy();
            })
            .map_err(|err| internal_error_with_message("failed to enable debug logging", err))?;
        tracing::info!("debug logging enabled");
        Ok(())
    }
}

#[derive(Clone)]
pub struct CommandHelper {
    data: Rc<CommandHelperData>,
}

struct CommandHelperData {
    app: Command,
    cwd: PathBuf,
    string_args: Vec<String>,
    matches: ArgMatches,
    global_args: GlobalArgs,
    config_env: ConfigEnv,
    raw_config: RawConfig,
    settings: UserSettings,
    revset_extensions: Arc<RevsetExtensions>,
    commit_template_extensions: Vec<Arc<dyn CommitTemplateLanguageExtension>>,
    operation_template_extensions: Vec<Arc<dyn OperationTemplateLanguageExtension>>,
    maybe_workspace_loader: Result<Box<dyn WorkspaceLoader>, CommandError>,
    store_factories: StoreFactories,
    working_copy_factories: WorkingCopyFactories,
}

impl CommandHelper {
    pub fn app(&self) -> &Command {
        &self.data.app
    }

    /// Canonical form of the current working directory path.
    ///
    /// A loaded `Workspace::workspace_root()` also returns a canonical path, so
    /// relative paths can be easily computed from these paths.
    pub fn cwd(&self) -> &Path {
        &self.data.cwd
    }

    pub fn string_args(&self) -> &Vec<String> {
        &self.data.string_args
    }

    pub fn matches(&self) -> &ArgMatches {
        &self.data.matches
    }

    pub fn global_args(&self) -> &GlobalArgs {
        &self.data.global_args
    }

    pub fn config_env(&self) -> &ConfigEnv {
        &self.data.config_env
    }

    /// Unprocessed (or unresolved) configuration data.
    ///
    /// Use this only if the unmodified config data is needed. For example, `jj
    /// config set` should use this to write updated data back to file.
    pub fn raw_config(&self) -> &RawConfig {
        &self.data.raw_config
    }

    pub fn settings(&self) -> &UserSettings {
        &self.data.settings
    }

    pub fn revset_extensions(&self) -> &Arc<RevsetExtensions> {
        &self.data.revset_extensions
    }

    /// Loads template aliases from the configs.
    ///
    /// For most commands that depend on a loaded repo, you should use
    /// `WorkspaceCommandHelper::template_aliases_map()` instead.
    fn load_template_aliases(&self, ui: &Ui) -> Result<TemplateAliasesMap, CommandError> {
        load_template_aliases(ui, self.settings().config())
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
        let mut diagnostics = TemplateDiagnostics::new();
        let aliases = self.load_template_aliases(ui)?;
        let template = template_builder::parse(
            language,
            &mut diagnostics,
            template_text,
            &aliases,
            wrap_self,
        )?;
        print_parse_diagnostics(ui, "In template expression", &diagnostics)?;
        Ok(template)
    }

    pub fn workspace_loader(&self) -> Result<&dyn WorkspaceLoader, CommandError> {
        self.data
            .maybe_workspace_loader
            .as_deref()
            .map_err(Clone::clone)
    }

    /// Loads workspace and repo, then snapshots the working copy if allowed.
    #[instrument(skip(self, ui))]
    pub fn workspace_helper(&self, ui: &Ui) -> Result<WorkspaceCommandHelper, CommandError> {
        let mut workspace_command = self.workspace_helper_no_snapshot(ui)?;

        let workspace_command = match workspace_command.maybe_snapshot_impl(ui) {
            Ok(()) => workspace_command,
            Err(SnapshotWorkingCopyError::Command(err)) => return Err(err),
            Err(SnapshotWorkingCopyError::StaleWorkingCopy(err)) => {
                let auto_update_stale = self.settings().get_bool("snapshot.auto-update-stale")?;
                if !auto_update_stale {
                    return Err(err);
                }

                // We detected the working copy was stale and the client is configured to
                // auto-update-stale, so let's do that now. We need to do it up here, not at a
                // lower level (e.g. inside snapshot_working_copy()) to avoid recursive locking
                // of the working copy.
                self.recover_stale_working_copy(ui)?
            }
        };

        Ok(workspace_command)
    }

    /// Loads workspace and repo, but never snapshots the working copy. Most
    /// commands should use `workspace_helper()` instead.
    #[instrument(skip(self, ui))]
    pub fn workspace_helper_no_snapshot(
        &self,
        ui: &Ui,
    ) -> Result<WorkspaceCommandHelper, CommandError> {
        let workspace = self.load_workspace()?;
        let op_head = self.resolve_operation(ui, workspace.repo_loader())?;
        let repo = workspace.repo_loader().load_at(&op_head)?;
        let env = self.workspace_environment(ui, &workspace)?;
        revset_util::warn_unresolvable_trunk(ui, repo.as_ref(), &env.revset_parse_context())?;
        WorkspaceCommandHelper::new(ui, workspace, repo, env, self.is_at_head_operation())
    }

    pub fn get_working_copy_factory(&self) -> Result<&dyn WorkingCopyFactory, CommandError> {
        let loader = self.workspace_loader()?;

        // We convert StoreLoadError -> WorkspaceLoadError -> CommandError
        let factory: Result<_, WorkspaceLoadError> =
            get_working_copy_factory(loader, &self.data.working_copy_factories)
                .map_err(|e| e.into());
        let factory = factory.map_err(|err| {
            map_workspace_load_error(err, self.data.global_args.repository.as_deref())
        })?;
        Ok(factory)
    }

    #[instrument(skip_all)]
    pub fn load_workspace(&self) -> Result<Workspace, CommandError> {
        let loader = self.workspace_loader()?;
        loader
            .load(
                &self.data.settings,
                &self.data.store_factories,
                &self.data.working_copy_factories,
            )
            .map_err(|err| {
                map_workspace_load_error(err, self.data.global_args.repository.as_deref())
            })
    }

    pub fn recover_stale_working_copy(
        &self,
        ui: &Ui,
    ) -> Result<WorkspaceCommandHelper, CommandError> {
        let workspace = self.load_workspace()?;
        let op_id = workspace.working_copy().operation_id();

        match workspace.repo_loader().load_operation(op_id) {
            Ok(op) => {
                let repo = workspace.repo_loader().load_at(&op)?;
                let mut workspace_command = self.for_workable_repo(ui, workspace, repo)?;

                // Snapshot the current working copy on top of the last known working-copy
                // operation, then merge the divergent operations. The wc_commit_id of the
                // merged repo wouldn't change because the old one wins, but it's probably
                // fine if we picked the new wc_commit_id.
                workspace_command.maybe_snapshot(ui)?;

                let wc_commit_id = workspace_command.get_wc_commit_id().unwrap();
                let repo = workspace_command.repo().clone();
                let stale_wc_commit = repo.store().get_commit(wc_commit_id)?;

                let mut workspace_command = self.workspace_helper_no_snapshot(ui)?;
                let checkout_options = workspace_command.checkout_options();

                let repo = workspace_command.repo().clone();
                let (mut locked_ws, desired_wc_commit) =
                    workspace_command.unchecked_start_working_copy_mutation()?;
                match WorkingCopyFreshness::check_stale(
                    locked_ws.locked_wc(),
                    &desired_wc_commit,
                    &repo,
                )? {
                    WorkingCopyFreshness::Fresh | WorkingCopyFreshness::Updated(_) => {
                        writeln!(
                            ui.status(),
                            "Attempted recovery, but the working copy is not stale"
                        )?;
                    }
                    WorkingCopyFreshness::WorkingCopyStale
                    | WorkingCopyFreshness::SiblingOperation => {
                        let stats = update_stale_working_copy(
                            locked_ws,
                            repo.op_id().clone(),
                            &stale_wc_commit,
                            &desired_wc_commit,
                            &checkout_options,
                        )?;

                        // TODO: Share this code with new/checkout somehow.
                        if let Some(mut formatter) = ui.status_formatter() {
                            write!(formatter, "Working copy now at: ")?;
                            formatter.with_label("working_copy", |fmt| {
                                workspace_command.write_commit_summary(fmt, &desired_wc_commit)
                            })?;
                            writeln!(formatter)?;
                        }
                        print_checkout_stats(ui, stats, &desired_wc_commit)?;

                        writeln!(
                            ui.status(),
                            "Updated working copy to fresh commit {}",
                            short_commit_hash(desired_wc_commit.id())
                        )?;
                    }
                };

                Ok(workspace_command)
            }
            Err(e @ OpStoreError::ObjectNotFound { .. }) => {
                writeln!(
                    ui.status(),
                    "Failed to read working copy's current operation; attempting recovery. Error \
                     message from read attempt: {e}"
                )?;

                let mut workspace_command = self.workspace_helper_no_snapshot(ui)?;
                workspace_command.create_and_check_out_recovery_commit(ui)?;
                Ok(workspace_command)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Loads command environment for the given `workspace`.
    pub fn workspace_environment(
        &self,
        ui: &Ui,
        workspace: &Workspace,
    ) -> Result<WorkspaceCommandEnvironment, CommandError> {
        WorkspaceCommandEnvironment::new(ui, self, workspace)
    }

    /// Returns true if the working copy to be loaded is writable, and therefore
    /// should usually be snapshotted.
    pub fn is_working_copy_writable(&self) -> bool {
        self.is_at_head_operation() && !self.data.global_args.ignore_working_copy
    }

    /// Returns true if the current operation is considered to be the head.
    pub fn is_at_head_operation(&self) -> bool {
        // TODO: should we accept --at-op=<head_id> as the head op? or should we
        // make --at-op=@ imply --ignore-working-copy (i.e. not at the head.)
        matches!(
            self.data.global_args.at_operation.as_deref(),
            None | Some("@")
        )
    }

    /// Resolves the current operation from the command-line argument.
    ///
    /// If no `--at-operation` is specified, the head operations will be
    /// loaded. If there are multiple heads, they'll be merged.
    #[instrument(skip_all)]
    pub fn resolve_operation(
        &self,
        ui: &Ui,
        repo_loader: &RepoLoader,
    ) -> Result<Operation, CommandError> {
        if let Some(op_str) = &self.data.global_args.at_operation {
            Ok(op_walk::resolve_op_for_load(repo_loader, op_str)?)
        } else {
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
                    let mut tx = start_repo_transaction(
                        &base_repo,
                        &self.data.settings,
                        &self.data.string_args,
                    );
                    for other_op_head in op_heads.into_iter().skip(1) {
                        tx.merge_operation(other_op_head)?;
                        let num_rebased = tx.repo_mut().rebase_descendants(&self.data.settings)?;
                        if num_rebased > 0 {
                            writeln!(
                                ui.status(),
                                "Rebased {num_rebased} descendant commits onto commits rewritten \
                                 by other operation"
                            )?;
                        }
                    }
                    Ok(tx
                        .write("reconcile divergent operations")
                        .leave_unpublished()
                        .operation()
                        .clone())
                },
            )
        }
    }

    /// Creates helper for the repo whose view is supposed to be in sync with
    /// the working copy. If `--ignore-working-copy` is not specified, the
    /// returned helper will attempt to update the working copy.
    #[instrument(skip_all)]
    pub fn for_workable_repo(
        &self,
        ui: &Ui,
        workspace: Workspace,
        repo: Arc<ReadonlyRepo>,
    ) -> Result<WorkspaceCommandHelper, CommandError> {
        let env = self.workspace_environment(ui, &workspace)?;
        let loaded_at_head = true;
        WorkspaceCommandHelper::new(ui, workspace, repo, env, loaded_at_head)
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

/// A advanceable bookmark to satisfy the "advance-bookmarks" feature.
///
/// This is a helper for `WorkspaceCommandTransaction`. It provides a
/// type-safe way to separate the work of checking whether a bookmark
/// can be advanced and actually advancing it. Advancing the bookmark
/// never fails, but can't be done until the new `CommitId` is
/// available. Splitting the work in this way also allows us to
/// identify eligible bookmarks without actually moving them and
/// return config errors to the user early.
pub struct AdvanceableBookmark {
    name: String,
    old_commit_id: CommitId,
}

/// Helper for parsing and evaluating settings for the advance-bookmarks
/// feature. Settings are configured in the jj config.toml as lists of
/// [`StringPattern`]s for enabled and disabled bookmarks. Example:
/// ```toml
/// [experimental-advance-branches]
/// # Enable the feature for all branches except "main".
/// enabled-branches = ["glob:*"]
/// disabled-branches = ["main"]
/// ```
struct AdvanceBookmarksSettings {
    enabled_bookmarks: Vec<StringPattern>,
    disabled_bookmarks: Vec<StringPattern>,
}

impl AdvanceBookmarksSettings {
    fn from_settings(settings: &UserSettings) -> Result<Self, CommandError> {
        let get_setting = |setting_key| {
            let name = ConfigNamePathBuf::from_iter(["experimental-advance-branches", setting_key]);
            match settings.get::<Vec<String>>(&name).optional()? {
                Some(patterns) => patterns
                    .into_iter()
                    .map(|s| {
                        StringPattern::parse(&s).map_err(|e| {
                            config_error_with_message(format!("Error parsing '{s}' for {name}"), e)
                        })
                    })
                    .collect(),
                None => Ok(Vec::new()),
            }
        };
        Ok(Self {
            enabled_bookmarks: get_setting("enabled-branches")?,
            disabled_bookmarks: get_setting("disabled-branches")?,
        })
    }

    /// Returns true if the advance-bookmarks feature is enabled for
    /// `bookmark_name`.
    fn bookmark_is_eligible(&self, bookmark_name: &str) -> bool {
        if self
            .disabled_bookmarks
            .iter()
            .any(|d| d.matches(bookmark_name))
        {
            return false;
        }
        self.enabled_bookmarks
            .iter()
            .any(|e| e.matches(bookmark_name))
    }

    /// Returns true if the config includes at least one "enabled-branches"
    /// pattern.
    fn feature_enabled(&self) -> bool {
        !self.enabled_bookmarks.is_empty()
    }
}

/// Metadata and configuration loaded for a specific workspace.
pub struct WorkspaceCommandEnvironment {
    command: CommandHelper,
    revset_aliases_map: RevsetAliasesMap,
    template_aliases_map: TemplateAliasesMap,
    path_converter: RepoPathUiConverter,
    workspace_id: WorkspaceId,
    immutable_heads_expression: Rc<UserRevsetExpression>,
    short_prefixes_expression: Option<Rc<UserRevsetExpression>>,
    conflict_marker_style: ConflictMarkerStyle,
}

impl WorkspaceCommandEnvironment {
    #[instrument(skip_all)]
    fn new(ui: &Ui, command: &CommandHelper, workspace: &Workspace) -> Result<Self, CommandError> {
        let revset_aliases_map = revset_util::load_revset_aliases(ui, command.settings().config())?;
        let template_aliases_map = command.load_template_aliases(ui)?;
        let path_converter = RepoPathUiConverter::Fs {
            cwd: command.cwd().to_owned(),
            base: workspace.workspace_root().to_owned(),
        };
        let mut env = Self {
            command: command.clone(),
            revset_aliases_map,
            template_aliases_map,
            path_converter,
            workspace_id: workspace.workspace_id().to_owned(),
            immutable_heads_expression: RevsetExpression::root(),
            short_prefixes_expression: None,
            conflict_marker_style: command.settings().get("ui.conflict-marker-style")?,
        };
        env.immutable_heads_expression = env.load_immutable_heads_expression(ui)?;
        env.short_prefixes_expression = env.load_short_prefixes_expression(ui)?;
        Ok(env)
    }

    pub fn settings(&self) -> &UserSettings {
        self.command.settings()
    }

    pub(crate) fn path_converter(&self) -> &RepoPathUiConverter {
        &self.path_converter
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.workspace_id
    }

    pub(crate) fn revset_parse_context(&self) -> RevsetParseContext {
        let workspace_context = RevsetWorkspaceContext {
            path_converter: &self.path_converter,
            workspace_id: &self.workspace_id,
        };
        let now = if let Some(timestamp) = self.settings().commit_timestamp() {
            chrono::Local
                .timestamp_millis_opt(timestamp.timestamp.0)
                .unwrap()
        } else {
            chrono::Local::now()
        };
        RevsetParseContext::new(
            &self.revset_aliases_map,
            self.settings().user_email(),
            now.into(),
            self.command.revset_extensions(),
            Some(workspace_context),
        )
    }

    /// Creates fresh new context which manages cache of short commit/change ID
    /// prefixes. New context should be created per repo view (or operation.)
    pub fn new_id_prefix_context(&self) -> IdPrefixContext {
        let context = IdPrefixContext::new(self.command.revset_extensions().clone());
        match &self.short_prefixes_expression {
            None => context,
            Some(expression) => context.disambiguate_within(expression.clone()),
        }
    }

    /// User-configured expression defining the immutable set.
    pub fn immutable_expression(&self) -> Rc<UserRevsetExpression> {
        // Negated ancestors expression `~::(<heads> | root())` is slightly
        // easier to optimize than negated union `~(::<heads> | root())`.
        self.immutable_heads_expression.ancestors()
    }

    /// User-configured expression defining the heads of the immutable set.
    pub fn immutable_heads_expression(&self) -> &Rc<UserRevsetExpression> {
        &self.immutable_heads_expression
    }

    /// User-configured conflict marker style for materializing conflicts
    pub fn conflict_marker_style(&self) -> ConflictMarkerStyle {
        self.conflict_marker_style
    }

    fn load_immutable_heads_expression(
        &self,
        ui: &Ui,
    ) -> Result<Rc<UserRevsetExpression>, CommandError> {
        let mut diagnostics = RevsetDiagnostics::new();
        let expression = revset_util::parse_immutable_heads_expression(
            &mut diagnostics,
            &self.revset_parse_context(),
        )
        .map_err(|e| config_error_with_message("Invalid `revset-aliases.immutable_heads()`", e))?;
        print_parse_diagnostics(ui, "In `revset-aliases.immutable_heads()`", &diagnostics)?;
        Ok(expression)
    }

    fn load_short_prefixes_expression(
        &self,
        ui: &Ui,
    ) -> Result<Option<Rc<UserRevsetExpression>>, CommandError> {
        let revset_string = self
            .settings()
            .get_string("revsets.short-prefixes")
            .optional()?
            .map_or_else(|| self.settings().get_string("revsets.log"), Ok)?;
        if revset_string.is_empty() {
            Ok(None)
        } else {
            let mut diagnostics = RevsetDiagnostics::new();
            let (expression, modifier) = revset::parse_with_modifier(
                &mut diagnostics,
                &revset_string,
                &self.revset_parse_context(),
            )
            .map_err(|err| config_error_with_message("Invalid `revsets.short-prefixes`", err))?;
            print_parse_diagnostics(ui, "In `revsets.short-prefixes`", &diagnostics)?;
            let (None | Some(RevsetModifier::All)) = modifier;
            Ok(Some(expression))
        }
    }

    fn find_immutable_commit<'a>(
        &self,
        repo: &dyn Repo,
        commits: impl IntoIterator<Item = &'a CommitId>,
    ) -> Result<Option<CommitId>, CommandError> {
        if self.command.global_args().ignore_immutable {
            let root_id = repo.store().root_commit_id();
            return Ok(commits.into_iter().find(|id| *id == root_id).cloned());
        }

        // Not using self.id_prefix_context() because the disambiguation data
        // must not be calculated and cached against arbitrary repo. It's also
        // unlikely that the immutable expression contains short hashes.
        let id_prefix_context = IdPrefixContext::new(self.command.revset_extensions().clone());
        let to_rewrite_revset =
            RevsetExpression::commits(commits.into_iter().cloned().collect_vec());
        let mut expression = RevsetExpressionEvaluator::new(
            repo,
            self.command.revset_extensions().clone(),
            &id_prefix_context,
            self.immutable_expression(),
        );
        expression.intersect_with(&to_rewrite_revset);

        let mut commit_id_iter = expression.evaluate_to_commit_ids().map_err(|e| {
            config_error_with_message("Invalid `revset-aliases.immutable_heads()`", e)
        })?;
        Ok(commit_id_iter.next().transpose()?)
    }

    /// Parses template of the given language into evaluation tree.
    ///
    /// `wrap_self` specifies the type of the top-level property, which should
    /// be one of the `L::wrap_*()` functions.
    pub fn parse_template<'a, C: Clone + 'a, L: TemplateLanguage<'a> + ?Sized>(
        &self,
        ui: &Ui,
        language: &L,
        template_text: &str,
        wrap_self: impl Fn(PropertyPlaceholder<C>) -> L::Property,
    ) -> Result<TemplateRenderer<'a, C>, CommandError> {
        let mut diagnostics = TemplateDiagnostics::new();
        let template = template_builder::parse(
            language,
            &mut diagnostics,
            template_text,
            &self.template_aliases_map,
            wrap_self,
        )?;
        print_parse_diagnostics(ui, "In template expression", &diagnostics)?;
        Ok(template)
    }

    /// Creates commit template language environment for this workspace and the
    /// given `repo`.
    pub fn commit_template_language<'a>(
        &'a self,
        repo: &'a dyn Repo,
        id_prefix_context: &'a IdPrefixContext,
    ) -> CommitTemplateLanguage<'a> {
        CommitTemplateLanguage::new(
            repo,
            &self.path_converter,
            &self.workspace_id,
            self.revset_parse_context(),
            id_prefix_context,
            self.immutable_expression(),
            self.conflict_marker_style,
            &self.command.data.commit_template_extensions,
        )
    }

    pub fn operation_template_extensions(&self) -> &[Arc<dyn OperationTemplateLanguageExtension>] {
        &self.command.data.operation_template_extensions
    }
}

/// Provides utilities for writing a command that works on a [`Workspace`]
/// (which most commands do).
pub struct WorkspaceCommandHelper {
    workspace: Workspace,
    user_repo: ReadonlyUserRepo,
    env: WorkspaceCommandEnvironment,
    // TODO: Parsed template can be cached if it doesn't capture 'repo lifetime
    commit_summary_template_text: String,
    op_summary_template_text: String,
    may_update_working_copy: bool,
    working_copy_shared_with_git: bool,
}

enum SnapshotWorkingCopyError {
    Command(CommandError),
    StaleWorkingCopy(CommandError),
}

impl SnapshotWorkingCopyError {
    fn into_command_error(self) -> CommandError {
        match self {
            Self::Command(err) => err,
            Self::StaleWorkingCopy(err) => err,
        }
    }
}

fn snapshot_command_error<E>(err: E) -> SnapshotWorkingCopyError
where
    E: Into<CommandError>,
{
    SnapshotWorkingCopyError::Command(err.into())
}

impl WorkspaceCommandHelper {
    #[instrument(skip_all)]
    fn new(
        ui: &Ui,
        workspace: Workspace,
        repo: Arc<ReadonlyRepo>,
        env: WorkspaceCommandEnvironment,
        loaded_at_head: bool,
    ) -> Result<Self, CommandError> {
        let settings = env.settings();
        let commit_summary_template_text = settings.get_string("templates.commit_summary")?;
        let op_summary_template_text = settings.get_string("templates.op_summary")?;
        let may_update_working_copy =
            loaded_at_head && !env.command.global_args().ignore_working_copy;
        let working_copy_shared_with_git = is_colocated_git_workspace(&workspace, &repo);
        let helper = Self {
            workspace,
            user_repo: ReadonlyUserRepo::new(repo),
            env,
            commit_summary_template_text,
            op_summary_template_text,
            may_update_working_copy,
            working_copy_shared_with_git,
        };
        // Parse commit_summary template early to report error before starting
        // mutable operation.
        helper.parse_operation_template(ui, &helper.op_summary_template_text)?;
        helper.parse_commit_template(ui, &helper.commit_summary_template_text)?;
        helper.parse_commit_template(ui, SHORT_CHANGE_ID_TEMPLATE_TEXT)?;
        Ok(helper)
    }

    pub fn settings(&self) -> &UserSettings {
        self.env.settings()
    }

    pub fn git_backend(&self) -> Option<&GitBackend> {
        self.user_repo.git_backend()
    }

    pub fn check_working_copy_writable(&self) -> Result<(), CommandError> {
        if self.may_update_working_copy {
            Ok(())
        } else {
            let hint = if self.env.command.global_args().ignore_working_copy {
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

    #[instrument(skip_all)]
    fn maybe_snapshot_impl(&mut self, ui: &Ui) -> Result<(), SnapshotWorkingCopyError> {
        if self.may_update_working_copy {
            if self.working_copy_shared_with_git {
                self.import_git_head(ui).map_err(snapshot_command_error)?;
            }
            // Because the Git refs (except HEAD) aren't imported yet, the ref
            // pointing to the new working-copy commit might not be exported.
            // In that situation, the ref would be conflicted anyway, so export
            // failure is okay.
            self.snapshot_working_copy(ui)?;

            // import_git_refs() can rebase the working-copy commit.
            if self.working_copy_shared_with_git {
                self.import_git_refs(ui).map_err(snapshot_command_error)?;
            }
        }
        Ok(())
    }

    /// Snapshot the working copy if allowed, and import Git refs if the working
    /// copy is collocated with Git.
    #[instrument(skip_all)]
    pub fn maybe_snapshot(&mut self, ui: &Ui) -> Result<(), CommandError> {
        self.maybe_snapshot_impl(ui)
            .map_err(|err| err.into_command_error())
    }

    /// Imports new HEAD from the colocated Git repo.
    ///
    /// If the Git HEAD has changed, this function checks out the new Git HEAD.
    /// The old working-copy commit will be abandoned if it's discardable. The
    /// working-copy state will be reset to point to the new Git HEAD. The
    /// working-copy contents won't be updated.
    #[instrument(skip_all)]
    fn import_git_head(&mut self, ui: &Ui) -> Result<(), CommandError> {
        assert!(self.may_update_working_copy);
        let command = self.env.command.clone();
        let mut tx = self.start_transaction();
        git::import_head(tx.repo_mut())?;
        if !tx.repo().has_changes() {
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
        let new_git_head = tx.repo().view().git_head().clone();
        if let Some(new_git_head_id) = new_git_head.as_normal() {
            let workspace_id = self.workspace_id().to_owned();
            let new_git_head_commit = tx.repo().store().get_commit(new_git_head_id)?;
            tx.repo_mut()
                .check_out(workspace_id, command.settings(), &new_git_head_commit)?;
            let mut locked_ws = self.workspace.start_working_copy_mutation()?;
            // The working copy was presumably updated by the git command that updated
            // HEAD, so we just need to reset our working copy
            // state to it without updating working copy files.
            locked_ws.locked_wc().reset(&new_git_head_commit)?;
            tx.repo_mut().rebase_descendants(command.settings())?;
            self.user_repo = ReadonlyUserRepo::new(tx.commit("import git head")?);
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
    /// bookmarks.
    ///
    /// If the working-copy branch is rebased, and if update is allowed, the
    /// new working-copy commit will be checked out.
    ///
    /// This function does not import the Git HEAD, but the HEAD may be reset to
    /// the working copy parent if the repository is colocated.
    #[instrument(skip_all)]
    fn import_git_refs(&mut self, ui: &Ui) -> Result<(), CommandError> {
        let git_settings = self.settings().git_settings()?;
        let mut tx = self.start_transaction();
        // Automated import shouldn't fail because of reserved remote name.
        let stats = git::import_some_refs(tx.repo_mut(), &git_settings, |ref_name| {
            !git::is_reserved_git_remote_ref(ref_name)
        })?;
        if !tx.repo().has_changes() {
            return Ok(());
        }

        print_git_import_stats(ui, tx.repo(), &stats, false)?;
        let mut tx = tx.into_inner();
        // Rebase here to show slightly different status message.
        let num_rebased = tx.repo_mut().rebase_descendants(self.settings())?;
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

    pub fn repo_path(&self) -> &Path {
        self.workspace.repo_path()
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn working_copy(&self) -> &dyn WorkingCopy {
        self.workspace.working_copy()
    }

    pub fn env(&self) -> &WorkspaceCommandEnvironment {
        &self.env
    }

    pub fn checkout_options(&self) -> CheckoutOptions {
        CheckoutOptions {
            conflict_marker_style: self.env.conflict_marker_style(),
        }
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

    fn create_and_check_out_recovery_commit(&mut self, ui: &Ui) -> Result<(), CommandError> {
        self.check_working_copy_writable()?;

        let workspace_id = self.workspace_id().clone();
        let mut locked_ws = self.workspace.start_working_copy_mutation()?;
        let (repo, new_commit) = working_copy::create_and_check_out_recovery_commit(
            locked_ws.locked_wc(),
            &self.user_repo.repo,
            workspace_id,
            self.env.settings(),
            "RECOVERY COMMIT FROM `jj workspace update-stale`

This commit contains changes that were written to the working copy by an
operation that was subsequently lost (or was at least unavailable when you ran
`jj workspace update-stale`). Because the operation was lost, we don't know
what the parent commits are supposed to be. That means that the diff compared
to the current parents may contain changes from multiple commits.
",
        )?;

        writeln!(
            ui.status(),
            "Created and checked out recovery commit {}",
            short_commit_hash(new_commit.id())
        )?;
        locked_ws.finish(repo.op_id().clone())?;
        self.user_repo = ReadonlyUserRepo::new(repo);

        self.maybe_snapshot(ui)
    }

    pub fn workspace_root(&self) -> &Path {
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
        self.path_converter().format_file_path(file)
    }

    /// Parses a path relative to cwd into a RepoPath, which is relative to the
    /// workspace root.
    pub fn parse_file_path(&self, input: &str) -> Result<RepoPathBuf, UiPathParseError> {
        self.path_converter().parse_file_path(input)
    }

    /// Parses the given strings as file patterns.
    pub fn parse_file_patterns(
        &self,
        ui: &Ui,
        values: &[String],
    ) -> Result<FilesetExpression, CommandError> {
        // TODO: This function might be superseded by parse_union_filesets(),
        // but it would be weird if parse_union_*() had a special case for the
        // empty arguments.
        if values.is_empty() {
            Ok(FilesetExpression::all())
        } else if self.settings().get_bool("ui.allow-filesets")? {
            self.parse_union_filesets(ui, values)
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
        ui: &Ui,
        file_args: &[String], // TODO: introduce FileArg newtype?
    ) -> Result<FilesetExpression, CommandError> {
        let mut diagnostics = FilesetDiagnostics::new();
        let expressions: Vec<_> = file_args
            .iter()
            .map(|arg| fileset::parse_maybe_bare(&mut diagnostics, arg, self.path_converter()))
            .try_collect()?;
        print_parse_diagnostics(ui, "In fileset expression", &diagnostics)?;
        Ok(FilesetExpression::union_all(expressions))
    }

    pub fn auto_tracking_matcher(&self, ui: &Ui) -> Result<Box<dyn Matcher>, CommandError> {
        let mut diagnostics = FilesetDiagnostics::new();
        let pattern = self.settings().get_string("snapshot.auto-track")?;
        let expression = fileset::parse(
            &mut diagnostics,
            &pattern,
            &RepoPathUiConverter::Fs {
                cwd: "".into(),
                base: "".into(),
            },
        )?;
        print_parse_diagnostics(ui, "In `snapshot.auto-track`", &diagnostics)?;
        Ok(expression.to_matcher())
    }

    pub fn snapshot_options_with_start_tracking_matcher<'a>(
        &self,
        start_tracking_matcher: &'a dyn Matcher,
    ) -> Result<SnapshotOptions<'a>, CommandError> {
        let base_ignores = self.base_ignores()?;
        let fsmonitor_settings = self.settings().fsmonitor_settings()?;
        let HumanByteSize(mut max_new_file_size) = self
            .settings()
            .get_value_with("snapshot.max-new-file-size", TryInto::try_into)?;
        if max_new_file_size == 0 {
            max_new_file_size = u64::MAX;
        }
        let conflict_marker_style = self.env.conflict_marker_style();
        Ok(SnapshotOptions {
            base_ignores,
            fsmonitor_settings,
            progress: None,
            start_tracking_matcher,
            max_new_file_size,
            conflict_marker_style,
        })
    }

    pub(crate) fn path_converter(&self) -> &RepoPathUiConverter {
        self.env.path_converter()
    }

    #[instrument(skip_all)]
    pub fn base_ignores(&self) -> Result<Arc<GitIgnoreFile>, GitIgnoreError> {
        let get_excludes_file_path = |config: &gix::config::File| -> Option<PathBuf> {
            // TODO: maybe use path() and interpolate(), which can process non-utf-8
            // path on Unix.
            if let Some(value) = config.string("core.excludesFile") {
                let path = str::from_utf8(&value)
                    .ok()
                    .map(file_util::expand_home_path)?;
                // The configured path is usually absolute, but if it's relative,
                // the "git" command would read the file at the work-tree directory.
                Some(self.workspace_root().join(path))
            } else {
                xdg_config_home().ok().map(|x| x.join("git").join("ignore"))
            }
        };

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

    /// Creates textual diff renderer of the specified `formats`.
    pub fn diff_renderer(&self, formats: Vec<DiffFormat>) -> DiffRenderer<'_> {
        DiffRenderer::new(
            self.repo().as_ref(),
            self.path_converter(),
            self.env.conflict_marker_style(),
            formats,
        )
    }

    /// Loads textual diff renderer from the settings and command arguments.
    pub fn diff_renderer_for(
        &self,
        args: &DiffFormatArgs,
    ) -> Result<DiffRenderer<'_>, CommandError> {
        let formats = diff_util::diff_formats_for(self.settings(), args)?;
        Ok(self.diff_renderer(formats))
    }

    /// Loads textual diff renderer from the settings and log-like command
    /// arguments. Returns `Ok(None)` if there are no command arguments that
    /// enable patch output.
    pub fn diff_renderer_for_log(
        &self,
        args: &DiffFormatArgs,
        patch: bool,
    ) -> Result<Option<DiffRenderer<'_>>, CommandError> {
        let formats = diff_util::diff_formats_for_log(self.settings(), args, patch)?;
        Ok((!formats.is_empty()).then(|| self.diff_renderer(formats)))
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
        let conflict_marker_style = self.env.conflict_marker_style();
        if let Some(name) = tool_name {
            Ok(DiffEditor::with_name(
                name,
                self.settings(),
                base_ignores,
                conflict_marker_style,
            )?)
        } else {
            Ok(DiffEditor::from_settings(
                ui,
                self.settings(),
                base_ignores,
                conflict_marker_style,
            )?)
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
        let conflict_marker_style = self.env.conflict_marker_style();
        if let Some(name) = tool_name {
            MergeEditor::with_name(
                name,
                self.settings(),
                self.path_converter(),
                conflict_marker_style,
            )
        } else {
            MergeEditor::from_settings(
                ui,
                self.settings(),
                self.path_converter(),
                conflict_marker_style,
            )
        }
    }

    pub fn resolve_single_op(&self, op_str: &str) -> Result<Operation, OpsetEvaluationError> {
        op_walk::resolve_op_with_repo(self.repo(), op_str)
    }

    /// Resolve a revset to a single revision. Return an error if the revset is
    /// empty or has multiple revisions.
    pub fn resolve_single_rev(
        &self,
        ui: &Ui,
        revision_arg: &RevisionArg,
    ) -> Result<Commit, CommandError> {
        let expression = self.parse_revset(ui, revision_arg)?;
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
        ui: &Ui,
        revision_args: &[RevisionArg],
    ) -> Result<IndexSet<Commit>, CommandError> {
        let mut all_commits = IndexSet::new();
        for revision_arg in revision_args {
            let (expression, modifier) = self.parse_revset_with_modifier(ui, revision_arg)?;
            let all = match modifier {
                Some(RevsetModifier::All) => true,
                None => self.settings().get_bool("ui.always-allow-large-revsets")?,
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
        ui: &Ui,
        revision_arg: &RevisionArg,
    ) -> Result<RevsetExpressionEvaluator<'_>, CommandError> {
        let (expression, modifier) = self.parse_revset_with_modifier(ui, revision_arg)?;
        // Whether the caller accepts multiple revisions or not, "all:" should
        // be valid. For example, "all:@" is a valid single-rev expression.
        let (None | Some(RevsetModifier::All)) = modifier;
        Ok(expression)
    }

    fn parse_revset_with_modifier(
        &self,
        ui: &Ui,
        revision_arg: &RevisionArg,
    ) -> Result<(RevsetExpressionEvaluator<'_>, Option<RevsetModifier>), CommandError> {
        let mut diagnostics = RevsetDiagnostics::new();
        let context = self.revset_parse_context();
        let (expression, modifier) =
            revset::parse_with_modifier(&mut diagnostics, revision_arg.as_ref(), &context)?;
        print_parse_diagnostics(ui, "In revset expression", &diagnostics)?;
        Ok((self.attach_revset_evaluator(expression), modifier))
    }

    /// Parses the given revset expressions and concatenates them all.
    pub fn parse_union_revsets(
        &self,
        ui: &Ui,
        revision_args: &[RevisionArg],
    ) -> Result<RevsetExpressionEvaluator<'_>, CommandError> {
        let mut diagnostics = RevsetDiagnostics::new();
        let context = self.revset_parse_context();
        let expressions: Vec<_> = revision_args
            .iter()
            .map(|arg| revset::parse_with_modifier(&mut diagnostics, arg.as_ref(), &context))
            .map_ok(|(expression, None | Some(RevsetModifier::All))| expression)
            .try_collect()?;
        print_parse_diagnostics(ui, "In revset expression", &diagnostics)?;
        let expression = RevsetExpression::union_all(&expressions);
        Ok(self.attach_revset_evaluator(expression))
    }

    pub fn attach_revset_evaluator(
        &self,
        expression: Rc<UserRevsetExpression>,
    ) -> RevsetExpressionEvaluator<'_> {
        RevsetExpressionEvaluator::new(
            self.repo().as_ref(),
            self.env.command.revset_extensions().clone(),
            self.id_prefix_context(),
            expression,
        )
    }

    pub(crate) fn revset_parse_context(&self) -> RevsetParseContext {
        self.env.revset_parse_context()
    }

    pub fn id_prefix_context(&self) -> &IdPrefixContext {
        self.user_repo
            .id_prefix_context
            .get_or_init(|| self.env.new_id_prefix_context())
    }

    pub fn template_aliases_map(&self) -> &TemplateAliasesMap {
        &self.env.template_aliases_map
    }

    /// Parses template of the given language into evaluation tree.
    ///
    /// `wrap_self` specifies the type of the top-level property, which should
    /// be one of the `L::wrap_*()` functions.
    pub fn parse_template<'a, C: Clone + 'a, L: TemplateLanguage<'a> + ?Sized>(
        &self,
        ui: &Ui,
        language: &L,
        template_text: &str,
        wrap_self: impl Fn(PropertyPlaceholder<C>) -> L::Property,
    ) -> Result<TemplateRenderer<'a, C>, CommandError> {
        self.env
            .parse_template(ui, language, template_text, wrap_self)
    }

    /// Parses template that is validated by `Self::new()`.
    fn reparse_valid_template<'a, C: Clone + 'a, L: TemplateLanguage<'a> + ?Sized>(
        &self,
        language: &L,
        template_text: &str,
        wrap_self: impl Fn(PropertyPlaceholder<C>) -> L::Property,
    ) -> TemplateRenderer<'a, C> {
        template_builder::parse(
            language,
            &mut TemplateDiagnostics::new(),
            template_text,
            &self.env.template_aliases_map,
            wrap_self,
        )
        .expect("parse error should be confined by WorkspaceCommandHelper::new()")
    }

    /// Parses commit template into evaluation tree.
    pub fn parse_commit_template(
        &self,
        ui: &Ui,
        template_text: &str,
    ) -> Result<TemplateRenderer<'_, Commit>, CommandError> {
        let language = self.commit_template_language();
        self.parse_template(
            ui,
            &language,
            template_text,
            CommitTemplateLanguage::wrap_commit,
        )
    }

    /// Parses commit template into evaluation tree.
    pub fn parse_operation_template(
        &self,
        ui: &Ui,
        template_text: &str,
    ) -> Result<TemplateRenderer<'_, Operation>, CommandError> {
        let language = self.operation_template_language();
        self.parse_template(
            ui,
            &language,
            template_text,
            OperationTemplateLanguage::wrap_operation,
        )
    }

    /// Creates commit template language environment for this workspace.
    pub fn commit_template_language(&self) -> CommitTemplateLanguage<'_> {
        self.env
            .commit_template_language(self.repo().as_ref(), self.id_prefix_context())
    }

    /// Creates operation template language environment for this workspace.
    pub fn operation_template_language(&self) -> OperationTemplateLanguage {
        OperationTemplateLanguage::new(
            self.repo().op_store().root_operation_id(),
            Some(self.repo().op_id()),
            self.env.operation_template_extensions(),
        )
    }

    /// Template for one-line summary of a commit.
    pub fn commit_summary_template(&self) -> TemplateRenderer<'_, Commit> {
        let language = self.commit_template_language();
        self.reparse_valid_template(
            &language,
            &self.commit_summary_template_text,
            CommitTemplateLanguage::wrap_commit,
        )
    }

    /// Template for one-line summary of an operation.
    pub fn operation_summary_template(&self) -> TemplateRenderer<'_, Operation> {
        let language = self.operation_template_language();
        self.reparse_valid_template(
            &language,
            &self.op_summary_template_text,
            OperationTemplateLanguage::wrap_operation,
        )
        .labeled("operation")
    }

    pub fn short_change_id_template(&self) -> TemplateRenderer<'_, Commit> {
        let language = self.commit_template_language();
        self.reparse_valid_template(
            &language,
            SHORT_CHANGE_ID_TEMPLATE_TEXT,
            CommitTemplateLanguage::wrap_commit,
        )
    }

    /// Returns one-line summary of the given `commit`.
    ///
    /// Use `write_commit_summary()` to get colorized output. Use
    /// `commit_summary_template()` if you have many commits to process.
    pub fn format_commit_summary(&self, commit: &Commit) -> String {
        let mut output = Vec::new();
        self.write_commit_summary(&mut PlainTextFormatter::new(&mut output), commit)
            .expect("write() to PlainTextFormatter should never fail");
        // Template output is usually UTF-8, but it can contain file content.
        output.into_string_lossy()
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
        let Some(commit_id) = self
            .env
            .find_immutable_commit(self.repo().as_ref(), commits)?
        else {
            return Ok(());
        };
        let error = if &commit_id == self.repo().store().root_commit_id() {
            user_error(format!("The root commit {commit_id:.12} is immutable"))
        } else {
            let mut error = user_error(format!("Commit {commit_id:.12} is immutable"));
            let commit = self.repo().store().get_commit(&commit_id)?;
            error.add_formatted_hint_with(|formatter| {
                write!(formatter, "Could not modify commit: ")?;
                self.write_commit_summary(formatter, &commit)?;
                Ok(())
            });
            error.add_hint(
                "Pass `--ignore-immutable` or configure the set of immutable commits via \
                 `revset-aliases.immutable_heads()`.",
            );
            error
        };
        Err(error)
    }

    #[instrument(skip_all)]
    fn snapshot_working_copy(&mut self, ui: &Ui) -> Result<(), SnapshotWorkingCopyError> {
        let workspace_id = self.workspace_id().to_owned();
        let get_wc_commit = |repo: &ReadonlyRepo| -> Result<Option<_>, _> {
            repo.view()
                .get_wc_commit_id(&workspace_id)
                .map(|id| repo.store().get_commit(id))
                .transpose()
                .map_err(snapshot_command_error)
        };
        let repo = self.repo().clone();
        let Some(wc_commit) = get_wc_commit(&repo)? else {
            // If the workspace has been deleted, it's unclear what to do, so we just skip
            // committing the working copy.
            return Ok(());
        };
        let auto_tracking_matcher = self
            .auto_tracking_matcher(ui)
            .map_err(snapshot_command_error)?;
        let options = self
            .snapshot_options_with_start_tracking_matcher(&auto_tracking_matcher)
            .map_err(snapshot_command_error)?;

        // Compare working-copy tree and operation with repo's, and reload as needed.
        let command = self.env.command.clone();
        let mut locked_ws = self
            .workspace
            .start_working_copy_mutation()
            .map_err(snapshot_command_error)?;
        let old_op_id = locked_ws.locked_wc().old_operation_id().clone();

        let (repo, wc_commit) =
            match WorkingCopyFreshness::check_stale(locked_ws.locked_wc(), &wc_commit, &repo) {
                Ok(WorkingCopyFreshness::Fresh) => (repo, wc_commit),
                Ok(WorkingCopyFreshness::Updated(wc_operation)) => {
                    let repo = repo
                        .reload_at(&wc_operation)
                        .map_err(snapshot_command_error)?;
                    let wc_commit = if let Some(wc_commit) = get_wc_commit(&repo)? {
                        wc_commit
                    } else {
                        return Ok(()); // The workspace has been deleted (see
                                       // above)
                    };
                    (repo, wc_commit)
                }
                Ok(WorkingCopyFreshness::WorkingCopyStale) => {
                    return Err(SnapshotWorkingCopyError::StaleWorkingCopy(
                        user_error_with_hint(
                            format!(
                                "The working copy is stale (not updated since operation {}).",
                                short_operation_hash(&old_op_id)
                            ),
                            "Run `jj workspace update-stale` to update it.
See https://jj-vcs.github.io/jj/latest/working-copy/#stale-working-copy \
                             for more information.",
                        ),
                    ));
                }
                Ok(WorkingCopyFreshness::SiblingOperation) => {
                    return Err(SnapshotWorkingCopyError::StaleWorkingCopy(internal_error(
                        format!(
                            "The repo was loaded at operation {}, which seems to be a sibling of \
                             the working copy's operation {}",
                            short_operation_hash(repo.op_id()),
                            short_operation_hash(&old_op_id)
                        ),
                    )));
                }
                Err(OpStoreError::ObjectNotFound { .. }) => {
                    return Err(SnapshotWorkingCopyError::StaleWorkingCopy(
                        user_error_with_hint(
                            "Could not read working copy's operation.",
                            "Run `jj workspace update-stale` to recover.
See https://jj-vcs.github.io/jj/latest/working-copy/#stale-working-copy \
                             for more information.",
                        ),
                    ));
                }
                Err(e) => return Err(snapshot_command_error(e)),
            };
        self.user_repo = ReadonlyUserRepo::new(repo);
        let (new_tree_id, stats) = {
            let mut options = options;
            let progress = crate::progress::snapshot_progress(ui);
            options.progress = progress.as_ref().map(|x| x as _);
            locked_ws
                .locked_wc()
                .snapshot(&options)
                .map_err(snapshot_command_error)?
        };
        if new_tree_id != *wc_commit.tree_id() {
            let mut tx = start_repo_transaction(
                &self.user_repo.repo,
                command.settings(),
                command.string_args(),
            );
            tx.set_is_snapshot(true);
            let mut_repo = tx.repo_mut();
            let commit = mut_repo
                .rewrite_commit(command.settings(), &wc_commit)
                .set_tree_id(new_tree_id)
                .write()
                .map_err(snapshot_command_error)?;
            mut_repo
                .set_wc_commit(workspace_id, commit.id().clone())
                .map_err(snapshot_command_error)?;

            // Rebase descendants
            let num_rebased = mut_repo
                .rebase_descendants(command.settings())
                .map_err(snapshot_command_error)?;
            if num_rebased > 0 {
                writeln!(
                    ui.status(),
                    "Rebased {num_rebased} descendant commits onto updated working copy"
                )
                .map_err(snapshot_command_error)?;
            }

            if self.working_copy_shared_with_git {
                let refs = git::export_refs(mut_repo).map_err(snapshot_command_error)?;
                print_failed_git_export(ui, &refs).map_err(snapshot_command_error)?;
            }

            let repo = tx
                .commit("snapshot working copy")
                .map_err(snapshot_command_error)?;
            self.user_repo = ReadonlyUserRepo::new(repo);
        }
        locked_ws
            .finish(self.user_repo.repo.op_id().clone())
            .map_err(snapshot_command_error)?;
        print_snapshot_stats(ui, &stats, &self.env.path_converter)
            .map_err(snapshot_command_error)?;
        Ok(())
    }

    fn update_working_copy(
        &mut self,
        ui: &Ui,
        maybe_old_commit: Option<&Commit>,
        new_commit: &Commit,
    ) -> Result<(), CommandError> {
        assert!(self.may_update_working_copy);
        let checkout_options = self.checkout_options();
        let stats = update_working_copy(
            &self.user_repo.repo,
            &mut self.workspace,
            maybe_old_commit,
            new_commit,
            &checkout_options,
        )?;
        if Some(new_commit) != maybe_old_commit {
            if let Some(mut formatter) = ui.status_formatter() {
                let template = self.commit_summary_template();
                write!(formatter, "Working copy now at: ")?;
                formatter.with_label("working_copy", |fmt| template.format(new_commit, fmt))?;
                writeln!(formatter)?;
                for parent in new_commit.parents() {
                    let parent = parent?;
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
                    print_conflicted_paths(conflicts, formatter.as_mut(), self)?;
                }
            }
        }
        Ok(())
    }

    pub fn start_transaction(&mut self) -> WorkspaceCommandTransaction {
        let tx =
            start_repo_transaction(self.repo(), self.settings(), self.env.command.string_args());
        let id_prefix_context = mem::take(&mut self.user_repo.id_prefix_context);
        WorkspaceCommandTransaction {
            helper: self,
            tx,
            id_prefix_context,
        }
    }

    fn finish_transaction(
        &mut self,
        ui: &Ui,
        mut tx: Transaction,
        description: impl Into<String>,
    ) -> Result<(), CommandError> {
        if !tx.repo().has_changes() {
            writeln!(ui.status(), "Nothing changed.")?;
            return Ok(());
        }
        let num_rebased = tx.repo_mut().rebase_descendants(self.settings())?;
        if num_rebased > 0 {
            writeln!(ui.status(), "Rebased {num_rebased} descendant commits")?;
        }

        for (workspace_id, wc_commit_id) in tx.repo().view().wc_commit_ids().clone().iter().sorted()
        //sorting otherwise non deterministic order (bad for tests)
        {
            if self
                .env
                .find_immutable_commit(tx.repo(), [wc_commit_id])?
                .is_some()
            {
                let wc_commit = tx.repo().store().get_commit(wc_commit_id)?;
                tx.repo_mut()
                    .check_out(workspace_id.clone(), self.settings(), &wc_commit)?;
                writeln!(
                    ui.warning_default(),
                    "The working-copy commit in workspace '{}' became immutable, so a new commit \
                     has been created on top of it.",
                    workspace_id.as_str()
                )?;
            }
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
                git::reset_head(tx.repo_mut(), &git_repo, wc_commit)?;
            }
            let refs = git::export_refs(tx.repo_mut())?;
            print_failed_git_export(ui, &refs)?;
        }

        self.user_repo = ReadonlyUserRepo::new(tx.commit(description)?);

        // Update working copy before reporting repo changes, so that
        // potential errors while reporting changes (broken pipe, etc)
        // don't leave the working copy in a stale state.
        if self.may_update_working_copy {
            if let Some(new_commit) = &maybe_new_wc_commit {
                self.update_working_copy(ui, maybe_old_wc_commit.as_ref(), new_commit)?;
            } else {
                // It seems the workspace was deleted, so we shouldn't try to
                // update it.
            }
        }

        self.report_repo_changes(ui, &old_repo)?;

        let settings = self.settings();
        let missing_user_name = settings.user_name().is_empty();
        let missing_user_mail = settings.user_email().is_empty();
        if missing_user_name || missing_user_mail {
            let mut writer = ui.warning_default();
            let not_configured_msg = match (missing_user_name, missing_user_mail) {
                (true, true) => "Name and email not configured.",
                (true, false) => "Name not configured.",
                (false, true) => "Email not configured.",
                _ => unreachable!(),
            };
            write!(writer, "{not_configured_msg} ")?;
            writeln!(
                writer,
                "Until configured, your commits will be created with the empty identity, and \
                 can't be pushed to remotes. To configure, run:",
            )?;
            if missing_user_name {
                writeln!(writer, r#"  jj config set --user user.name "Some One""#)?;
            }
            if missing_user_mail {
                writeln!(
                    writer,
                    r#"  jj config set --user user.email "someone@example.com""#
                )?;
            }
        }
        Ok(())
    }

    /// Inform the user about important changes to the repo since the previous
    /// operation (when `old_repo` was loaded).
    fn report_repo_changes(
        &self,
        ui: &Ui,
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
        let conflicts = RevsetExpression::filter(RevsetFilterPredicate::HasConflict)
            .filtered(RevsetFilterPredicate::File(FilesetExpression::all()));
        let removed_conflicts_expr = new_heads.range(&old_heads).intersection(&conflicts);
        let added_conflicts_expr = old_heads.range(&new_heads).intersection(&conflicts);

        let get_commits =
            |expr: Rc<ResolvedRevsetExpression>| -> Result<Vec<Commit>, CommandError> {
                let commits = expr
                    .evaluate(new_repo)?
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
        revset_util::warn_unresolvable_trunk(ui, new_repo, &self.env.revset_parse_context())?;

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
            .evaluate(repo)?;

        let root_conflict_commits: Vec<_> = root_conflicts_revset
            .iter()
            .commits(repo.store())
            .try_collect()?;

        if !root_conflict_commits.is_empty() {
            fmt.push_label("hint")?;
            if only_one_conflicted_commit {
                writeln!(fmt, "To resolve the conflicts, start by updating to it:",)?;
            } else if root_conflict_commits.len() == 1 {
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
            let format_short_change_id = self.short_change_id_template();
            for commit in root_conflict_commits {
                write!(fmt, "  jj new ")?;
                format_short_change_id.format(&commit, fmt)?;
                writeln!(fmt)?;
            }
            writeln!(
                fmt,
                r#"Then use `jj resolve`, or edit the conflict markers in the file directly.
Once the conflicts are resolved, you may want to inspect the result with `jj diff`.
Then run `jj squash` to move the resolution into the conflicted commit."#,
            )?;
            fmt.pop_label()?;
        }
        Ok(())
    }

    /// Identifies bookmarks which are eligible to be moved automatically
    /// during `jj commit` and `jj new`. Whether a bookmark is eligible is
    /// determined by its target and the user and repo config for
    /// "advance-bookmarks".
    ///
    /// Returns a Vec of bookmarks in `repo` that point to any of the `from`
    /// commits and that are eligible to advance. The `from` commits are
    /// typically the parents of the target commit of `jj commit` or `jj new`.
    ///
    /// Bookmarks are not moved until
    /// `WorkspaceCommandTransaction::advance_bookmarks()` is called with the
    /// `AdvanceableBookmark`s returned by this function.
    ///
    /// Returns an empty `std::Vec` if no bookmarks are eligible to advance.
    pub fn get_advanceable_bookmarks<'a>(
        &self,
        from: impl IntoIterator<Item = &'a CommitId>,
    ) -> Result<Vec<AdvanceableBookmark>, CommandError> {
        let ab_settings = AdvanceBookmarksSettings::from_settings(self.settings())?;
        if !ab_settings.feature_enabled() {
            // Return early if we know that there's no work to do.
            return Ok(Vec::new());
        }

        let mut advanceable_bookmarks = Vec::new();
        for from_commit in from {
            for (name, _) in self.repo().view().local_bookmarks_for_commit(from_commit) {
                if ab_settings.bookmark_is_eligible(name) {
                    advanceable_bookmarks.push(AdvanceableBookmark {
                        name: name.to_owned(),
                        old_commit_id: from_commit.clone(),
                    });
                }
            }
        }

        Ok(advanceable_bookmarks)
    }
}

/// An ongoing [`Transaction`] tied to a particular workspace.
///
/// `WorkspaceCommandTransaction`s are created with
/// [`WorkspaceCommandHelper::start_transaction`] and committed with
/// [`WorkspaceCommandTransaction::finish`]. The inner `Transaction` can also be
/// extracted using [`WorkspaceCommandTransaction::into_inner`] in situations
/// where finer-grained control over the `Transaction` is necessary.
#[must_use]
pub struct WorkspaceCommandTransaction<'a> {
    helper: &'a mut WorkspaceCommandHelper,
    tx: Transaction,
    /// Cache of index built against the current MutableRepo state.
    id_prefix_context: OnceCell<IdPrefixContext>,
}

impl WorkspaceCommandTransaction<'_> {
    /// Workspace helper that may use the base repo.
    pub fn base_workspace_helper(&self) -> &WorkspaceCommandHelper {
        self.helper
    }

    pub fn settings(&self) -> &UserSettings {
        self.helper.settings()
    }

    pub fn base_repo(&self) -> &Arc<ReadonlyRepo> {
        self.tx.base_repo()
    }

    pub fn repo(&self) -> &MutableRepo {
        self.tx.repo()
    }

    pub fn repo_mut(&mut self) -> &mut MutableRepo {
        self.id_prefix_context.take(); // invalidate
        self.tx.repo_mut()
    }

    pub fn check_out(&mut self, commit: &Commit) -> Result<Commit, CheckOutCommitError> {
        let workspace_id = self.helper.workspace_id().to_owned();
        let settings = self.helper.settings();
        self.id_prefix_context.take(); // invalidate
        self.tx.repo_mut().check_out(workspace_id, settings, commit)
    }

    pub fn edit(&mut self, commit: &Commit) -> Result<(), EditCommitError> {
        let workspace_id = self.helper.workspace_id().to_owned();
        self.id_prefix_context.take(); // invalidate
        self.tx.repo_mut().edit(workspace_id, commit)
    }

    pub fn format_commit_summary(&self, commit: &Commit) -> String {
        let mut output = Vec::new();
        self.write_commit_summary(&mut PlainTextFormatter::new(&mut output), commit)
            .expect("write() to PlainTextFormatter should never fail");
        // Template output is usually UTF-8, but it can contain file content.
        output.into_string_lossy()
    }

    pub fn write_commit_summary(
        &self,
        formatter: &mut dyn Formatter,
        commit: &Commit,
    ) -> std::io::Result<()> {
        self.commit_summary_template().format(commit, formatter)
    }

    /// Template for one-line summary of a commit within transaction.
    pub fn commit_summary_template(&self) -> TemplateRenderer<'_, Commit> {
        let language = self.commit_template_language();
        self.helper.reparse_valid_template(
            &language,
            &self.helper.commit_summary_template_text,
            CommitTemplateLanguage::wrap_commit,
        )
    }

    /// Creates commit template language environment capturing the current
    /// transaction state.
    pub fn commit_template_language(&self) -> CommitTemplateLanguage<'_> {
        let id_prefix_context = self
            .id_prefix_context
            .get_or_init(|| self.helper.env.new_id_prefix_context());
        self.helper
            .env
            .commit_template_language(self.tx.repo(), id_prefix_context)
    }

    /// Parses commit template with the current transaction state.
    pub fn parse_commit_template(
        &self,
        ui: &Ui,
        template_text: &str,
    ) -> Result<TemplateRenderer<'_, Commit>, CommandError> {
        let language = self.commit_template_language();
        self.helper.env.parse_template(
            ui,
            &language,
            template_text,
            CommitTemplateLanguage::wrap_commit,
        )
    }

    pub fn finish(self, ui: &Ui, description: impl Into<String>) -> Result<(), CommandError> {
        self.helper.finish_transaction(ui, self.tx, description)
    }

    /// Returns the wrapped [`Transaction`] for circumstances where
    /// finer-grained control is needed. The caller becomes responsible for
    /// finishing the `Transaction`, including rebasing descendants and updating
    /// the working copy, if applicable.
    pub fn into_inner(self) -> Transaction {
        self.tx
    }

    /// Moves each bookmark in `bookmarks` from an old commit it's associated
    /// with (configured by `get_advanceable_bookmarks`) to the `move_to`
    /// commit. If the bookmark is conflicted before the update, it will
    /// remain conflicted after the update, but the conflict will involve
    /// the `move_to` commit instead of the old commit.
    pub fn advance_bookmarks(&mut self, bookmarks: Vec<AdvanceableBookmark>, move_to: &CommitId) {
        for bookmark in bookmarks {
            // This removes the old commit ID from the bookmark's RefTarget and
            // replaces it with the `move_to` ID.
            self.repo_mut().merge_local_bookmark(
                &bookmark.name,
                &RefTarget::normal(bookmark.old_commit_id),
                &RefTarget::normal(move_to.clone()),
            );
        }
    }
}

pub fn find_workspace_dir(cwd: &Path) -> &Path {
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
        WorkspaceLoadError::StoreLoadError(StoreLoadError::Signing(err)) => user_error(err),
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

fn update_stale_working_copy(
    mut locked_ws: LockedWorkspace,
    op_id: OperationId,
    stale_commit: &Commit,
    new_commit: &Commit,
    options: &CheckoutOptions,
) -> Result<CheckoutStats, CommandError> {
    // The same check as start_working_copy_mutation(), but with the stale
    // working-copy commit.
    if stale_commit.tree_id() != locked_ws.locked_wc().old_tree_id() {
        return Err(user_error("Concurrent working copy operation. Try again."));
    }
    let stats = locked_ws
        .locked_wc()
        .check_out(new_commit, options)
        .map_err(|err| {
            internal_error_with_message(
                format!("Failed to check out commit {}", new_commit.id().hex()),
                err,
            )
        })?;
    locked_ws.finish(op_id)?;

    Ok(stats)
}

#[instrument(skip_all)]
pub fn print_conflicted_paths(
    conflicts: Vec<(RepoPathBuf, BackendResult<MergedTreeValue>)>,
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

    for ((_, conflict), formatted_path) in std::iter::zip(conflicts, formatted_paths) {
        // TODO: Display the error for the path instead of failing the whole command if
        // `conflict` is an error?
        let conflict = conflict?.simplify();
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
        // `removes` of a conflict (see e.g. https://github.com/jj-vcs/jj/pull/978). In
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
            io::Result::Ok(())
        })?;
        writeln!(formatter)?;
    }
    Ok(())
}

pub fn print_snapshot_stats(
    ui: &Ui,
    stats: &SnapshotStats,
    path_converter: &RepoPathUiConverter,
) -> io::Result<()> {
    // It might make sense to add files excluded by snapshot.auto-track to the
    // untracked_paths, but they shouldn't be warned every time we do snapshot.
    // These paths will have to be printed by "jj status" instead.
    if !stats.untracked_paths.is_empty() {
        writeln!(ui.warning_default(), "Refused to snapshot some files:")?;
        let mut formatter = ui.stderr_formatter();
        for (path, reason) in &stats.untracked_paths {
            let ui_path = path_converter.format_file_path(path);
            let message = match reason {
                UntrackedReason::FileTooLarge { size, max_size } => {
                    // Show both exact and human bytes sizes to avoid something
                    // like '1.0MiB, maximum size allowed is ~1.0MiB'
                    let size_approx = HumanByteSize(*size);
                    let max_size_approx = HumanByteSize(*max_size);
                    format!(
                        "{size_approx} ({size} bytes); the maximum size allowed is \
                         {max_size_approx} ({max_size} bytes)",
                    )
                }
            };
            writeln!(formatter, "  {ui_path}: {message}")?;
        }
    }

    if let Some(size) = stats
        .untracked_paths
        .values()
        .map(|reason| match reason {
            UntrackedReason::FileTooLarge { size, .. } => *size,
        })
        .max()
    {
        writedoc!(
            ui.hint_default(),
            r"
            This is to prevent large files from being added by accident. You can fix this by:
              - Adding the file to `.gitignore`
              - Run `jj config set --repo snapshot.max-new-file-size {size}`
                This will increase the maximum file size allowed for new files, in this repository only.
              - Run `jj --config snapshot.max-new-file-size={size} st`
                This will increase the maximum file size allowed for new files, for this command only.
            "
        )?;
    }
    Ok(())
}

pub fn print_checkout_stats(
    ui: &Ui,
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
        writeln!(
            ui.hint_default(),
            "Inspect the changes compared to the intended target with `jj diff --from {}`.
Discard the conflicting changes with `jj restore --from {}`.",
            short_commit_hash(new_commit.id()),
            short_commit_hash(new_commit.id())
        )?;
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
        // TODO: propagate errors
        explicit_paths.retain(|&path| tree.path_value(path).unwrap().is_absent());
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

pub fn print_trackable_remote_bookmarks(ui: &Ui, view: &View) -> io::Result<()> {
    let remote_bookmark_names = view
        .bookmarks()
        .filter(|(_, bookmark_target)| bookmark_target.local_target.is_present())
        .flat_map(|(name, bookmark_target)| {
            bookmark_target
                .remote_refs
                .into_iter()
                .filter(|&(_, remote_ref)| !remote_ref.is_tracking())
                .map(move |(remote, _)| format!("{name}@{remote}"))
        })
        .collect_vec();
    if remote_bookmark_names.is_empty() {
        return Ok(());
    }

    if let Some(mut formatter) = ui.status_formatter() {
        writeln!(
            formatter.labeled("hint").with_heading("Hint: "),
            "The following remote bookmarks aren't associated with the existing local bookmarks:"
        )?;
        for full_name in &remote_bookmark_names {
            write!(formatter, "  ")?;
            writeln!(formatter.labeled("bookmark"), "{full_name}")?;
        }
        writeln!(
            formatter.labeled("hint").with_heading("Hint: "),
            "Run `jj bookmark track {names}` to keep local bookmarks updated on future pulls.",
            names = remote_bookmark_names.join(" "),
        )?;
    }
    Ok(())
}

pub fn update_working_copy(
    repo: &Arc<ReadonlyRepo>,
    workspace: &mut Workspace,
    old_commit: Option<&Commit>,
    new_commit: &Commit,
    options: &CheckoutOptions,
) -> Result<Option<CheckoutStats>, CommandError> {
    let old_tree_id = old_commit.map(|commit| commit.tree_id().clone());
    let stats = if Some(new_commit.tree_id()) != old_tree_id.as_ref() {
        // TODO: CheckoutError::ConcurrentCheckout should probably just result in a
        // warning for most commands (but be an error for the checkout command)
        let stats = workspace
            .check_out(
                repo.op_id().clone(),
                old_tree_id.as_ref(),
                new_commit,
                options,
            )
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
    stacked_config: &StackedConfig,
) -> Result<TemplateAliasesMap, CommandError> {
    let table_name = ConfigNamePathBuf::from_iter(["template-aliases"]);
    let mut aliases_map = TemplateAliasesMap::new();
    // Load from all config layers in order. 'f(x)' in default layer should be
    // overridden by 'f(a)' in user.
    for layer in stacked_config.layers() {
        let table = match layer.look_up_table(&table_name) {
            Ok(Some(table)) => table,
            Ok(None) => continue,
            Err(item) => {
                return Err(ConfigGetError::Type {
                    name: table_name.to_string(),
                    error: format!("Expected a table, but is {}", item.type_name()).into(),
                    source_path: layer.path.clone(),
                }
                .into());
            }
        };
        for (decl, item) in table {
            let r = item
                .as_str()
                .ok_or_else(|| format!("Expected a string, but is {}", item.type_name()))
                .and_then(|v| aliases_map.insert(decl, v).map_err(|e| e.to_string()));
            if let Err(s) = r {
                writeln!(
                    ui.warning_default(),
                    r#"Failed to load "{table_name}.{decl}": {s}"#
                )?;
            }
        }
    }
    Ok(aliases_map)
}

/// Helper to reformat content of log-like commands.
#[derive(Clone, Debug)]
pub struct LogContentFormat {
    width: usize,
    word_wrap: bool,
}

impl LogContentFormat {
    /// Creates new formatting helper for the terminal.
    pub fn new(ui: &Ui, settings: &UserSettings) -> Result<Self, ConfigGetError> {
        Ok(LogContentFormat {
            width: ui.term_width(),
            word_wrap: settings.get_bool("ui.log-word-wrap")?,
        })
    }

    /// Subtracts the given `width` and returns new formatting helper.
    #[must_use]
    pub fn sub_width(&self, width: usize) -> Self {
        LogContentFormat {
            width: self.width.saturating_sub(width),
            word_wrap: self.word_wrap,
        }
    }

    /// Current width available to content.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Writes content which will optionally be wrapped at the current width.
    pub fn write<E: From<io::Error>>(
        &self,
        formatter: &mut dyn Formatter,
        content_fn: impl FnOnce(&mut dyn Formatter) -> Result<(), E>,
    ) -> Result<(), E> {
        if self.word_wrap {
            let mut recorder = FormatRecorder::new();
            content_fn(&mut recorder)?;
            text_util::write_wrapped(formatter, &recorder, self.width)?;
        } else {
            content_fn(formatter)?;
        }
        Ok(())
    }
}

pub fn run_ui_editor(settings: &UserSettings, edit_path: &Path) -> Result<(), CommandError> {
    // Work around UNC paths not being well supported on Windows (no-op for
    // non-Windows): https://github.com/jj-vcs/jj/issues/3986
    let edit_path = dunce::simplified(edit_path);
    let editor: CommandNameAndArgs = settings.get("ui.editor")?;
    let mut cmd = editor.to_command();
    cmd.arg(edit_path);
    tracing::info!(?cmd, "running editor");
    let exit_status = cmd.status().map_err(|err| {
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
    format!("{commit_id:.12}")
}

pub fn short_change_hash(change_id: &ChangeId) -> String {
    format!("{change_id:.12}")
}

pub fn short_operation_hash(operation_id: &OperationId) -> String {
    format!("{operation_id:.12}")
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
        format_instructions: impl FnOnce() -> String,
    ) -> Result<MergedTreeId, CommandError> {
        match self {
            DiffSelector::NonInteractive => Ok(restore_tree(right_tree, left_tree, matcher)?),
            DiffSelector::Interactive(editor) => {
                Ok(editor.edit(left_tree, right_tree, matcher, format_instructions)?)
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RemoteBookmarkName {
    pub bookmark: String,
    pub remote: String,
}

impl fmt::Display for RemoteBookmarkName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let RemoteBookmarkName { bookmark, remote } = self;
        write!(f, "{bookmark}@{remote}")
    }
}

#[derive(Clone, Debug)]
pub struct RemoteBookmarkNamePattern {
    pub bookmark: StringPattern,
    pub remote: StringPattern,
}

impl FromStr for RemoteBookmarkNamePattern {
    type Err = String;

    fn from_str(src: &str) -> Result<Self, Self::Err> {
        // The kind prefix applies to both bookmark and remote fragments. It's
        // weird that unanchored patterns like substring:bookmark@remote is split
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
        // TODO: maybe reuse revset parser to handle bookmark/remote name containing @
        let (bookmark, remote) = pat.rsplit_once('@').ok_or_else(|| {
            "remote bookmark must be specified in bookmark@remote form".to_owned()
        })?;
        Ok(RemoteBookmarkNamePattern {
            bookmark: to_pattern(bookmark)?,
            remote: to_pattern(remote)?,
        })
    }
}

impl RemoteBookmarkNamePattern {
    pub fn is_exact(&self) -> bool {
        self.bookmark.is_exact() && self.remote.is_exact()
    }
}

impl fmt::Display for RemoteBookmarkNamePattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let RemoteBookmarkNamePattern { bookmark, remote } = self;
        write!(f, "{bookmark}@{remote}")
    }
}

/// Jujutsu (An experimental VCS)
///
/// To get started, see the tutorial at https://jj-vcs.github.io/jj/latest/tutorial/.
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
    /// stale working-copy commit, you can use `--ignore-working-copy`.
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
    /// most recent operation, or at the merge of the divergent operations if
    /// any.
    ///
    /// You can use `--at-op=<operation ID>` to see what the repo looked like at
    /// an earlier operation. For example `jj --at-op=<operation ID> st` will
    /// show you what `jj st` would have shown you when the given operation had
    /// just finished. `--at-op=@` is pretty much the same as the default except
    /// that divergent operations will never be merged.
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
        add = ArgValueCandidates::new(complete::operations),
    )]
    pub at_operation: Option<String>,
    /// Enable debug logging
    #[arg(long, global = true)]
    pub debug: bool,

    #[command(flatten)]
    pub early_args: EarlyArgs,
}

#[derive(clap::Args, Clone, Debug)]
pub struct EarlyArgs {
    /// When to colorize output (always, never, debug, auto)
    #[arg(long, value_name = "WHEN", global = true)]
    pub color: Option<ColorChoice>,
    /// Silence non-primary command output
    ///
    /// For example, `jj file list` will still list files, but it won't tell
    /// you if the working copy was snapshotted or if descendants were rebased.
    ///
    /// Warnings and errors will still be printed.
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    // Parsing with ignore_errors will crash if this is bool, so use
    // Option<bool>.
    pub quiet: Option<bool>,
    /// Disable the pager
    #[arg(long, global = true, action = ArgAction::SetTrue)]
    // Parsing with ignore_errors will crash if this is bool, so use
    // Option<bool>.
    pub no_pager: Option<bool>,
    /// Additional configuration options (can be repeated)
    ///
    /// The name should be specified as TOML dotted keys. The value should be
    /// specified as a TOML expression. If string value doesn't contain any TOML
    /// constructs (such as array notation), quotes can be omitted.
    #[arg(long, value_name = "NAME=VALUE", global = true)]
    pub config: Vec<String>,
    /// Additional configuration options (can be repeated) (DEPRECATED)
    // TODO: delete --config-toml in jj 0.31+
    #[arg(long, value_name = "TOML", global = true, hide = true)]
    pub config_toml: Vec<String>,
    /// Additional configuration files (can be repeated)
    #[arg(long, value_name = "PATH", global = true, value_hint = clap::ValueHint::FilePath)]
    pub config_file: Vec<String>,
}

impl EarlyArgs {
    pub(crate) fn merged_config_args(&self, matches: &ArgMatches) -> Vec<(ConfigArgKind, &str)> {
        merge_args_with(
            matches,
            &[
                ("config", &self.config),
                ("config_toml", &self.config_toml),
                ("config_file", &self.config_file),
            ],
            |id, value| match id {
                "config" => (ConfigArgKind::Item, value.as_ref()),
                "config_toml" => (ConfigArgKind::Toml, value.as_ref()),
                "config_file" => (ConfigArgKind::File, value.as_ref()),
                _ => unreachable!("unexpected id {id:?}"),
            },
        )
    }
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

/// Merges multiple clap args in order of appearance.
///
/// The `id_values` is a list of `(id, values)` pairs, where `id` is the name of
/// the clap `Arg`, and `values` are the parsed values for that arg. The
/// `convert` function transforms each `(id, value)` pair to e.g. an enum.
///
/// This is a workaround for <https://github.com/clap-rs/clap/issues/3146>.
pub fn merge_args_with<'k, 'v, T, U>(
    matches: &ArgMatches,
    id_values: &[(&'k str, &'v [T])],
    mut convert: impl FnMut(&'k str, &'v T) -> U,
) -> Vec<U> {
    let mut pos_values: Vec<(usize, U)> = Vec::new();
    for (id, values) in id_values {
        pos_values.extend(itertools::zip_eq(
            matches.indices_of(id).into_iter().flatten(),
            values.iter().map(|v| convert(id, v)),
        ));
    }
    pos_values.sort_unstable_by_key(|&(pos, _)| pos);
    pos_values.into_iter().map(|(_, value)| value).collect()
}

fn get_string_or_array(
    config: &StackedConfig,
    key: &'static str,
) -> Result<Vec<String>, ConfigGetError> {
    config
        .get(key)
        .map(|string| vec![string])
        .or_else(|_| config.get::<Vec<String>>(key))
}

fn resolve_default_command(
    ui: &Ui,
    config: &StackedConfig,
    app: &Command,
    mut string_args: Vec<String>,
) -> Result<Vec<String>, CommandError> {
    const PRIORITY_FLAGS: &[&str] = &["--help", "-h", "--version", "-V"];

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
                writeln!(
                    ui.hint_default(),
                    "Use `jj -h` for a list of available commands."
                )?;
                writeln!(
                    ui.hint_no_heading(),
                    "Run `jj config set --user ui.default-command log` to disable this message."
                )?;
            }
            let default_command = args.unwrap_or_else(|| vec!["log".to_string()]);

            // Insert the default command directly after the path to the binary.
            string_args.splice(1..1, default_command);
        }
    }
    Ok(string_args)
}

fn resolve_aliases(
    ui: &Ui,
    config: &StackedConfig,
    app: &Command,
    mut string_args: Vec<String>,
) -> Result<Vec<String>, CommandError> {
    let defined_aliases: HashSet<_> = config.table_keys("aliases").collect();
    let mut resolved_aliases = HashSet::new();
    let mut real_commands = HashSet::new();
    for command in app.get_subcommands() {
        real_commands.insert(command.get_name());
        for alias in command.get_all_aliases() {
            real_commands.insert(alias);
        }
    }
    for alias in defined_aliases.intersection(&real_commands).sorted() {
        writeln!(
            ui.warning_default(),
            "Cannot define an alias that overrides the built-in command '{alias}'"
        )?;
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
                if resolved_aliases.contains(&*alias_name) {
                    return Err(user_error(format!(
                        r#"Recursive alias definition involving "{alias_name}""#
                    )));
                }
                if let Some(&alias_name) = defined_aliases.get(&*alias_name) {
                    let alias_definition: Vec<String> = config.get(["aliases", alias_name])?;
                    assert!(string_args.ends_with(&alias_args));
                    string_args.truncate(string_args.len() - 1 - alias_args.len());
                    string_args.extend(alias_definition);
                    string_args.extend_from_slice(&alias_args);
                    resolved_aliases.insert(alias_name);
                    continue;
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
fn parse_early_args(
    app: &Command,
    args: &[String],
) -> Result<(EarlyArgs, Vec<ConfigLayer>), CommandError> {
    // ignore_errors() bypasses errors like missing subcommand
    let early_matches = app
        .clone()
        .disable_version_flag(true)
        // Do not emit DisplayHelp error
        .disable_help_flag(true)
        // Do not stop parsing at -h/--help
        .arg(
            clap::Arg::new("help")
                .short('h')
                .long("help")
                .global(true)
                .action(ArgAction::Count),
        )
        .ignore_errors(true)
        .try_get_matches_from(args)?;
    let args = EarlyArgs::from_arg_matches(&early_matches).unwrap();

    let mut config_layers = parse_config_args(&args.merged_config_args(&early_matches))?;
    // Command arguments overrides any other configuration including the
    // variables loaded from --config* arguments.
    let mut layer = ConfigLayer::empty(ConfigSource::CommandArg);
    if let Some(choice) = args.color {
        layer.set_value("ui.color", choice.to_string()).unwrap();
    }
    if args.quiet.unwrap_or_default() {
        layer.set_value("ui.quiet", true).unwrap();
    }
    if args.no_pager.unwrap_or_default() {
        layer.set_value("ui.paginate", "never").unwrap();
    }
    if !layer.is_empty() {
        config_layers.push(layer);
    }
    Ok((args, config_layers))
}

fn handle_shell_completion(
    ui: &Ui,
    app: &Command,
    config: &StackedConfig,
    cwd: &Path,
) -> Result<(), CommandError> {
    let mut args = vec![];
    // Take the first two arguments as is, they must be passed to clap_complete
    // without any changes. They are usually "jj --".
    args.extend(env::args_os().take(2));

    // Make sure aliases are expanded before passing them to clap_complete. We
    // skip the first two args ("jj" and "--") for alias resolution, then we
    // stitch the args back together, like clap_complete expects them.
    let orig_args = env::args_os().skip(2);
    if orig_args.len() > 0 {
        let arg_index: Option<usize> = env::var("_CLAP_COMPLETE_INDEX")
            .ok()
            .and_then(|s| s.parse().ok());
        let resolved_aliases = if let Some(index) = arg_index {
            // As of clap_complete 4.5.38, zsh completion script doesn't pad an
            // empty arg at the complete position. If the args doesn't include a
            // command name, the default command would be expanded at that
            // position. Therefore, no other command names would be suggested.
            // TODO: Maybe we should instead expand args[..index] + [""], adjust
            // the index accordingly, strip the last "", and append remainder?
            let pad_len = usize::saturating_sub(index + 1, orig_args.len());
            let padded_args = orig_args.chain(iter::repeat(OsString::new()).take(pad_len));
            expand_args(ui, app, padded_args, config)?
        } else {
            expand_args(ui, app, orig_args, config)?
        };
        args.extend(resolved_aliases.into_iter().map(OsString::from));
    }
    let ran_completion = clap_complete::CompleteEnv::with_factory(|| {
        app.clone()
            // for completing aliases
            .allow_external_subcommands(true)
    })
    .try_complete(args.iter(), Some(cwd))?;
    assert!(
        ran_completion,
        "This function should not be called without the COMPLETE variable set."
    );
    Ok(())
}

pub fn expand_args(
    ui: &Ui,
    app: &Command,
    args_os: impl IntoIterator<Item = OsString>,
    config: &StackedConfig,
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
    resolve_aliases(ui, config, app, string_args)
}

fn parse_args(
    app: &Command,
    tracing_subscription: &TracingSubscription,
    string_args: &[String],
) -> Result<(ArgMatches, Args), CommandError> {
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
    // Template output is usually UTF-8, but it can contain file content.
    output.into_string_lossy()
}

/// CLI command builder and runner.
#[must_use]
pub struct CliRunner {
    tracing_subscription: TracingSubscription,
    app: Command,
    config_layers: Vec<ConfigLayer>,
    store_factories: StoreFactories,
    working_copy_factories: WorkingCopyFactories,
    workspace_loader_factory: Box<dyn WorkspaceLoaderFactory>,
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
            config_layers: crate::config::default_config_layers(),
            store_factories: StoreFactories::default(),
            working_copy_factories: default_working_copy_factories(),
            workspace_loader_factory: Box::new(DefaultWorkspaceLoaderFactory),
            revset_extensions: Default::default(),
            commit_template_extensions: vec![],
            operation_template_extensions: vec![],
            dispatch_fn: Box::new(crate::commands::run_command),
            start_hook_fns: vec![],
            process_global_args_fns: vec![],
        }
    }

    /// Set the name of the CLI application to be displayed in help messages.
    pub fn name(mut self, name: &str) -> Self {
        self.app = self.app.name(name.to_string());
        self
    }

    /// Set the about message to be displayed in help messages.
    pub fn about(mut self, about: &str) -> Self {
        self.app = self.app.about(about.to_string());
        self
    }

    /// Set the version to be displayed by `jj version`.
    pub fn version(mut self, version: &str) -> Self {
        self.app = self.app.version(version.to_string());
        self
    }

    /// Adds default configs in addition to the normal defaults.
    ///
    /// The `layer.source` must be `Default`. Other sources such as `User` would
    /// be replaced by loaded configuration.
    pub fn add_extra_config(mut self, layer: ConfigLayer) -> Self {
        assert_eq!(layer.source, ConfigSource::Default);
        self.config_layers.push(layer);
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

    pub fn set_workspace_loader_factory(
        mut self,
        workspace_loader_factory: Box<dyn WorkspaceLoaderFactory>,
    ) -> Self {
        self.workspace_loader_factory = workspace_loader_factory;
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
    fn run_internal(self, ui: &mut Ui, mut raw_config: RawConfig) -> Result<(), CommandError> {
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
        let mut config_env = ConfigEnv::from_environment()?;
        // Use cwd-relative workspace configs to resolve default command and
        // aliases. WorkspaceLoader::init() won't do any heavy lifting other
        // than the path resolution.
        let maybe_cwd_workspace_loader = self
            .workspace_loader_factory
            .create(find_workspace_dir(&cwd))
            .map_err(|err| map_workspace_load_error(err, None));
        config_env.reload_user_config(&mut raw_config)?;
        if let Ok(loader) = &maybe_cwd_workspace_loader {
            config_env.reset_repo_path(loader.repo_path());
            config_env.reload_repo_config(&mut raw_config)?;
        }
        let mut config = config_env.resolve_config(&raw_config)?;
        ui.reset(&config)?;

        if env::var_os("COMPLETE").is_some() {
            return handle_shell_completion(ui, &self.app, &config, &cwd);
        }

        let string_args = expand_args(ui, &self.app, env::args_os(), &config)?;
        let (args, config_layers) = parse_early_args(&self.app, &string_args)?;
        if !config_layers.is_empty() {
            raw_config.as_mut().extend_layers(config_layers);
            config = config_env.resolve_config(&raw_config)?;
            ui.reset(&config)?;
        }
        if !args.config_toml.is_empty() {
            writeln!(
                ui.warning_default(),
                "--config-toml is deprecated; use --config or --config-file instead."
            )?;
        }
        let (matches, args) = parse_args(&self.app, &self.tracing_subscription, &string_args)
            .map_err(|err| map_clap_cli_error(err, ui, &config))?;
        for process_global_args_fn in self.process_global_args_fns {
            process_global_args_fn(ui, &matches)?;
        }

        let maybe_workspace_loader = if let Some(path) = &args.global_args.repository {
            // TODO: maybe path should be canonicalized by WorkspaceLoader?
            let abs_path = cwd.join(path);
            let abs_path = abs_path.canonicalize().unwrap_or(abs_path);
            // Invalid -R path is an error. No need to proceed.
            let loader = self
                .workspace_loader_factory
                .create(&abs_path)
                .map_err(|err| map_workspace_load_error(err, Some(path)))?;
            config_env.reset_repo_path(loader.repo_path());
            config_env.reload_repo_config(&mut raw_config)?;
            config = config_env.resolve_config(&raw_config)?;
            Ok(loader)
        } else {
            maybe_cwd_workspace_loader
        };

        // Apply workspace configs and --config arguments.
        ui.reset(&config)?;

        // If -R is specified, check if the expanded arguments differ. Aliases
        // can also be injected by --config, but that's obviously wrong.
        if args.global_args.repository.is_some() {
            let new_string_args = expand_args(ui, &self.app, env::args_os(), &config).ok();
            if new_string_args.as_ref() != Some(&string_args) {
                writeln!(
                    ui.warning_default(),
                    "Command aliases cannot be loaded from -R/--repository path"
                )?;
            }
        }

        let settings = UserSettings::from_config(config)?;
        let command_helper_data = CommandHelperData {
            app: self.app,
            cwd,
            string_args,
            matches,
            global_args: args.global_args,
            config_env,
            raw_config,
            settings,
            revset_extensions: self.revset_extensions.into(),
            commit_template_extensions: self.commit_template_extensions,
            operation_template_extensions: self.operation_template_extensions,
            maybe_workspace_loader,
            store_factories: self.store_factories,
            working_copy_factories: self.working_copy_factories,
        };
        let command_helper = CommandHelper {
            data: Rc::new(command_helper_data),
        };
        for start_hook_fn in self.start_hook_fns {
            start_hook_fn(ui, &command_helper)?;
        }
        (self.dispatch_fn)(ui, &command_helper)
    }

    #[must_use]
    #[instrument(skip(self))]
    pub fn run(mut self) -> ExitCode {
        // Tell crossterm to ignore NO_COLOR (we check it ourselves)
        crossterm::style::force_color_output(true);
        let config = config_from_environment(self.config_layers.drain(..));
        // Set up ui assuming the default config has no conditional variables.
        // If it had, the configuration will be fixed by the next ui.reset().
        let mut ui = Ui::with_config(config.as_ref())
            .expect("default config should be valid, env vars are stringly typed");
        let result = self.run_internal(&mut ui, config);
        let exit_code = handle_command_result(&mut ui, result);
        ui.finalize_pager();
        exit_code
    }
}

fn map_clap_cli_error(mut cmd_err: CommandError, ui: &Ui, config: &StackedConfig) -> CommandError {
    let Some(err) = cmd_err.error.downcast_ref::<clap::Error>() else {
        return cmd_err;
    };
    if let (Some(ContextValue::String(arg)), Some(ContextValue::String(value))) = (
        err.get(ContextKind::InvalidArg),
        err.get(ContextKind::InvalidValue),
    ) {
        if arg.as_str() == "--template <TEMPLATE>" && value.is_empty() {
            // Suppress the error, it's less important than the original error.
            if let Ok(template_aliases) = load_template_aliases(ui, config) {
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

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    use super::*;

    #[derive(clap::Parser, Clone, Debug)]
    pub struct TestArgs {
        #[arg(long)]
        pub foo: Vec<u32>,
        #[arg(long)]
        pub bar: Vec<u32>,
        #[arg(long)]
        pub baz: bool,
    }

    #[test]
    fn test_merge_args_with() {
        let command = TestArgs::command();
        let parse = |args: &[&str]| -> Vec<(&'static str, u32)> {
            let matches = command.clone().try_get_matches_from(args).unwrap();
            let args = TestArgs::from_arg_matches(&matches).unwrap();
            merge_args_with(
                &matches,
                &[("foo", &args.foo), ("bar", &args.bar)],
                |id, value| (id, *value),
            )
        };

        assert_eq!(parse(&["jj"]), vec![]);
        assert_eq!(parse(&["jj", "--foo=1"]), vec![("foo", 1)]);
        assert_eq!(
            parse(&["jj", "--foo=1", "--bar=2"]),
            vec![("foo", 1), ("bar", 2)]
        );
        assert_eq!(
            parse(&["jj", "--foo=1", "--baz", "--bar=2", "--foo", "3"]),
            vec![("foo", 1), ("bar", 2), ("foo", 3)]
        );
    }
}
