// Copyright 2020 Google LLC
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

extern crate chrono;
extern crate clap;
extern crate clap_mangen;
extern crate config;

use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::fmt::Debug;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use std::{fs, io};

use clap::{ArgGroup, CommandFactory, Subcommand};
use config::Value;
use criterion::Criterion;
use git2::{Oid, Repository};
use itertools::Itertools;
use jujutsu_lib::backend::{BackendError, CommitId, Timestamp, TreeId, TreeValue};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::dag_walk::topo_order_reverse;
use jujutsu_lib::diff::{Diff, DiffHunk};
use jujutsu_lib::files::DiffLine;
use jujutsu_lib::git::{GitExportError, GitFetchError, GitImportError, GitRefUpdate};
use jujutsu_lib::gitignore::GitIgnoreFile;
use jujutsu_lib::index::{HexPrefix, IndexEntry};
use jujutsu_lib::matchers::{EverythingMatcher, Matcher, PrefixMatcher, Visit};
use jujutsu_lib::op_heads_store::{OpHeadResolutionError, OpHeads, OpHeadsStore};
use jujutsu_lib::op_store::{OpStore, OpStoreError, OperationId, RefTarget, WorkspaceId};
use jujutsu_lib::operation::Operation;
use jujutsu_lib::refs::{classify_branch_push_action, BranchPushAction, BranchPushUpdate};
use jujutsu_lib::repo::{MutableRepo, ReadonlyRepo, RepoRef};
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::revset::{RevsetError, RevsetExpression, RevsetParseError};
use jujutsu_lib::revset_graph_iterator::{RevsetGraphEdge, RevsetGraphEdgeType};
use jujutsu_lib::rewrite::{back_out_commit, merge_commit_trees, rebase_commit, DescendantRebaser};
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::store::Store;
use jujutsu_lib::transaction::Transaction;
use jujutsu_lib::tree::{merge_trees, Tree, TreeDiffIterator, TreeMergeError};
use jujutsu_lib::view::View;
use jujutsu_lib::working_copy::{
    CheckoutStats, LockedWorkingCopy, ResetError, SnapshotError, WorkingCopy,
};
use jujutsu_lib::workspace::{Workspace, WorkspaceInitError, WorkspaceLoadError};
use jujutsu_lib::{conflicts, dag_walk, diff, files, git, revset, tree};
use maplit::{hashmap, hashset};
use pest::Parser;

use self::chrono::{FixedOffset, TimeZone, Utc};
use crate::commands::CommandError::UserError;
use crate::diff_edit::DiffEditError;
use crate::formatter::Formatter;
use crate::graphlog::{AsciiGraphDrawer, Edge};
use crate::template_parser::TemplateParser;
use crate::templater::Template;
use crate::ui;
use crate::ui::{ColorChoice, FilePathParseError, Ui};

pub enum CommandError {
    UserError(String),
    /// Invalid command line
    CliError(String),
    BrokenPipe,
    InternalError(String),
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
        CommandError::UserError(format!("Config error: {err}"))
    }
}

impl From<BackendError> for CommandError {
    fn from(err: BackendError) -> Self {
        CommandError::UserError(format!("Unexpected error from store: {err}"))
    }
}

impl From<WorkspaceInitError> for CommandError {
    fn from(_: WorkspaceInitError) -> Self {
        CommandError::UserError("The target repo already exists".to_string())
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
        CommandError::UserError(format!("Failed to edit diff: {err}"))
    }
}

impl From<git2::Error> for CommandError {
    fn from(err: git2::Error) -> Self {
        CommandError::UserError(format!("Git operation failed: {err}"))
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
        match err {
            GitExportError::ConflictedBranch(branch_name) => {
                CommandError::UserError(format!("Cannot export conflicted branch '{branch_name}'"))
            }
            GitExportError::InternalGitError(err) => CommandError::InternalError(format!(
                "Failed to export refs to underlying Git repo: {err}"
            )),
        }
    }
}

impl From<RevsetParseError> for CommandError {
    fn from(err: RevsetParseError) -> Self {
        CommandError::UserError(format!("Failed to parse revset: {err}"))
    }
}

impl From<RevsetError> for CommandError {
    fn from(err: RevsetError) -> Self {
        CommandError::UserError(format!("{err}"))
    }
}

impl From<FilePathParseError> for CommandError {
    fn from(err: FilePathParseError) -> Self {
        match err {
            FilePathParseError::InputNotInRepo(input) => {
                CommandError::UserError(format!("Path \"{input}\" is not in the repo"))
            }
        }
    }
}

struct CommandHelper<'help> {
    app: clap::Command<'help>,
    string_args: Vec<String>,
    global_args: GlobalArgs,
}

impl<'help> CommandHelper<'help> {
    fn new(app: clap::Command<'help>, string_args: Vec<String>, global_args: GlobalArgs) -> Self {
        Self {
            app,
            string_args,
            global_args,
        }
    }

    fn global_args(&self) -> &GlobalArgs {
        &self.global_args
    }

    fn workspace_helper(&self, ui: &mut Ui) -> Result<WorkspaceCommandHelper, CommandError> {
        let wc_path_str = self.global_args.repository.as_deref().unwrap_or(".");
        let wc_path = ui.cwd().join(wc_path_str);
        let workspace = match Workspace::load(ui.settings(), wc_path) {
            Ok(workspace) => workspace,
            Err(WorkspaceLoadError::NoWorkspaceHere(wc_path)) => {
                let mut message = format!("There is no jj repo in \"{}\"", wc_path_str);
                let git_dir = wc_path.join(".git");
                if git_dir.is_dir() {
                    // TODO: Make this hint separate from the error, so the caller can format
                    // it differently.
                    message += "
It looks like this is a git repo. You can create a jj repo backed by it by running this:
jj init --git-repo=.";
                }
                return Err(CommandError::UserError(message));
            }
            Err(WorkspaceLoadError::RepoDoesNotExist(repo_dir)) => {
                return Err(CommandError::UserError(format!(
                    "The repository directory at {} is missing. Was it moved?",
                    repo_dir.to_str().unwrap()
                )));
            }
        };
        let repo_loader = workspace.repo_loader();
        let op_heads = resolve_op_for_load(
            repo_loader.op_store(),
            repo_loader.op_heads_store(),
            &self.global_args.at_operation,
        )?;
        let repo = match op_heads {
            OpHeads::Single(op) => repo_loader.load_at(&op),
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
                return Ok(workspace_command);
            }
        };
        self.for_loaded_repo(ui, workspace, repo)
    }

    fn for_loaded_repo(
        &self,
        ui: &mut Ui,
        workspace: Workspace,
        repo: Arc<ReadonlyRepo>,
    ) -> Result<WorkspaceCommandHelper, CommandError> {
        WorkspaceCommandHelper::for_loaded_repo(
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
struct WorkspaceCommandHelper {
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
    fn for_loaded_repo(
        ui: &mut Ui,
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
        let mut helper = Self {
            cwd: ui.cwd().to_owned(),
            string_args,
            global_args: global_args.clone(),
            settings: ui.settings().clone(),
            workspace,
            repo,
            may_update_working_copy,
            working_copy_shared_with_git,
        };
        if may_update_working_copy {
            if working_copy_shared_with_git {
                helper.import_git_refs_and_head(ui, maybe_git_repo.as_ref().unwrap())?;
            }
            helper.commit_working_copy(ui)?;
        }
        Ok(helper)
    }

    fn check_working_copy_writable(&self) -> Result<(), CommandError> {
        if self.may_update_working_copy {
            Ok(())
        } else if self.global_args.no_commit_working_copy {
            Err(CommandError::UserError(
                "This command must be able to update the working copy (don't use \
                 --no-commit-working-copy)."
                    .to_string(),
            ))
        } else {
            Err(CommandError::UserError(
                "This command must be able to update the working copy (don't use --at-op)."
                    .to_string(),
            ))
        }
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
                let workspace_id = self.workspace.workspace_id();
                let mut locked_working_copy = self.workspace.working_copy_mut().start_mutation();
                if let Some(old_checkout) = self.repo.view().get_checkout(&workspace_id) {
                    tx.mut_repo().record_abandoned_commit(old_checkout.clone());
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
        if let Some(checkout_id) = mut_repo.view().get_checkout(&self.workspace_id()) {
            let first_parent_id =
                mut_repo.index().entry_by_id(checkout_id).unwrap().parents()[0].commit_id();
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

    fn repo(&self) -> &Arc<ReadonlyRepo> {
        &self.repo
    }

    fn repo_mut(&mut self) -> &mut Arc<ReadonlyRepo> {
        &mut self.repo
    }

    fn working_copy(&self) -> &WorkingCopy {
        self.workspace.working_copy()
    }

    fn start_working_copy_mutation(&mut self) -> Result<(LockedWorkingCopy, Commit), CommandError> {
        self.check_working_copy_writable()?;
        let current_checkout_id = self.repo.view().get_checkout(&self.workspace_id());
        let current_checkout = if let Some(current_checkout_id) = current_checkout_id {
            self.repo.store().get_commit(current_checkout_id)?
        } else {
            return Err(CommandError::UserError(
                "Nothing checked out in this workspace".to_string(),
            ));
        };

        let locked_working_copy = self.workspace.working_copy_mut().start_mutation();
        if current_checkout.tree_id() != locked_working_copy.old_tree_id() {
            return Err(CommandError::UserError(
                "Concurrent working copy operation. Try again.".to_string(),
            ));
        }

        Ok((locked_working_copy, current_checkout))
    }

    fn workspace_root(&self) -> &PathBuf {
        self.workspace.workspace_root()
    }

    fn workspace_id(&self) -> WorkspaceId {
        self.workspace.workspace_id()
    }

    fn working_copy_shared_with_git(&self) -> bool {
        self.working_copy_shared_with_git
    }

    fn format_file_path(&self, file: &RepoPath) -> String {
        ui::relative_path(&self.cwd, &file.to_fs_path(self.workspace_root()))
            .to_str()
            .unwrap()
            .to_owned()
    }

    fn git_config(&self) -> Result<git2::Config, git2::Error> {
        if let Some(git_repo) = self.repo.store().git_repo() {
            git_repo.config()
        } else {
            git2::Config::open_default()
        }
    }

    fn base_ignores(&self) -> Arc<GitIgnoreFile> {
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

    fn resolve_single_op(&self, op_str: &str) -> Result<Operation, CommandError> {
        // When resolving the "@" operation in a `ReadonlyRepo`, we resolve it to the
        // operation the repo was loaded at.
        resolve_single_op(
            self.repo.op_store(),
            self.repo.op_heads_store(),
            self.repo.operation(),
            op_str,
        )
    }

    fn resolve_single_rev(&self, revision_str: &str) -> Result<Commit, CommandError> {
        let revset_expression = revset::parse(revision_str)?;
        let revset =
            revset_expression.evaluate(self.repo.as_repo_ref(), Some(&self.workspace_id()))?;
        let mut iter = revset.iter().commits(self.repo.store());
        match iter.next() {
            None => Err(CommandError::UserError(format!(
                "Revset \"{}\" didn't resolve to any revisions",
                revision_str
            ))),
            Some(commit) => {
                if iter.next().is_some() {
                    return Err(CommandError::UserError(format!(
                        "Revset \"{}\" resolved to more than one revision",
                        revision_str
                    )));
                } else {
                    Ok(commit?)
                }
            }
        }
    }

    fn resolve_revset(&self, revision_str: &str) -> Result<Vec<Commit>, CommandError> {
        let revset_expression = revset::parse(revision_str)?;
        let revset =
            revset_expression.evaluate(self.repo.as_repo_ref(), Some(&self.workspace_id()))?;
        Ok(revset
            .iter()
            .commits(self.repo.store())
            .map(Result::unwrap)
            .collect())
    }

    fn check_rewriteable(&self, commit: &Commit) -> Result<(), CommandError> {
        if commit.id() == self.repo.store().root_commit_id() {
            return Err(CommandError::UserError(
                "Cannot rewrite the root commit".to_string(),
            ));
        }
        Ok(())
    }

    fn check_non_empty(&self, commits: &[Commit]) -> Result<(), CommandError> {
        if commits.is_empty() {
            return Err(CommandError::UserError("Empty revision set".to_string()));
        }
        Ok(())
    }

    fn commit_working_copy(&mut self, ui: &mut Ui) -> Result<(), CommandError> {
        let repo = self.repo.clone();
        let workspace_id = self.workspace_id();
        let checkout_id = match repo.view().get_checkout(&self.workspace_id()) {
            Some(checkout_id) => checkout_id.clone(),
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
        let checkout_commit = repo.store().get_commit(&checkout_id)?;
        let wc_tree_id = locked_wc.old_tree_id().clone();
        if *checkout_commit.tree_id() != wc_tree_id {
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
                        wc_operation.id().hex(),
                        repo_operation.id().hex()
                    )?;
                    locked_wc.check_out(&checkout_commit.tree()).unwrap();
                } else {
                    return Err(CommandError::InternalError(format!(
                        "The repo was loaded at operation {}, which seems to be a sibling of the \
                         working copy's operation {}",
                        repo_operation.id().hex(),
                        wc_operation.id().hex()
                    )));
                }
            } else {
                return Err(CommandError::InternalError(format!(
                    "The repo was loaded at operation {}, which seems unrelated to the working \
                     copy's operation {}",
                    repo_operation.id().hex(),
                    wc_operation.id().hex()
                )));
            }
        }
        let new_tree_id = locked_wc.snapshot(base_ignores)?;
        if new_tree_id != *checkout_commit.tree_id() {
            let mut tx = self.repo.start_transaction("commit working copy");
            let mut_repo = tx.mut_repo();
            let commit = CommitBuilder::for_rewrite_from(&self.settings, &checkout_commit)
                .set_tree(new_tree_id)
                .write_to_repo(mut_repo);
            mut_repo.set_checkout(workspace_id, commit.id().clone());

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
            locked_wc.finish(self.repo.op_id().clone());
        } else {
            locked_wc.discard();
        }
        Ok(())
    }

    fn edit_diff(
        &self,
        ui: &mut Ui,
        left_tree: &Tree,
        right_tree: &Tree,
        instructions: &str,
    ) -> Result<TreeId, DiffEditError> {
        crate::diff_edit::edit_diff(
            ui,
            &self.settings,
            left_tree,
            right_tree,
            instructions,
            self.base_ignores(),
        )
    }

    fn select_diff(
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
                &self.settings,
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

    fn start_transaction(&self, description: &str) -> Transaction {
        let mut tx = self.repo.start_transaction(description);
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
        let quoted_strings = self.string_args.iter().map(shell_escape).collect_vec();
        tx.set_tag("args".to_string(), quoted_strings.join(" "));
        tx
    }

    fn finish_transaction(&mut self, ui: &mut Ui, mut tx: Transaction) -> Result<(), CommandError> {
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
        }
        let maybe_old_commit = tx
            .base_repo()
            .view()
            .get_checkout(&self.workspace_id())
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
        if self.working_copy_shared_with_git {
            let git_repo = self.repo.store().git_repo().unwrap();
            git::export_refs(&self.repo, &git_repo)?;
        }
        Ok(())
    }
}

fn print_checkout_stats(ui: &mut Ui, stats: CheckoutStats) -> Result<(), std::io::Error> {
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
            OpHeads::Unresolved { .. } => Err(UserError(format!(
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
            return Err(UserError(format!(
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
        return Err(CommandError::UserError(format!(
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
        Err(CommandError::UserError(format!(
            "No operation ID matching \"{}\"",
            op_str
        )))
    } else if matches.len() == 1 {
        Ok(matches.pop().unwrap())
    } else {
        Err(CommandError::UserError(format!(
            "Operation ID prefix \"{}\" is ambiguous",
            op_str
        )))
    }
}

fn repo_paths_from_values(
    ui: &Ui,
    wc_path: &Path,
    values: &[String],
) -> Result<Vec<RepoPath>, CommandError> {
    if !values.is_empty() {
        // TODO: Add support for globs and other formats
        let mut paths = vec![];
        for value in values {
            let repo_path = ui.parse_file_path(wc_path, value)?;
            paths.push(repo_path);
        }
        Ok(paths)
    } else {
        Ok(vec![])
    }
}

fn matcher_from_values(
    ui: &Ui,
    wc_path: &Path,
    values: &[String],
) -> Result<Box<dyn Matcher>, CommandError> {
    let paths = repo_paths_from_values(ui, wc_path, values)?;
    if paths.is_empty() {
        Ok(Box::new(EverythingMatcher))
    } else {
        Ok(Box::new(PrefixMatcher::new(&paths)))
    }
}

fn update_working_copy(
    ui: &mut Ui,
    repo: &Arc<ReadonlyRepo>,
    workspace_id: &WorkspaceId,
    wc: &mut WorkingCopy,
    old_commit: Option<&Commit>,
) -> Result<Option<CheckoutStats>, CommandError> {
    let new_commit_id = match repo.view().get_checkout(workspace_id) {
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
        None
    };
    if Some(&new_commit) != old_commit {
        ui.write("Working copy now at: ")?;
        ui.write_commit_summary(repo.as_repo_ref(), workspace_id, &new_commit)?;
        ui.write("\n")?;
    }
    Ok(stats)
}

/// Jujutsu (An experimental VCS)
///
/// To get started, see the tutorial at https://github.com/martinvonz/jj/blob/main/docs/tutorial.md.
#[derive(clap::Parser, Clone, Debug)]
#[clap(
    name = "jj",
    author = "Martin von Zweigbergk <martinvonz@google.com>",
    version
)]
#[clap(mut_arg("help", |arg| {
    arg
        .help("Print help information, more help with --help than with -h")
        .help_heading("GLOBAL OPTIONS")
    }))]
struct Args {
    #[clap(flatten)]
    global_args: GlobalArgs,
    #[clap(subcommand)]
    command: Commands,
}

#[derive(clap::Args, Clone, Debug)]
struct GlobalArgs {
    /// Path to repository to operate on
    ///
    /// By default, Jujutsu searches for the closest .jj/ directory in an
    /// ancestor of the current working directory.
    #[clap(long, short = 'R', global = true, help_heading = "GLOBAL OPTIONS")]
    repository: Option<String>,
    /// Don't commit the working copy
    ///
    /// By default, Jujutsu commits the working copy on every command, unless
    /// you load the repo at a specific operation with `--at-operation`. If
    /// you want to avoid committing the working and instead see a possibly
    /// stale working copy commit, you can use `--no-commit-working-copy`.
    /// This may be useful e.g. in a command prompt, especially if you have
    /// another process that commits the working copy.
    #[clap(long, global = true, help_heading = "GLOBAL OPTIONS")]
    no_commit_working_copy: bool,
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
    #[clap(
        long,
        visible_alias = "at-op",
        global = true,
        help_heading = "GLOBAL OPTIONS",
        default_value = "@"
    )]
    at_operation: String,
    /// When to colorize output (always, never, auto)
    #[clap(
        long,
        value_name = "WHEN",
        global = true,
        help_heading = "GLOBAL OPTIONS"
    )]
    color: Option<ColorChoice>,
}

#[derive(Subcommand, Clone, Debug)]
enum Commands {
    Init(InitArgs),
    Checkout(CheckoutArgs),
    Untrack(UntrackArgs),
    Files(FilesArgs),
    Print(PrintArgs),
    Diff(DiffArgs),
    Show(ShowArgs),
    Status(StatusArgs),
    Log(LogArgs),
    Obslog(ObslogArgs),
    Describe(DescribeArgs),
    Close(CloseArgs),
    Open(OpenArgs),
    Duplicate(DuplicateArgs),
    Abandon(AbandonArgs),
    Edit(EditArgs),
    New(NewArgs),
    Move(MoveArgs),
    Squash(SquashArgs),
    Unsquash(UnsquashArgs),
    Restore(RestoreArgs),
    Touchup(TouchupArgs),
    Split(SplitArgs),
    Merge(MergeArgs),
    Rebase(RebaseArgs),
    Backout(BackoutArgs),
    #[clap(subcommand)]
    Branch(BranchSubcommand),
    /// Undo an operation (shortcut for `jj op undo`)
    Undo(OperationUndoArgs),
    #[clap(subcommand)]
    #[clap(visible_alias = "op")]
    Operation(OperationCommands),
    #[clap(subcommand)]
    Workspace(WorkspaceCommands),
    Sparse(SparseArgs),
    #[clap(subcommand)]
    Git(GitCommands),
    #[clap(subcommand)]
    Bench(BenchCommands),
    #[clap(subcommand)]
    Debug(DebugCommands),
    /// An alias or an unknown command
    #[clap(external_subcommand)]
    Alias(Vec<String>),
}

/// Create a new repo in the given directory
///
/// If the given directory does not exist, it will be created. If no directory
/// is given, the current directory is used.
#[derive(clap::Args, Clone, Debug)]
#[clap(group(ArgGroup::new("backend").args(&["git", "git-repo"])))]
struct InitArgs {
    /// The destination directory
    #[clap(default_value = ".")]
    destination: String,
    /// Use the Git backend, creating a jj repo backed by a Git repo
    #[clap(long)]
    git: bool,
    /// Path to a git repo the jj repo will be backed by
    #[clap(long)]
    git_repo: Option<String>,
}

/// Update the working copy to another revision
///
/// If the revision is closed or has conflicts, then a new, open revision will
/// be created on top, and that will be checked out. For more information, see
/// https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
#[clap(visible_aliases = &["co", "update", "up"])]
struct CheckoutArgs {
    /// The revision to update to
    revision: String,
}

/// Stop tracking specified paths in the working copy
#[derive(clap::Args, Clone, Debug)]
struct UntrackArgs {
    /// Paths to untrack
    #[clap(required = true)]
    paths: Vec<String>,
}

/// List files in a revision
#[derive(clap::Args, Clone, Debug)]
struct FilesArgs {
    /// The revision to list files in
    #[clap(long, short, default_value = "@")]
    revision: String,
    /// Only list files matching these prefixes (instead of all files)
    paths: Vec<String>,
}

/// Print contents of a file in a revision
#[derive(clap::Args, Clone, Debug)]
struct PrintArgs {
    /// The revision to get the file contents from
    #[clap(long, short, default_value = "@")]
    revision: String,
    /// The file to print
    path: String,
}

#[derive(clap::Args, Clone, Debug)]
#[clap(group(ArgGroup::new("format").args(&["summary", "git", "color-words"])))]
struct DiffFormatArgs {
    /// For each path, show only whether it was modified, added, or removed
    #[clap(long, short)]
    summary: bool,
    /// Show a Git-format diff
    #[clap(long)]
    git: bool,
    /// Show a word-level diff with changes indicated only by color
    #[clap(long)]
    color_words: bool,
}

/// Show changes in a revision
///
/// With the `-r` option, which is the default, shows the changes compared to
/// the parent revision. If there are several parent revisions (i.e., the given
/// revision is a merge), then they will be merged and the changes from the
/// result to the given revision will be shown.
///
/// With the `--from` and/or `--to` options, shows the difference from/to the
/// given revisions. If either is left out, it defaults to the current checkout.
/// For example, `jj diff --from main` shows the changes from "main" (perhaps a
/// branch name) to the current checkout.
#[derive(clap::Args, Clone, Debug)]
struct DiffArgs {
    /// Show changes changes in this revision, compared to its parent(s)
    #[clap(long, short)]
    revision: Option<String>,
    /// Show changes from this revision
    #[clap(long, conflicts_with = "revision")]
    from: Option<String>,
    /// Show changes to this revision
    #[clap(long, conflicts_with = "revision")]
    to: Option<String>,
    /// Restrict the diff to these paths
    paths: Vec<String>,
    #[clap(flatten)]
    format: DiffFormatArgs,
}

/// Show commit description and changes in a revision
#[derive(clap::Args, Clone, Debug)]
struct ShowArgs {
    /// Show changes changes in this revision, compared to its parent(s)
    #[clap(default_value = "@")]
    revision: String,
    #[clap(flatten)]
    format: DiffFormatArgs,
}

/// Show high-level repo status
///
/// This includes:
///
///  * The working copy commit and its (first) parent, and a summary of the
///    changes between them
///
///  * Conflicted branches (see https://github.com/martinvonz/jj/blob/main/docs/branches.md)
#[derive(clap::Args, Clone, Debug)]
#[clap(visible_alias = "st")]
struct StatusArgs {}

/// Show commit history
#[derive(clap::Args, Clone, Debug)]
struct LogArgs {
    /// Which revisions to show
    #[clap(
        long,
        short,
        default_value = "remote_branches().. | (remote_branches()..)-"
    )]
    revisions: String,
    /// Show revisions in the opposite order (older revisions first)
    #[clap(long)]
    reversed: bool,
    /// Don't show the graph, show a flat list of revisions
    #[clap(long)]
    no_graph: bool,
    /// Render each revision using the given template (the syntax is not yet
    /// documented and is likely to change)
    #[clap(long, short = 'T')]
    template: Option<String>,
    /// Show patch
    #[clap(long, short = 'p')]
    patch: bool,
    #[clap(flatten)]
    diff_format: DiffFormatArgs,
}

