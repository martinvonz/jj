// Copyright 2022 Google LLC
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
use std::env::ArgsOs;
use std::ffi::OsString;
use std::fmt::Debug;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use clap;
use clap::{ArgMatches, FromArgMatches};
use git2::{Oid, Repository};
use itertools::Itertools;
use jujutsu_lib::backend::{BackendError, CommitId, TreeId};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::git::{GitExportError, GitImportError};
use jujutsu_lib::gitignore::GitIgnoreFile;
use jujutsu_lib::matchers::{EverythingMatcher, Matcher, PrefixMatcher, Visit};
use jujutsu_lib::op_heads_store::{OpHeadResolutionError, OpHeads, OpHeadsStore};
use jujutsu_lib::op_store::{OpStore, OpStoreError, OperationId, WorkspaceId};
use jujutsu_lib::operation::Operation;
use jujutsu_lib::repo::{BackendFactories, MutableRepo, ReadonlyRepo, RepoRef, RewriteRootCommit};
use jujutsu_lib::repo_path::{FsPathParseError, RepoPath};
use jujutsu_lib::revset::{
    Revset, RevsetError, RevsetExpression, RevsetParseError, RevsetWorkspaceContext,
};
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::transaction::Transaction;
use jujutsu_lib::tree::{Tree, TreeMergeError};
use jujutsu_lib::working_copy::{
    CheckoutStats, LockedWorkingCopy, ResetError, SnapshotError, WorkingCopy,
};
use jujutsu_lib::workspace::{Workspace, WorkspaceInitError, WorkspaceLoadError};
use jujutsu_lib::{dag_walk, file_util, git, revset};

use crate::config::read_config;
use crate::diff_edit::DiffEditError;
use crate::formatter::Formatter;
use crate::templater::TemplateFormatter;
use crate::ui::{ColorChoice, Ui};