/// Show how a change has evolved
///
/// Show how a change has evolved as it's been updated, rebased, etc.
#[derive(clap::Args, Clone, Debug)]
struct ObslogArgs {
    #[clap(long, short, default_value = "@")]
    revision: String,
    /// Don't show the graph, show a flat list of revisions
    #[clap(long)]
    no_graph: bool,
    /// Render each revision using the given template (the syntax is not yet
    /// documented and is likely to change)
    #[clap(long, short = 'T')]
    template: Option<String>,
    /// Show patch compared to the previous version of this change
    ///
    /// If the previous version has different parents, it will be temporarily
    /// rebased to the parents of the new version, so the diff is not
    /// contaminated by unrelated changes.
    #[clap(long, short = 'p')]
    patch: bool,
    #[clap(flatten)]
    diff_format: DiffFormatArgs,
}

/// Edit the change description
///
/// Starts an editor to let you edit the description of a change. The editor
/// will be $EDITOR, or `pico` if that's not defined.
#[derive(clap::Args, Clone, Debug)]
struct DescribeArgs {
    /// The revision whose description to edit
    #[clap(default_value = "@")]
    revision: String,
    /// The change description to use (don't open editor)
    #[clap(long, short)]
    message: Option<String>,
    /// Read the change description from stdin
    #[clap(long)]
    stdin: bool,
}

/// Mark a revision closed
///
/// For information about open/closed revisions, see
/// https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
#[clap(visible_alias = "commit")]
struct CloseArgs {
    /// The revision to close
    #[clap(default_value = "@")]
    revision: String,
    /// The change description to use (don't open editor)
    #[clap(long, short)]
    message: Option<String>,
    /// Also edit the description
    #[clap(long, short)]
    edit: bool,
}

/// Mark a revision open
///
/// For information about open/closed revisions,
/// see https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
#[clap(visible_alias = "uncommit")]
struct OpenArgs {
    /// The revision to open
    revision: String,
}

/// Create a new change with the same content as an existing one
///
/// For information about open/closed revisions, see
/// https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
struct DuplicateArgs {
    /// The revision to duplicate
    #[clap(default_value = "@")]
    revision: String,
}

/// Abandon a revision
///
/// Abandon a revision, rebasing descendants onto its parent(s). The behavior is
/// similar to `jj restore`; the difference is that `jj abandon` gives you a new
/// change, while `jj restore` updates the existing change.
#[derive(clap::Args, Clone, Debug)]
struct AbandonArgs {
    /// The revision(s) to abandon
    #[clap(default_value = "@")]
    revisions: Vec<String>,
}

/// Edit a commit in the working copy
///
/// Puts the contents of a commit in the working copy for editing. Any changes
/// you make in the working copy will update (amend) the commit.
#[derive(clap::Args, Clone, Debug)]
struct EditArgs {
    /// The commit to edit
    revision: String,
}

/// Create a new, empty change and check it out
///
/// This may be useful if you want to make some changes you're unsure of on top
/// of the working copy. If the changes turned out to be useful, you can `jj
/// squash` them into the previous working copy. If they turned out to be
/// unsuccessful, you can `jj abandon` them.
#[derive(clap::Args, Clone, Debug)]
struct NewArgs {
    /// Parent of the new change
    #[clap(default_value = "@")]
    revision: String,
    /// The change description to use
    #[clap(long, short, default_value = "")]
    message: String,
}

/// Move changes from one revision into another
///
/// Use `--interactive` to move only part of the source revision into the
/// destination. The selected changes (or all the changes in the source revision
/// if not using `--interactive`) will be moved into the destination. The
/// changes will be removed from the source. If that means that the source is
/// now empty compared to its parent, it will be abandoned unless it's checked
/// out. Without `--interactive`, the source change will always be empty.
#[derive(clap::Args, Clone, Debug)]
#[clap(group(ArgGroup::new("to_move").args(&["from", "to"]).multiple(true).required(true)))]
struct MoveArgs {
    /// Move part of this change into the destination
    #[clap(long)]
    from: Option<String>,
    /// Move part of the source into this change
    #[clap(long)]
    to: Option<String>,
    /// Interactively choose which parts to move
    #[clap(long, short)]
    interactive: bool,
    /// Move only changes to these paths (instead of all paths)
    #[clap(conflicts_with = "interactive")]
    paths: Vec<String>,
}

/// Move changes from a revision into its parent
///
/// After moving the changes into the parent, the child revision will have the
/// same content state as before. If that means that the change is now empty
/// compared to its parent, it will be abandoned unless it's checked out.
/// Without `--interactive`, the child change will always be empty.
#[derive(clap::Args, Clone, Debug)]
#[clap(visible_alias = "amend")]
struct SquashArgs {
    #[clap(long, short, default_value = "@")]
    revision: String,
    /// Interactively choose which parts to squash
    #[clap(long, short)]
    interactive: bool,
    /// Move only changes to these paths (instead of all paths)
    #[clap(conflicts_with = "interactive")]
    paths: Vec<String>,
}

/// Move changes from a revision's parent into the revision
///
/// After moving the changes out of the parent, the child revision will have the
/// same content state as before. If moving the change out of the parent change
/// made it empty compared to its parent, it will be abandoned unless it's
/// checked out. Without `--interactive`, the parent change will always become
/// empty.
#[derive(clap::Args, Clone, Debug)]
#[clap(visible_alias = "unamend")]
struct UnsquashArgs {
    #[clap(long, short, default_value = "@")]
    revision: String,
    /// Interactively choose which parts to unsquash
    // TODO: It doesn't make much sense to run this without -i. We should make that
    // the default.
    #[clap(long, short)]
    interactive: bool,
}

/// Restore paths from another revision
///
/// That means that the paths get the same content in the destination (`--to`)
/// as they had in the source (`--from`). This is typically used for undoing
/// changes to some paths in the working copy (`jj restore <paths>`).
///
/// When neither `--from` nor `--to` is specified, the command restores into the
/// working copy from its parent. If one of `--from` or `--to` is specified, the
/// other one defaults to the working copy.
///
/// If you restore from a revision where the path has conflicts, then the
/// destination revision will have the same conflict. If the destination is the
/// working copy, then a new commit will be created on top for resolving the
/// conflict (as if you had run `jj checkout` on the new revision). Taken
/// together, that means that if you're already resolving conflicts and you want
/// to restart the resolution of some file, you may want to run `jj restore
/// <path>; jj squash`.
#[derive(clap::Args, Clone, Debug)]
struct RestoreArgs {
    /// Revision to restore from (source)
    #[clap(long)]
    from: Option<String>,
    /// Revision to restore into (destination)
    #[clap(long)]
    to: Option<String>,
    /// Interactively choose which parts to restore
    #[clap(long, short)]
    interactive: bool,
    /// Restore only these paths (instead of all paths)
    #[clap(conflicts_with = "interactive")]
    paths: Vec<String>,
}

/// Touch up the content changes in a revision
///
/// Starts a diff editor (`meld` by default) on the changes in the revision.
/// Edit the right side of the diff until it looks the way you want. Once you
/// close the editor, the revision will be updated. Descendants will be rebased
/// on top as usual, which may result in conflicts. See `jj squash -i` or `jj
/// unsquash -i` if you instead want to move changes into or out of the parent
/// revision.
#[derive(clap::Args, Clone, Debug)]
struct TouchupArgs {
    /// The revision to touch up
    #[clap(long, short, default_value = "@")]
    revision: String,
}

/// Split a revision in two
///
/// Starts a diff editor (`meld` by default) on the changes in the revision.
/// Edit the right side of the diff until it has the content you want in the
/// first revision. Once you close the editor, your edited content will replace
/// the previous revision. The remaining changes will be put in a new revision
/// on top. You will be asked to enter a change description for each.
#[derive(clap::Args, Clone, Debug)]
struct SplitArgs {
    /// The revision to split
    #[clap(long, short, default_value = "@")]
    revision: String,
    /// Put these paths in the first commit and don't run the diff editor
    paths: Vec<String>,
}

/// Merge work from multiple branches
///
/// Unlike most other VCSs, `jj merge` does not implicitly include the working
/// copy revision's parent as one of the parents of the merge; you need to
/// explicitly list all revisions that should become parents of the merge. Also,
/// you need to explicitly check out the resulting revision if you want to.
#[derive(clap::Args, Clone, Debug)]
struct MergeArgs {
    revisions: Vec<String>,
    /// The change description to use (don't open editor)
    #[clap(long, short)]
    message: Option<String>,
}

/// Move revisions to a different parent
///
/// There are three different ways of specifying which revisions to rebase:
/// `-b` to rebase a whole branch, `-s` to rebase a revision and its
/// descendants, and `-r` to rebase a single commit. If none of them is
/// specified, it defaults to `-b @`.
///
/// With `-b`, it rebases the whole branch containing the specified revision.
/// Unlike `-s` and `-r`, the `-b` mode takes the destination into account
/// when calculating the set of revisions to rebase. That set includes the
/// specified revision and all ancestors that are not also ancestors
/// of the destination. It also includes all descendants of those commits. For
/// example, `jj rebase -b B -d D` or `jj rebase -b C -d D`  would transform
/// your history like this:
///
/// D          B'
/// |          |
/// | C        D
/// | |   =>   |
/// | B        | C'
/// |/         |/
/// A          A
///
/// With `-s`, it rebases the specified revision and its descendants onto the
/// destination. For example, `jj rebase -s C -d D` would transform your history
/// like this:
///
/// D          C'
/// |          |
/// | C        D
/// | |   =>   |
/// | B        | B
/// |/         |/
/// A          A
///
/// With `-r`, it rebases only the specified revision onto the destination. Any
/// "hole" left behind will be filled by rebasing descendants onto the specified
/// revision's parent(s). For example, `jj rebase -r B -d D` would transform
/// your history like this:
///
/// D          B'
/// |          |
/// | C        D
/// | |   =>   |
/// | B        | C'
/// |/         |/
/// A          A
#[derive(clap::Args, Clone, Debug)]
#[clap(verbatim_doc_comment)]
#[clap(group(ArgGroup::new("to_rebase").args(&["branch", "source", "revision"])))]
struct RebaseArgs {
    /// Rebase the whole branch (relative to destination's ancestors)
    #[clap(long, short)]
    branch: Option<String>,
    /// Rebase this revision and its descendants
    #[clap(long, short)]
    source: Option<String>,
    /// Rebase only this revision, rebasing descendants onto this revision's
    /// parent(s)
    #[clap(long, short)]
    revision: Option<String>,
    /// The revision to rebase onto
    #[clap(long, short, required = true)]
    destination: Vec<String>,
}

/// Apply the reverse of a revision on top of another revision
#[derive(clap::Args, Clone, Debug)]
struct BackoutArgs {
    /// The revision to apply the reverse of
    #[clap(long, short, default_value = "@")]
    revision: String,
    /// The revision to apply the reverse changes on top of
    // TODO: It seems better to default this to `@-`. Maybe the working
    // copy should be rebased on top?
    #[clap(long, short, default_value = "@")]
    destination: Vec<String>,
}

/// Manage branches.
///
/// For information about branches, see
/// https://github.com/martinvonz/jj/blob/main/docs/branches.md.
#[derive(clap::Subcommand, Clone, Debug)]
enum BranchSubcommand {
    /// Create a new branch.
    #[clap(visible_alias("c"))]
    Create {
        /// The branch's target revision.
        #[clap(long, short)]
        revision: Option<String>,

        /// The branches to create.
        #[clap(required = true)]
        names: Vec<String>,
    },

    /// Delete an existing branch and propagate the deletion to remotes on the
    /// next push.
    #[clap(visible_alias("d"))]
    Delete {
        /// The branches to delete.
        #[clap(required = true)]
        names: Vec<String>,
    },

    /// Delete the local version of an existing branch, without propagating the
    /// deletion to remotes.
    #[clap(visible_alias("f"))]
    Forget {
        /// The branches to delete.
        #[clap(required = true)]
        names: Vec<String>,
    },

    /// List branches and their targets
    ///
    /// A remote branch will be included only if its target is different from
    /// the local target. For a conflicted branch (both local and remote), old
    /// target revisions are preceded by a "-" and new target revisions are
    /// preceded by a "+". For information about branches, see
    /// https://github.com/martinvonz/jj/blob/main/docs/branches.md.
    #[clap(visible_alias("l"))]
    List,

    /// Update a given branch to point to a certain commit.
    #[clap(visible_alias("s"))]
    Set {
        /// The branch's target revision.
        #[clap(long, short)]
        revision: Option<String>,

        /// Allow moving the branch backwards or sideways.
        #[clap(long)]
        allow_backwards: bool,

        /// The branches to update.
        #[clap(required = true)]
        names: Vec<String>,
    },
}

/// Commands for working with the operation log
///
/// Commands for working with the operation log. For information about the
/// operation log, see https://github.com/martinvonz/jj/blob/main/docs/operation-log.md.
#[derive(Subcommand, Clone, Debug)]
enum OperationCommands {
    Log(OperationLogArgs),
    Undo(OperationUndoArgs),
    Restore(OperationRestoreArgs),
}

/// Show the operation log
#[derive(clap::Args, Clone, Debug)]
struct OperationLogArgs {}

/// Restore to the state at an operation
#[derive(clap::Args, Clone, Debug)]
struct OperationRestoreArgs {
    /// The operation to restore to
    operation: String,
}

/// Undo an operation
#[derive(clap::Args, Clone, Debug)]
struct OperationUndoArgs {
    /// The operation to undo
    #[clap(default_value = "@")]
    operation: String,
}

/// Commands for working with workspaces
#[derive(Subcommand, Clone, Debug)]
enum WorkspaceCommands {
    Add(WorkspaceAddArgs),
    Forget(WorkspaceForgetArgs),
    List(WorkspaceListArgs),
}

/// Add a workspace
#[derive(clap::Args, Clone, Debug)]
struct WorkspaceAddArgs {
    /// Where to create the new workspace
    destination: String,
    /// A name for the workspace
    ///
    /// To override the default, which is the basename of the destination
    /// directory.
    #[clap(long)]
    name: Option<String>,
}

/// Stop tracking a workspace's checkout in the repo
///
/// The workspace will not be touched on disk. It can be deleted from disk
/// before or after running this command.
#[derive(clap::Args, Clone, Debug)]
struct WorkspaceForgetArgs {
    /// Name of the workspace to forget (the current workspace by default)
    workspace: Option<String>,
}

/// List workspaces
#[derive(clap::Args, Clone, Debug)]
struct WorkspaceListArgs {}