#[derive(Debug)]
pub enum CommandError {
    UserError {
        message: String,
        hint: Option<String>,
    },
    ConfigError(String),
    /// Invalid command line
    CliError(String),
    /// Invalid command line detected by clap
    ClapCliError(clap::Error),
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

impl From<RewriteRootCommit> for CommandError {
    fn from(err: RewriteRootCommit) -> Self {
        user_error(err.to_string())
    }
}

impl From<BackendError> for CommandError {
    fn from(err: BackendError) -> Self {
        user_error(format!("Unexpected error from store: {err}"))
    }
}

impl From<WorkspaceInitError> for CommandError {
    fn from(_: WorkspaceInitError) -> Self {
        user_error("The target repo already exists")
    }
}

impl From<OpHeadResolutionError> for CommandError {
    fn from(err: OpHeadResolutionError) -> Self {
        match err {
            OpHeadResolutionError::NoHeads => {
                CommandError::InternalError("Corrupt repository: the are no operations".to_string())
            }
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

impl From<git2::Error> for CommandError {
    fn from(err: git2::Error) -> Self {
        user_error(format!("Git operation failed: {err}"))
    }
}

impl From<GitImportError> for CommandError {
    fn from(err: GitImportError) -> Self {
        CommandError::InternalError(format!(
            "Failed to import refs from underlying Git repo: {err}"
        ))
    }
}

impl From<GitExportError> for CommandError {
    fn from(err: GitExportError) -> Self {
        CommandError::InternalError(format!(
            "Failed to export refs to underlying Git repo: {err}"
        ))
    }
}

impl From<RevsetParseError> for CommandError {
    fn from(err: RevsetParseError) -> Self {
        user_error(format!("Failed to parse revset: {err}"))
    }
}

impl From<RevsetError> for CommandError {
    fn from(err: RevsetError) -> Self {
        user_error(format!("{err}"))
    }
}

impl From<FsPathParseError> for CommandError {
    fn from(err: FsPathParseError) -> Self {
        user_error(format!("{err}"))
    }
}

impl From<clap::Error> for CommandError {
    fn from(err: clap::Error) -> Self {
        CommandError::ClapCliError(err)
    }
}

pub struct CommandHelper {
    app: clap::Command,
    string_args: Vec<String>,
    global_args: GlobalArgs,
    backend_factories: BackendFactories,
}

impl CommandHelper {
    pub fn new(app: clap::Command, string_args: Vec<String>, global_args: GlobalArgs) -> Self {
        Self {
            app,
            string_args,
            global_args,
            backend_factories: BackendFactories::default(),
        }
    }

    pub fn app(&self) -> &clap::Command {
        &self.app
    }

    pub fn string_args(&self) -> &Vec<String> {
        &self.string_args
    }

    pub fn global_args(&self) -> &GlobalArgs {
        &self.global_args
    }

    pub fn set_backend_factories(&mut self, backend_factories: BackendFactories) {
        self.backend_factories = backend_factories;
    }

    pub fn workspace_helper(&self, ui: &mut Ui) -> Result<WorkspaceCommandHelper, CommandError> {
        let wc_path_str = self.global_args.repository.as_deref().unwrap_or(".");
        let wc_path = ui.cwd().join(wc_path_str);
        let workspace =
            Workspace::load(ui.settings(), &wc_path, &self.backend_factories).map_err(|err| {
                match err {
                    WorkspaceLoadError::NoWorkspaceHere(wc_path) => {
                        let message = format!("There is no jj repo in \"{}\"", wc_path_str);
                        let git_dir = wc_path.join(".git");
                        if git_dir.is_dir() {
                            user_error_with_hint(
                                message,
                                "It looks like this is a git repo. You can create a jj repo \
                                 backed by it by running this:
jj init --git-repo=.",
                            )
                        } else {
                            user_error(message)
                        }
                    }
                    WorkspaceLoadError::RepoDoesNotExist(repo_dir) => user_error(format!(
                        "The repository directory at {} is missing. Was it moved?",
                        repo_dir.to_str().unwrap()
                    )),
                    WorkspaceLoadError::Path(e) => user_error(format!("{}: {}", e, e.error)),
                    WorkspaceLoadError::NonUnicodePath => user_error(err.to_string()),
                }
            })?;
        let repo_loader = workspace.repo_loader();
        let op_heads = resolve_op_for_load(
            repo_loader.op_store(),
            repo_loader.op_heads_store(),
            &self.global_args.at_operation,
        )?;
        let mut workspace_command = match op_heads {
            OpHeads::Single(op) => {
                let repo = repo_loader.load_at(&op);
                self.for_loaded_repo(ui, workspace, repo)?
            }
            OpHeads::Unresolved {
                locked_op_heads,
                op_heads,
            } => {
                writeln!(
                    ui,
                    "Concurrent modification detected, resolving automatically.",
                )?;
                let base_repo = repo_loader.load_at(&op_heads[0]);
                // TODO: It may be helpful to print each operation we're merging here
                let mut workspace_command = self.for_loaded_repo(ui, workspace, base_repo)?;
                let mut tx = workspace_command.start_transaction("resolve concurrent operations");
                for other_op_head in op_heads.into_iter().skip(1) {
                    tx.merge_operation(other_op_head);
                    let num_rebased = tx.mut_repo().rebase_descendants(ui.settings())?;
                    if num_rebased > 0 {
                        writeln!(
                            ui,
                            "Rebased {} descendant commits onto commits rewritten by other \
                             operation",
                            num_rebased
                        )?;
                    }
                }
                let merged_repo = tx.write().leave_unpublished();
                locked_op_heads.finish(merged_repo.operation());
                workspace_command.repo = merged_repo;
                workspace_command
            }
        };
        workspace_command.snapshot(ui)?;
        Ok(workspace_command)
    }

    pub fn for_loaded_repo(
        &self,
        ui: &Ui,
        workspace: Workspace,
        repo: Arc<ReadonlyRepo>,
    ) -> Result<WorkspaceCommandHelper, CommandError> {
        WorkspaceCommandHelper::new(
            ui,
            workspace,
            self.string_args.clone(),
            &self.global_args,
            repo,
        )
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
    repo: Arc<ReadonlyRepo>,
    may_update_working_copy: bool,
    working_copy_shared_with_git: bool,
}

impl WorkspaceCommandHelper {
    pub fn new(
        ui: &Ui,
        workspace: Workspace,
        string_args: Vec<String>,
        global_args: &GlobalArgs,
        repo: Arc<ReadonlyRepo>,
    ) -> Result<Self, CommandError> {
        let loaded_at_head = &global_args.at_operation == "@";
        let may_update_working_copy = loaded_at_head && !global_args.no_commit_working_copy;
        let mut working_copy_shared_with_git = false;
        let maybe_git_repo = repo.store().git_repo();
        if let Some(git_workdir) = maybe_git_repo
            .as_ref()
            .and_then(|git_repo| git_repo.workdir())
            .and_then(|workdir| workdir.canonicalize().ok())
        {
            working_copy_shared_with_git = git_workdir == workspace.workspace_root().as_path();
        }
        Ok(Self {
            cwd: ui.cwd().to_owned(),
            string_args,
            global_args: global_args.clone(),
            settings: ui.settings().clone(),
            workspace,
            repo,
            may_update_working_copy,
            working_copy_shared_with_git,
        })
    }

    fn check_working_copy_writable(&self) -> Result<(), CommandError> {
        if self.may_update_working_copy {
            Ok(())
        } else {
            let hint = if self.global_args.no_commit_working_copy {
                "Don't use --no-commit-working-copy."
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
    pub fn snapshot(&mut self, ui: &mut Ui) -> Result<(), CommandError> {
        if self.may_update_working_copy {
            if self.working_copy_shared_with_git {
                let maybe_git_repo = self.repo.store().git_repo();
                self.import_git_refs_and_head(ui, maybe_git_repo.as_ref().unwrap())?;
            }
            self.commit_working_copy(ui)?;
        }
        Ok(())
    }

    fn import_git_refs_and_head(
        &mut self,
        ui: &mut Ui,
        git_repo: &Repository,
    ) -> Result<(), CommandError> {
        let mut tx = self.start_transaction("import git refs");
        git::import_refs(tx.mut_repo(), git_repo)?;
        if tx.mut_repo().has_changes() {
            let old_git_head = self.repo.view().git_head();
            let new_git_head = tx.mut_repo().view().git_head();
            // If the Git HEAD has changed, abandon our old checkout and check out the new
            // Git HEAD.
            if new_git_head != old_git_head && new_git_head.is_some() {
                let workspace_id = self.workspace_id();
                let mut locked_working_copy = self.workspace.working_copy_mut().start_mutation();
                if let Some(old_wc_commit_id) = self.repo.view().get_wc_commit_id(&workspace_id) {
                    tx.mut_repo()
                        .record_abandoned_commit(old_wc_commit_id.clone());
                }
                let new_checkout = self
                    .repo
                    .store()
                    .get_commit(new_git_head.as_ref().unwrap())?;
                tx.mut_repo()
                    .check_out(workspace_id, &self.settings, &new_checkout);
                // The working copy was presumably updated by the git command that updated HEAD,
                // so we just need to reset our working copy state to it without updating
                // working copy files.
                locked_working_copy.reset(&new_checkout.tree())?;
                tx.mut_repo().rebase_descendants(&self.settings)?;
                self.repo = tx.commit();
                locked_working_copy.finish(self.repo.op_id().clone());
            } else {
                let num_rebased = tx.mut_repo().rebase_descendants(ui.settings())?;
                if num_rebased > 0 {
                    writeln!(
                        ui,
                        "Rebased {} descendant commits off of commits rewritten from git",
                        num_rebased
                    )?;
                }
                self.finish_transaction(ui, tx)?;
            }
        }
        Ok(())
    }

    fn export_head_to_git(&self, mut_repo: &mut MutableRepo) -> Result<(), CommandError> {
        let git_repo = mut_repo.store().git_repo().unwrap();
        let current_git_head_ref = git_repo.find_reference("HEAD").unwrap();
        let current_git_commit_id = current_git_head_ref
            .peel_to_commit()
            .ok()
            .map(|commit| commit.id());
        if let Some(wc_commit_id) = mut_repo.view().get_wc_commit_id(&self.workspace_id()) {
            let first_parent_id = mut_repo
                .index()
                .entry_by_id(wc_commit_id)
                .unwrap()
                .parents()[0]
                .commit_id();
            if first_parent_id != *mut_repo.store().root_commit_id() {
                if let Some(current_git_commit_id) = current_git_commit_id {
                    git_repo.set_head_detached(current_git_commit_id)?;
                }
                let new_git_commit_id = Oid::from_bytes(first_parent_id.as_bytes()).unwrap();
                let new_git_commit = git_repo.find_commit(new_git_commit_id)?;
                git_repo.reset(new_git_commit.as_object(), git2::ResetType::Mixed, None)?;
                mut_repo.set_git_head(first_parent_id);
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
        &self.repo
    }

    pub fn working_copy(&self) -> &WorkingCopy {
        self.workspace.working_copy()
    }

    pub fn start_working_copy_mutation(
        &mut self,
    ) -> Result<(LockedWorkingCopy, Commit), CommandError> {
        self.check_working_copy_writable()?;
        let wc_commit_id = self.repo.view().get_wc_commit_id(&self.workspace_id());
        let wc_commit = if let Some(wc_commit_id) = wc_commit_id {
            self.repo.store().get_commit(wc_commit_id)?
        } else {
            return Err(user_error("Nothing checked out in this workspace"));
        };

        let locked_working_copy = self.workspace.working_copy_mut().start_mutation();
        if wc_commit.tree_id() != locked_working_copy.old_tree_id() {
            return Err(user_error("Concurrent working copy operation. Try again."));
        }

        Ok((locked_working_copy, wc_commit))
    }

    pub fn workspace_root(&self) -> &PathBuf {
        self.workspace.workspace_root()
    }

    pub fn workspace_id(&self) -> WorkspaceId {
        self.workspace.workspace_id().clone()
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
            let paths = values
                .iter()
                .map(|v| self.parse_file_path(v))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Box::new(PrefixMatcher::new(&paths)))
        }
    }

    pub fn git_config(&self) -> Result<git2::Config, git2::Error> {
        if let Some(git_repo) = self.repo.store().git_repo() {
            git_repo.config()
        } else {
            git2::Config::open_default()
        }
    }

    pub fn base_ignores(&self) -> Arc<GitIgnoreFile> {
        let mut git_ignores = GitIgnoreFile::empty();
        if let Ok(excludes_file_str) = self
            .git_config()
            .and_then(|git_config| git_config.get_string("core.excludesFile"))
        {
            let excludes_file_path = expand_git_path(excludes_file_str);
            git_ignores = git_ignores.chain_with_file("", excludes_file_path);
        }
        if let Some(git_repo) = self.repo.store().git_repo() {
            git_ignores =
                git_ignores.chain_with_file("", git_repo.path().join("info").join("exclude"));
        }
        git_ignores
    }

    pub fn resolve_single_op(&self, op_str: &str) -> Result<Operation, CommandError> {
        // When resolving the "@" operation in a `ReadonlyRepo`, we resolve it to the
        // operation the repo was loaded at.
        resolve_single_op(
            self.repo.op_store(),
            self.repo.op_heads_store(),
            self.repo.operation(),
            op_str,
        )
    }

    pub fn resolve_single_rev(&self, revision_str: &str) -> Result<Commit, CommandError> {
        let revset_expression = self.parse_revset(revision_str)?;
        let revset = self.evaluate_revset(&revset_expression)?;
        let mut iter = revset.iter().commits(self.repo.store());
        match iter.next() {
            None => Err(user_error(format!(
                "Revset \"{}\" didn't resolve to any revisions",
                revision_str
            ))),
            Some(commit) => {
                if iter.next().is_some() {
                    Err(user_error(format!(
                        "Revset \"{}\" resolved to more than one revision",
                        revision_str
                    )))
                } else {
                    Ok(commit?)
                }
            }
        }
    }

    pub fn resolve_revset(&self, revision_str: &str) -> Result<Vec<Commit>, CommandError> {
        let revset_expression = self.parse_revset(revision_str)?;
        let revset = self.evaluate_revset(&revset_expression)?;
        Ok(revset
            .iter()
            .commits(self.repo.store())
            .map(Result::unwrap)
            .collect())
    }

    pub fn parse_revset(
        &self,
        revision_str: &str,
    ) -> Result<Rc<RevsetExpression>, RevsetParseError> {
        let expression = revset::parse(revision_str, Some(&self.revset_context()))?;
        Ok(revset::optimize(expression))
    }

    pub fn evaluate_revset<'repo>(
        &'repo self,
        revset_expression: &RevsetExpression,
    ) -> Result<Box<dyn Revset<'repo> + 'repo>, RevsetError> {
        revset_expression.evaluate(self.repo.as_repo_ref(), Some(&self.revset_context()))
    }

    fn revset_context(&self) -> RevsetWorkspaceContext {
        RevsetWorkspaceContext {
            cwd: &self.cwd,
            workspace_id: self.workspace.workspace_id(),
            workspace_root: self.workspace.workspace_root(),
        }
    }

    pub fn check_rewriteable(&self, commit: &Commit) -> Result<(), CommandError> {
        if commit.id() == self.repo.store().root_commit_id() {
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

    pub fn commit_working_copy(&mut self, ui: &mut Ui) -> Result<(), CommandError> {
        let repo = self.repo.clone();
        let workspace_id = self.workspace_id();
        let wc_commit_id = match repo.view().get_wc_commit_id(&self.workspace_id()) {
            Some(wc_commit_id) => wc_commit_id.clone(),
            None => {
                // If the workspace has been deleted, it's unclear what to do, so we just skip
                // committing the working copy.
                return Ok(());
            }
        };
        let base_ignores = self.base_ignores();
        let mut locked_wc = self.workspace.working_copy_mut().start_mutation();
        // Check if the working copy commit matches the repo's view. It's fine if it
        // doesn't, but we'll need to reload the repo so the new commit is
        // in the index and view, and so we don't cause unnecessary
        // divergence.
        let wc_commit = repo.store().get_commit(&wc_commit_id)?;
        let wc_tree_id = locked_wc.old_tree_id().clone();
        if *wc_commit.tree_id() != wc_tree_id {
            let wc_operation_data = self
                .repo
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
                &|op: &Operation| op.parents(),
                &|op: &Operation| op.id().clone(),
            );
            if let Some(ancestor_op) = maybe_ancestor_op {
                if ancestor_op.id() == repo_operation.id() {
                    // The working copy was updated since we loaded the repo. We reload the repo
                    // at the working copy's operation.
                    self.repo = repo.reload_at(&wc_operation);
                } else if ancestor_op.id() == wc_operation.id() {
                    // The working copy was not updated when some repo operation committed,
                    // meaning that it's stale compared to the repo view. We update the working
                    // copy to what the view says.
                    writeln!(
                        ui,
                        "The working copy is stale (not updated since operation {}), now updating \
                         to operation {}",
                        short_operation_hash(wc_operation.id()),
                        short_operation_hash(repo_operation.id()),
                    )?;
                    locked_wc.check_out(&wc_commit.tree()).map_err(|err| {
                        CommandError::InternalError(format!(
                            "Failed to check out commit {}: {}",
                            wc_commit.id().hex(),
                            err
                        ))
                    })?;
                } else {
                    return Err(CommandError::InternalError(format!(
                        "The repo was loaded at operation {}, which seems to be a sibling of the \
                         working copy's operation {}",
                        short_operation_hash(repo_operation.id()),
                        short_operation_hash(wc_operation.id())
                    )));
                }
            } else {
                return Err(CommandError::InternalError(format!(
                    "The repo was loaded at operation {}, which seems unrelated to the working \
                     copy's operation {}",
                    short_operation_hash(repo_operation.id()),
                    short_operation_hash(wc_operation.id())
                )));
            }
        }
        let new_tree_id = locked_wc.snapshot(base_ignores)?;
        if new_tree_id != *wc_commit.tree_id() {
            let mut tx = self
                .repo
                .start_transaction(&self.settings, "commit working copy");
            let mut_repo = tx.mut_repo();
            let commit = CommitBuilder::for_rewrite_from(&self.settings, &wc_commit)
                .set_tree(new_tree_id)
                .write_to_repo(mut_repo);
            mut_repo
                .set_wc_commit(workspace_id, commit.id().clone())
                .unwrap();

            // Rebase descendants
            let num_rebased = mut_repo.rebase_descendants(&self.settings)?;
            if num_rebased > 0 {
                writeln!(
                    ui,
                    "Rebased {} descendant commits onto updated working copy",
                    num_rebased
                )?;
            }

            self.repo = tx.commit();
        }
        locked_wc.finish(self.repo.op_id().clone());
        Ok(())
    }

    pub fn edit_diff(
        &self,
        ui: &mut Ui,
        left_tree: &Tree,
        right_tree: &Tree,
        instructions: &str,
    ) -> Result<TreeId, DiffEditError> {
        crate::diff_edit::edit_diff(ui, left_tree, right_tree, instructions, self.base_ignores())
    }

    pub fn select_diff(
        &self,
        ui: &mut Ui,
        left_tree: &Tree,
        right_tree: &Tree,
        instructions: &str,
        interactive: bool,
        matcher: &dyn Matcher,
    ) -> Result<TreeId, CommandError> {
        if interactive {
            Ok(crate::diff_edit::edit_diff(
                ui,
                left_tree,
                right_tree,
                instructions,
                self.base_ignores(),
            )?)
        } else if matcher.visit(&RepoPath::root()) == Visit::AllRecursively {
            // Optimization for a common case
            Ok(right_tree.id().clone())
        } else {
            let mut tree_builder = self.repo().store().tree_builder(left_tree.id().clone());
            for (repo_path, diff) in left_tree.diff(right_tree, matcher) {
                match diff.into_options().1 {
                    Some(value) => {
                        tree_builder.set(repo_path, value);
                    }
                    None => {
                        tree_builder.remove(repo_path);
                    }
                }
            }
            Ok(tree_builder.write_tree())
        }
    }

    pub fn start_transaction(&self, description: &str) -> Transaction {
        let mut tx = self.repo.start_transaction(&self.settings, description);
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
        quoted_strings.extend(self.string_args.iter().skip(1).map(shell_escape));
        tx.set_tag("args".to_string(), quoted_strings.join(" "));
        tx
    }

    pub fn finish_transaction(
        &mut self,
        ui: &mut Ui,
        mut tx: Transaction,
    ) -> Result<(), CommandError> {
        let mut_repo = tx.mut_repo();
        let store = mut_repo.store().clone();
        if !mut_repo.has_changes() {
            writeln!(ui, "Nothing changed.")?;
            return Ok(());
        }
        let num_rebased = mut_repo.rebase_descendants(ui.settings())?;
        if num_rebased > 0 {
            writeln!(ui, "Rebased {} descendant commits", num_rebased)?;
        }
        if self.working_copy_shared_with_git {
            self.export_head_to_git(mut_repo)?;
            let git_repo = self.repo.store().git_repo().unwrap();
            git::export_refs(mut_repo, &git_repo)?;
        }
        let maybe_old_commit = tx
            .base_repo()
            .view()
            .get_wc_commit_id(&self.workspace_id())
            .map(|commit_id| store.get_commit(commit_id))
            .transpose()?;
        self.repo = tx.commit();
        if self.may_update_working_copy {
            let stats = update_working_copy(
                ui,
                &self.repo,
                &self.workspace_id(),
                self.workspace.working_copy_mut(),
                maybe_old_commit.as_ref(),
            )?;
            if let Some(stats) = stats {
                print_checkout_stats(ui, stats)?;
            }
        }
        let settings = ui.settings();
        if settings.user_name() == UserSettings::user_name_placeholder()
            || settings.user_email() == UserSettings::user_email_placeholder()
        {
            ui.write_warn(r#"Name and email not configured. Add something like the following to $HOME/.jjconfig.toml:
  user.name = "Some One"
  user.email = "someone@example.com""#)?;
        }
        Ok(())
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

/// Expands "~/" to "$HOME/" as Git seems to do for e.g. core.excludesFile.
fn expand_git_path(path_str: String) -> PathBuf {
    if let Some(remainder) = path_str.strip_prefix("~/") {
        if let Ok(home_dir_str) = std::env::var("HOME") {
            return PathBuf::from(home_dir_str).join(remainder);
        }
    }
    PathBuf::from(path_str)
}

fn resolve_op_for_load(
    op_store: &Arc<dyn OpStore>,
    op_heads_store: &Arc<OpHeadsStore>,
    op_str: &str,
) -> Result<OpHeads, CommandError> {
    if op_str == "@" {
        Ok(op_heads_store.get_heads(op_store)?)
    } else if op_str == "@-" {
        match op_heads_store.get_heads(op_store)? {
            OpHeads::Single(current_op) => {
                let resolved_op = resolve_single_op(op_store, op_heads_store, &current_op, op_str)?;
                Ok(OpHeads::Single(resolved_op))
            }
            OpHeads::Unresolved { .. } => Err(user_error(format!(
                r#"The "{op_str}" expression resolved to more than one operation"#
            ))),
        }
    } else {
        let operation = resolve_single_op_from_store(op_store, op_heads_store, op_str)?;
        Ok(OpHeads::Single(operation))
    }
}

fn resolve_single_op(
    op_store: &Arc<dyn OpStore>,
    op_heads_store: &Arc<OpHeadsStore>,
    current_op: &Operation,
    op_str: &str,
) -> Result<Operation, CommandError> {
    if op_str == "@" {
        Ok(current_op.clone())
    } else if op_str == "@-" {
        let parent_ops = current_op.parents();
        if parent_ops.len() != 1 {
            return Err(user_error(format!(
                r#"The "{op_str}" expression resolved to more than one operation"#
            )));
        }
        Ok(parent_ops[0].clone())
    } else {
        resolve_single_op_from_store(op_store, op_heads_store, op_str)
    }
}

fn find_all_operations(
    op_store: &Arc<dyn OpStore>,
    op_heads_store: &Arc<OpHeadsStore>,
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
    op_heads_store: &Arc<OpHeadsStore>,
    op_str: &str,
) -> Result<Operation, CommandError> {
    if op_str.is_empty() || !op_str.as_bytes().iter().all(|b| b.is_ascii_hexdigit()) {
        return Err(user_error(format!(
            "Operation ID \"{}\" is not a valid hexadecimal prefix",
            op_str
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
        Err(user_error(format!(
            "No operation ID matching \"{}\"",
            op_str
        )))
    } else if matches.len() == 1 {
        Ok(matches.pop().unwrap())
    } else {
        Err(user_error(format!(
            "Operation ID prefix \"{}\" is ambiguous",
            op_str
        )))
    }
}

pub fn resolve_base_revs(
    workspace_command: &WorkspaceCommandHelper,
    revisions: &[String],
) -> Result<Vec<Commit>, CommandError> {
    let mut commits = vec![];
    for revision_str in revisions {
        let commit = workspace_command.resolve_single_rev(revision_str)?;
        if let Some(i) = commits.iter().position(|c| c == &commit) {
            return Err(user_error(format!(
                r#"Revset "{}" and "{}" resolved to the same revision {}"#,
                revisions[i],
                revision_str,
                short_commit_hash(commit.id()),
            )));
        }
        commits.push(commit);
    }

    let root_commit_id = workspace_command.repo().store().root_commit_id();
    if commits.len() >= 2 && commits.iter().any(|c| c.id() == root_commit_id) {
        Err(user_error("Cannot merge with root revision"))
    } else {
        Ok(commits)
    }
}

fn update_working_copy(
    ui: &mut Ui,
    repo: &Arc<ReadonlyRepo>,
    workspace_id: &WorkspaceId,
    wc: &mut WorkingCopy,
    old_commit: Option<&Commit>,
) -> Result<Option<CheckoutStats>, CommandError> {
    let new_commit_id = match repo.view().get_wc_commit_id(workspace_id) {
        Some(new_commit_id) => new_commit_id,
        None => {
            // It seems the workspace was deleted, so we shouldn't try to update it.
            return Ok(None);
        }
    };
    let new_commit = repo.store().get_commit(new_commit_id)?;
    let old_tree_id = old_commit.map(|commit| commit.tree_id().clone());
    let stats = if Some(new_commit.tree_id()) != old_tree_id.as_ref() {
        // TODO: CheckoutError::ConcurrentCheckout should probably just result in a
        // warning for most commands (but be an error for the checkout command)
        let stats = wc
            .check_out(
                repo.op_id().clone(),
                old_tree_id.as_ref(),
                &new_commit.tree(),
            )
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
        let locked_wc = wc.start_mutation();
        locked_wc.finish(repo.op_id().clone());
        None
    };
    if Some(&new_commit) != old_commit {
        ui.write("Working copy now at: ")?;
        write_commit_summary(
            ui.stdout_formatter().as_mut(),
            repo.as_repo_ref(),
            workspace_id,
            &new_commit,
            ui.settings(),
        )?;
        ui.write("\n")?;
    }
    Ok(stats)
}

pub fn write_commit_summary(
    formatter: &mut dyn Formatter,
    repo: RepoRef,
    workspace_id: &WorkspaceId,
    commit: &Commit,
    settings: &UserSettings,
) -> std::io::Result<()> {
    let template_string = settings
        .config()
        .get_string("template.commit_summary")
        .unwrap_or_else(|_| String::from(r#"commit_id.short() " " description.first_line()"#));
    let template =
        crate::template_parser::parse_commit_template(repo, workspace_id, &template_string);
    let mut template_writer = TemplateFormatter::new(template, formatter);
    template_writer.format(commit)?;
    Ok(())
}

pub fn short_commit_description(commit: &Commit) -> String {
    let first_line = commit.description().split('\n').next().unwrap();
    format!("{} ({})", short_commit_hash(commit.id()), first_line)
}

pub fn short_commit_hash(commit_id: &CommitId) -> String {
    commit_id.hex()[0..12].to_string()
}

pub fn short_operation_hash(operation_id: &OperationId) -> String {
    operation_id.hex()[0..12].to_string()
}

/// Jujutsu (An experimental VCS)
///
/// To get started, see the tutorial at https://github.com/martinvonz/jj/blob/main/docs/tutorial.md.
#[derive(clap::Parser, Clone, Debug)]
#[command(
    name = "jj",
    author = "Martin von Zweigbergk <martinvonz@google.com>",
    version
)]
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
    /// Don't commit the working copy
    ///
    /// By default, Jujutsu commits the working copy on every command, unless
    /// you load the repo at a specific operation with `--at-operation`. If
    /// you want to avoid committing the working and instead see a possibly
    /// stale working copy commit, you can use `--no-commit-working-copy`.
    /// This may be useful e.g. in a command prompt, especially if you have
    /// another process that commits the working copy.
    #[arg(long, global = true, help_heading = "Global Options")]
    pub no_commit_working_copy: bool,
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
    /// When loading the repo at an earlier operation, the working copy will not
    /// be automatically committed.
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
    /// When to colorize output (always, never, auto)
    #[arg(
        long,
        value_name = "WHEN",
        global = true,
        help_heading = "Global Options"
    )]
    pub color: Option<ColorChoice>,
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
    /// Enable verbose logging
    #[arg(long, short = 'v', global = true, help_heading = "Global Options")]
    pub verbose: bool,
}

pub fn create_ui() -> (Ui, Result<(), CommandError>) {
    // TODO: We need to do some argument parsing here, at least for things like
    // --config, and for reading user configs from the repo pointed to by -R.
    match read_config() {
        Ok(user_settings) => (Ui::for_terminal(user_settings), Ok(())),
        Err(err) => {
            let ui = Ui::for_terminal(UserSettings::default());
            (ui, Err(CommandError::ConfigError(err.to_string())))
        }
    }
}

fn string_list_from_config(value: config::Value) -> Option<Vec<String>> {
    match value {
        config::Value {
            kind: config::ValueKind::Array(elements),
            ..
        } => {
            let mut strings = vec![];
            for arg in elements {
                match arg {
                    config::Value {
                        kind: config::ValueKind::String(string_value),
                        ..
                    } => {
                        strings.push(string_value);
                    }
                    _ => {
                        return None;
                    }
                }
            }
            Some(strings)
        }
        _ => None,
    }
}

fn resolve_aliases(
    user_settings: &UserSettings,
    app: &clap::Command,
    string_args: &[String],
) -> Result<Vec<String>, CommandError> {
    let mut resolved_aliases = HashSet::new();
    let mut string_args = string_args.to_vec();
    let mut real_commands = HashSet::new();
    for command in app.get_subcommands() {
        real_commands.insert(command.get_name().to_string());
        for alias in command.get_all_aliases() {
            real_commands.insert(alias.to_string());
        }
    }
    loop {
        let app_clone = app.clone().allow_external_subcommands(true);
        let matches = app_clone.try_get_matches_from(&string_args)?;
        if let Some((command_name, submatches)) = matches.subcommand() {
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
                match user_settings
                    .config()
                    .get::<config::Value>(&format!("alias.{}", alias_name))
                {
                    Ok(value) => {
                        if let Some(alias_definition) = string_list_from_config(value) {
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
                    }
                    Err(config::ConfigError::NotFound(_)) => {
                        // Not a real command and not an alias, so return what we've resolved so far
                        return Ok(string_args);
                    }
                    Err(err) => {
                        return Err(CommandError::from(err));
                    }
                }
            }
        }
        return Ok(string_args);
    }
}

pub fn parse_args(
    ui: &mut Ui,
    app: clap::Command,
    args_os: ArgsOs,
) -> Result<(CommandHelper, ArgMatches), CommandError> {
    let mut string_args: Vec<String> = vec![];
    for arg_os in args_os {
        if let Some(string_arg) = arg_os.to_str() {
            string_args.push(string_arg.to_owned());
        } else {
            return Err(CommandError::CliError("Non-utf8 argument".to_string()));
        }
    }

    let string_args = resolve_aliases(ui.settings(), &app, &string_args)?;
    let matches = app.clone().try_get_matches_from(&string_args)?;

    let mut args: Args = Args::from_arg_matches(&matches).unwrap();
    if let Some(choice) = args.global_args.color {
        args.global_args
            .config_toml
            .push(format!("ui.color=\"{}\"", choice.to_string()));
    }
    if !args.global_args.config_toml.is_empty() {
        ui.extra_toml_settings(&args.global_args.config_toml)?;
    }
    let command_helper = CommandHelper::new(app, string_args, args.global_args);
    Ok((command_helper, matches))
}

// TODO: Return std::process::ExitCode instead, once our MSRV is >= 1.61
#[must_use]
pub fn handle_command_result(ui: &mut Ui, result: Result<(), CommandError>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(CommandError::UserError { message, hint }) => {
            ui.write_error(&format!("Error: {}\n", message)).unwrap();
            if let Some(hint) = hint {
                ui.write_hint(&format!("Hint: {}\n", hint)).unwrap();
            }
            1
        }
        Err(CommandError::ConfigError(message)) => {
            ui.write_error(&format!("Config error: {}\n", message))
                .unwrap();
            1
        }
        Err(CommandError::CliError(message)) => {
            ui.write_error(&format!("Error: {}\n", message)).unwrap();
            2
        }
        Err(CommandError::ClapCliError(inner)) => {
            let clap_str = if ui.color() {
                inner.render().ansi().to_string()
            } else {
                inner.render().to_string()
            };

            // Definitions for exit codes and streams come from
            // https://github.com/clap-rs/clap/blob/master/src/error/mod.rs
            match inner.kind() {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                    ui.write(&clap_str).unwrap();
                    0
                }
                _ => {
                    ui.write_stderr(&clap_str).unwrap();
                    2
                }
            }
        }
        Err(CommandError::BrokenPipe) => 3,
        Err(CommandError::InternalError(message)) => {
            ui.write_error(&format!("Internal error: {}\n", message))
                .unwrap();
            255
        }
    }
}