/// Manage which paths from the current checkout are present in the working copy
#[derive(clap::Args, Clone, Debug)]
struct SparseArgs {
    /// Patterns to add to the working copy
    #[clap(long)]
    add: Vec<String>,
    /// Patterns to remove from the working copy
    #[clap(long, conflicts_with = "clear")]
    remove: Vec<String>,
    /// Include no files in the working copy (combine with --add)
    #[clap(long)]
    clear: bool,
    /// Include all files in the working copy
    #[clap(long, conflicts_with_all = &["add", "remove", "clear"])]
    reset: bool,
    /// List patterns
    #[clap(long, conflicts_with_all = &["add", "remove", "clear", "reset"])]
    list: bool,
}

/// Commands for working with the underlying Git repo
///
/// For a comparison with Git, including a table of commands, see
/// https://github.com/martinvonz/jj/blob/main/docs/git-comparison.md.
#[derive(Subcommand, Clone, Debug)]
enum GitCommands {
    #[clap(subcommand)]
    Remote(GitRemoteCommands),
    Fetch(GitFetchArgs),
    Clone(GitCloneArgs),
    Push(GitPushArgs),
    Import(GitImportArgs),
    Export(GitExportArgs),
}

/// Manage Git remotes
///
/// The Git repo will be a bare git repo stored inside the `.jj/` directory.
#[derive(Subcommand, Clone, Debug)]
enum GitRemoteCommands {
    Add(GitRemoteAddArgs),
    Remove(GitRemoteRemoveArgs),
    List(GitRemoteListArgs),
}

/// Add a Git remote
#[derive(clap::Args, Clone, Debug)]
struct GitRemoteAddArgs {
    /// The remote's name
    remote: String,
    /// The remote's URL
    url: String,
}

/// Remove a Git remote and forget its branches
#[derive(clap::Args, Clone, Debug)]
struct GitRemoteRemoveArgs {
    /// The remote's name
    remote: String,
}

/// List Git remotes
#[derive(clap::Args, Clone, Debug)]
struct GitRemoteListArgs {}

/// Fetch from a Git remote
#[derive(clap::Args, Clone, Debug)]
struct GitFetchArgs {
    /// The remote to fetch from (only named remotes are supported)
    #[clap(long, default_value = "origin")]
    remote: String,
}

/// Create a new repo backed by a clone of a Git repo
///
/// The Git repo will be a bare git repo stored inside the `.jj/` directory.
#[derive(clap::Args, Clone, Debug)]
struct GitCloneArgs {
    /// URL or path of the Git repo to clone
    source: String,
    /// The directory to write the Jujutsu repo to
    destination: Option<String>,
}

/// Push to a Git remote
///
/// By default, pushes any branches pointing to `@`. Use `--branch` to push a
/// specific branch. Use `--all` to push all branches. Use `--change` to
/// generate a branch name based on a specific commit's change ID.
#[derive(clap::Args, Clone, Debug)]
#[clap(group(ArgGroup::new("what").args(&["branch", "all", "change"])))]
struct GitPushArgs {
    /// The remote to push to (only named remotes are supported)
    #[clap(long, default_value = "origin")]
    remote: String,
    /// Push only this branch
    #[clap(long)]
    branch: Option<String>,
    /// Push all branches
    #[clap(long)]
    all: bool,
    /// Push this commit by creating a branch based on its change ID
    #[clap(long)]
    change: Option<String>,
    /// Only display what will change on the remote
    #[clap(long)]
    dry_run: bool,
}

/// Update repo with changes made in the underlying Git repo
#[derive(clap::Args, Clone, Debug)]
struct GitImportArgs {}

/// Update the underlying Git repo with changes made in the repo
#[derive(clap::Args, Clone, Debug)]
struct GitExportArgs {}

/// Commands for benchmarking internal operations
#[derive(Subcommand, Clone, Debug)]
enum BenchCommands {
    #[clap(name = "commonancestors")]
    CommonAncestors(BenchCommonAncestorsArgs),
    #[clap(name = "isancestor")]
    IsAncestor(BenchIsAncestorArgs),
    #[clap(name = "walkrevs")]
    WalkRevs(BenchWalkRevsArgs),
    #[clap(name = "resolveprefix")]
    ResolvePrefix(BenchResolvePrefixArgs),
}

/// Find the common ancestor(s) of a set of commits
#[derive(clap::Args, Clone, Debug)]
struct BenchCommonAncestorsArgs {
    revision1: String,
    revision2: String,
}

/// Checks if the first commit is an ancestor of the second commit
#[derive(clap::Args, Clone, Debug)]
struct BenchIsAncestorArgs {
    ancestor: String,
    descendant: String,
}

/// Walk revisions that are ancestors of the second argument but not ancestors
/// of the first
#[derive(clap::Args, Clone, Debug)]
struct BenchWalkRevsArgs {
    unwanted: String,
    wanted: String,
}

/// Resolve a commit ID prefix
#[derive(clap::Args, Clone, Debug)]
struct BenchResolvePrefixArgs {
    prefix: String,
}

/// Low-level commands not intended for users
#[derive(Subcommand, Clone, Debug)]
#[clap(hide = true)]
enum DebugCommands {
    Completion(DebugCompletionArgs),
    Mangen(DebugMangenArgs),
    #[clap(name = "resolverev")]
    ResolveRev(DebugResolveRevArgs),
    #[clap(name = "workingcopy")]
    WorkingCopy(DebugWorkingCopyArgs),
    Template(DebugTemplateArgs),
    Index(DebugIndexArgs),
    #[clap(name = "reindex")]
    ReIndex(DebugReIndexArgs),
    Operation(DebugOperationArgs),
}

/// Print a command-line-completion script
#[derive(clap::Args, Clone, Debug)]
struct DebugCompletionArgs {
    /// Print a completion script for Bash
    ///
    /// Apply it by running this:
    ///
    /// source <(jj debug completion)
    #[clap(long, verbatim_doc_comment)]
    bash: bool,
    /// Print a completion script for Fish
    ///
    /// Apply it by running this:
    ///
    /// autoload -U compinit
    /// compinit
    /// source <(jj debug completion --zsh | sed '$d')  # remove the last line
    /// compdef _jj jj
    #[clap(long, verbatim_doc_comment)]
    fish: bool,
    /// Print a completion script for Zsh
    ///
    /// Apply it by running this:
    ///
    /// jj debug completion --fish | source
    #[clap(long, verbatim_doc_comment)]
    zsh: bool,
}

/// Print a ROFF (manpage)
#[derive(clap::Args, Clone, Debug)]
struct DebugMangenArgs {}

/// Resolve a revision identifier to its full ID
#[derive(clap::Args, Clone, Debug)]
struct DebugResolveRevArgs {
    #[clap(long, short, default_value = "@")]
    revision: String,
}

/// Show information about the working copy state
#[derive(clap::Args, Clone, Debug)]
struct DebugWorkingCopyArgs {}

/// Parse a template
#[derive(clap::Args, Clone, Debug)]
struct DebugTemplateArgs {
    template: String,
}

/// Show commit index stats
#[derive(clap::Args, Clone, Debug)]
struct DebugIndexArgs {}

/// Rebuild commit index
#[derive(clap::Args, Clone, Debug)]
struct DebugReIndexArgs {}

/// Show information about an operation and its view
#[derive(clap::Args, Clone, Debug)]
struct DebugOperationArgs {
    #[clap(default_value = "@")]
    operation: String,
}

fn short_commit_description(commit: &Commit) -> String {
    let first_line = commit.description().split('\n').next().unwrap();
    format!("{} ({})", short_commit_hash(commit.id()), first_line)
}

fn short_commit_hash(commit_id: &CommitId) -> String {
    commit_id.hex()[0..12].to_string()
}

fn add_to_git_exclude(ui: &mut Ui, git_repo: &git2::Repository) -> Result<(), CommandError> {
    let exclude_file_path = git_repo.path().join("info").join("exclude");
    if exclude_file_path.exists() {
        match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&exclude_file_path)
        {
            Ok(mut exclude_file) => {
                let mut buf = vec![];
                exclude_file.read_to_end(&mut buf)?;
                let pattern = b"\n/.jj/\n";
                if !buf.windows(pattern.len()).any(|window| window == pattern) {
                    exclude_file.seek(SeekFrom::End(0))?;
                    if !buf.ends_with(b"\n") {
                        exclude_file.write_all(b"\n")?;
                    }
                    exclude_file.write_all(b"/.jj/\n")?;
                }
            }
            Err(err) => {
                ui.write_error(&format!(
                    "Failed to add `.jj/` to {}: {}\n",
                    exclude_file_path.to_string_lossy(),
                    err
                ))?;
            }
        }
    } else {
        ui.write_error(&format!(
            "Failed to add `.jj/` to {} because it doesn't exist\n",
            exclude_file_path.to_string_lossy()
        ))?;
    }
    Ok(())
}

fn cmd_init(ui: &mut Ui, command: &CommandHelper, args: &InitArgs) -> Result<(), CommandError> {
    if command.global_args().repository.is_some() {
        return Err(CommandError::UserError(
            "'--repository' cannot be used with 'init'".to_string(),
        ));
    }
    let wc_path = ui.cwd().join(&args.destination);
    if wc_path.exists() {
        assert!(wc_path.is_dir());
    } else {
        fs::create_dir(&wc_path).unwrap();
    }
    let wc_path = wc_path.canonicalize().unwrap();

    if let Some(git_store_str) = &args.git_repo {
        let mut git_store_path = ui.cwd().join(git_store_str);
        if !git_store_path.ends_with(".git") {
            git_store_path = git_store_path.join(".git");
        }
        git_store_path = git_store_path.canonicalize().unwrap();
        // If the git repo is inside the workspace, use a relative path to it so the
        // whole workspace can be moved without breaking.
        if let Ok(relative_path) = git_store_path.strip_prefix(&wc_path) {
            git_store_path = PathBuf::from("..")
                .join("..")
                .join("..")
                .join(relative_path);
        }
        let (workspace, repo) =
            Workspace::init_external_git(ui.settings(), wc_path.clone(), git_store_path)?;
        let git_repo = repo.store().git_repo().unwrap();
        let mut workspace_command = command.for_loaded_repo(ui, workspace, repo)?;
        if workspace_command.working_copy_shared_with_git() {
            add_to_git_exclude(ui, &git_repo)?;
        } else {
            let mut tx = workspace_command.start_transaction("import git refs");
            git::import_refs(tx.mut_repo(), &git_repo)?;
            if let Some(git_head_id) = tx.mut_repo().view().git_head() {
                let git_head_commit = tx.mut_repo().store().get_commit(&git_head_id)?;
                tx.mut_repo().check_out(
                    workspace_command.workspace_id(),
                    ui.settings(),
                    &git_head_commit,
                );
            }
            if tx.mut_repo().has_changes() {
                workspace_command.finish_transaction(ui, tx)?;
            }
        }
    } else if args.git {
        Workspace::init_internal_git(ui.settings(), wc_path.clone())?;
    } else {
        Workspace::init_local(ui.settings(), wc_path.clone())?;
    };
    let cwd = ui.cwd().canonicalize().unwrap();
    let relative_wc_path = ui::relative_path(&cwd, &wc_path);
    writeln!(ui, "Initialized repo in \"{}\"", relative_wc_path.display())?;
    Ok(())
}

fn cmd_checkout(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &CheckoutArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let new_commit = workspace_command.resolve_single_rev(&args.revision)?;
    let workspace_id = workspace_command.workspace_id();
    if ui.settings().enable_open_commits() {
        if workspace_command.repo().view().get_checkout(&workspace_id) == Some(new_commit.id()) {
            ui.write("Already on that commit\n")?;
        } else {
            let mut tx = workspace_command
                .start_transaction(&format!("check out commit {}", new_commit.id().hex()));
            if new_commit.is_open() {
                tx.mut_repo().edit(workspace_id, &new_commit);
            } else {
                tx.mut_repo()
                    .check_out(workspace_id, ui.settings(), &new_commit);
            }
            workspace_command.finish_transaction(ui, tx)?;
        }
    } else {
        let mut tx = workspace_command
            .start_transaction(&format!("check out commit {}", new_commit.id().hex()));
        tx.mut_repo()
            .check_out(workspace_id, ui.settings(), &new_commit);
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_untrack(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &UntrackArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let store = workspace_command.repo().store().clone();
    let matcher = matcher_from_values(ui, workspace_command.workspace_root(), &args.paths)?;

    let mut tx = workspace_command.start_transaction("untrack paths");
    let base_ignores = workspace_command.base_ignores();
    let (mut locked_working_copy, current_checkout) =
        workspace_command.start_working_copy_mutation()?;
    // Create a new tree without the unwanted files
    let mut tree_builder = store.tree_builder(current_checkout.tree_id().clone());
    for (path, _value) in current_checkout.tree().entries_matching(matcher.as_ref()) {
        tree_builder.remove(path);
    }
    let new_tree_id = tree_builder.write_tree();
    let new_tree = store.get_tree(&RepoPath::root(), &new_tree_id)?;
    // Reset the working copy to the new tree
    locked_working_copy.reset(&new_tree)?;
    // Commit the working copy again so we can inform the user if paths couldn't be
    // untracked because they're not ignored.
    let wc_tree_id = locked_working_copy.snapshot(base_ignores)?;
    if wc_tree_id != new_tree_id {
        let wc_tree = store.get_tree(&RepoPath::root(), &wc_tree_id)?;
        let added_back = wc_tree.entries_matching(matcher.as_ref()).collect_vec();
        if !added_back.is_empty() {
            locked_working_copy.discard();
            let path = &added_back[0].0;
            let ui_path = workspace_command.format_file_path(path);
            if added_back.len() > 1 {
                return Err(CommandError::UserError(format!(
                    "'{}' and {} other files would be added back because they're not ignored. \
                     Make sure they're ignored, then try again.",
                    ui_path,
                    added_back.len() - 1
                )));
            } else {
                return Err(CommandError::UserError(format!(
                    "'{}' would be added back because it's not ignored. Make sure it's ignored, \
                     then try again.",
                    ui_path
                )));
            }
        } else {
            // This means there were some concurrent changes made in the working copy. We
            // don't want to mix those in, so reset the working copy again.
            locked_working_copy.reset(&new_tree)?;
        }
    }
    CommitBuilder::for_rewrite_from(ui.settings(), &current_checkout)
        .set_tree(new_tree_id)
        .write_to_repo(tx.mut_repo());
    let num_rebased = tx.mut_repo().rebase_descendants(ui.settings())?;
    if num_rebased > 0 {
        writeln!(ui, "Rebased {} descendant commits", num_rebased)?;
    }
    let repo = tx.commit();
    locked_working_copy.finish(repo.op_id().clone());
    Ok(())
}

fn cmd_files(ui: &mut Ui, command: &CommandHelper, args: &FilesArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    let matcher = matcher_from_values(ui, workspace_command.workspace_root(), &args.paths)?;
    for (name, _value) in commit.tree().entries_matching(matcher.as_ref()) {
        writeln!(ui, "{}", &workspace_command.format_file_path(&name))?;
    }
    Ok(())
}

fn cmd_print(ui: &mut Ui, command: &CommandHelper, args: &PrintArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    let path = ui.parse_file_path(workspace_command.workspace_root(), &args.path)?;
    let repo = workspace_command.repo();
    match commit.tree().path_value(&path) {
        None => {
            return Err(CommandError::UserError("No such path".to_string()));
        }
        Some(TreeValue::Normal { id, .. }) => {
            let mut contents = repo.store().read_file(&path, &id)?;
            std::io::copy(&mut contents, &mut ui.stdout_formatter().as_mut())?;
        }
        Some(TreeValue::Conflict(id)) => {
            let conflict = repo.store().read_conflict(&path, &id)?;
            let mut contents = vec![];
            conflicts::materialize_conflict(repo.store(), &path, &conflict, &mut contents).unwrap();
            ui.stdout_formatter().write_all(&contents)?;
        }
        _ => {
            return Err(CommandError::UserError(
                "Path exists but is not a file".to_string(),
            ));
        }
    }
    Ok(())
}

fn show_color_words_diff_hunks(
    left: &[u8],
    right: &[u8],
    formatter: &mut dyn Formatter,
) -> io::Result<()> {
    let num_context_lines = 3;
    let mut context = VecDeque::new();
    // Have we printed "..." for any skipped context?
    let mut skipped_context = false;
    // Are the lines in `context` to be printed before the next modified line?
    let mut context_before = true;
    for diff_line in files::diff(left, right) {
        if diff_line.is_unmodified() {
            context.push_back(diff_line.clone());
            if context.len() > num_context_lines {
                if context_before {
                    context.pop_front();
                } else {
                    context.pop_back();
                }
                if !context_before {
                    for line in &context {
                        show_color_words_diff_line(formatter, line)?;
                    }
                    context.clear();
                    context_before = true;
                }
                if !skipped_context {
                    formatter.write_bytes(b"    ...\n")?;
                    skipped_context = true;
                }
            }
        } else {
            for line in &context {
                show_color_words_diff_line(formatter, line)?;
            }
            context.clear();
            show_color_words_diff_line(formatter, &diff_line)?;
            context_before = false;
            skipped_context = false;
        }
    }
    if !context_before {
        for line in &context {
            show_color_words_diff_line(formatter, line)?;
        }
    }

    // If the last diff line doesn't end with newline, add it.
    let no_hunk = left.is_empty() && right.is_empty();
    let any_last_newline = left.ends_with(b"\n") || right.ends_with(b"\n");
    if !skipped_context && !no_hunk && !any_last_newline {
        formatter.write_bytes(b"\n")?;
    }

    Ok(())
}

fn show_color_words_diff_line(
    formatter: &mut dyn Formatter,
    diff_line: &DiffLine,
) -> io::Result<()> {
    if diff_line.has_left_content {
        formatter.add_label(String::from("removed"))?;
        formatter.write_bytes(format!("{:>4}", diff_line.left_line_number).as_bytes())?;
        formatter.remove_label()?;
        formatter.write_bytes(b" ")?;
    } else {
        formatter.write_bytes(b"     ")?;
    }
    if diff_line.has_right_content {
        formatter.add_label(String::from("added"))?;
        formatter.write_bytes(format!("{:>4}", diff_line.right_line_number).as_bytes())?;
        formatter.remove_label()?;
        formatter.write_bytes(b": ")?;
    } else {
        formatter.write_bytes(b"    : ")?;
    }
    for hunk in &diff_line.hunks {
        match hunk {
            DiffHunk::Matching(data) => {
                formatter.write_bytes(data)?;
            }
            DiffHunk::Different(data) => {
                let before = data[0];
                let after = data[1];
                if !before.is_empty() {
                    formatter.add_label(String::from("removed"))?;
                    formatter.write_bytes(before)?;
                    formatter.remove_label()?;
                }
                if !after.is_empty() {
                    formatter.add_label(String::from("added"))?;
                    formatter.write_bytes(after)?;
                    formatter.remove_label()?;
                }
            }
        }
    }

    Ok(())
}

fn cmd_diff(ui: &mut Ui, command: &CommandHelper, args: &DiffArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let from_tree;
    let to_tree;
    if args.from.is_some() || args.to.is_some() {
        let from = workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"))?;
        from_tree = from.tree();
        let to = workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"))?;
        to_tree = to.tree();
    } else {
        let commit =
            workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"))?;
        let parents = commit.parents();
        from_tree = merge_commit_trees(workspace_command.repo().as_repo_ref(), &parents);
        to_tree = commit.tree()
    }
    let workspace_root = workspace_command.workspace_root();
    let matcher = matcher_from_values(ui, workspace_root, &args.paths)?;
    let diff_iterator = from_tree.diff(&to_tree, matcher.as_ref());
    show_diff(
        ui.stdout_formatter().as_mut(),
        &workspace_command,
        diff_iterator,
        diff_format_for(ui, &args.format),
    )?;
    Ok(())
}

fn cmd_show(ui: &mut Ui, command: &CommandHelper, args: &ShowArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    let parents = commit.parents();
    let from_tree = merge_commit_trees(workspace_command.repo().as_repo_ref(), &parents);
    let to_tree = commit.tree();
    let diff_iterator = from_tree.diff(&to_tree, &EverythingMatcher);
    // TODO: Add branches, tags, etc
    // TODO: Indent the description like Git does
    let template_string = r#"
            "Commit ID: " commit_id "\n"
            "Change ID: " change_id "\n"
            "Author: " author " <" author.email() "> (" author.timestamp() ")\n"
            "Committer: " committer " <" committer.email() "> (" committer.timestamp() ")\n"
            "\n"
            description
            "\n""#;
    let template_string = if ui.settings().enable_open_commits() {
        format!(
            r#"
            label(if(open, "open"), {template_string})"#
        )
    } else {
        String::from(template_string)
    };
    let template = crate::template_parser::parse_commit_template(
        workspace_command.repo().as_repo_ref(),
        &workspace_command.workspace_id(),
        &template_string,
    );
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();
    template.format(&commit, formatter)?;
    show_diff(
        formatter,
        &workspace_command,
        diff_iterator,
        diff_format_for(ui, &args.format),
    )?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiffFormat {
    Summary,
    Git,
    ColorWords,
}

fn diff_format_for(ui: &Ui, args: &DiffFormatArgs) -> DiffFormat {
    if args.summary {
        DiffFormat::Summary
    } else if args.git {
        DiffFormat::Git
    } else if args.color_words {
        DiffFormat::ColorWords
    } else {
        match ui.settings().config().get_string("diff.format") {
            Ok(value) if &value == "summary" => DiffFormat::Summary,
            Ok(value) if &value == "git" => DiffFormat::Git,
            Ok(value) if &value == "color-words" => DiffFormat::ColorWords,
            _ => DiffFormat::ColorWords,
        }
    }
}

fn show_diff(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    tree_diff: TreeDiffIterator,
    format: DiffFormat,
) -> Result<(), CommandError> {
    match format {
        DiffFormat::Summary => {
            show_diff_summary(formatter, workspace_command, tree_diff)?;
        }
        DiffFormat::Git => {
            show_git_diff(formatter, workspace_command, tree_diff)?;
        }
        DiffFormat::ColorWords => {
            show_color_words_diff(formatter, workspace_command, tree_diff)?;
        }
    }
    Ok(())
}

fn diff_content(
    repo: &Arc<ReadonlyRepo>,
    path: &RepoPath,
    value: &TreeValue,
) -> Result<Vec<u8>, CommandError> {
    match value {
        TreeValue::Normal { id, .. } => {
            let mut file_reader = repo.store().read_file(path, id).unwrap();
            let mut content = vec![];
            file_reader.read_to_end(&mut content)?;
            Ok(content)
        }
        TreeValue::Symlink(id) => {
            let target = repo.store().read_symlink(path, id)?;
            Ok(target.into_bytes())
        }
        TreeValue::Tree(_) => {
            panic!(
                "Got an unexpected tree in a diff of path {}",
                path.to_internal_file_string()
            );
        }
        TreeValue::GitSubmodule(id) => {
            Ok(format!("Git submodule checked out at {}", id.hex()).into_bytes())
        }
        TreeValue::Conflict(id) => {
            let conflict = repo.store().read_conflict(path, id).unwrap();
            let mut content = vec![];
            conflicts::materialize_conflict(repo.store(), path, &conflict, &mut content).unwrap();
            Ok(content)
        }
    }
}

fn basic_diff_file_type(value: &TreeValue) -> String {
    match value {
        TreeValue::Normal { executable, .. } => {
            if *executable {
                "executable file".to_string()
            } else {
                "regular file".to_string()
            }
        }
        TreeValue::Symlink(_) => "symlink".to_string(),
        TreeValue::Tree(_) => "tree".to_string(),
        TreeValue::GitSubmodule(_) => "Git submodule".to_string(),
        TreeValue::Conflict(_) => "conflict".to_string(),
    }
}

fn show_color_words_diff(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    tree_diff: TreeDiffIterator,
) -> Result<(), CommandError> {
    let repo = workspace_command.repo();
    formatter.add_label(String::from("diff"))?;
    for (path, diff) in tree_diff {
        let ui_path = workspace_command.format_file_path(&path);
        match diff {
            tree::Diff::Added(right_value) => {
                let right_content = diff_content(repo, &path, &right_value)?;
                let description = basic_diff_file_type(&right_value);
                formatter.add_label(String::from("header"))?;
                formatter.write_str(&format!("Added {} {}:\n", description, ui_path))?;
                formatter.remove_label()?;
                show_color_words_diff_hunks(&[], &right_content, formatter)?;
            }
            tree::Diff::Modified(left_value, right_value) => {
                let left_content = diff_content(repo, &path, &left_value)?;
                let right_content = diff_content(repo, &path, &right_value)?;
                let description = match (left_value, right_value) {
                    (
                        TreeValue::Normal {
                            executable: left_executable,
                            ..
                        },
                        TreeValue::Normal {
                            executable: right_executable,
                            ..
                        },
                    ) => {
                        if left_executable && right_executable {
                            "Modified executable file".to_string()
                        } else if left_executable {
                            "Executable file became non-executable at".to_string()
                        } else if right_executable {
                            "Non-executable file became executable at".to_string()
                        } else {
                            "Modified regular file".to_string()
                        }
                    }
                    (TreeValue::Conflict(_), TreeValue::Conflict(_)) => {
                        "Modified conflict in".to_string()
                    }
                    (TreeValue::Conflict(_), _) => "Resolved conflict in".to_string(),
                    (_, TreeValue::Conflict(_)) => "Created conflict in".to_string(),
                    (TreeValue::Symlink(_), TreeValue::Symlink(_)) => {
                        "Symlink target changed at".to_string()
                    }
                    (left_value, right_value) => {
                        let left_type = basic_diff_file_type(&left_value);
                        let right_type = basic_diff_file_type(&right_value);
                        let (first, rest) = left_type.split_at(1);
                        format!(
                            "{}{} became {} at",
                            first.to_ascii_uppercase(),
                            rest,
                            right_type
                        )
                    }
                };
                formatter.add_label(String::from("header"))?;
                formatter.write_str(&format!("{} {}:\n", description, ui_path))?;
                formatter.remove_label()?;
                show_color_words_diff_hunks(&left_content, &right_content, formatter)?;
            }
            tree::Diff::Removed(left_value) => {
                let left_content = diff_content(repo, &path, &left_value)?;
                let description = basic_diff_file_type(&left_value);
                formatter.add_label(String::from("header"))?;
                formatter.write_str(&format!("Removed {} {}:\n", description, ui_path))?;
                formatter.remove_label()?;
                show_color_words_diff_hunks(&left_content, &[], formatter)?;
            }
        }
    }
    formatter.remove_label()?;
    Ok(())
}

struct GitDiffPart {
    mode: String,
    hash: String,
    content: Vec<u8>,
}

fn git_diff_part(
    repo: &Arc<ReadonlyRepo>,
    path: &RepoPath,
    value: &TreeValue,
) -> Result<GitDiffPart, CommandError> {
    let mode;
    let hash;
    let mut content = vec![];
    match value {
        TreeValue::Normal { id, executable } => {
            mode = if *executable {
                "100755".to_string()
            } else {
                "100644".to_string()
            };
            hash = id.hex();
            let mut file_reader = repo.store().read_file(path, id).unwrap();
            file_reader.read_to_end(&mut content)?;
        }
        TreeValue::Symlink(id) => {
            mode = "120000".to_string();
            hash = id.hex();
            let target = repo.store().read_symlink(path, id)?;
            content = target.into_bytes();
        }
        TreeValue::Tree(_) => {
            panic!(
                "Got an unexpected tree in a diff of path {}",
                path.to_internal_file_string()
            );
        }
        TreeValue::GitSubmodule(id) => {
            // TODO: What should we actually do here?
            mode = "040000".to_string();
            hash = id.hex();
        }
        TreeValue::Conflict(id) => {
            mode = "100644".to_string();
            hash = id.hex();
            let conflict = repo.store().read_conflict(path, id).unwrap();
            conflicts::materialize_conflict(repo.store(), path, &conflict, &mut content).unwrap();
        }
    }
    let hash = hash[0..10].to_string();
    Ok(GitDiffPart {
        mode,
        hash,
        content,
    })
}

#[derive(PartialEq)]
enum DiffLineType {
    Context,
    Removed,
    Added,
}

struct UnifiedDiffHunk<'content> {
    left_line_range: Range<usize>,
    right_line_range: Range<usize>,
    lines: Vec<(DiffLineType, &'content [u8])>,
}

fn unified_diff_hunks<'content>(
    left_content: &'content [u8],
    right_content: &'content [u8],
    num_context_lines: usize,
) -> Vec<UnifiedDiffHunk<'content>> {
    let mut hunks = vec![];
    let mut current_hunk = UnifiedDiffHunk {
        left_line_range: 1..1,
        right_line_range: 1..1,
        lines: vec![],
    };
    let mut show_context_after = false;
    let diff = Diff::for_tokenizer(&[left_content, right_content], &diff::find_line_ranges);
    for hunk in diff.hunks() {
        match hunk {
            DiffHunk::Matching(content) => {
                let lines = content.split_inclusive(|b| *b == b'\n').collect_vec();
                // Number of context lines to print after the previous non-matching hunk.
                let num_after_lines = lines.len().min(if show_context_after {
                    num_context_lines
                } else {
                    0
                });
                current_hunk.left_line_range.end += num_after_lines;
                current_hunk.right_line_range.end += num_after_lines;
                for line in lines.iter().take(num_after_lines) {
                    current_hunk.lines.push((DiffLineType::Context, line));
                }
                let num_skip_lines = lines
                    .len()
                    .saturating_sub(num_after_lines)
                    .saturating_sub(num_context_lines);
                if num_skip_lines > 0 {
                    let left_start = current_hunk.left_line_range.end + num_skip_lines;
                    let right_start = current_hunk.right_line_range.end + num_skip_lines;
                    if !current_hunk.lines.is_empty() {
                        hunks.push(current_hunk);
                    }
                    current_hunk = UnifiedDiffHunk {
                        left_line_range: left_start..left_start,
                        right_line_range: right_start..right_start,
                        lines: vec![],
                    };
                }
                let num_before_lines = lines.len() - num_after_lines - num_skip_lines;
                current_hunk.left_line_range.end += num_before_lines;
                current_hunk.right_line_range.end += num_before_lines;
                for line in lines.iter().skip(num_after_lines + num_skip_lines) {
                    current_hunk.lines.push((DiffLineType::Context, line));
                }
            }
            DiffHunk::Different(content) => {
                show_context_after = true;
                let left_lines = content[0].split_inclusive(|b| *b == b'\n').collect_vec();
                let right_lines = content[1].split_inclusive(|b| *b == b'\n').collect_vec();
                if !left_lines.is_empty() {
                    current_hunk.left_line_range.end += left_lines.len();
                    for line in left_lines {
                        current_hunk.lines.push((DiffLineType::Removed, line));
                    }
                }
                if !right_lines.is_empty() {
                    current_hunk.right_line_range.end += right_lines.len();
                    for line in right_lines {
                        current_hunk.lines.push((DiffLineType::Added, line));
                    }
                }
            }
        }
    }
    if !current_hunk
        .lines
        .iter()
        .all(|(diff_type, _line)| *diff_type == DiffLineType::Context)
    {
        hunks.push(current_hunk);
    }
    hunks
}

fn show_unified_diff_hunks(
    formatter: &mut dyn Formatter,
    left_content: &[u8],
    right_content: &[u8],
) -> Result<(), CommandError> {
    for hunk in unified_diff_hunks(left_content, right_content, 3) {
        formatter.add_label(String::from("hunk_header"))?;
        writeln!(
            formatter,
            "@@ -{},{} +{},{} @@",
            hunk.left_line_range.start,
            hunk.left_line_range.len(),
            hunk.right_line_range.start,
            hunk.right_line_range.len()
        )?;
        formatter.remove_label()?;
        for (line_type, content) in hunk.lines {
            match line_type {
                DiffLineType::Context => {
                    formatter.add_label(String::from("context"))?;
                    formatter.write_str(" ")?;
                    formatter.write_all(content)?;
                    formatter.remove_label()?;
                }
                DiffLineType::Removed => {
                    formatter.add_label(String::from("removed"))?;
                    formatter.write_str("-")?;
                    formatter.write_all(content)?;
                    formatter.remove_label()?;
                }
                DiffLineType::Added => {
                    formatter.add_label(String::from("added"))?;
                    formatter.write_str("+")?;
                    formatter.write_all(content)?;
                    formatter.remove_label()?;
                }
            }
            if !content.ends_with(b"\n") {
                formatter.write_str("\n\\ No newline at end of file\n")?;
            }
        }
    }
    Ok(())
}

fn show_git_diff(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    tree_diff: TreeDiffIterator,
) -> Result<(), CommandError> {
    let repo = workspace_command.repo();
    formatter.add_label(String::from("diff"))?;
    for (path, diff) in tree_diff {
        let path_string = path.to_internal_file_string();
        formatter.add_label(String::from("file_header"))?;
        writeln!(formatter, "diff --git a/{} b/{}", path_string, path_string)?;
        match diff {
            tree::Diff::Added(right_value) => {
                let right_part = git_diff_part(repo, &path, &right_value)?;
                writeln!(formatter, "new file mode {}", &right_part.mode)?;
                writeln!(formatter, "index 0000000000..{}", &right_part.hash)?;
                writeln!(formatter, "--- /dev/null")?;
                writeln!(formatter, "+++ b/{}", path_string)?;
                formatter.remove_label()?;
                show_unified_diff_hunks(formatter, &[], &right_part.content)?;
            }
            tree::Diff::Modified(left_value, right_value) => {
                let left_part = git_diff_part(repo, &path, &left_value)?;
                let right_part = git_diff_part(repo, &path, &right_value)?;
                if left_part.mode != right_part.mode {
                    writeln!(formatter, "old mode {}", &left_part.mode)?;
                    writeln!(formatter, "new mode {}", &right_part.mode)?;
                    if left_part.hash != right_part.hash {
                        writeln!(formatter, "index {}...{}", &left_part.hash, right_part.hash)?;
                    }
                } else if left_part.hash != right_part.hash {
                    writeln!(
                        formatter,
                        "index {}...{} {}",
                        &left_part.hash, right_part.hash, left_part.mode
                    )?;
                }
                if left_part.content != right_part.content {
                    writeln!(formatter, "--- a/{}", path_string)?;
                    writeln!(formatter, "+++ b/{}", path_string)?;
                }
                formatter.remove_label()?;
                show_unified_diff_hunks(formatter, &left_part.content, &right_part.content)?;
            }
            tree::Diff::Removed(left_value) => {
                let left_part = git_diff_part(repo, &path, &left_value)?;
                writeln!(formatter, "deleted file mode {}", &left_part.mode)?;
                writeln!(formatter, "index {}..0000000000", &left_part.hash)?;
                writeln!(formatter, "--- a/{}", path_string)?;
                writeln!(formatter, "+++ /dev/null")?;
                formatter.remove_label()?;
                show_unified_diff_hunks(formatter, &left_part.content, &[])?;
            }
        }
    }
    formatter.remove_label()?;
    Ok(())
}

fn show_diff_summary(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    tree_diff: TreeDiffIterator,
) -> io::Result<()> {
    formatter.add_label(String::from("diff"))?;
    for (repo_path, diff) in tree_diff {
        match diff {
            tree::Diff::Modified(_, _) => {
                formatter.add_label(String::from("modified"))?;
                writeln!(
                    formatter,
                    "M {}",
                    workspace_command.format_file_path(&repo_path)
                )?;
                formatter.remove_label()?;
            }
            tree::Diff::Added(_) => {
                formatter.add_label(String::from("added"))?;
                writeln!(
                    formatter,
                    "A {}",
                    workspace_command.format_file_path(&repo_path)
                )?;
                formatter.remove_label()?;
            }
            tree::Diff::Removed(_) => {
                formatter.add_label(String::from("removed"))?;
                writeln!(
                    formatter,
                    "R {}",
                    workspace_command.format_file_path(&repo_path)
                )?;
                formatter.remove_label()?;
            }
        }
    }
    formatter.remove_label()?;
    Ok(())
}

fn cmd_status(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &StatusArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let maybe_checkout_id = repo.view().get_checkout(&workspace_command.workspace_id());
    let maybe_checkout = maybe_checkout_id
        .map(|id| repo.store().get_commit(id))
        .transpose()?;
    if let Some(checkout_commit) = &maybe_checkout {
        ui.write("Parent commit: ")?;
        let workspace_id = workspace_command.workspace_id();
        ui.write_commit_summary(
            repo.as_repo_ref(),
            &workspace_id,
            &checkout_commit.parents()[0],
        )?;
        ui.write("\n")?;
        ui.write("Working copy : ")?;
        ui.write_commit_summary(repo.as_repo_ref(), &workspace_id, checkout_commit)?;
        ui.write("\n")?;
    } else {
        ui.write("No working copy\n")?;
    }

    let mut conflicted_local_branches = vec![];
    let mut conflicted_remote_branches = vec![];
    for (branch_name, branch_target) in repo.view().branches() {
        if let Some(local_target) = &branch_target.local_target {
            if local_target.is_conflict() {
                conflicted_local_branches.push(branch_name.clone());
            }
        }
        for (remote_name, remote_target) in &branch_target.remote_targets {
            if remote_target.is_conflict() {
                conflicted_remote_branches.push((branch_name.clone(), remote_name.clone()));
            }
        }
    }
    if !conflicted_local_branches.is_empty() {
        ui.stdout_formatter().add_label("conflict".to_string())?;
        writeln!(ui, "These branches have conflicts:")?;
        ui.stdout_formatter().remove_label()?;
        for branch_name in conflicted_local_branches {
            write!(ui, "  ")?;
            ui.stdout_formatter().add_label("branch".to_string())?;
            write!(ui, "{}", branch_name)?;
            ui.stdout_formatter().remove_label()?;
            writeln!(ui)?;
        }
        writeln!(
            ui,
            "  Use `jj branch list` to see details. Use `jj branch set <name> -r <rev>` to \
             resolve."
        )?;
    }
    if !conflicted_remote_branches.is_empty() {
        ui.stdout_formatter().add_label("conflict".to_string())?;
        writeln!(ui, "These remote branches have conflicts:")?;
        ui.stdout_formatter().remove_label()?;
        for (branch_name, remote_name) in conflicted_remote_branches {
            write!(ui, "  ")?;
            ui.stdout_formatter().add_label("branch".to_string())?;
            write!(ui, "{}@{}", branch_name, remote_name)?;
            ui.stdout_formatter().remove_label()?;
            writeln!(ui)?;
        }
        writeln!(
            ui,
            "  Use `jj branch list` to see details. Use `jj git fetch` to resolve."
        )?;
    }

    if let Some(checkout_commit) = &maybe_checkout {
        let parent_tree = checkout_commit.parents()[0].tree();
        let tree = checkout_commit.tree();
        if tree.id() == parent_tree.id() {
            ui.write("The working copy is clean\n")?;
        } else {
            ui.write("Working copy changes:\n")?;
            show_diff_summary(
                ui.stdout_formatter().as_mut(),
                &workspace_command,
                parent_tree.diff(&tree, &EverythingMatcher),
            )?;
        }

        let conflicts = tree.conflicts();
        if !conflicts.is_empty() {
            ui.stdout_formatter().add_label("conflict".to_string())?;
            writeln!(ui, "There are unresolved conflicts at these paths:")?;
            ui.stdout_formatter().remove_label()?;
            for (path, _) in conflicts {
                writeln!(ui, "{}", &workspace_command.format_file_path(&path))?;
            }
        }
    }

    Ok(())
}

fn log_template(settings: &UserSettings) -> String {
    // TODO: define a method on boolean values, so we can get auto-coloring
    //       with e.g. `conflict.then("conflict")`
    let default_template = r#"
            commit_id.short()
            " " change_id.short()
            " " author.email()
            " " label("timestamp", author.timestamp())
            " " branches
            " " tags
            " " checkouts
            if(is_git_head, label("git_head", " HEAD@git"))
            if(divergent, label("divergent", " divergent"))
            if(conflict, label("conflict", " conflict"))
            "\n"
            description.first_line()
            "\n""#;
    let default_template = if settings.enable_open_commits() {
        format!(
            r#"
            label(if(open, "open"), {default_template})"#
        )
    } else {
        String::from(default_template)
    };
    settings
        .config()
        .get_string("template.log.graph")
        .unwrap_or(default_template)
}

fn cmd_log(ui: &mut Ui, command: &CommandHelper, args: &LogArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;

    let revset_expression = revset::parse(&args.revisions)?;
    let repo = workspace_command.repo();
    let workspace_id = workspace_command.workspace_id();
    let checkout_id = repo.view().get_checkout(&workspace_id);
    let revset = revset_expression.evaluate(repo.as_repo_ref(), Some(&workspace_id))?;
    let store = repo.store();
    let diff_format = (args.patch || args.diff_format.git || args.diff_format.summary)
        .then(|| diff_format_for(ui, &args.diff_format));

    let template_string = match &args.template {
        Some(value) => value.to_string(),
        None => log_template(ui.settings()),
    };
    let template = crate::template_parser::parse_commit_template(
        repo.as_repo_ref(),
        &workspace_id,
        &template_string,
    );

    let mut formatter = ui.stdout_formatter();
    let mut formatter = formatter.as_mut();
    formatter.add_label(String::from("log"))?;

    if !args.no_graph {
        let mut graph = AsciiGraphDrawer::new(&mut formatter);
        let iter: Box<dyn Iterator<Item = (IndexEntry, Vec<RevsetGraphEdge>)>> = if args.reversed {
            Box::new(revset.iter().graph().reversed())
        } else {
            Box::new(revset.iter().graph())
        };
        for (index_entry, edges) in iter {
            let mut graphlog_edges = vec![];
            // TODO: Should we update RevsetGraphIterator to yield this flag instead of all
            // the missing edges since we don't care about where they point here
            // anyway?
            let mut has_missing = false;
            for edge in edges {
                match edge.edge_type {
                    RevsetGraphEdgeType::Missing => {
                        has_missing = true;
                    }
                    RevsetGraphEdgeType::Direct => graphlog_edges.push(Edge::Present {
                        direct: true,
                        target: edge.target,
                    }),
                    RevsetGraphEdgeType::Indirect => graphlog_edges.push(Edge::Present {
                        direct: false,
                        target: edge.target,
                    }),
                }
            }
            if has_missing {
                graphlog_edges.push(Edge::Missing);
            }
            let mut buffer = vec![];
            let commit_id = index_entry.commit_id();
            let commit = store.get_commit(&commit_id)?;
            let is_checkout = Some(&commit_id) == checkout_id;
            {
                let writer = Box::new(&mut buffer);
                let mut formatter = ui.new_formatter(writer);
                if is_checkout {
                    formatter.add_label("checkout".to_string())?;
                }
                template.format(&commit, formatter.as_mut())?;
                if is_checkout {
                    formatter.remove_label()?;
                }
            }
            if !buffer.ends_with(b"\n") {
                buffer.push(b'\n');
            }
            if let Some(diff_format) = diff_format {
                let writer = Box::new(&mut buffer);
                let mut formatter = ui.new_formatter(writer);
                show_patch(formatter.as_mut(), &workspace_command, &commit, diff_format)?;
            }
            let node_symbol = if is_checkout { b"@" } else { b"o" };
            graph.add_node(
                &index_entry.position(),
                &graphlog_edges,
                node_symbol,
                &buffer,
            )?;
        }
    } else {
        let iter: Box<dyn Iterator<Item = IndexEntry>> = if args.reversed {
            Box::new(revset.iter().reversed())
        } else {
            Box::new(revset.iter())
        };
        for index_entry in iter {
            let commit = store.get_commit(&index_entry.commit_id())?;
            template.format(&commit, formatter)?;
            if let Some(diff_format) = diff_format {
                show_patch(formatter, &workspace_command, &commit, diff_format)?;
            }
        }
    }

    Ok(())
}

fn show_patch(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    commit: &Commit,
    format: DiffFormat,
) -> Result<(), CommandError> {
    let parents = commit.parents();
    let from_tree = merge_commit_trees(workspace_command.repo().as_repo_ref(), &parents);
    let to_tree = commit.tree();
    let diff_iterator = from_tree.diff(&to_tree, &EverythingMatcher);
    show_diff(formatter, workspace_command, diff_iterator, format)
}

fn cmd_obslog(ui: &mut Ui, command: &CommandHelper, args: &ObslogArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;

    let start_commit = workspace_command.resolve_single_rev(&args.revision)?;
    let workspace_id = workspace_command.workspace_id();
    let checkout_id = workspace_command.repo().view().get_checkout(&workspace_id);

    let diff_format = (args.patch || args.diff_format.git || args.diff_format.summary)
        .then(|| diff_format_for(ui, &args.diff_format));

    let template_string = match &args.template {
        Some(value) => value.to_string(),
        None => log_template(ui.settings()),
    };
    let template = crate::template_parser::parse_commit_template(
        workspace_command.repo().as_repo_ref(),
        &workspace_id,
        &template_string,
    );

    let mut formatter = ui.stdout_formatter();
    let mut formatter = formatter.as_mut();
    formatter.add_label(String::from("log"))?;

    let commits = topo_order_reverse(
        vec![start_commit],
        Box::new(|commit: &Commit| commit.id().clone()),
        Box::new(|commit: &Commit| commit.predecessors()),
    );
    if !args.no_graph {
        let mut graph = AsciiGraphDrawer::new(&mut formatter);
        for commit in commits {
            let mut edges = vec![];
            for predecessor in &commit.predecessors() {
                edges.push(Edge::direct(predecessor.id().clone()));
            }
            let mut buffer = vec![];
            {
                let writer = Box::new(&mut buffer);
                let mut formatter = ui.new_formatter(writer);
                template.format(&commit, formatter.as_mut())?;
            }
            if !buffer.ends_with(b"\n") {
                buffer.push(b'\n');
            }
            if let Some(diff_format) = diff_format {
                let writer = Box::new(&mut buffer);
                let mut formatter = ui.new_formatter(writer);
                show_predecessor_patch(
                    formatter.as_mut(),
                    &workspace_command,
                    &commit,
                    diff_format,
                )?;
            }
            let node_symbol = if Some(commit.id()) == checkout_id {
                b"@"
            } else {
                b"o"
            };
            graph.add_node(commit.id(), &edges, node_symbol, &buffer)?;
        }
    } else {
        for commit in commits {
            template.format(&commit, formatter)?;
            if let Some(diff_format) = diff_format {
                show_predecessor_patch(formatter, &workspace_command, &commit, diff_format)?;
            }
        }
    }

    Ok(())
}

fn show_predecessor_patch(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    commit: &Commit,
    diff_format: DiffFormat,
) -> Result<(), CommandError> {
    if let Some(predecessor) = commit.predecessors().first() {
        let predecessor_tree = if predecessor.parent_ids() == commit.parent_ids() {
            predecessor.tree()
        } else {
            // Rebase the predecessor to have the current commit's parent(s) and use that
            // tree as base
            let new_parent_tree =
                merge_commit_trees(workspace_command.repo().as_repo_ref(), &commit.parents());
            let old_parent_tree = merge_commit_trees(
                workspace_command.repo().as_repo_ref(),
                &predecessor.parents(),
            );
            let rebased_tree_id =
                merge_trees(&new_parent_tree, &old_parent_tree, &predecessor.tree())?;
            workspace_command
                .repo()
                .store()
                .get_tree(&RepoPath::root(), &rebased_tree_id)?
        };
        let diff_iterator = predecessor_tree.diff(&commit.tree(), &EverythingMatcher);
        show_diff(formatter, workspace_command, diff_iterator, diff_format)?;
    }
    Ok(())
}

fn edit_description(
    ui: &Ui,
    repo: &ReadonlyRepo,
    description: &str,
) -> Result<String, CommandError> {
    let random: u32 = rand::random();
    let description_file_path = repo.repo_path().join(format!("description-{}.txt", random));
    {
        let mut description_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .truncate(true)
            .open(&description_file_path)
            .unwrap_or_else(|_| panic!("failed to open {:?} for write", &description_file_path));
        description_file.write_all(description.as_bytes()).unwrap();
        description_file
            .write_all(b"\nJJ: Lines starting with \"JJ: \" (like this one) will be removed.\n")
            .unwrap();
    }

    let editor = ui
        .settings()
        .config()
        .get_string("ui.editor")
        .unwrap_or_else(|_| "pico".to_string());
    // Handle things like `EDITOR=emacs -nw`
    let args = editor.split(' ').collect_vec();
    let editor_args = if args.len() > 1 { &args[1..] } else { &[] };
    let exit_status = std::process::Command::new(args[0])
        .args(editor_args)
        .arg(&description_file_path)
        .status()
        .map_err(|_| CommandError::UserError(format!("Failed to run editor '{editor}'")))?;
    if !exit_status.success() {
        return Err(CommandError::UserError(format!(
            "Editor '{editor}' exited with an error"
        )));
    }

    let mut description_file = OpenOptions::new()
        .read(true)
        .open(&description_file_path)
        .unwrap_or_else(|_| panic!("failed to open {:?} for read", &description_file_path));
    let mut buf = vec![];
    description_file.read_to_end(&mut buf).unwrap();
    let description = String::from_utf8(buf).unwrap();
    // Delete the file only if everything went well.
    // TODO: Tell the user the name of the file we left behind.
    std::fs::remove_file(description_file_path).ok();
    let mut lines = description
        .split_inclusive('\n')
        .filter(|line| !line.starts_with("JJ: "))
        .collect_vec();
    // Remove trailing blank lines
    while matches!(lines.last(), Some(&"\n") | Some(&"\r\n")) {
        lines.pop().unwrap();
    }
    Ok(lines.join(""))
}

fn cmd_describe(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DescribeArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewriteable(&commit)?;
    let description;
    if args.stdin {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer).unwrap();
        description = buffer;
    } else if let Some(message) = &args.message {
        description = message.to_owned()
    } else {
        description = edit_description(ui, workspace_command.repo(), commit.description())?;
    }
    if description == *commit.description() {
        ui.write("Nothing changed.\n")?;
    } else {
        let mut tx =
            workspace_command.start_transaction(&format!("describe commit {}", commit.id().hex()));
        CommitBuilder::for_rewrite_from(ui.settings(), &commit)
            .set_description(description)
            .write_to_repo(tx.mut_repo());
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_open(ui: &mut Ui, command: &CommandHelper, args: &OpenArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewriteable(&commit)?;
    let mut tx = workspace_command.start_transaction(&format!("open commit {}", commit.id().hex()));
    CommitBuilder::for_rewrite_from(ui.settings(), &commit)
        .set_open(true)
        .write_to_repo(tx.mut_repo());
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_close(ui: &mut Ui, command: &CommandHelper, args: &CloseArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewriteable(&commit)?;
    let mut commit_builder =
        CommitBuilder::for_rewrite_from(ui.settings(), &commit).set_open(false);
    let description = if let Some(message) = &args.message {
        message.to_string()
    } else if commit.description().is_empty() {
        edit_description(
            ui,
            workspace_command.repo(),
            "\n\nJJ: Enter commit description.\n",
        )?
    } else if args.edit {
        edit_description(ui, workspace_command.repo(), commit.description())?
    } else {
        commit.description().to_string()
    };
    commit_builder = commit_builder.set_description(description);
    let mut tx =
        workspace_command.start_transaction(&format!("close commit {}", commit.id().hex()));
    let new_commit = commit_builder.write_to_repo(tx.mut_repo());
    let workspace_ids = tx.mut_repo().view().workspaces_for_checkout(commit.id());
    if !workspace_ids.is_empty() {
        let new_checkout = CommitBuilder::for_open_commit(
            ui.settings(),
            new_commit.id().clone(),
            new_commit.tree_id().clone(),
        )
        .write_to_repo(tx.mut_repo());
        for workspace_id in workspace_ids {
            tx.mut_repo().edit(workspace_id, &new_checkout);
        }
    }
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_duplicate(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DuplicateArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let predecessor = workspace_command.resolve_single_rev(&args.revision)?;
    let mut tx = workspace_command
        .start_transaction(&format!("duplicate commit {}", predecessor.id().hex()));
    let mut_repo = tx.mut_repo();
    let new_commit = CommitBuilder::for_rewrite_from(ui.settings(), &predecessor)
        .generate_new_change_id()
        .write_to_repo(mut_repo);
    ui.write("Created: ")?;
    ui.write_commit_summary(
        mut_repo.as_repo_ref(),
        &workspace_command.workspace_id(),
        &new_commit,
    )?;
    ui.write("\n")?;
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_abandon(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &AbandonArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let to_abandon = {
        let mut acc = Vec::new();
        for revset in &args.revisions {
            let revisions = workspace_command.resolve_revset(revset)?;
            workspace_command.check_non_empty(&revisions)?;
            for commit in &revisions {
                workspace_command.check_rewriteable(commit)?;
            }
            acc.extend(revisions);
        }
        acc
    };
    let transaction_description = if to_abandon.len() == 1 {
        format!("abandon commit {}", to_abandon[0].id().hex())
    } else {
        format!(
            "abandon commit {} and {} more",
            to_abandon[0].id().hex(),
            to_abandon.len() - 1
        )
    };
    let mut tx = workspace_command.start_transaction(&transaction_description);
    for commit in to_abandon {
        tx.mut_repo().record_abandoned_commit(commit.id().clone());
    }
    let num_rebased = tx.mut_repo().rebase_descendants(ui.settings())?;
    if num_rebased > 0 {
        writeln!(
            ui,
            "Rebased {} descendant commits onto parents of abandoned commits",
            num_rebased
        )?;
    }
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_edit(ui: &mut Ui, command: &CommandHelper, args: &EditArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let new_commit = workspace_command.resolve_single_rev(&args.revision)?;
    let workspace_id = workspace_command.workspace_id();
    if workspace_command.repo().view().get_checkout(&workspace_id) == Some(new_commit.id()) {
        ui.write("Already editing that commit\n")?;
    } else {
        let mut tx =
            workspace_command.start_transaction(&format!("edit commit {}", new_commit.id().hex()));
        tx.mut_repo().edit(workspace_id, &new_commit);
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_new(ui: &mut Ui, command: &CommandHelper, args: &NewArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let parent = workspace_command.resolve_single_rev(&args.revision)?;
    let commit_builder = CommitBuilder::for_open_commit(
        ui.settings(),
        parent.id().clone(),
        parent.tree_id().clone(),
    )
    .set_description(args.message.clone());
    let mut tx = workspace_command.start_transaction("new empty commit");
    let mut_repo = tx.mut_repo();
    let new_commit = commit_builder.write_to_repo(mut_repo);
    let workspace_id = workspace_command.workspace_id();
    mut_repo.edit(workspace_id, &new_commit);
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_move(ui: &mut Ui, command: &CommandHelper, args: &MoveArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let source = workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"))?;
    let mut destination =
        workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"))?;
    if source.id() == destination.id() {
        return Err(CommandError::UserError(String::from(
            "Source and destination cannot be the same.",
        )));
    }
    workspace_command.check_rewriteable(&source)?;
    workspace_command.check_rewriteable(&destination)?;
    let mut tx = workspace_command.start_transaction(&format!(
        "move changes from {} to {}",
        source.id().hex(),
        destination.id().hex()
    ));
    let mut_repo = tx.mut_repo();
    let repo = workspace_command.repo();
    let parent_tree = merge_commit_trees(repo.as_repo_ref(), &source.parents());
    let source_tree = source.tree();
    let instructions = format!(
        "\
You are moving changes from: {}
into commit: {}

The left side of the diff shows the contents of the parent commit. The
right side initially shows the contents of the commit you're moving
changes from.

Adjust the right side until the diff shows the changes you want to move
to the destination. If you don't make any changes, then all the changes
from the source will be moved into the destination.
",
        short_commit_description(&source),
        short_commit_description(&destination)
    );
    let matcher = matcher_from_values(ui, workspace_command.workspace_root(), &args.paths)?;
    let new_parent_tree_id = workspace_command.select_diff(
        ui,
        &parent_tree,
        &source_tree,
        &instructions,
        args.interactive,
        matcher.as_ref(),
    )?;
    if &new_parent_tree_id == parent_tree.id() {
        return Err(CommandError::UserError(String::from("No changes to move")));
    }
    let new_parent_tree = repo
        .store()
        .get_tree(&RepoPath::root(), &new_parent_tree_id)?;
    // Apply the reverse of the selected changes onto the source
    let new_source_tree_id = merge_trees(&source_tree, &new_parent_tree, &parent_tree)?;
    if new_source_tree_id == *parent_tree.id() && !repo.view().is_checkout(source.id()) {
        mut_repo.record_abandoned_commit(source.id().clone());
    } else {
        CommitBuilder::for_rewrite_from(ui.settings(), &source)
            .set_tree(new_source_tree_id)
            .write_to_repo(mut_repo);
    }
    if repo.index().is_ancestor(source.id(), destination.id()) {
        // If we're moving changes to a descendant, first rebase descendants onto the
        // rewritten source. Otherwise it will likely already have the content
        // changes we're moving, so applying them will have no effect and the
        // changes will disappear.
        let mut rebaser = mut_repo.create_descendant_rebaser(ui.settings());
        rebaser.rebase_all()?;
        let rebased_destination_id = rebaser.rebased().get(destination.id()).unwrap().clone();
        destination = mut_repo.store().get_commit(&rebased_destination_id)?;
    }
    // Apply the selected changes onto the destination
    let new_destination_tree_id = merge_trees(&destination.tree(), &parent_tree, &new_parent_tree)?;
    CommitBuilder::for_rewrite_from(ui.settings(), &destination)
        .set_tree(new_destination_tree_id)
        .write_to_repo(mut_repo);
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_squash(ui: &mut Ui, command: &CommandHelper, args: &SquashArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewriteable(&commit)?;
    let parents = commit.parents();
    if parents.len() != 1 {
        return Err(CommandError::UserError(String::from(
            "Cannot squash merge commits",
        )));
    }
    let parent = &parents[0];
    workspace_command.check_rewriteable(parent)?;
    let mut tx =
        workspace_command.start_transaction(&format!("squash commit {}", commit.id().hex()));
    let instructions = format!(
        "\
You are moving changes from: {}
into its parent: {}

The left side of the diff shows the contents of the parent commit. The
right side initially shows the contents of the commit you're moving
changes from.

Adjust the right side until the diff shows the changes you want to move
to the destination. If you don't make any changes, then all the changes
from the source will be moved into the parent.
",
        short_commit_description(&commit),
        short_commit_description(parent)
    );
    let matcher = matcher_from_values(ui, workspace_command.workspace_root(), &args.paths)?;
    let new_parent_tree_id = workspace_command.select_diff(
        ui,
        &parent.tree(),
        &commit.tree(),
        &instructions,
        args.interactive,
        matcher.as_ref(),
    )?;
    if &new_parent_tree_id == parent.tree_id() {
        return Err(CommandError::UserError(String::from("No changes selected")));
    }
    // Abandon the child if the parent now has all the content from the child
    // (always the case in the non-interactive case).
    let abandon_child =
        &new_parent_tree_id == commit.tree_id() && !tx.base_repo().view().is_checkout(commit.id());
    let mut_repo = tx.mut_repo();
    let new_parent = CommitBuilder::for_rewrite_from(ui.settings(), parent)
        .set_tree(new_parent_tree_id)
        .set_predecessors(vec![parent.id().clone(), commit.id().clone()])
        .write_to_repo(mut_repo);
    if abandon_child {
        mut_repo.record_abandoned_commit(commit.id().clone());
    } else {
        // Commit the remainder on top of the new parent commit.
        CommitBuilder::for_rewrite_from(ui.settings(), &commit)
            .set_parents(vec![new_parent.id().clone()])
            .write_to_repo(mut_repo);
    }
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_unsquash(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &UnsquashArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewriteable(&commit)?;
    let parents = commit.parents();
    if parents.len() != 1 {
        return Err(CommandError::UserError(String::from(
            "Cannot unsquash merge commits",
        )));
    }
    let parent = &parents[0];
    workspace_command.check_rewriteable(parent)?;
    let mut tx =
        workspace_command.start_transaction(&format!("unsquash commit {}", commit.id().hex()));
    let parent_base_tree =
        merge_commit_trees(workspace_command.repo().as_repo_ref(), &parent.parents());
    let new_parent_tree_id;
    if args.interactive {
        let instructions = format!(
            "\
You are moving changes from: {}
into its child: {}

The diff initially shows the parent commit's changes.

Adjust the right side until it shows the contents you want to keep in
the parent commit. The changes you edited out will be moved into the
child commit. If you don't make any changes, then the operation will be
aborted.
",
            short_commit_description(parent),
            short_commit_description(&commit)
        );
        new_parent_tree_id =
            workspace_command.edit_diff(ui, &parent_base_tree, &parent.tree(), &instructions)?;
        if &new_parent_tree_id == parent_base_tree.id() {
            return Err(CommandError::UserError(String::from("No changes selected")));
        }
    } else {
        new_parent_tree_id = parent_base_tree.id().clone();
    }
    // Abandon the parent if it is now empty (always the case in the non-interactive
    // case).
    if &new_parent_tree_id == parent_base_tree.id()
        && !tx.base_repo().view().is_checkout(parent.id())
    {
        tx.mut_repo().record_abandoned_commit(parent.id().clone());
        // Commit the new child on top of the parent's parents.
        CommitBuilder::for_rewrite_from(ui.settings(), &commit)
            .set_parents(parent.parent_ids())
            .write_to_repo(tx.mut_repo());
    } else {
        let new_parent = CommitBuilder::for_rewrite_from(ui.settings(), parent)
            .set_tree(new_parent_tree_id)
            .set_predecessors(vec![parent.id().clone(), commit.id().clone()])
            .write_to_repo(tx.mut_repo());
        // Commit the new child on top of the new parent.
        CommitBuilder::for_rewrite_from(ui.settings(), &commit)
            .set_parents(vec![new_parent.id().clone()])
            .write_to_repo(tx.mut_repo());
    }
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_restore(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &RestoreArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let (from_str, to_str) = match (args.from.as_deref(), args.to.as_deref()) {
        (None, None) => ("@-", "@"),
        (Some(from), None) => (from, "@"),
        (None, Some(to)) => ("@", to),
        (Some(from), Some(to)) => (from, to),
    };
    let from_commit = workspace_command.resolve_single_rev(from_str)?;
    let to_commit = workspace_command.resolve_single_rev(to_str)?;
    workspace_command.check_rewriteable(&to_commit)?;
    let tree_id;
    if args.interactive {
        let instructions = format!(
            "\
You are restoring state from: {}
into: {}

The left side of the diff shows the contents of the commit you're
restoring from. The right side initially shows the contents of the
commit you're restoring into.

Adjust the right side until it has the changes you wanted from the left
side. If you don't make any changes, then the operation will be aborted.
",
            short_commit_description(&from_commit),
            short_commit_description(&to_commit)
        );
        tree_id = workspace_command.edit_diff(
            ui,
            &from_commit.tree(),
            &to_commit.tree(),
            &instructions,
        )?;
    } else if !args.paths.is_empty() {
        let matcher = matcher_from_values(ui, workspace_command.workspace_root(), &args.paths)?;
        let mut tree_builder = workspace_command
            .repo()
            .store()
            .tree_builder(to_commit.tree_id().clone());
        for (repo_path, diff) in from_commit.tree().diff(&to_commit.tree(), matcher.as_ref()) {
            match diff.into_options().0 {
                Some(value) => {
                    tree_builder.set(repo_path, value);
                }
                None => {
                    tree_builder.remove(repo_path);
                }
            }
        }
        tree_id = tree_builder.write_tree();
    } else {
        tree_id = from_commit.tree_id().clone();
    }
    if &tree_id == to_commit.tree_id() {
        ui.write("Nothing changed.\n")?;
    } else {
        let mut tx = workspace_command
            .start_transaction(&format!("restore into commit {}", to_commit.id().hex()));
        let mut_repo = tx.mut_repo();
        let new_commit = CommitBuilder::for_rewrite_from(ui.settings(), &to_commit)
            .set_tree(tree_id)
            .write_to_repo(mut_repo);
        ui.write("Created ")?;
        ui.write_commit_summary(
            mut_repo.as_repo_ref(),
            &workspace_command.workspace_id(),
            &new_commit,
        )?;
        ui.write("\n")?;
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_touchup(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &TouchupArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewriteable(&commit)?;
    let base_tree = merge_commit_trees(workspace_command.repo().as_repo_ref(), &commit.parents());
    let instructions = format!(
        "\
You are editing changes in: {}

The diff initially shows the commit's changes.

Adjust the right side until it shows the contents you want. If you
don't make any changes, then the operation will be aborted.",
        short_commit_description(&commit)
    );
    let tree_id = workspace_command.edit_diff(ui, &base_tree, &commit.tree(), &instructions)?;
    if &tree_id == commit.tree_id() {
        ui.write("Nothing changed.\n")?;
    } else {
        let mut tx =
            workspace_command.start_transaction(&format!("edit commit {}", commit.id().hex()));
        let mut_repo = tx.mut_repo();
        let new_commit = CommitBuilder::for_rewrite_from(ui.settings(), &commit)
            .set_tree(tree_id)
            .write_to_repo(mut_repo);
        ui.write("Created ")?;
        ui.write_commit_summary(
            mut_repo.as_repo_ref(),
            &workspace_command.workspace_id(),
            &new_commit,
        )?;
        ui.write("\n")?;
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_split(ui: &mut Ui, command: &CommandHelper, args: &SplitArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    workspace_command.check_rewriteable(&commit)?;
    let base_tree = merge_commit_trees(workspace_command.repo().as_repo_ref(), &commit.parents());
    let instructions = format!(
        "\
You are splitting a commit in two: {}

The diff initially shows the changes in the commit you're splitting.

Adjust the right side until it shows the contents you want for the first
commit. The remainder will be in the second commit. If you don't make
any changes, then the operation will be aborted.
",
        short_commit_description(&commit)
    );
    let matcher = matcher_from_values(ui, workspace_command.workspace_root(), &args.paths)?;
    let tree_id = workspace_command.select_diff(
        ui,
        &base_tree,
        &commit.tree(),
        &instructions,
        args.paths.is_empty(),
        matcher.as_ref(),
    )?;
    if &tree_id == commit.tree_id() {
        ui.write("Nothing changed.\n")?;
    } else {
        let mut tx =
            workspace_command.start_transaction(&format!("split commit {}", commit.id().hex()));
        let first_description = edit_description(
            ui,
            tx.base_repo(),
            &("JJ: Enter commit description for the first part.\n".to_string()
                + commit.description()),
        )?;
        let first_commit = CommitBuilder::for_rewrite_from(ui.settings(), &commit)
            .set_tree(tree_id)
            .set_description(first_description)
            .write_to_repo(tx.mut_repo());
        let second_description = edit_description(
            ui,
            tx.base_repo(),
            &("JJ: Enter commit description for the second part.\n".to_string()
                + commit.description()),
        )?;
        let second_commit = CommitBuilder::for_rewrite_from(ui.settings(), &commit)
            .set_parents(vec![first_commit.id().clone()])
            .set_tree(commit.tree_id().clone())
            .generate_new_change_id()
            .set_description(second_description)
            .write_to_repo(tx.mut_repo());
        let mut rebaser = DescendantRebaser::new(
            ui.settings(),
            tx.mut_repo(),
            hashmap! { commit.id().clone() => hashset!{second_commit.id().clone()} },
            hashset! {},
        );
        rebaser.rebase_all()?;
        let num_rebased = rebaser.rebased().len();
        if num_rebased > 0 {
            writeln!(ui, "Rebased {} descendant commits", num_rebased)?;
        }
        ui.write("First part: ")?;
        ui.write_commit_summary(
            tx.repo().as_repo_ref(),
            &workspace_command.workspace_id(),
            &first_commit,
        )?;
        ui.write("\nSecond part: ")?;
        ui.write_commit_summary(
            tx.repo().as_repo_ref(),
            &workspace_command.workspace_id(),
            &second_commit,
        )?;
        ui.write("\n")?;
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_merge(ui: &mut Ui, command: &CommandHelper, args: &MergeArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let revision_args = &args.revisions;
    if revision_args.len() < 2 {
        return Err(CommandError::UserError(String::from(
            "Merge requires at least two revisions",
        )));
    }
    let mut commits = vec![];
    let mut parent_ids = vec![];
    for revision_arg in revision_args {
        // TODO: Should we allow each argument to resolve to multiple revisions?
        // It would be neat to be able to do `jj merge main` when `main` is conflicted,
        // but I'm not sure it would actually be useful.
        let commit = workspace_command.resolve_single_rev(revision_arg)?;
        parent_ids.push(commit.id().clone());
        commits.push(commit);
    }
    let description = if let Some(message) = &args.message {
        message.to_string()
    } else {
        edit_description(
            ui,
            workspace_command.repo(),
            "\n\nJJ: Enter commit description for the merge commit.\n",
        )?
    };
    let merged_tree = merge_commit_trees(workspace_command.repo().as_repo_ref(), &commits);
    let mut tx = workspace_command.start_transaction("merge commits");
    CommitBuilder::for_new_commit(ui.settings(), merged_tree.id().clone())
        .set_parents(parent_ids)
        .set_description(description)
        .set_open(false)
        .write_to_repo(tx.mut_repo());
    workspace_command.finish_transaction(ui, tx)?;

    Ok(())
}

fn cmd_rebase(ui: &mut Ui, command: &CommandHelper, args: &RebaseArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let mut new_parents = vec![];
    for revision_str in &args.destination {
        let destination = workspace_command.resolve_single_rev(revision_str)?;
        new_parents.push(destination);
    }
    if let Some(rev_str) = &args.revision {
        rebase_revision(ui, &mut workspace_command, &new_parents, rev_str)?;
    } else if let Some(source_str) = &args.source {
        rebase_descendants(ui, &mut workspace_command, &new_parents, source_str)?;
    } else {
        let branch_str = args.branch.as_deref().unwrap_or("@");
        rebase_branch(ui, &mut workspace_command, &new_parents, branch_str)?;
    }
    Ok(())
}

fn rebase_branch(
    ui: &mut Ui,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parents: &[Commit],
    branch_str: &str,
) -> Result<(), CommandError> {
    let branch_commit = workspace_command.resolve_single_rev(branch_str)?;
    let mut tx = workspace_command
        .start_transaction(&format!("rebase branch at {}", branch_commit.id().hex()));
    check_rebase_destinations(workspace_command, new_parents, &branch_commit)?;

    let parent_ids = new_parents
        .iter()
        .map(|commit| commit.id().clone())
        .collect_vec();
    let roots_expression = RevsetExpression::commits(parent_ids)
        .range(&RevsetExpression::commit(branch_commit.id().clone()))
        .roots();
    let mut num_rebased = 0;
    let store = workspace_command.repo.store();
    for root_result in roots_expression
        .evaluate(
            workspace_command.repo().as_repo_ref(),
            Some(&workspace_command.workspace_id()),
        )
        .unwrap()
        .iter()
        .commits(store)
    {
        let root_commit = root_result?;
        workspace_command.check_rewriteable(&root_commit)?;
        rebase_commit(ui.settings(), tx.mut_repo(), &root_commit, new_parents);
        num_rebased += 1;
    }
    num_rebased += tx.mut_repo().rebase_descendants(ui.settings())?;
    writeln!(ui, "Rebased {} commits", num_rebased)?;
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn rebase_descendants(
    ui: &mut Ui,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parents: &[Commit],
    source_str: &str,
) -> Result<(), CommandError> {
    let old_commit = workspace_command.resolve_single_rev(source_str)?;
    workspace_command.check_rewriteable(&old_commit)?;
    check_rebase_destinations(workspace_command, new_parents, &old_commit)?;
    let mut tx = workspace_command.start_transaction(&format!(
        "rebase commit {} and descendants",
        old_commit.id().hex()
    ));
    rebase_commit(ui.settings(), tx.mut_repo(), &old_commit, new_parents);
    let num_rebased = tx.mut_repo().rebase_descendants(ui.settings())? + 1;
    writeln!(ui, "Rebased {} commits", num_rebased)?;
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn rebase_revision(
    ui: &mut Ui,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parents: &[Commit],
    rev_str: &str,
) -> Result<(), CommandError> {
    let old_commit = workspace_command.resolve_single_rev(rev_str)?;
    workspace_command.check_rewriteable(&old_commit)?;
    check_rebase_destinations(workspace_command, new_parents, &old_commit)?;
    let mut tx =
        workspace_command.start_transaction(&format!("rebase commit {}", old_commit.id().hex()));
    rebase_commit(ui.settings(), tx.mut_repo(), &old_commit, new_parents);
    // Manually rebase children because we don't want to rebase them onto the
    // rewritten commit. (But we still want to record the commit as rewritten so
    // branches and the working copy get updated to the rewritten commit.)
    let children_expression = RevsetExpression::commit(old_commit.id().clone()).children();
    let mut num_rebased_descendants = 0;
    let store = workspace_command.repo.store();
    for child_commit in children_expression
        .evaluate(
            workspace_command.repo().as_repo_ref(),
            Some(&workspace_command.workspace_id()),
        )
        .unwrap()
        .iter()
        .commits(store)
    {
        rebase_commit(
            ui.settings(),
            tx.mut_repo(),
            &child_commit?,
            &old_commit.parents(),
        );
        num_rebased_descendants += 1;
    }
    num_rebased_descendants += tx.mut_repo().rebase_descendants(ui.settings())?;
    if num_rebased_descendants > 0 {
        writeln!(
            ui,
            "Also rebased {} descendant commits onto parent of rebased commit",
            num_rebased_descendants
        )?;
    }
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn check_rebase_destinations(
    workspace_command: &WorkspaceCommandHelper,
    new_parents: &[Commit],
    commit: &Commit,
) -> Result<(), CommandError> {
    for parent in new_parents {
        if workspace_command
            .repo
            .index()
            .is_ancestor(commit.id(), parent.id())
        {
            return Err(CommandError::UserError(format!(
                "Cannot rebase {} onto descendant {}",
                short_commit_hash(commit.id()),
                short_commit_hash(parent.id())
            )));
        }
    }
    Ok(())
}

fn cmd_backout(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BackoutArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit_to_back_out = workspace_command.resolve_single_rev(&args.revision)?;
    let mut parents = vec![];
    for revision_str in &args.destination {
        let destination = workspace_command.resolve_single_rev(revision_str)?;
        parents.push(destination);
    }
    let mut tx = workspace_command.start_transaction(&format!(
        "back out commit {}",
        commit_to_back_out.id().hex()
    ));
    back_out_commit(ui.settings(), tx.mut_repo(), &commit_to_back_out, &parents);
    workspace_command.finish_transaction(ui, tx)?;

    Ok(())
}

fn is_fast_forward(repo: RepoRef, branch_name: &str, new_target_id: &CommitId) -> bool {
    if let Some(current_target) = repo.view().get_local_branch(branch_name) {
        current_target
            .adds()
            .iter()
            .any(|add| repo.index().is_ancestor(add, new_target_id))
    } else {
        true
    }
}

fn cmd_branch(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &BranchSubcommand,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let view = workspace_command.repo().view();
    fn validate_branch_names_exist<'a>(
        view: &'a View,
        names: &'a [String],
    ) -> Result<(), CommandError> {
        for branch_name in names {
            if view.get_local_branch(branch_name).is_none() {
                return Err(CommandError::UserError(format!(
                    "No such branch: {}",
                    branch_name
                )));
            }
        }
        Ok(())
    }

    fn make_branch_term(branch_names: &[impl AsRef<str>]) -> String {
        match branch_names {
            [branch_name] => format!("branch {}", branch_name.as_ref()),
            branch_names => {
                format!(
                    "branches {}",
                    branch_names.iter().map(AsRef::as_ref).join(", ")
                )
            }
        }
    }

    match subcommand {
        BranchSubcommand::Create { revision, names } => {
            let branch_names: Vec<&str> = names
                .iter()
                .map(|branch_name| match view.get_local_branch(branch_name) {
                    Some(_) => Err(CommandError::UserError(format!(
                        "Branch already exists: {} (use `jj branch set` to update it)",
                        branch_name
                    ))),
                    None => Ok(branch_name.as_str()),
                })
                .try_collect()?;

            if branch_names.len() > 1 {
                ui.write_warn(format!(
                    "warning: Creating multiple branches ({}).\n",
                    branch_names.len()
                ))?;
            }

            let target_commit =
                workspace_command.resolve_single_rev(revision.as_deref().unwrap_or("@"))?;
            let mut tx = workspace_command.start_transaction(&format!(
                "create {} pointing to commit {}",
                make_branch_term(&branch_names),
                target_commit.id().hex()
            ));
            for branch_name in branch_names {
                tx.mut_repo().set_local_branch(
                    branch_name.to_string(),
                    RefTarget::Normal(target_commit.id().clone()),
                );
            }
            workspace_command.finish_transaction(ui, tx)?;
        }

        BranchSubcommand::Set {
            revision,
            allow_backwards,
            names: branch_names,
        } => {
            if branch_names.len() > 1 {
                ui.write_warn(format!(
                    "warning: Updating multiple branches ({}).\n",
                    branch_names.len()
                ))?;
            }

            let target_commit =
                workspace_command.resolve_single_rev(revision.as_deref().unwrap_or("@"))?;
            if !allow_backwards
                && !branch_names.iter().all(|branch_name| {
                    is_fast_forward(
                        workspace_command.repo().as_repo_ref(),
                        branch_name,
                        target_commit.id(),
                    )
                })
            {
                return Err(CommandError::UserError(
                    "Use --allow-backwards to allow moving a branch backwards or sideways"
                        .to_string(),
                ));
            }
            let mut tx = workspace_command.start_transaction(&format!(
                "point {} to commit {}",
                make_branch_term(branch_names),
                target_commit.id().hex()
            ));
            for branch_name in branch_names {
                tx.mut_repo().set_local_branch(
                    branch_name.to_string(),
                    RefTarget::Normal(target_commit.id().clone()),
                );
            }
            workspace_command.finish_transaction(ui, tx)?;
        }

        BranchSubcommand::Delete { names } => {
            validate_branch_names_exist(view, names)?;
            let mut tx =
                workspace_command.start_transaction(&format!("delete {}", make_branch_term(names)));
            for branch_name in names {
                tx.mut_repo().remove_local_branch(branch_name);
            }
            workspace_command.finish_transaction(ui, tx)?;
        }

        BranchSubcommand::Forget { names } => {
            validate_branch_names_exist(view, names)?;
            let mut tx =
                workspace_command.start_transaction(&format!("forget {}", make_branch_term(names)));
            for branch_name in names {
                tx.mut_repo().remove_branch(branch_name);
            }
            workspace_command.finish_transaction(ui, tx)?;
        }

        BranchSubcommand::List => {
            list_branches(ui, &workspace_command)?;
        }
    }

    Ok(())
}

fn list_branches(
    ui: &mut Ui,
    workspace_command: &WorkspaceCommandHelper,
) -> Result<(), CommandError> {
    let repo = workspace_command.repo();

    let workspace_id = workspace_command.workspace_id();
    let print_branch_target =
        |ui: &mut Ui, target: Option<&RefTarget>| -> Result<(), CommandError> {
            match target {
                Some(RefTarget::Normal(id)) => {
                    write!(ui, ": ")?;
                    let commit = repo.store().get_commit(id)?;
                    ui.write_commit_summary(repo.as_repo_ref(), &workspace_id, &commit)?;
                    writeln!(ui)?;
                }
                Some(RefTarget::Conflict { adds, removes }) => {
                    write!(ui, " ")?;
                    ui.stdout_formatter().add_label("conflict".to_string())?;
                    write!(ui, "(conflicted)")?;
                    ui.stdout_formatter().remove_label()?;
                    writeln!(ui, ":")?;
                    for id in removes {
                        let commit = repo.store().get_commit(id)?;
                        write!(ui, "  - ")?;
                        ui.write_commit_summary(repo.as_repo_ref(), &workspace_id, &commit)?;
                        writeln!(ui)?;
                    }
                    for id in adds {
                        let commit = repo.store().get_commit(id)?;
                        write!(ui, "  + ")?;
                        ui.write_commit_summary(repo.as_repo_ref(), &workspace_id, &commit)?;
                        writeln!(ui)?;
                    }
                }
                None => {
                    writeln!(ui, " (deleted)")?;
                }
            }
            Ok(())
        };

    let index = repo.index();
    for (name, branch_target) in repo.view().branches() {
        ui.stdout_formatter().add_label("branch".to_string())?;
        write!(ui, "{}", name)?;
        ui.stdout_formatter().remove_label()?;
        print_branch_target(ui, branch_target.local_target.as_ref())?;

        for (remote, remote_target) in branch_target
            .remote_targets
            .iter()
            .sorted_by_key(|(name, _target)| name.to_owned())
        {
            if Some(remote_target) == branch_target.local_target.as_ref() {
                continue;
            }
            write!(ui, "  ")?;
            ui.stdout_formatter().add_label("branch".to_string())?;
            write!(ui, "@{}", remote)?;
            ui.stdout_formatter().remove_label()?;
            if let Some(local_target) = branch_target.local_target.as_ref() {
                let remote_ahead_count = index
                    .walk_revs(&remote_target.adds(), &local_target.adds())
                    .count();
                let local_ahead_count = index
                    .walk_revs(&local_target.adds(), &remote_target.adds())
                    .count();
                if remote_ahead_count != 0 && local_ahead_count == 0 {
                    write!(ui, " (ahead by {} commits)", remote_ahead_count)?;
                } else if remote_ahead_count == 0 && local_ahead_count != 0 {
                    write!(ui, " (behind by {} commits)", local_ahead_count)?;
                } else if remote_ahead_count != 0 && local_ahead_count != 0 {
                    write!(
                        ui,
                        " (ahead by {} commits, behind by {} commits)",
                        remote_ahead_count, local_ahead_count
                    )?;
                }
            }
            print_branch_target(ui, Some(remote_target))?;
        }
    }

    Ok(())
}

fn cmd_debug(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &DebugCommands,
) -> Result<(), CommandError> {
    match subcommand {
        DebugCommands::Completion(completion_matches) => {
            let mut app = command.app.clone();
            let mut buf = vec![];
            let shell = if completion_matches.zsh {
                clap_complete::Shell::Zsh
            } else if completion_matches.fish {
                clap_complete::Shell::Fish
            } else {
                clap_complete::Shell::Bash
            };
            clap_complete::generate(shell, &mut app, "jj", &mut buf);
            ui.stdout_formatter().write_all(&buf)?;
        }
        DebugCommands::Mangen(_mangen_matches) => {
            let mut buf = vec![];
            let man = clap_mangen::Man::new(command.app.clone());
            man.render(&mut buf)?;
            ui.stdout_formatter().write_all(&buf)?;
        }
        DebugCommands::ResolveRev(resolve_matches) => {
            let workspace_command = command.workspace_helper(ui)?;
            let commit = workspace_command.resolve_single_rev(&resolve_matches.revision)?;
            writeln!(ui, "{}", commit.id().hex())?;
        }
        DebugCommands::WorkingCopy(_wc_matches) => {
            let workspace_command = command.workspace_helper(ui)?;
            let wc = workspace_command.working_copy();
            writeln!(ui, "Current operation: {:?}", wc.operation_id())?;
            writeln!(ui, "Current tree: {:?}", wc.current_tree_id())?;
            for (file, state) in wc.file_states() {
                writeln!(
                    ui,
                    "{:?} {:13?} {:10?} {:?}",
                    state.file_type, state.size, state.mtime.0, file
                )?;
            }
        }
        DebugCommands::Template(template_matches) => {
            let parse = TemplateParser::parse(
                crate::template_parser::Rule::template,
                &template_matches.template,
            );
            writeln!(ui, "{:?}", parse)?;
        }
        DebugCommands::Index(_index_matches) => {
            let workspace_command = command.workspace_helper(ui)?;
            let stats = workspace_command.repo().index().stats();
            writeln!(ui, "Number of commits: {}", stats.num_commits)?;
            writeln!(ui, "Number of merges: {}", stats.num_merges)?;
            writeln!(ui, "Max generation number: {}", stats.max_generation_number)?;
            writeln!(ui, "Number of heads: {}", stats.num_heads)?;
            writeln!(ui, "Number of changes: {}", stats.num_changes)?;
            writeln!(ui, "Stats per level:")?;
            for (i, level) in stats.levels.iter().enumerate() {
                writeln!(ui, "  Level {}:", i)?;
                writeln!(ui, "    Number of commits: {}", level.num_commits)?;
                writeln!(ui, "    Name: {}", level.name.as_ref().unwrap())?;
            }
        }
        DebugCommands::ReIndex(_reindex_matches) => {
            let mut workspace_command = command.workspace_helper(ui)?;
            let mut_repo = Arc::get_mut(workspace_command.repo_mut()).unwrap();
            let index = mut_repo.reindex();
            writeln!(ui, "Finished indexing {:?} commits.", index.num_commits())?;
        }
        DebugCommands::Operation(operation_args) => {
            let workspace_command = command.workspace_helper(ui)?;
            let op = workspace_command.resolve_single_op(&operation_args.operation)?;
            writeln!(ui, "{:#?}", op.store_operation())?;
            writeln!(ui, "{:#?}", op.view().store_view())?;
        }
    }
    Ok(())
}

fn run_bench<R, O>(ui: &mut Ui, id: &str, mut routine: R) -> io::Result<()>
where
    R: (FnMut() -> O) + Copy,
    O: Debug,
{
    let mut criterion = Criterion::default();
    let before = Instant::now();
    let result = routine();
    let after = Instant::now();
    writeln!(
        ui,
        "First run took {:?} and produced: {:?}",
        after.duration_since(before),
        result
    )?;
    criterion.bench_function(id, |bencher: &mut criterion::Bencher| {
        bencher.iter(routine);
    });
    Ok(())
}

fn cmd_bench(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &BenchCommands,
) -> Result<(), CommandError> {
    match subcommand {
        BenchCommands::CommonAncestors(command_matches) => {
            let workspace_command = command.workspace_helper(ui)?;
            let commit1 = workspace_command.resolve_single_rev(&command_matches.revision1)?;
            let commit2 = workspace_command.resolve_single_rev(&command_matches.revision2)?;
            let index = workspace_command.repo().index();
            let routine =
                || index.common_ancestors(&[commit1.id().clone()], &[commit2.id().clone()]);
            run_bench(
                ui,
                &format!(
                    "commonancestors-{}-{}",
                    &command_matches.revision1, &command_matches.revision2
                ),
                routine,
            )?;
        }
        BenchCommands::IsAncestor(command_matches) => {
            let workspace_command = command.workspace_helper(ui)?;
            let ancestor_commit =
                workspace_command.resolve_single_rev(&command_matches.ancestor)?;
            let descendant_commit =
                workspace_command.resolve_single_rev(&command_matches.descendant)?;
            let index = workspace_command.repo().index();
            let routine = || index.is_ancestor(ancestor_commit.id(), descendant_commit.id());
            run_bench(
                ui,
                &format!(
                    "isancestor-{}-{}",
                    &command_matches.ancestor, &command_matches.descendant
                ),
                routine,
            )?;
        }
        BenchCommands::WalkRevs(command_matches) => {
            let workspace_command = command.workspace_helper(ui)?;
            let unwanted_commit =
                workspace_command.resolve_single_rev(&command_matches.unwanted)?;
            let wanted_commit = workspace_command.resolve_single_rev(&command_matches.wanted)?;
            let index = workspace_command.repo().index();
            let routine = || {
                index
                    .walk_revs(
                        &[wanted_commit.id().clone()],
                        &[unwanted_commit.id().clone()],
                    )
                    .count()
            };
            run_bench(
                ui,
                &format!(
                    "walkrevs-{}-{}",
                    &command_matches.unwanted, &command_matches.wanted
                ),
                routine,
            )?;
        }
        BenchCommands::ResolvePrefix(command_matches) => {
            let workspace_command = command.workspace_helper(ui)?;
            let prefix = HexPrefix::new(command_matches.prefix.clone()).unwrap();
            let index = workspace_command.repo().index();
            let routine = || index.resolve_prefix(&prefix);
            run_bench(ui, &format!("resolveprefix-{}", prefix.hex()), routine)?;
        }
    }
    Ok(())
}

fn format_timestamp(timestamp: &Timestamp) -> String {
    let utc = Utc
        .timestamp(
            timestamp.timestamp.0 as i64 / 1000,
            (timestamp.timestamp.0 % 1000) as u32 * 1000000,
        )
        .with_timezone(&FixedOffset::east(timestamp.tz_offset * 60));
    utc.format("%Y-%m-%d %H:%M:%S.%3f %:z").to_string()
}

fn cmd_op_log(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &OperationLogArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let head_op = repo.operation().clone();
    let head_op_id = head_op.id().clone();
    let mut formatter = ui.stdout_formatter();
    let mut formatter = formatter.as_mut();
    struct OpTemplate;
    impl Template<Operation> for OpTemplate {
        fn format(&self, op: &Operation, formatter: &mut dyn Formatter) -> io::Result<()> {
            // TODO: Make this templated
            formatter.add_label("id".to_string())?;
            formatter.write_str(&op.id().hex()[0..12])?;
            formatter.remove_label()?;
            formatter.write_str(" ")?;
            let metadata = &op.store_operation().metadata;
            formatter.add_label("user".to_string())?;
            formatter.write_str(&format!("{}@{}", metadata.username, metadata.hostname))?;
            formatter.remove_label()?;
            formatter.write_str(" ")?;
            formatter.add_label("time".to_string())?;
            formatter.write_str(&format!(
                "{} - {}",
                format_timestamp(&metadata.start_time),
                format_timestamp(&metadata.end_time)
            ))?;
            formatter.remove_label()?;
            formatter.write_str("\n")?;
            formatter.add_label("description".to_string())?;
            formatter.write_str(&metadata.description)?;
            formatter.remove_label()?;
            for (key, value) in &metadata.tags {
                formatter.add_label("tags".to_string())?;
                formatter.write_str(&format!("\n{}: {}", key, value))?;
                formatter.remove_label()?;
            }
            Ok(())
        }
    }
    let template = OpTemplate;

    let mut graph = AsciiGraphDrawer::new(&mut formatter);
    for op in topo_order_reverse(
        vec![head_op],
        Box::new(|op: &Operation| op.id().clone()),
        Box::new(|op: &Operation| op.parents()),
    ) {
        let mut edges = vec![];
        for parent in op.parents() {
            edges.push(Edge::direct(parent.id().clone()));
        }
        let is_head_op = op.id() == &head_op_id;
        let mut buffer = vec![];
        {
            let writer = Box::new(&mut buffer);
            let mut formatter = ui.new_formatter(writer);
            formatter.add_label("op-log".to_string())?;
            if is_head_op {
                formatter.add_label("head".to_string())?;
            }
            template.format(&op, formatter.as_mut())?;
            if is_head_op {
                formatter.remove_label()?;
            }
            formatter.remove_label()?;
        }
        if !buffer.ends_with(b"\n") {
            buffer.push(b'\n');
        }
        let node_symbol = if is_head_op { b"@" } else { b"o" };
        graph.add_node(op.id(), &edges, node_symbol, &buffer)?;
    }

    Ok(())
}

fn cmd_op_undo(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationUndoArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let bad_op = workspace_command.resolve_single_op(&args.operation)?;
    let parent_ops = bad_op.parents();
    if parent_ops.len() > 1 {
        return Err(CommandError::UserError(
            "Cannot undo a merge operation".to_string(),
        ));
    }
    if parent_ops.is_empty() {
        return Err(CommandError::UserError(
            "Cannot undo repo initialization".to_string(),
        ));
    }

    let mut tx =
        workspace_command.start_transaction(&format!("undo operation {}", bad_op.id().hex()));
    let repo_loader = workspace_command.repo().loader();
    let bad_repo = repo_loader.load_at(&bad_op);
    let parent_repo = repo_loader.load_at(&parent_ops[0]);
    tx.mut_repo().merge(&bad_repo, &parent_repo);
    workspace_command.finish_transaction(ui, tx)?;

    Ok(())
}

fn cmd_op_restore(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &OperationRestoreArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let target_op = workspace_command.resolve_single_op(&args.operation)?;
    let mut tx = workspace_command
        .start_transaction(&format!("restore to operation {}", target_op.id().hex()));
    tx.mut_repo().set_view(target_op.view().take_store_view());
    workspace_command.finish_transaction(ui, tx)?;

    Ok(())
}

fn cmd_operation(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &OperationCommands,
) -> Result<(), CommandError> {
    match subcommand {
        OperationCommands::Log(command_matches) => cmd_op_log(ui, command, command_matches),
        OperationCommands::Restore(command_matches) => cmd_op_restore(ui, command, command_matches),
        OperationCommands::Undo(command_matches) => cmd_op_undo(ui, command, command_matches),
    }
}

fn cmd_workspace(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &WorkspaceCommands,
) -> Result<(), CommandError> {
    match subcommand {
        WorkspaceCommands::Add(command_matches) => cmd_workspace_add(ui, command, command_matches),
        WorkspaceCommands::Forget(command_matches) => {
            cmd_workspace_forget(ui, command, command_matches)
        }
        WorkspaceCommands::List(command_matches) => {
            cmd_workspace_list(ui, command, command_matches)
        }
    }
}

fn cmd_workspace_add(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &WorkspaceAddArgs,
) -> Result<(), CommandError> {
    let old_workspace_command = command.workspace_helper(ui)?;
    let destination_path = ui.cwd().join(&args.destination);
    if destination_path.exists() {
        return Err(CommandError::UserError(
            "Workspace already exists".to_string(),
        ));
    } else {
        fs::create_dir(&destination_path).unwrap();
    }
    let name = if let Some(name) = &args.name {
        name.to_string()
    } else {
        destination_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    };
    let workspace_id = WorkspaceId::new(name.clone());
    let repo = old_workspace_command.repo();
    if repo.view().get_checkout(&workspace_id).is_some() {
        return Err(UserError(format!(
            "Workspace named '{}' already exists",
            name
        )));
    }
    let (new_workspace, repo) = Workspace::init_workspace_with_existing_repo(
        ui.settings(),
        destination_path.clone(),
        repo,
        workspace_id,
    )?;
    writeln!(
        ui,
        "Created workspace in \"{}\"",
        ui::relative_path(old_workspace_command.workspace_root(), &destination_path).display()
    )?;

    let mut new_workspace_command = WorkspaceCommandHelper::for_loaded_repo(
        ui,
        new_workspace,
        command.string_args.clone(),
        command.global_args(),
        repo,
    )?;
    let mut tx = new_workspace_command
        .start_transaction(&format!("Initial checkout in workspace {}", &name));
    // Check out a parent of the checkout of the current workspace, or the root if
    // there is no checkout in the current workspace.
    let new_checkout_commit = if let Some(old_checkout_id) = new_workspace_command
        .repo()
        .view()
        .get_checkout(&old_workspace_command.workspace_id())
    {
        new_workspace_command
            .repo()
            .store()
            .get_commit(old_checkout_id)?
            .parents()[0]
            .clone()
    } else {
        new_workspace_command.repo().store().root_commit()
    };
    tx.mut_repo().check_out(
        new_workspace_command.workspace_id(),
        ui.settings(),
        &new_checkout_commit,
    );
    new_workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_workspace_forget(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &WorkspaceForgetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let workspace_id = if let Some(workspace_str) = &args.workspace {
        WorkspaceId::new(workspace_str.to_string())
    } else {
        workspace_command.workspace_id()
    };
    if workspace_command
        .repo()
        .view()
        .get_checkout(&workspace_id)
        .is_none()
    {
        return Err(UserError("No such workspace".to_string()));
    }

    let mut tx =
        workspace_command.start_transaction(&format!("forget workspace {}", workspace_id.as_str()));
    tx.mut_repo().remove_checkout(&workspace_id);
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_workspace_list(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &WorkspaceListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    for (workspace_id, checkout_id) in repo.view().checkouts().iter().sorted() {
        write!(ui, "{}: ", workspace_id.as_str())?;
        let commit = repo.store().get_commit(checkout_id)?;
        ui.write_commit_summary(repo.as_repo_ref(), workspace_id, &commit)?;
        writeln!(ui)?;
    }
    Ok(())
}

fn cmd_sparse(ui: &mut Ui, command: &CommandHelper, args: &SparseArgs) -> Result<(), CommandError> {
    if args.list {
        let workspace_command = command.workspace_helper(ui)?;
        for path in workspace_command.working_copy().sparse_patterns() {
            let ui_path = workspace_command.format_file_path(&path);
            writeln!(ui, "{}", ui_path)?;
        }
    } else {
        let mut workspace_command = command.workspace_helper(ui)?;
        let workspace_root = workspace_command.workspace_root().clone();
        let paths_to_add = repo_paths_from_values(ui, &workspace_root, &args.add)?;
        let (mut locked_wc, _current_checkout) = workspace_command.start_working_copy_mutation()?;
        let mut new_patterns = HashSet::new();
        if args.reset {
            new_patterns.insert(RepoPath::root());
        } else {
            if !args.clear {
                new_patterns.extend(locked_wc.sparse_patterns());
                let paths_to_remove = repo_paths_from_values(ui, &workspace_root, &args.remove)?;
                for path in paths_to_remove {
                    new_patterns.remove(&path);
                }
            }
            for path in paths_to_add {
                new_patterns.insert(path);
            }
        }
        let new_patterns = new_patterns.into_iter().sorted().collect();
        let stats = locked_wc.set_sparse_patterns(new_patterns).map_err(|err| {
            CommandError::InternalError(format!("Failed to update working copy paths: {err}"))
        })?;
        let operation_id = locked_wc.old_operation_id().clone();
        locked_wc.finish(operation_id);
        print_checkout_stats(ui, stats)?;
    }
    Ok(())
}

fn get_git_repo(store: &Store) -> Result<git2::Repository, CommandError> {
    match store.git_repo() {
        None => Err(CommandError::UserError(
            "The repo is not backed by a git repo".to_string(),
        )),
        Some(git_repo) => Ok(git_repo),
    }
}

fn cmd_git_remote_add(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitRemoteAddArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    if git_repo.find_remote(&args.remote).is_ok() {
        return Err(CommandError::UserError("Remote already exists".to_string()));
    }
    git_repo
        .remote(&args.remote, &args.url)
        .map_err(|err| CommandError::UserError(err.to_string()))?;
    Ok(())
}

fn cmd_git_remote_remove(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitRemoteRemoveArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    if git_repo.find_remote(&args.remote).is_err() {
        return Err(CommandError::UserError("Remote doesn't exist".to_string()));
    }
    git_repo
        .remote_delete(&args.remote)
        .map_err(|err| CommandError::UserError(err.to_string()))?;
    let mut branches_to_delete = vec![];
    for (branch, target) in repo.view().branches() {
        if target.remote_targets.contains_key(&args.remote) {
            branches_to_delete.push(branch.clone());
        }
    }
    if !branches_to_delete.is_empty() {
        let mut tx =
            workspace_command.start_transaction(&format!("remove git remote {}", &args.remote));
        for branch in branches_to_delete {
            tx.mut_repo().remove_remote_branch(&branch, &args.remote);
        }
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_git_remote_list(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &GitRemoteListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    for remote_name in git_repo.remotes()?.iter().flatten() {
        let remote = git_repo.find_remote(remote_name)?;
        writeln!(ui, "{} {}", remote_name, remote.url().unwrap_or("<no URL>"))?;
    }
    Ok(())
}

fn cmd_git_fetch(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitFetchArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    let mut tx =
        workspace_command.start_transaction(&format!("fetch from git remote {}", &args.remote));
    git::fetch(tx.mut_repo(), &git_repo, &args.remote)
        .map_err(|err| CommandError::UserError(err.to_string()))?;
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn clone_destination_for_source(source: &str) -> Option<&str> {
    let destination = source.strip_suffix(".git").unwrap_or(source);
    let destination = destination.strip_suffix('/').unwrap_or(destination);
    destination
        .rsplit_once(&['/', '\\', ':'][..])
        .map(|(_, name)| name)
}

fn is_empty_dir(path: &Path) -> bool {
    if let Ok(mut entries) = path.read_dir() {
        entries.next().is_none()
    } else {
        false
    }
}

fn cmd_git_clone(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitCloneArgs,
) -> Result<(), CommandError> {
    if command.global_args().repository.is_some() {
        return Err(CommandError::UserError(
            "'--repository' cannot be used with 'git clone'".to_string(),
        ));
    }
    let source = &args.source;
    let wc_path_str = args
        .destination
        .as_deref()
        .or_else(|| clone_destination_for_source(source))
        .ok_or_else(|| {
            CommandError::UserError(
                "No destination specified and wasn't able to guess it".to_string(),
            )
        })?;
    let wc_path = ui.cwd().join(wc_path_str);
    let wc_path_existed = wc_path.exists();
    if wc_path_existed {
        if !is_empty_dir(&wc_path) {
            return Err(CommandError::UserError(
                "Destination path exists and is not an empty directory".to_string(),
            ));
        }
    } else {
        fs::create_dir(&wc_path).unwrap();
    }

    let clone_result = do_git_clone(ui, command, source, &wc_path);
    if clone_result.is_err() {
        // Canonicalize because fs::remove_dir_all() doesn't seem to like e.g.
        // `/some/path/.`
        let canonical_wc_path = wc_path.canonicalize().unwrap();
        if let Err(err) = fs::remove_dir_all(canonical_wc_path.join(".jj")).and_then(|_| {
            if !wc_path_existed {
                fs::remove_dir(&canonical_wc_path)
            } else {
                Ok(())
            }
        }) {
            writeln!(
                ui,
                "Failed to clean up {}: {}",
                canonical_wc_path.display(),
                err
            )
            .ok();
        }
    }

    if let (mut workspace_command, Some(default_branch)) = clone_result? {
        let default_branch_target = workspace_command
            .repo()
            .view()
            .get_remote_branch(&default_branch, "origin");
        if let Some(RefTarget::Normal(commit_id)) = default_branch_target {
            let mut checkout_tx =
                workspace_command.start_transaction("check out git remote's default branch");
            if let Ok(commit) = workspace_command.repo().store().get_commit(&commit_id) {
                checkout_tx.mut_repo().check_out(
                    workspace_command.workspace_id(),
                    ui.settings(),
                    &commit,
                );
            }
            workspace_command.finish_transaction(ui, checkout_tx)?;
        }
    }
    Ok(())
}

fn do_git_clone(
    ui: &mut Ui,
    command: &CommandHelper,
    source: &str,
    wc_path: &Path,
) -> Result<(WorkspaceCommandHelper, Option<String>), CommandError> {
    let (workspace, repo) = Workspace::init_internal_git(ui.settings(), wc_path.to_path_buf())?;
    let git_repo = get_git_repo(repo.store())?;
    writeln!(ui, r#"Fetching into new repo in "{}""#, wc_path.display())?;
    let mut workspace_command = command.for_loaded_repo(ui, workspace, repo)?;
    let remote_name = "origin";
    git_repo.remote(remote_name, source).unwrap();
    let mut fetch_tx = workspace_command.start_transaction("fetch from git remote into empty repo");
    let maybe_default_branch =
        git::fetch(fetch_tx.mut_repo(), &git_repo, remote_name).map_err(|err| match err {
            GitFetchError::NoSuchRemote(_) => {
                panic!("should't happen as we just created the git remote")
            }
            GitFetchError::InternalGitError(err) => {
                CommandError::UserError(format!("Fetch failed: {err}"))
            }
        })?;
    workspace_command.finish_transaction(ui, fetch_tx)?;
    Ok((workspace_command, maybe_default_branch))
}

fn cmd_git_push(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitPushArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let mut tx;
    let mut branch_updates = vec![];
    if let Some(branch_name) = &args.branch {
        if let Some(update) = branch_updates_for_push(
            workspace_command.repo().as_repo_ref(),
            &args.remote,
            branch_name,
        )? {
            branch_updates.push((branch_name.clone(), update));
        } else {
            writeln!(
                ui,
                "Branch {}@{} already matches {}",
                branch_name, &args.remote, branch_name
            )?;
        }
        tx = workspace_command.start_transaction(&format!(
            "push branch {branch_name} to git remote {}",
            &args.remote
        ));
    } else if let Some(change_str) = &args.change {
        let commit = workspace_command.resolve_single_rev(change_str)?;
        let branch_name = format!(
            "{}{}",
            ui.settings().push_branch_prefix(),
            commit.change_id().hex()
        );
        if workspace_command
            .repo()
            .view()
            .get_local_branch(&branch_name)
            .is_none()
        {
            writeln!(
                ui,
                "Creating branch {} for revision {}",
                branch_name, change_str
            )?;
        }
        tx = workspace_command.start_transaction(&format!(
            "push change {} to git remote {}",
            commit.change_id().hex(),
            &args.remote
        ));
        tx.mut_repo()
            .set_local_branch(branch_name.clone(), RefTarget::Normal(commit.id().clone()));
        if let Some(update) =
            branch_updates_for_push(tx.mut_repo().as_repo_ref(), &args.remote, &branch_name)?
        {
            branch_updates.push((branch_name.clone(), update));
        } else {
            writeln!(
                ui,
                "Branch {}@{} already matches {}",
                branch_name, &args.remote, branch_name
            )?;
        }
    } else if args.all {
        // TODO: Is it useful to warn about conflicted branches?
        for (branch_name, branch_target) in workspace_command.repo().view().branches() {
            let push_action = classify_branch_push_action(branch_target, &args.remote);
            match push_action {
                BranchPushAction::AlreadyMatches => {}
                BranchPushAction::LocalConflicted => {}
                BranchPushAction::RemoteConflicted => {}
                BranchPushAction::Update(update) => {
                    branch_updates.push((branch_name.clone(), update));
                }
            }
        }
        tx = workspace_command
            .start_transaction(&format!("push all branches to git remote {}", &args.remote));
    } else {
        match workspace_command
            .repo()
            .view()
            .get_checkout(&workspace_command.workspace_id())
        {
            None => {
                return Err(UserError(
                    "Nothing checked out in this workspace".to_string(),
                ));
            }
            Some(checkout) => {
                let desired_target = Some(RefTarget::Normal(checkout.clone()));
                for (branch_name, branch_target) in workspace_command.repo().view().branches() {
                    if branch_target.local_target == desired_target {
                        let push_action = classify_branch_push_action(branch_target, &args.remote);
                        match push_action {
                            BranchPushAction::AlreadyMatches => {}
                            BranchPushAction::LocalConflicted => {}
                            BranchPushAction::RemoteConflicted => {}
                            BranchPushAction::Update(update) => {
                                branch_updates.push((branch_name.clone(), update));
                            }
                        }
                    }
                }
            }
        }
        if branch_updates.is_empty() {
            return Err(UserError("No current branch.".to_string()));
        }
        tx = workspace_command.start_transaction(&format!(
            "push current branch(es) to git remote {}",
            &args.remote
        ));
    }

    if branch_updates.is_empty() {
        writeln!(ui, "Nothing changed.")?;
        return Ok(());
    }

    let repo = workspace_command.repo();

    let mut ref_updates = vec![];
    let mut new_heads = vec![];
    for (branch_name, update) in &branch_updates {
        let qualified_name = format!("refs/heads/{}", branch_name);
        if let Some(new_target) = &update.new_target {
            new_heads.push(new_target.clone());
            let force = match &update.old_target {
                None => false,
                Some(old_target) => !repo.index().is_ancestor(old_target, new_target),
            };
            ref_updates.push(GitRefUpdate {
                qualified_name,
                force,
                new_target: Some(new_target.clone()),
            });
        } else {
            ref_updates.push(GitRefUpdate {
                qualified_name,
                force: false,
                new_target: None,
            });
        }
    }

    // Check if there are conflicts in any commits we're about to push that haven't
    // already been pushed.
    let mut old_heads = vec![];
    for branch_target in repo.view().branches().values() {
        if let Some(old_head) = branch_target.remote_targets.get(&args.remote) {
            old_heads.extend(old_head.adds());
        }
    }
    if old_heads.is_empty() {
        old_heads.push(repo.store().root_commit_id().clone());
    }
    for index_entry in repo.index().walk_revs(&new_heads, &old_heads) {
        let commit = repo.store().get_commit(&index_entry.commit_id())?;
        let mut reasons = vec![];
        if commit.description().is_empty() {
            reasons.push("it has no description");
        }
        if commit.author().name == UserSettings::user_name_placeholder()
            || commit.author().email == UserSettings::user_email_placeholder()
            || commit.committer().name == UserSettings::user_name_placeholder()
            || commit.committer().email == UserSettings::user_email_placeholder()
        {
            reasons.push("it has no author and/or committer set");
        }
        if commit.tree().has_conflict() {
            reasons.push("it has conflicts");
        }
        if !reasons.is_empty() {
            return Err(UserError(format!(
                "Won't push commit {} since {}",
                short_commit_hash(commit.id()),
                reasons.join(" and ")
            )));
        }
    }

    writeln!(ui, "Branch changes to push to {}:", &args.remote)?;
    for (branch_name, update) in &branch_updates {
        match (&update.old_target, &update.new_target) {
            (Some(old_target), Some(new_target)) => {
                writeln!(
                    ui,
                    "  Move branch {branch_name} from {} to {}",
                    short_commit_hash(old_target),
                    short_commit_hash(new_target)
                )?;
            }
            (Some(old_target), None) => {
                writeln!(
                    ui,
                    "  Delete branch {branch_name} from {}",
                    short_commit_hash(old_target)
                )?;
            }
            (None, Some(new_target)) => {
                writeln!(
                    ui,
                    "  Add branch {branch_name} to {}",
                    short_commit_hash(new_target)
                )?;
            }
            (None, None) => {
                panic!("Not pushing any change to branch {branch_name}");
            }
        }
    }

    if args.dry_run {
        writeln!(ui, "Dry-run requested, not pushing.")?;
        return Ok(());
    }

    let git_repo = get_git_repo(repo.store())?;
    git::push_updates(&git_repo, &args.remote, &ref_updates)
        .map_err(|err| CommandError::UserError(err.to_string()))?;
    git::import_refs(tx.mut_repo(), &git_repo)?;
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn branch_updates_for_push(
    repo: RepoRef,
    remote_name: &str,
    branch_name: &str,
) -> Result<Option<BranchPushUpdate>, CommandError> {
    let maybe_branch_target = repo.view().get_branch(branch_name);
    let branch_target = maybe_branch_target
        .ok_or_else(|| CommandError::UserError(format!("Branch {} doesn't exist", branch_name)))?;
    let push_action = classify_branch_push_action(branch_target, remote_name);

    match push_action {
        BranchPushAction::AlreadyMatches => Ok(None),
        BranchPushAction::LocalConflicted => Err(CommandError::UserError(format!(
            "Branch {} is conflicted",
            branch_name
        ))),
        BranchPushAction::RemoteConflicted => Err(CommandError::UserError(format!(
            "Branch {}@{} is conflicted",
            branch_name, remote_name
        ))),
        BranchPushAction::Update(update) => Ok(Some(update)),
    }
}

fn cmd_git_import(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &GitImportArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    let mut tx = workspace_command.start_transaction("import git refs");
    git::import_refs(tx.mut_repo(), &git_repo)?;
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_git_export(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &GitExportArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    git::export_refs(repo, &git_repo)?;
    Ok(())
}

fn cmd_git(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &GitCommands,
) -> Result<(), CommandError> {
    match subcommand {
        GitCommands::Fetch(command_matches) => cmd_git_fetch(ui, command, command_matches),
        GitCommands::Clone(command_matches) => cmd_git_clone(ui, command, command_matches),
        GitCommands::Remote(GitRemoteCommands::Add(command_matches)) => {
            cmd_git_remote_add(ui, command, command_matches)
        }
        GitCommands::Remote(GitRemoteCommands::Remove(command_matches)) => {
            cmd_git_remote_remove(ui, command, command_matches)
        }
        GitCommands::Remote(GitRemoteCommands::List(command_matches)) => {
            cmd_git_remote_list(ui, command, command_matches)
        }
        GitCommands::Push(command_matches) => cmd_git_push(ui, command, command_matches),
        GitCommands::Import(command_matches) => cmd_git_import(ui, command, command_matches),
        GitCommands::Export(command_matches) => cmd_git_export(ui, command, command_matches),
    }
}

fn string_list_from_config(value: config::Value) -> Option<Vec<String>> {
    match value {
        Value {
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

fn parse_args(settings: &UserSettings, string_args: &[String]) -> Result<Args, CommandError> {
    let mut resolved_aliases = HashSet::new();
    let mut string_args = string_args.to_vec();
    loop {
        let args: Args = clap::Parser::parse_from(&string_args);
        if let Commands::Alias(alias_args) = &args.command {
            let alias_name = &alias_args[0];
            if resolved_aliases.contains(alias_name) {
                return Err(CommandError::UserError(format!(
                    r#"Recursive alias definition involving "{alias_name}""#
                )));
            }
            match settings
                .config()
                .get::<config::Value>(&format!("alias.{}", alias_name))
            {
                Ok(value) => {
                    if let Some(alias_definition) = string_list_from_config(value) {
                        assert!(string_args.ends_with(alias_args.as_slice()));
                        string_args.truncate(string_args.len() - alias_args.len());
                        string_args.extend(alias_definition);
                        string_args.extend_from_slice(&alias_args[1..]);
                        resolved_aliases.insert(alias_name.clone());
                    } else {
                        return Err(CommandError::UserError(format!(
                            r#"Alias definition for "{alias_name}" must be a string list"#
                        )));
                    }
                }
                Err(config::ConfigError::NotFound(_)) => {
                    let mut app = Args::command();
                    app.error(clap::ErrorKind::ArgumentNotFound, format!(
                        r#"Found argument '{alias_name}' which wasn't expected, or isn't valid in this context"#
                    )).exit();
                }
                Err(err) => {
                    return Err(CommandError::from(err));
                }
            }
        } else {
            return Ok(args);
        }
    }
}

pub fn dispatch<I, T>(ui: &mut Ui, args: I) -> Result<(), CommandError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut string_args: Vec<String> = vec![];
    for arg in args {
        let os_string_arg = arg.clone().into();
        if let Some(string_arg) = os_string_arg.to_str() {
            string_args.push(string_arg.to_owned());
        } else {
            return Err(CommandError::CliError("Non-utf8 argument".to_string()));
        }
    }

    let args = parse_args(ui.settings(), &string_args)?;
    if let Some(choice) = args.global_args.color {
        // Here we assume ui was created for_terminal().
        ui.reset_color_for_terminal(choice);
    }
    let app = Args::command();
    let command_helper = CommandHelper::new(app, string_args, args.global_args.clone());
    match &args.command {
        Commands::Init(sub_args) => cmd_init(ui, &command_helper, sub_args),
        Commands::Checkout(sub_args) => cmd_checkout(ui, &command_helper, sub_args),
        Commands::Untrack(sub_args) => cmd_untrack(ui, &command_helper, sub_args),
        Commands::Files(sub_args) => cmd_files(ui, &command_helper, sub_args),
        Commands::Print(sub_args) => cmd_print(ui, &command_helper, sub_args),
        Commands::Diff(sub_args) => cmd_diff(ui, &command_helper, sub_args),
        Commands::Show(sub_args) => cmd_show(ui, &command_helper, sub_args),
        Commands::Status(sub_args) => cmd_status(ui, &command_helper, sub_args),
        Commands::Log(sub_args) => cmd_log(ui, &command_helper, sub_args),
        Commands::Obslog(sub_args) => cmd_obslog(ui, &command_helper, sub_args),
        Commands::Describe(sub_args) => cmd_describe(ui, &command_helper, sub_args),
        Commands::Close(sub_args) => cmd_close(ui, &command_helper, sub_args),
        Commands::Open(sub_args) => cmd_open(ui, &command_helper, sub_args),
        Commands::Duplicate(sub_args) => cmd_duplicate(ui, &command_helper, sub_args),
        Commands::Abandon(sub_args) => cmd_abandon(ui, &command_helper, sub_args),
        Commands::Edit(sub_args) => cmd_edit(ui, &command_helper, sub_args),
        Commands::New(sub_args) => cmd_new(ui, &command_helper, sub_args),
        Commands::Move(sub_args) => cmd_move(ui, &command_helper, sub_args),
        Commands::Squash(sub_args) => cmd_squash(ui, &command_helper, sub_args),
        Commands::Unsquash(sub_args) => cmd_unsquash(ui, &command_helper, sub_args),
        Commands::Restore(sub_args) => cmd_restore(ui, &command_helper, sub_args),
        Commands::Touchup(sub_args) => cmd_touchup(ui, &command_helper, sub_args),
        Commands::Split(sub_args) => cmd_split(ui, &command_helper, sub_args),
        Commands::Merge(sub_args) => cmd_merge(ui, &command_helper, sub_args),
        Commands::Rebase(sub_args) => cmd_rebase(ui, &command_helper, sub_args),
        Commands::Backout(sub_args) => cmd_backout(ui, &command_helper, sub_args),
        Commands::Branch(sub_args) => cmd_branch(ui, &command_helper, sub_args),
        Commands::Undo(sub_args) => cmd_op_undo(ui, &command_helper, sub_args),
        Commands::Operation(sub_args) => cmd_operation(ui, &command_helper, sub_args),
        Commands::Workspace(sub_args) => cmd_workspace(ui, &command_helper, sub_args),
        Commands::Sparse(sub_args) => cmd_sparse(ui, &command_helper, sub_args),
        Commands::Git(sub_args) => cmd_git(ui, &command_helper, sub_args),
        Commands::Bench(sub_args) => cmd_bench(ui, &command_helper, sub_args),
        Commands::Debug(sub_args) => cmd_debug(ui, &command_helper, sub_args),
        Commands::Alias(_) => panic!("Unresolved alias"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_app() {
        Args::command().debug_assert();
    }
}
