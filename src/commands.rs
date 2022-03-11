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

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::fmt::Debug;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
use std::{fs, io};

use clap::{crate_version, Arg, ArgMatches, Command};
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
use jujutsu_lib::index::HexPrefix;
use jujutsu_lib::matchers::{EverythingMatcher, Matcher, PrefixMatcher};
use jujutsu_lib::op_heads_store::OpHeadsStore;
use jujutsu_lib::op_store::{OpStore, OpStoreError, OperationId, RefTarget, WorkspaceId};
use jujutsu_lib::operation::Operation;
use jujutsu_lib::refs::{classify_branch_push_action, BranchPushAction};
use jujutsu_lib::repo::{MutableRepo, ReadonlyRepo, RepoRef};
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::revset::{RevsetError, RevsetExpression, RevsetParseError};
use jujutsu_lib::revset_graph_iterator::RevsetGraphEdgeType;
use jujutsu_lib::rewrite::{back_out_commit, merge_commit_trees, rebase_commit, DescendantRebaser};
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::store::Store;
use jujutsu_lib::transaction::Transaction;
use jujutsu_lib::tree::{merge_trees, Tree, TreeDiffIterator};
use jujutsu_lib::working_copy::{CheckoutStats, ResetError, WorkingCopy};
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
use crate::ui::{FilePathParseError, Ui};

enum CommandError {
    UserError(String),
    BrokenPipe,
    InternalError(String),
}

impl From<std::io::Error> for CommandError {
    fn from(err: std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::BrokenPipe {
            CommandError::BrokenPipe
        } else {
            // TODO: Record the error as a chained cause
            CommandError::InternalError(format!("I/O error: {}", err))
        }
    }
}

impl From<BackendError> for CommandError {
    fn from(err: BackendError) -> Self {
        CommandError::UserError(format!("Unexpected error from store: {}", err))
    }
}

impl From<WorkspaceInitError> for CommandError {
    fn from(_: WorkspaceInitError) -> Self {
        CommandError::UserError("The target repo already exists".to_string())
    }
}

impl From<ResetError> for CommandError {
    fn from(_: ResetError) -> Self {
        CommandError::InternalError("Failed to reset the working copy".to_string())
    }
}

impl From<DiffEditError> for CommandError {
    fn from(err: DiffEditError) -> Self {
        CommandError::UserError(format!("Failed to edit diff: {}", err))
    }
}

impl From<git2::Error> for CommandError {
    fn from(err: git2::Error) -> Self {
        CommandError::UserError(format!("Git operation failed: {}", err))
    }
}

impl From<GitImportError> for CommandError {
    fn from(err: GitImportError) -> Self {
        CommandError::InternalError(format!(
            "Failed to import refs from underlying Git repo: {}",
            err
        ))
    }
}

impl From<GitExportError> for CommandError {
    fn from(err: GitExportError) -> Self {
        match err {
            GitExportError::ConflictedBranch(branch_name) => CommandError::UserError(format!(
                "Cannot export conflicted branch '{}'",
                branch_name
            )),
            GitExportError::InternalGitError(err) => CommandError::InternalError(format!(
                "Failed to export refs to underlying Git repo: {}",
                err
            )),
        }
    }
}

impl From<RevsetParseError> for CommandError {
    fn from(err: RevsetParseError) -> Self {
        CommandError::UserError(format!("Failed to parse revset: {}", err))
    }
}

impl From<RevsetError> for CommandError {
    fn from(err: RevsetError) -> Self {
        CommandError::UserError(format!("{}", err))
    }
}

impl From<FilePathParseError> for CommandError {
    fn from(err: FilePathParseError) -> Self {
        match err {
            FilePathParseError::InputNotInRepo(input) => {
                CommandError::UserError(format!("Path \"{}\" is not in the repo", input))
            }
        }
    }
}

struct CommandHelper<'help> {
    app: clap::Command<'help>,
    string_args: Vec<String>,
    root_args: ArgMatches,
}

impl<'help> CommandHelper<'help> {
    fn new(app: clap::Command<'help>, string_args: Vec<String>, root_args: ArgMatches) -> Self {
        Self {
            app,
            string_args,
            root_args,
        }
    }

    fn root_args(&self) -> &ArgMatches {
        &self.root_args
    }

    fn workspace_helper(&self, ui: &Ui) -> Result<WorkspaceCommandHelper, CommandError> {
        let wc_path_str = self.root_args.value_of("repository").unwrap();
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
        let op_str = self.root_args.value_of("at_op").unwrap();
        let repo = if op_str == "@" {
            repo_loader.load_at_head()
        } else {
            let op = resolve_single_op_from_store(
                repo_loader.op_store(),
                repo_loader.op_heads_store(),
                op_str,
            )?;
            repo_loader.load_at(&op)
        };
        self.for_loaded_repo(ui, workspace, repo)
    }

    fn for_loaded_repo(
        &self,
        ui: &Ui,
        workspace: Workspace,
        repo: Arc<ReadonlyRepo>,
    ) -> Result<WorkspaceCommandHelper, CommandError> {
        WorkspaceCommandHelper::for_loaded_repo(
            ui,
            workspace,
            self.string_args.clone(),
            &self.root_args,
            repo,
        )
    }
}

// Provides utilities for writing a command that works on a workspace (like most
// commands do).
struct WorkspaceCommandHelper {
    string_args: Vec<String>,
    settings: UserSettings,
    workspace: Workspace,
    repo: Arc<ReadonlyRepo>,
    may_update_working_copy: bool,
    working_copy_shared_with_git: bool,
    working_copy_committed: bool,
    // Whether to rebase descendants when the transaction finishes. This should generally be true
    // for commands that rewrite commits.
    rebase_descendants: bool,
}

impl WorkspaceCommandHelper {
    fn for_loaded_repo(
        ui: &Ui,
        workspace: Workspace,
        string_args: Vec<String>,
        root_args: &ArgMatches,
        repo: Arc<ReadonlyRepo>,
    ) -> Result<Self, CommandError> {
        let loaded_at_head = root_args.value_of("at_op").unwrap() == "@";
        let may_update_working_copy =
            loaded_at_head && !root_args.is_present("no_commit_working_copy");
        let mut working_copy_shared_with_git = false;
        let maybe_git_repo = repo.store().git_repo();
        if let Some(git_repo) = &maybe_git_repo {
            working_copy_shared_with_git =
                git_repo.workdir() == Some(workspace.workspace_root().as_path());
        }
        let mut helper = Self {
            string_args,
            settings: ui.settings().clone(),
            workspace,
            repo,
            may_update_working_copy,
            working_copy_shared_with_git,
            working_copy_committed: false,
            rebase_descendants: true,
        };
        if working_copy_shared_with_git && may_update_working_copy {
            helper.import_git_refs_and_head(maybe_git_repo.as_ref().unwrap())?;
        }
        Ok(helper)
    }

    fn import_git_refs_and_head(&mut self, git_repo: &Repository) -> Result<(), CommandError> {
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
                tx.mut_repo().rebase_descendants(&self.settings);
                self.repo = tx.commit();
                locked_working_copy.finish(self.repo.op_id().clone());
            } else {
                self.repo = tx.commit();
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

    fn rebase_descendants(mut self, value: bool) -> Self {
        self.rebase_descendants = value;
        self
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

    fn working_copy_mut(&mut self) -> &mut WorkingCopy {
        self.workspace.working_copy_mut()
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

    fn base_ignores(&self) -> Arc<GitIgnoreFile> {
        let mut git_ignores = GitIgnoreFile::empty();
        if let Ok(home_dir) = std::env::var("HOME") {
            let home_dir_path = PathBuf::from(home_dir);
            // TODO: Look up the name of the file in the core.excludesFile config instead of
            // hard-coding its name like this.
            git_ignores = git_ignores.chain_with_file("", home_dir_path.join(".gitignore"));
        }
        if let Some(git_repo) = self.repo.store().git_repo() {
            git_ignores =
                git_ignores.chain_with_file("", git_repo.path().join("info").join("exclude"));
        }
        git_ignores
    }

    fn resolve_revision_arg(
        &mut self,
        ui: &mut Ui,
        args: &ArgMatches,
    ) -> Result<Commit, CommandError> {
        self.resolve_single_rev(ui, args.value_of("revision").unwrap())
    }

    fn resolve_single_rev(
        &mut self,
        ui: &mut Ui,
        revision_str: &str,
    ) -> Result<Commit, CommandError> {
        let revset_expression = self.parse_revset(ui, revision_str)?;
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

    fn resolve_revset(
        &mut self,
        ui: &mut Ui,
        revision_str: &str,
    ) -> Result<Vec<Commit>, CommandError> {
        let revset_expression = self.parse_revset(ui, revision_str)?;
        let revset =
            revset_expression.evaluate(self.repo.as_repo_ref(), Some(&self.workspace_id()))?;
        Ok(revset
            .iter()
            .commits(self.repo.store())
            .map(Result::unwrap)
            .collect())
    }

    fn parse_revset(
        &mut self,
        ui: &mut Ui,
        revision_str: &str,
    ) -> Result<Rc<RevsetExpression>, CommandError> {
        let expression = revset::parse(revision_str)?;
        // If the revset is exactly "@", then we need to commit the working copy. If
        // it's another symbol, then we don't. If it's more complex, then we do
        // (just to be safe). TODO: Maybe make this smarter. How do we generally
        // figure out if a revset needs to commit the working copy? For example,
        // "@-" should perhaps not result in a new working copy commit, but
        // "@--" should. "foo++" is probably also should, since we would
        // otherwise need to evaluate the revset and see if "foo::" includes the
        // parent of the current checkout. Other interesting cases include some kind of
        // reference pointing to the working copy commit. If it's a
        // type of reference that would get updated when the commit gets rewritten, then
        // we probably should create a new working copy commit.
        let mentions_checkout = match expression.as_ref() {
            RevsetExpression::Symbol(name) => name == "@",
            _ => true,
        };
        if mentions_checkout && !self.working_copy_committed {
            self.maybe_commit_working_copy(ui)?;
        }
        Ok(expression)
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
        if !self.may_update_working_copy {
            return Err(UserError(
                "Refusing to update working copy (maybe because you're using --at-op)".to_string(),
            ));
        }
        self.maybe_commit_working_copy(ui)?;
        Ok(())
    }

    fn maybe_commit_working_copy(&mut self, ui: &mut Ui) -> Result<(), CommandError> {
        if !self.may_update_working_copy {
            return Ok(());
        }
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
        let checkout_commit = repo.store().get_commit(&checkout_id).unwrap();
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
        let new_tree_id = locked_wc.write_tree(base_ignores);
        if new_tree_id != *checkout_commit.tree_id() {
            let mut tx = self.repo.start_transaction("commit working copy");
            let mut_repo = tx.mut_repo();
            let commit = CommitBuilder::for_rewrite_from(
                &self.settings,
                self.repo.store(),
                &checkout_commit,
            )
            .set_tree(new_tree_id)
            .write_to_repo(mut_repo);
            mut_repo.set_checkout(workspace_id, commit.id().clone());

            // Rebase descendants
            let num_rebased = mut_repo.rebase_descendants(&self.settings);
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
        self.working_copy_committed = true;
        Ok(())
    }

    fn edit_diff(
        &self,
        left_tree: &Tree,
        right_tree: &Tree,
        instructions: &str,
    ) -> Result<TreeId, DiffEditError> {
        crate::diff_edit::edit_diff(
            &self.settings,
            left_tree,
            right_tree,
            instructions,
            self.base_ignores(),
        )
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
        if self.rebase_descendants {
            let num_rebased = mut_repo.rebase_descendants(ui.settings());
            if num_rebased > 0 {
                writeln!(ui, "Rebased {} descendant commits", num_rebased)?;
            }
        }
        if self.working_copy_shared_with_git {
            self.export_head_to_git(mut_repo)?;
        }
        let maybe_old_commit = tx
            .base_repo()
            .view()
            .get_checkout(&self.workspace_id())
            .map(|commit_id| store.get_commit(commit_id).unwrap());
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
                if stats.added_files > 0 || stats.updated_files > 0 || stats.removed_files > 0 {
                    writeln!(
                        ui,
                        "Added {} files, modified {} files, removed {} files",
                        stats.added_files, stats.updated_files, stats.removed_files
                    )?;
                }
            }
        }
        if self.working_copy_shared_with_git {
            let git_repo = self.repo.store().git_repo().unwrap();
            git::export_refs(&self.repo, &git_repo)?;
        }
        Ok(())
    }
}

fn rev_arg<'help>() -> Arg<'help> {
    Arg::new("revision")
        .long("revision")
        .short('r')
        .takes_value(true)
        .default_value("@")
}

fn paths_arg<'help>() -> Arg<'help> {
    Arg::new("paths").index(1).multiple_occurrences(true)
}

fn message_arg<'help>() -> Arg<'help> {
    Arg::new("message")
        .long("message")
        .short('m')
        .takes_value(true)
}

fn op_arg<'help>() -> Arg<'help> {
    Arg::new("operation")
        .long("operation")
        .alias("op")
        .short('o')
        .takes_value(true)
        .default_value("@")
}

fn resolve_single_op(repo: &ReadonlyRepo, op_str: &str) -> Result<Operation, CommandError> {
    if op_str == "@" {
        // Get it from the repo to make sure that it refers to the operation the repo
        // was loaded at
        Ok(repo.operation().clone())
    } else {
        resolve_single_op_from_store(repo.op_store(), repo.op_heads_store(), op_str)
    }
}

fn find_all_operations(
    op_store: &Arc<dyn OpStore>,
    op_heads_store: &Arc<OpHeadsStore>,
) -> Vec<Operation> {
    let mut visited = HashSet::new();
    let mut work: VecDeque<_> = op_heads_store.get_op_heads().into_iter().collect();
    let mut operations = vec![];
    while !work.is_empty() {
        let op_id = work.pop_front().unwrap();
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
                    "Failed to read operation: {:?}",
                    err
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

fn matcher_from_values(
    ui: &Ui,
    wc_path: &Path,
    values: Option<clap::Values>,
) -> Result<Box<dyn Matcher>, CommandError> {
    if let Some(values) = values {
        // TODO: Add support for globs and other formats
        let mut paths = vec![];
        for value in values {
            let repo_path = ui.parse_file_path(wc_path, value)?;
            paths.push(repo_path);
        }
        Ok(Box::new(PrefixMatcher::new(&paths)))
    } else {
        Ok(Box::new(EverythingMatcher))
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
    let new_commit = repo.store().get_commit(new_commit_id).unwrap();
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

fn get_app<'help>() -> Command<'help> {
    let init_command = Command::new("init")
        .about("Create a new repo in the given directory")
        .long_about(
            "Create a new repo in the given directory. If the given directory does not exist, it \
             will be created. If no directory is given, the current directory is used.",
        )
        .arg(
            Arg::new("destination")
                .index(1)
                .default_value(".")
                .help("The destination directory"),
        )
        .arg(
            Arg::new("git")
                .long("git")
                .help("Use the Git backend, creating a jj repo backed by a Git repo"),
        )
        .arg(
            Arg::new("git-repo")
                .long("git-repo")
                .takes_value(true)
                .conflicts_with("git")
                .help("Path to a git repo the jj repo will be backed by"),
        );
    let checkout_command = Command::new("checkout")
        .alias("co")
        .about("Update the working copy to another revision")
        .long_about(
            "Update the working copy to another revision. If the revision is closed or has \
             conflicts, then a new, open revision will be created on top, and that will be checked \
             out. For more information, see \
             https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.",
        )
        .arg(
            Arg::new("revision")
                .index(1)
                .required(true)
                .help("The revision to update to"),
        );
    let untrack_command = Command::new("untrack")
        .about("Stop tracking specified paths in the working copy")
        .arg(paths_arg());
    let files_command = Command::new("files")
        .about("List files in a revision")
        .arg(rev_arg().help("The revision to list files in"))
        .arg(paths_arg());
    let diff_command = Command::new("diff")
        .about("Show changes in a revision")
        .long_about(
            "Show changes in a revision.

With the `-r` option, which is the default, shows the changes compared to the parent revision. If \
             there are several parent revisions (i.e., the given revision is a merge), then they \
             will be merged and the changes from the result to the given revision will be shown.

With the `--from` and/or `--to` options, shows the difference from/to the given revisions. If \
             either is left out, it defaults to the current checkout. For example, `jj diff \
             --from main` shows the changes from \"main\" (perhaps a branch name) to the current \
             checkout.",
        )
        .arg(
            Arg::new("summary")
                .long("summary")
                .short('s')
                .help("For each path, show only whether it was modified, added, or removed"),
        )
        .arg(
            Arg::new("git")
                .long("git")
                .conflicts_with("summary")
                .help("Show a Git-format diff"),
        )
        .arg(
            Arg::new("color-words")
                .long("color-words")
                .conflicts_with("summary")
                .conflicts_with("git")
                .help("Show a word-level diff with changes indicated only by color"),
        )
        .arg(
            Arg::new("revision")
                .long("revision")
                .short('r')
                .takes_value(true)
                .help("Show changes changes in this revision, compared to its parent(s)"),
        )
        .arg(
            Arg::new("from")
                .long("from")
                .conflicts_with("revision")
                .takes_value(true)
                .help("Show changes from this revision"),
        )
        .arg(
            Arg::new("to")
                .long("to")
                .conflicts_with("revision")
                .takes_value(true)
                .help("Show changes to this revision"),
        )
        .arg(paths_arg());
    let show_command = Command::new("show")
        .about("Show commit description and changes in a revision")
        .long_about("Show commit description and changes in a revision")
        .arg(
            Arg::new("summary")
                .long("summary")
                .short('s')
                .help("For each path, show only whether it was modified, added, or removed"),
        )
        .arg(
            Arg::new("git")
                .long("git")
                .conflicts_with("summary")
                .help("Show a Git-format diff"),
        )
        .arg(
            Arg::new("color-words")
                .long("color-words")
                .conflicts_with("summary")
                .conflicts_with("git")
                .help("Show a word-level diff with changes indicated only by color"),
        )
        .arg(
            Arg::new("revision")
                .index(1)
                .default_value("@")
                .help("Show changes changes in this revision, compared to its parent(s)"),
        );
    let status_command = Command::new("status")
        .alias("st")
        .about("Show high-level repo status")
        .long_about(
            "Show high-level repo status. This includes:

 * The working copy commit and its (first) \
             parent, and a summary of the changes between them

 * Conflicted branches (see https://github.com/martinvonz/jj/blob/main/docs/branches.md)\
             ",
        );
    let log_command = Command::new("log")
        .about("Show commit history")
        .arg(
            Arg::new("template")
                .long("template")
                .short('T')
                .takes_value(true)
                .help(
                    "Render each revision using the given template (the syntax is not yet \
                     documented and is likely to change)",
                ),
        )
        .arg(
            Arg::new("revisions")
                .long("revisions")
                .short('r')
                .takes_value(true)
                .default_value(":heads()")
                .help("Which revisions to show"),
        )
        .arg(
            Arg::new("no-graph")
                .long("no-graph")
                .help("Don't show the graph, show a flat list of revisions"),
        );
    let obslog_command = Command::new("obslog")
        .about("Show how a change has evolved")
        .long_about("Show how a change has evolved as it's been updated, rebased, etc.")
        .arg(rev_arg())
        .arg(
            Arg::new("template")
                .long("template")
                .short('T')
                .takes_value(true)
                .help(
                    "Render each revision using the given template (the syntax is not yet \
                     documented)",
                ),
        )
        .arg(
            Arg::new("no-graph")
                .long("no-graph")
                .help("Don't show the graph, show a flat list of revisions"),
        );
    let describe_command = Command::new("describe")
        .about("Edit the change description")
        .about("Edit the description of a change")
        .long_about(
            "Starts an editor to let you edit the description of a change. The editor will be \
             $EDITOR, or `pico` if that's not defined.",
        )
        .arg(
            Arg::new("revision")
                .index(1)
                .default_value("@")
                .help("The revision whose description to edit"),
        )
        .arg(message_arg().help("The change description to use (don't open editor)"))
        .arg(
            Arg::new("stdin")
                .long("stdin")
                .help("Read the change description from stdin"),
        );
    let close_command = Command::new("close")
        .alias("commit")
        .about("Mark a revision closed")
        .long_about(
            "Mark a revision closed. For information about open/closed revisions, see \
            https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.",
        )
        .arg(
            Arg::new("revision")
                .index(1)
                .default_value("@")
                .help("The revision to close"),
        )
        .arg(
            Arg::new("edit")
                .long("edit")
                .short('e')
                .help("Also edit the description"),
        )
        .arg(message_arg().help("The change description to use (don't open editor)"));
    let open_command = Command::new("open")
        .about("Mark a revision open")
        .alias("uncommit")
        .long_about(
            "Mark a revision open. For information about open/closed revisions, see \
            https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.",
        )
        .arg(
            Arg::new("revision")
                .index(1)
                .required(true)
                .help("The revision to open"),
        );
    let duplicate_command = Command::new("duplicate")
        .about("Create a new change with the same content as an existing one")
        .arg(
            Arg::new("revision")
                .index(1)
                .default_value("@")
                .help("The revision to duplicate"),
        );
    let abandon_command = Command::new("abandon")
        .about("Abandon a revision")
        .long_about(
            "Abandon a revision, rebasing descendants onto its parent(s). The behavior is similar \
             to `jj restore`; the difference is that `jj abandon` gives you a new change, while \
             `jj restore` updates the existing change.",
        )
        .arg(
            Arg::new("revision")
                .index(1)
                .default_value("@")
                .help("The revision(s) to abandon"),
        );
    let new_command = Command::new("new")
        .about("Create a new, empty change")
        .long_about(
            "Create a new, empty change. This may be useful if you want to make some changes \
             you're unsure of on top of the working copy. If the changes turned out to useful, \
             you can `jj squash` them into the previous working copy. If they turned out to be \
             unsuccessful, you can `jj abandon` them and `jj co @-` the previous working copy.",
        )
        .arg(
            Arg::new("revision")
                .index(1)
                .default_value("@")
                .help("Parent of the new change")
                .long_help(
                    "Parent of the new change. If the parent is the working copy, then the new \
                     change will be checked out.",
                ),
        );
    let move_command = Command::new("move")
        .about("Move changes from one revision into another")
        .long_about(
            "Move changes from a revision into another revision. Use `--interactive` to move only \
             part of the source revision into the destination. The selected changes (or all the \
             changes in the source revision if not using `--interactive`) will be moved into the \
             destination. The changes will be removed from the source. If that means that the \
             source is now empty compared to its parent, it will be abandoned.",
        )
        .arg(
            Arg::new("from")
                .long("from")
                .takes_value(true)
                .default_value("@")
                .help("Move part of this change into the destination"),
        )
        .arg(
            Arg::new("to")
                .long("to")
                .takes_value(true)
                .default_value("@")
                .help("Move part of the source into this change"),
        )
        .arg(
            Arg::new("interactive")
                .long("interactive")
                .short('i')
                .help("Interactively choose which parts to move"),
        );
    let squash_command = Command::new("squash")
        .alias("amend")
        .about("Move changes from a revision into its parent")
        .long_about(
            "Move changes from a revision into its parent. After moving the changes into the \
             parent, the child revision will have the same content state as before. If that means \
             that the change is now empty compared to its parent, it will be abandoned. This will \
             always be the case without `--interactive`.",
        )
        .arg(rev_arg())
        .arg(
            Arg::new("interactive")
                .long("interactive")
                .short('i')
                .help("Interactively choose which parts to squash"),
        );
    // TODO: It doesn't make much sense to run this without -i. We should make that
    // the default. We should also abandon the parent commit if that becomes empty.
    let unsquash_command = Command::new("unsquash")
        .alias("unamend")
        .about("Move changes from a revision's parent into the revision")
        .arg(rev_arg())
        .arg(
            Arg::new("interactive")
                .long("interactive")
                .short('i')
                .help("Interactively choose which parts to unsquash"),
        );
    let restore_command = Command::new("restore")
        .about("Restore paths from another revision")
        .long_about(
            "Restore paths from another revision. That means that the paths get the same content \
             in the destination (`--to`) as they had in the source (`--from`). This is typically \
             used for undoing changes to some paths in the working copy (`jj restore <paths>`).

 If you restore from a revision where the path has conflicts, then the destination revision will \
             have the same conflict. If the destination is the working copy, then a new commit \
             will be created on top for resolving the conflict (as if you had run `jj checkout` \
             on the new revision). Taken together, that means that if you're already resolving \
             conflicts and you want to restart the resolution of some file, you may want to run \
             `jj restore <path>; jj squash`.",
        )
        .arg(
            Arg::new("from")
                .long("from")
                .takes_value(true)
                .default_value("@-")
                .help("Revision to restore from (source)"),
        )
        .arg(
            Arg::new("to")
                .long("to")
                .takes_value(true)
                .default_value("@")
                .help("Revision to restore into (destination)"),
        )
        .arg(
            Arg::new("interactive")
                .long("interactive")
                .short('i')
                .help("Interactively choose which parts to restore"),
        )
        .arg(paths_arg());
    let edit_command = Command::new("edit")
        .about("Edit the content changes in a revision")
        .long_about(
            "Lets you interactively edit the content changes in a revision.

Starts a diff editor (`meld` by default) on the changes in the revision. Edit the right side of \
             the diff until it looks the way you want. Once you close the editor, the revision \
             will be updated. Descendants will be rebased on top as usual, which may result in \
             conflicts. See `jj squash -i` or `jj unsquash -i` if you instead want to move \
             changes into or out of the parent revision.",
        )
        .arg(rev_arg().help("The revision to edit"));
    let split_command = Command::new("split")
        .about("Split a revision in two")
        .long_about(
            "Lets you interactively split a revision in two.

Starts a diff editor (`meld` by default) on the changes in the revision. Edit the right side of \
             the diff until it has the content you want in the first revision. Once you close the \
             editor, your edited content will replace the previous revision. The remaining \
             changes will be put in a new revision on top. You will be asked to enter a change \
             description for each.",
        )
        .arg(rev_arg().help("The revision to split"));
    let merge_command = Command::new("merge")
        .about("Merge work from multiple branches")
        .long_about(
            "Merge work from multiple branches.

Unlike most other VCSs, `jj merge` does not implicitly include the working copy revision's parent \
             as one of the parents of the merge; you need to explicitly list all revisions that \
             should become parents of the merge. Also, you need to explicitly check out the \
             resulting revision if you want to.",
        )
        .arg(
            Arg::new("revisions")
                .index(1)
                .required(true)
                .multiple_occurrences(true),
        )
        .arg(message_arg().help("The change description to use (don't open editor)"));
    let rebase_command = Command::new("rebase")
        .about("Move a revision to a different parent")
        .long_about(
            "Move a revision to a different parent.

With `-s`, rebases the specified revision and its descendants onto the destination. For example,
`jj rebase -s B -d D` would transform your history like this:

D          C'
|          |
| C        B'
| |   =>   |
| B        D
|/         |
A          A

With `-r`, rebases only the specified revision onto the destination. Any \"hole\" left behind will \
             be filled by rebasing descendants onto the specified revision's parent(s). For \
             example, `jj rebase -r B -d D` would transform your history like this:

D          B'
|          |
| C        D
| |   =>   |
| B        | C'
|/         |/
A          A",
        )
        .arg(
            Arg::new("revision")
                .long("revision")
                .short('r')
                .takes_value(true)
                .help(
                    "Rebase only this revision, rebasing descendants onto this revision's \
                     parent(s)",
                ),
        )
        .arg(
            Arg::new("source")
                .long("source")
                .short('s')
                .conflicts_with("revision")
                .takes_value(true)
                .required(false)
                .multiple_occurrences(false)
                .help("Rebase this revision and its descendants"),
        )
        .arg(
            Arg::new("destination")
                .long("destination")
                .short('d')
                .takes_value(true)
                .required(true)
                .multiple_occurrences(true)
                .help("The revision to rebase onto"),
        );
    // TODO: It seems better to default the destination to `@-`. Maybe the working
    // copy should be rebased on top?
    let backout_command = Command::new("backout")
        .about("Apply the reverse of a revision on top of another revision")
        .arg(rev_arg().help("The revision to apply the reverse of"))
        .arg(
            Arg::new("destination")
                .long("destination")
                .short('d')
                .takes_value(true)
                .default_value("@")
                .multiple_occurrences(true)
                .help("The revision to apply the reverse changes on top of"),
        );
    let branch_command = Command::new("branch")
        .about("Create, update, or delete a branch")
        .long_about(
            "Create, update, or delete a branch. For information about branches, see \
            https://github.com/martinvonz/jj/blob/main/docs/branches.md.",
        )
        .arg(rev_arg().help("The branch's target revision"))
        .arg(
            Arg::new("allow-backwards")
                .long("allow-backwards")
                .help("Allow moving the branch backwards or sideways"),
        )
        .arg(
            Arg::new("delete")
                .long("delete")
                .help("Delete the branch locally")
                .long_help(
                    "Delete the branch locally. The deletion will be propagated to remotes on \
                     push.",
                ),
        )
        .arg(
            Arg::new("forget")
                .long("forget")
                .help("Forget the branch")
                .long_help(
                    "Forget everything about the branch. There will be no record of its position \
                     on remotes (no branchname@remotename in log output). Pushing will not affect \
                     the branch on the remote. Pulling will bring the branch back if it still \
                     exists on the remote.",
                ),
        )
        .arg(
            Arg::new("name")
                .index(1)
                .required(true)
                .help("The name of the branch to move or delete"),
        );
    let branches_command = Command::new("branches").about("List branches").long_about(
        "\
List branches and their targets. A remote branch will be included only if its target is different \
         from the local target. For a conflicted branch (both local and remote), old target \
         revisions are preceded by a \"-\" and new target revisions are preceded by a \"+\".

For information about branches, see https://github.com/martinvonz/jj/blob/main/docs/branches.md",
    );
    let undo_command = Command::new("undo")
        .about("Undo an operation")
        .arg(op_arg().help("The operation to undo"));
    let operation_command = Command::new("operation")
        .alias("op")
        .about("Commands for working with the operation log")
        .long_about(
            "Commands for working with the operation log. For information about the \
            operation log, see https://github.com/martinvonz/jj/blob/main/docs/operation-log.md.",
        )
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(Command::new("log").about("Show the operation log"))
        .subcommand(undo_command.clone())
        .subcommand(
            Command::new("restore")
                .about("Restore to the state at an operation")
                .arg(op_arg().help("The operation to restore to")),
        );
    let undo_command = undo_command.about("Undo an operation (shortcut for `jj op undo`)");
    let workspace_command = Command::new("workspace")
        .about("Commands for working with workspaces")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("add")
                .about("Add a workspace")
                .arg(
                    Arg::new("destination")
                        .index(1)
                        .required(true)
                        .help("Where to create the new workspace"),
                )
                .arg(
                    Arg::new("name")
                        .long("name")
                        .takes_value(true)
                        .help("A name for the workspace")
                        .long_help(
                            "A name for the workspace, to override the default, which is the \
                             basename of the destination directory.",
                        ),
                ),
        )
        .subcommand(
            Command::new("forget")
                .about("Stop tracking a workspace's checkout")
                .long_about(
                    "Stop tracking a workspace's checkout in the repo. The workspace will not be \
                     touched on disk. It can be deleted from disk before or after running this \
                     command.",
                )
                .arg(Arg::new("workspace").index(1)),
        )
        .subcommand(Command::new("list").about("List workspaces"));
    let git_command = Command::new("git")
        .about("Commands for working with the underlying Git repo")
        .long_about(
            "Commands for working with the underlying Git repo.

For a comparison with Git, including a table of commands, see \
https://github.com/martinvonz/jj/blob/main/docs/git-comparison.md.\
             ",
        )
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("remote")
                .about("Manage Git remotes")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(
                    Command::new("add")
                        .about("Add a Git remote")
                        .arg(
                            Arg::new("remote")
                                .index(1)
                                .required(true)
                                .help("The remote's name"),
                        )
                        .arg(
                            Arg::new("url")
                                .index(2)
                                .required(true)
                                .help("The remote's URL"),
                        ),
                )
                .subcommand(
                    Command::new("remove")
                        .about("Remove a Git remote and forget its branches")
                        .arg(
                            Arg::new("remote")
                                .index(1)
                                .required(true)
                                .help("The remote's name"),
                        ),
                ),
        )
        .subcommand(
            Command::new("fetch").about("Fetch from a Git remote").arg(
                Arg::new("remote")
                    .long("remote")
                    .takes_value(true)
                    .default_value("origin")
                    .help("The remote to fetch from (only named remotes are supported)"),
            ),
        )
        .subcommand(
            Command::new("clone")
                .about("Create a new repo backed by a clone of a Git repo")
                .long_about(
                    "Create a new repo backed by a clone of a Git repo. The Git repo will be a \
                     bare git repo stored inside the `.jj/` directory.",
                )
                .arg(
                    Arg::new("source")
                        .index(1)
                        .required(true)
                        .help("URL or path of the Git repo to clone"),
                )
                .arg(
                    Arg::new("destination")
                        .index(2)
                        .help("The directory to write the Jujutsu repo to"),
                ),
        )
        .subcommand(
            Command::new("push")
                .about("Push to a Git remote")
                .long_about(
                    "Push to a Git remote

By default, all branches are pushed. Use `--branch` if you want to push only one branch.",
                )
                .arg(
                    Arg::new("branch")
                        .long("branch")
                        .takes_value(true)
                        .help("Push only this branch"),
                )
                .arg(
                    Arg::new("remote")
                        .long("remote")
                        .takes_value(true)
                        .default_value("origin")
                        .help("The remote to push to (only named remotes are supported)"),
                ),
        )
        .subcommand(
            Command::new("import")
                .about("Update repo with changes made in the underlying Git repo"),
        )
        .subcommand(
            Command::new("export")
                .about("Update the underlying Git repo with changes made in the repo"),
        );
    let bench_command = Command::new("bench")
        .about("Commands for benchmarking internal operations")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("commonancestors")
                .about("Find the common ancestor(s) of a set of commits")
                .arg(Arg::new("revision1").index(1).required(true))
                .arg(Arg::new("revision2").index(2).required(true)),
        )
        .subcommand(
            Command::new("isancestor")
                .about("Checks if the first commit is an ancestor of the second commit")
                .arg(Arg::new("ancestor").index(1).required(true))
                .arg(Arg::new("descendant").index(2).required(true)),
        )
        .subcommand(
            Command::new("walkrevs")
                .about(
                    "Walk revisions that are ancestors of the second argument but not ancestors \
                     of the first",
                )
                .arg(Arg::new("unwanted").index(1).required(true))
                .arg(Arg::new("wanted").index(2).required(true)),
        )
        .subcommand(
            Command::new("resolveprefix")
                .about("Resolve a commit ID prefix")
                .arg(Arg::new("prefix").index(1).required(true)),
        );
    let debug_command = Command::new("debug")
        .about("Low-level commands not intended for users")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("completion")
                .about("Print a command-line-completion script")
                .arg(Arg::new("bash").long("bash"))
                .arg(Arg::new("fish").long("fish"))
                .arg(Arg::new("zsh").long("zsh")),
        )
        .subcommand(Command::new("mangen").about("Print a ROFF (manpage)"))
        .subcommand(
            Command::new("resolverev")
                .about("Resolve a revision identifier to its full ID")
                .arg(rev_arg()),
        )
        .subcommand(
            Command::new("workingcopy").about("Show information about the working copy state"),
        )
        .subcommand(
            Command::new("writeworkingcopy").about("Write a tree from the working copy state"),
        )
        .subcommand(
            Command::new("template")
                .about("Parse a template")
                .arg(Arg::new("template").index(1).required(true)),
        )
        .subcommand(Command::new("index").about("Show commit index stats"))
        .subcommand(Command::new("reindex").about("Rebuild commit index"));
    Command::new("jj")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .version(crate_version!())
        .author("Martin von Zweigbergk <martinvonz@google.com>")
        .about(
            "Jujutsu (An experimental VCS)

To get started, see the tutorial at https://github.com/martinvonz/jj/blob/main/docs/tutorial.md.\
             ",
        )
        .mut_arg("help", |arg| {
            arg.help("Print help information, more help with --help than with -h")
        })
        .arg(
            Arg::new("repository")
                .long("repository")
                .short('R')
                .global(true)
                .takes_value(true)
                .default_value(".")
                .help("Path to repository to operate on")
                .long_help(
                    "Path to repository to operate on. By default, Jujutsu searches for the \
                     closest .jj/ directory in an ancestor of the current working directory.",
                ),
        )
        .arg(
            Arg::new("no_commit_working_copy")
                .long("no-commit-working-copy")
                .global(true)
                .help("Don't commit the working copy")
                .long_help(
                    "Don't commit the working copy. By default, Jujutsu commits the working copy \
                     on every command, unless you load the repo at a specific operation with \
                     `--at-operation`. If you want to avoid committing the working and instead \
                     see a possibly stale working copy commit, you can use \
                     `--no-commit-working-copy`. This may be useful e.g. in a command prompt, \
                     especially if you have another process that commits the working copy.",
                ),
        )
        .arg(
            Arg::new("at_op")
                .long("at-operation")
                .alias("at-op")
                .global(true)
                .takes_value(true)
                .default_value("@")
                .help("Operation to load the repo at")
                .long_help(
                    "Operation to load the repo at. By default, Jujutsu loads the repo at the \
                     most recent operation. You can use `--at-op=<operation ID>` to see what the \
                     repo looked like at an earlier operation. For example `jj --at-op=<operation \
                     ID> st` will show you what `jj st` would have shown you when the given \
                     operation had just finished.

Use `jj op log` to find the operation ID you want. Any unambiguous prefix of the operation ID is \
                     enough.

When loading the repo at an earlier operation, the working copy will not be automatically \
                     committed.

It is possible to mutating commands when loading the repo at an earlier operation. Doing that is \
                     equivalent to having run concurrent commands starting at the earlier \
                     operation. There's rarely a reason to do that, but it is possible.
",
                ),
        )
        .subcommand(init_command)
        .subcommand(checkout_command)
        .subcommand(untrack_command)
        .subcommand(files_command)
        .subcommand(diff_command)
        .subcommand(show_command)
        .subcommand(status_command)
        .subcommand(log_command)
        .subcommand(obslog_command)
        .subcommand(describe_command)
        .subcommand(close_command)
        .subcommand(open_command)
        .subcommand(duplicate_command)
        .subcommand(abandon_command)
        .subcommand(new_command)
        .subcommand(move_command)
        .subcommand(squash_command)
        .subcommand(unsquash_command)
        .subcommand(restore_command)
        .subcommand(edit_command)
        .subcommand(split_command)
        .subcommand(merge_command)
        .subcommand(rebase_command)
        .subcommand(backout_command)
        .subcommand(branch_command)
        .subcommand(branches_command)
        .subcommand(operation_command)
        .subcommand(undo_command)
        .subcommand(workspace_command)
        .subcommand(git_command)
        .subcommand(bench_command)
        .subcommand(debug_command)
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

fn cmd_init(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    if command.root_args.occurrences_of("repository") > 0 {
        return Err(CommandError::UserError(
            "'--repository' cannot be used with 'init'".to_string(),
        ));
    }
    let wc_path_str = args.value_of("destination").unwrap();
    let wc_path = ui.cwd().join(wc_path_str);
    if wc_path.exists() {
        assert!(wc_path.is_dir());
    } else {
        fs::create_dir(&wc_path).unwrap();
    }
    let wc_path = std::fs::canonicalize(&wc_path).unwrap();

    if let Some(git_store_str) = args.value_of("git-repo") {
        let mut git_store_path = ui.cwd().join(git_store_str);
        if !git_store_path.ends_with(".git") {
            git_store_path = git_store_path.join(".git");
        }
        git_store_path = std::fs::canonicalize(&git_store_path).unwrap();
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
        }
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
        // TODO: Check out a recent commit. Maybe one with the highest generation
        // number.
        if tx.mut_repo().has_changes() {
            workspace_command.finish_transaction(ui, tx)?;
        }
    } else if args.is_present("git") {
        Workspace::init_internal_git(ui.settings(), wc_path.clone())?;
    } else {
        Workspace::init_local(ui.settings(), wc_path.clone())?;
    };
    let cwd = std::fs::canonicalize(&ui.cwd()).unwrap();
    let relative_wc_path = ui::relative_path(&cwd, &wc_path);
    writeln!(ui, "Initialized repo in \"{}\"", relative_wc_path.display())?;
    Ok(())
}

fn cmd_checkout(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let new_commit = workspace_command.resolve_revision_arg(ui, args)?;
    let workspace_id = workspace_command.workspace_id();
    if workspace_command.repo().view().get_checkout(&workspace_id) == Some(new_commit.id()) {
        ui.write("Already on that commit\n")?;
    } else {
        workspace_command.commit_working_copy(ui)?;
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
    args: &ArgMatches,
) -> Result<(), CommandError> {
    // TODO: We should probably check that the repo was loaded at head.
    let mut workspace_command = command.workspace_helper(ui)?;
    workspace_command.maybe_commit_working_copy(ui)?;
    let store = workspace_command.repo().store().clone();
    let matcher = matcher_from_values(
        ui,
        workspace_command.workspace_root(),
        args.values_of("paths"),
    )?;

    let current_checkout_id = workspace_command
        .repo
        .view()
        .get_checkout(&workspace_command.workspace_id());
    let current_checkout = if let Some(current_checkout_id) = current_checkout_id {
        store.get_commit(current_checkout_id).unwrap()
    } else {
        return Err(CommandError::UserError(
            "Nothing checked out in this workspace".to_string(),
        ));
    };

    let mut tx = workspace_command.start_transaction("untrack paths");
    let base_ignores = workspace_command.base_ignores();
    let mut locked_working_copy = workspace_command.working_copy_mut().start_mutation();
    if current_checkout.tree_id() != locked_working_copy.old_tree_id() {
        return Err(CommandError::UserError(
            "Concurrent working copy operation. Try again.".to_string(),
        ));
    }
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
    let wc_tree_id = locked_working_copy.write_tree(base_ignores);
    if wc_tree_id != new_tree_id {
        let wc_tree = store.get_tree(&RepoPath::root(), &wc_tree_id)?;
        let added_back = wc_tree.entries_matching(matcher.as_ref()).collect_vec();
        if !added_back.is_empty() {
            locked_working_copy.discard();
            let path = &added_back[0].0;
            let ui_path = ui.format_file_path(workspace_command.workspace_root(), path);
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
    CommitBuilder::for_rewrite_from(ui.settings(), &store, &current_checkout)
        .set_tree(new_tree_id)
        .write_to_repo(tx.mut_repo());
    let num_rebased = tx.mut_repo().rebase_descendants(ui.settings());
    if num_rebased > 0 {
        writeln!(ui, "Rebased {} descendant commits", num_rebased)?;
    }
    let repo = tx.commit();
    locked_working_copy.finish(repo.op_id().clone());
    Ok(())
}

fn cmd_files(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_revision_arg(ui, args)?;
    let matcher = matcher_from_values(
        ui,
        workspace_command.workspace_root(),
        args.values_of("paths"),
    )?;
    for (name, _value) in commit.tree().entries_matching(matcher.as_ref()) {
        writeln!(
            ui,
            "{}",
            &ui.format_file_path(workspace_command.workspace_root(), &name)
        )?;
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

fn cmd_diff(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let from_tree;
    let to_tree;
    if args.is_present("from") || args.is_present("to") {
        let from =
            workspace_command.resolve_single_rev(ui, args.value_of("from").unwrap_or("@"))?;
        from_tree = from.tree();
        let to = workspace_command.resolve_single_rev(ui, args.value_of("to").unwrap_or("@"))?;
        to_tree = to.tree();
    } else {
        let commit =
            workspace_command.resolve_single_rev(ui, args.value_of("revision").unwrap_or("@"))?;
        let parents = commit.parents();
        from_tree = merge_commit_trees(workspace_command.repo().as_repo_ref(), &parents);
        to_tree = commit.tree()
    }
    let repo = workspace_command.repo();
    let workspace_root = workspace_command.workspace_root();
    let matcher = matcher_from_values(ui, workspace_root, args.values_of("paths"))?;
    let diff_iterator = from_tree.diff(&to_tree, matcher.as_ref());
    show_diff(ui, repo, workspace_root, args, diff_iterator)?;
    Ok(())
}

fn cmd_show(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(ui, args.value_of("revision").unwrap())?;
    let parents = commit.parents();
    let from_tree = merge_commit_trees(workspace_command.repo().as_repo_ref(), &parents);
    let to_tree = commit.tree();
    let repo = workspace_command.repo();
    let diff_iterator = from_tree.diff(&to_tree, &EverythingMatcher);
    let workspace_root = workspace_command.workspace_root();
    // TODO: Add branches, tags, etc
    // TODO: Indent the description like Git does
    let template_string = r#"
            label(if(open, "open"),
            "Commit ID: " commit_id "\n"
            "Change ID: " change_id "\n"
            "Author: " author " <" author.email() "> (" author.timestamp() ")\n"
            "Committer: " committer " <" committer.email() "> (" committer.timestamp() ")\n"
            "\n"
            description
            "\n"
            )"#;
    let template = crate::template_parser::parse_commit_template(
        repo.as_repo_ref(),
        &workspace_command.workspace_id(),
        template_string,
    );
    template.format(&commit, ui.stdout_formatter().as_mut())?;
    show_diff(ui, repo, workspace_root, args, diff_iterator)?;
    Ok(())
}

fn show_diff(
    ui: &mut Ui,
    repo: &Arc<ReadonlyRepo>,
    workspace_root: &Path,
    args: &ArgMatches,
    tree_diff: TreeDiffIterator,
) -> Result<(), CommandError> {
    enum Format {
        Summary,
        Git,
        ColorWords,
    }
    let format = {
        if args.is_present("summary") {
            Format::Summary
        } else if args.is_present("git") {
            Format::Git
        } else if args.is_present("color-words") {
            Format::ColorWords
        } else {
            match ui.settings().config().get_str("diff.format") {
                Ok(value) if &value == "summary" => Format::Summary,
                Ok(value) if &value == "git" => Format::Git,
                Ok(value) if &value == "color-words" => Format::ColorWords,
                _ => Format::ColorWords,
            }
        }
    };
    match format {
        Format::Summary => {
            show_diff_summary(ui, workspace_root, tree_diff)?;
        }
        Format::Git => {
            show_git_diff(ui, repo, tree_diff)?;
        }
        Format::ColorWords => {
            show_color_words_diff(ui, workspace_root, repo, tree_diff)?;
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
            let conflict = repo.store().read_conflict(id).unwrap();
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
    ui: &mut Ui,
    workspace_root: &Path,
    repo: &Arc<ReadonlyRepo>,
    tree_diff: TreeDiffIterator,
) -> Result<(), CommandError> {
    let mut formatter = ui.stdout_formatter();
    formatter.add_label(String::from("diff"))?;
    for (path, diff) in tree_diff {
        let ui_path = ui.format_file_path(workspace_root, &path);
        match diff {
            tree::Diff::Added(right_value) => {
                let right_content = diff_content(repo, &path, &right_value)?;
                let description = basic_diff_file_type(&right_value);
                formatter.add_label(String::from("header"))?;
                formatter.write_str(&format!("Added {} {}:\n", description, ui_path))?;
                formatter.remove_label()?;
                show_color_words_diff_hunks(&[], &right_content, formatter.as_mut())?;
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
                show_color_words_diff_hunks(&left_content, &right_content, formatter.as_mut())?;
            }
            tree::Diff::Removed(left_value) => {
                let left_content = diff_content(repo, &path, &left_value)?;
                let description = basic_diff_file_type(&left_value);
                formatter.add_label(String::from("header"))?;
                formatter.write_str(&format!("Removed {} {}:\n", description, ui_path))?;
                formatter.remove_label()?;
                show_color_words_diff_hunks(&left_content, &[], formatter.as_mut())?;
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
            let conflict = repo.store().read_conflict(id).unwrap();
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
                // TODO: Remove this statement once https://github.com/rust-lang/rust/issues/89716
                // has been fixed and released for long enough.
                let lines = if content.is_empty() { vec![] } else { lines };
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
                // TODO: Remove these two statements once https://github.com/rust-lang/rust/issues/89716
                // has been fixed and released for long enough.
                let left_lines = if content[0].is_empty() {
                    vec![]
                } else {
                    left_lines
                };
                let right_lines = if content[1].is_empty() {
                    vec![]
                } else {
                    right_lines
                };
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
    ui: &mut Ui,
    repo: &Arc<ReadonlyRepo>,
    tree_diff: TreeDiffIterator,
) -> Result<(), CommandError> {
    let mut formatter = ui.stdout_formatter();
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
                show_unified_diff_hunks(formatter.as_mut(), &[], &right_part.content)?;
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
                show_unified_diff_hunks(
                    formatter.as_mut(),
                    &left_part.content,
                    &right_part.content,
                )?;
            }
            tree::Diff::Removed(left_value) => {
                let left_part = git_diff_part(repo, &path, &left_value)?;
                writeln!(formatter, "deleted file mode {}", &left_part.mode)?;
                writeln!(formatter, "index {}..0000000000", &left_part.hash)?;
                writeln!(formatter, "--- a/{}", path_string)?;
                writeln!(formatter, "+++ /dev/null")?;
                formatter.remove_label()?;
                show_unified_diff_hunks(formatter.as_mut(), &left_part.content, &[])?;
            }
        }
    }
    formatter.remove_label()?;
    Ok(())
}

fn show_diff_summary(
    ui: &mut Ui,
    workspace_root: &Path,
    tree_diff: TreeDiffIterator,
) -> io::Result<()> {
    ui.stdout_formatter().add_label(String::from("diff"))?;
    for (repo_path, diff) in tree_diff {
        match diff {
            tree::Diff::Modified(_, _) => {
                ui.stdout_formatter().add_label(String::from("modified"))?;
                writeln!(ui, "M {}", ui.format_file_path(workspace_root, &repo_path))?;
                ui.stdout_formatter().remove_label()?;
            }
            tree::Diff::Added(_) => {
                ui.stdout_formatter().add_label(String::from("added"))?;
                writeln!(ui, "A {}", ui.format_file_path(workspace_root, &repo_path))?;
                ui.stdout_formatter().remove_label()?;
            }
            tree::Diff::Removed(_) => {
                ui.stdout_formatter().add_label(String::from("removed"))?;
                writeln!(ui, "R {}", ui.format_file_path(workspace_root, &repo_path))?;
                ui.stdout_formatter().remove_label()?;
            }
        }
    }
    ui.stdout_formatter().remove_label()?;
    Ok(())
}

fn cmd_status(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    workspace_command.maybe_commit_working_copy(ui)?;
    let repo = workspace_command.repo();
    let maybe_checkout_id = repo.view().get_checkout(&workspace_command.workspace_id());
    let maybe_checkout = maybe_checkout_id.map(|id| repo.store().get_commit(id).unwrap());
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
            "  Use `jj branches` to see details. Use `jj branch <name> -r <rev>` to resolve."
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
            "  Use `jj branches` to see details. Use `jj git pull` to resolve."
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
                ui,
                workspace_command.workspace_root(),
                parent_tree.diff(&tree, &EverythingMatcher),
            )?;
        }

        let conflicts = tree.conflicts();
        if !conflicts.is_empty() {
            ui.stdout_formatter().add_label("conflict".to_string())?;
            writeln!(ui, "There are unresolved conflicts at these paths:")?;
            ui.stdout_formatter().remove_label()?;
            for (path, _) in conflicts {
                writeln!(
                    ui,
                    "{}",
                    &ui.format_file_path(workspace_command.workspace_root(), &path)
                )?;
            }
        }
    }

    Ok(())
}

fn log_template(settings: &UserSettings) -> String {
    // TODO: define a method on boolean values, so we can get auto-coloring
    //       with e.g. `conflict.then("conflict")`
    let default_template = r#"
            label(if(open, "open"),
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
            "\n"
            )"#;
    settings
        .config()
        .get_str("template.log.graph")
        .unwrap_or_else(|_| String::from(default_template))
}

fn cmd_log(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let revset_expression =
        workspace_command.parse_revset(ui, args.value_of("revisions").unwrap())?;
    let repo = workspace_command.repo();
    let workspace_id = workspace_command.workspace_id();
    let checkout_id = repo.view().get_checkout(&workspace_id);
    let revset = revset_expression.evaluate(repo.as_repo_ref(), Some(&workspace_id))?;
    let store = repo.store();

    let template_string = match args.value_of("template") {
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

    if !args.is_present("no-graph") {
        let mut graph = AsciiGraphDrawer::new(&mut formatter);
        for (index_entry, edges) in revset.iter().graph() {
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
            let is_checkout = Some(&commit_id) == checkout_id;
            {
                let writer = Box::new(&mut buffer);
                let mut formatter = ui.new_formatter(writer);
                if is_checkout {
                    formatter.add_label("checkout".to_string())?;
                }
                let commit = store.get_commit(&commit_id).unwrap();
                template.format(&commit, formatter.as_mut())?;
                if is_checkout {
                    formatter.remove_label()?;
                }
            }
            if !buffer.ends_with(b"\n") {
                buffer.push(b'\n');
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
        for index_entry in revset.iter() {
            let commit = store.get_commit(&index_entry.commit_id()).unwrap();
            template.format(&commit, formatter)?;
        }
    }

    Ok(())
}

fn cmd_obslog(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let start_commit = workspace_command.resolve_revision_arg(ui, args)?;
    let workspace_id = workspace_command.workspace_id();
    let checkout_id = workspace_command.repo().view().get_checkout(&workspace_id);

    let template_string = match args.value_of("template") {
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
    if !args.is_present("no-graph") {
        let mut graph = AsciiGraphDrawer::new(&mut formatter);
        for commit in commits {
            let mut edges = vec![];
            for predecessor in commit.predecessors() {
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
        }
    }

    Ok(())
}

fn edit_description(repo: &ReadonlyRepo, description: &str) -> String {
    let random: u32 = rand::random();
    let description_file_path = repo.repo_path().join(format!("description-{}", random));
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

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "pico".to_string());
    // Handle things like `EDITOR=emacs -nw`
    let args = editor.split(' ').collect_vec();
    let editor_args = if args.len() > 1 { &args[1..] } else { &[] };
    let exit_status = std::process::Command::new(args[0])
        .args(editor_args)
        .arg(&description_file_path)
        .status()
        .expect("failed to run editor");
    assert!(exit_status.success(), "failed to run editor");

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
    lines.join("")
}

fn cmd_describe(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_revision_arg(ui, args)?;
    workspace_command.check_rewriteable(&commit)?;
    let repo = workspace_command.repo();
    let description;
    if args.is_present("stdin") {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer).unwrap();
        description = buffer;
    } else if args.is_present("message") {
        description = args.value_of("message").unwrap().to_owned()
    } else {
        description = edit_description(repo, commit.description());
    }
    if description == *commit.description() {
        ui.write("Nothing changed.\n")?;
    } else {
        let mut tx =
            workspace_command.start_transaction(&format!("describe commit {}", commit.id().hex()));
        CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
            .set_description(description)
            .write_to_repo(tx.mut_repo());
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_open(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_revision_arg(ui, args)?;
    workspace_command.check_rewriteable(&commit)?;
    let repo = workspace_command.repo();
    let mut tx = workspace_command.start_transaction(&format!("open commit {}", commit.id().hex()));
    CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
        .set_open(true)
        .write_to_repo(tx.mut_repo());
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_close(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_revision_arg(ui, args)?;
    workspace_command.check_rewriteable(&commit)?;
    let repo = workspace_command.repo();
    let mut commit_builder =
        CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit).set_open(false);
    let description = if args.is_present("message") {
        args.value_of("message").unwrap().to_string()
    } else if commit.description().is_empty() {
        edit_description(repo, "\n\nJJ: Enter commit description.\n")
    } else if args.is_present("edit") {
        edit_description(repo, commit.description())
    } else {
        commit.description().to_string()
    };
    commit_builder = commit_builder.set_description(description);
    let mut tx =
        workspace_command.start_transaction(&format!("close commit {}", commit.id().hex()));
    commit_builder.write_to_repo(tx.mut_repo());
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_duplicate(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let predecessor = workspace_command.resolve_revision_arg(ui, args)?;
    let repo = workspace_command.repo();
    let mut tx = workspace_command
        .start_transaction(&format!("duplicate commit {}", predecessor.id().hex()));
    let mut_repo = tx.mut_repo();
    let new_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &predecessor)
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
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let to_abandon = workspace_command.resolve_revset(ui, args.value_of("revision").unwrap())?;
    workspace_command.check_non_empty(&to_abandon)?;
    for commit in &to_abandon {
        workspace_command.check_rewriteable(commit)?;
    }
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
    let num_rebased = tx.mut_repo().rebase_descendants(ui.settings());
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

fn cmd_new(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let parent = workspace_command.resolve_revision_arg(ui, args)?;
    let repo = workspace_command.repo();
    let commit_builder = CommitBuilder::for_open_commit(
        ui.settings(),
        repo.store(),
        parent.id().clone(),
        parent.tree().id().clone(),
    );
    let mut tx = workspace_command.start_transaction("new empty commit");
    let mut_repo = tx.mut_repo();
    let new_commit = commit_builder.write_to_repo(mut_repo);
    let workspace_id = workspace_command.workspace_id();
    if mut_repo.view().get_checkout(&workspace_id) == Some(parent.id()) {
        mut_repo.check_out(workspace_id, ui.settings(), &new_commit);
    }
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_move(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let source = workspace_command.resolve_single_rev(ui, args.value_of("from").unwrap())?;
    let mut destination = workspace_command.resolve_single_rev(ui, args.value_of("to").unwrap())?;
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
    let new_parent_tree_id = if args.is_present("interactive") {
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
        workspace_command.edit_diff(&parent_tree, &source_tree, &instructions)?
    } else {
        source_tree.id().clone()
    };
    if &new_parent_tree_id == parent_tree.id() {
        return Err(CommandError::UserError(String::from("No changes to move")));
    }
    let new_parent_tree = repo
        .store()
        .get_tree(&RepoPath::root(), &new_parent_tree_id)?;
    // Apply the reverse of the selected changes onto the source
    let new_source_tree_id = merge_trees(&source_tree, &new_parent_tree, &parent_tree)?;
    if new_source_tree_id == *parent_tree.id() {
        mut_repo.record_abandoned_commit(source.id().clone());
    } else {
        CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &source)
            .set_tree(new_source_tree_id)
            .write_to_repo(mut_repo);
    }
    if repo.index().is_ancestor(source.id(), destination.id()) {
        // If we're moving changes to a descendant, first rebase descendants onto the
        // rewritten source. Otherwise it will likely already have the content
        // changes we're moving, so applying them will have no effect and the
        // changes will disappear.
        let mut rebaser = mut_repo.create_descendant_rebaser(ui.settings());
        rebaser.rebase_all();
        let rebased_destination_id = rebaser.rebased().get(destination.id()).unwrap().clone();
        destination = mut_repo
            .store()
            .get_commit(&rebased_destination_id)
            .unwrap();
    }
    // Apply the selected changes onto the destination
    let new_destination_tree_id = merge_trees(&destination.tree(), &parent_tree, &new_parent_tree)?;
    CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &destination)
        .set_tree(new_destination_tree_id)
        .write_to_repo(mut_repo);
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_squash(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_revision_arg(ui, args)?;
    workspace_command.check_rewriteable(&commit)?;
    let repo = workspace_command.repo();
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
    let mut_repo = tx.mut_repo();
    let new_parent_tree_id;
    if args.is_present("interactive") {
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
        new_parent_tree_id =
            workspace_command.edit_diff(&parent.tree(), &commit.tree(), &instructions)?;
        if &new_parent_tree_id == parent.tree().id() {
            return Err(CommandError::UserError(String::from("No changes selected")));
        }
    } else {
        new_parent_tree_id = commit.tree().id().clone();
    }
    // Abandon the child if the parent now has all the content from the child
    // (always the case in the non-interactive case).
    let abandon_child = &new_parent_tree_id == commit.tree().id();
    let new_parent = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), parent)
        .set_tree(new_parent_tree_id)
        .set_predecessors(vec![parent.id().clone(), commit.id().clone()])
        .write_to_repo(mut_repo);
    if abandon_child {
        mut_repo.record_abandoned_commit(commit.id().clone());
    } else {
        // Commit the remainder on top of the new parent commit.
        CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
            .set_parents(vec![new_parent.id().clone()])
            .write_to_repo(mut_repo);
    }
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_unsquash(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_revision_arg(ui, args)?;
    workspace_command.check_rewriteable(&commit)?;
    let repo = workspace_command.repo();
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
    let mut_repo = tx.mut_repo();
    let parent_base_tree = merge_commit_trees(repo.as_repo_ref(), &parent.parents());
    let new_parent_tree_id;
    if args.is_present("interactive") {
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
            workspace_command.edit_diff(&parent_base_tree, &parent.tree(), &instructions)?;
        if &new_parent_tree_id == parent_base_tree.id() {
            return Err(CommandError::UserError(String::from("No changes selected")));
        }
    } else {
        new_parent_tree_id = parent_base_tree.id().clone();
    }
    // Abandon the parent if it is now empty (always the case in the non-interactive
    // case).
    if &new_parent_tree_id == parent_base_tree.id() {
        mut_repo.record_abandoned_commit(parent.id().clone());
        // Commit the new child on top of the parent's parents.
        CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
            .set_parents(parent.parent_ids())
            .write_to_repo(mut_repo);
    } else {
        let new_parent = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), parent)
            .set_tree(new_parent_tree_id)
            .set_predecessors(vec![parent.id().clone(), commit.id().clone()])
            .write_to_repo(mut_repo);
        // Commit the new child on top of the new parent.
        CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
            .set_parents(vec![new_parent.id().clone()])
            .write_to_repo(mut_repo);
    }
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_restore(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let from_commit = workspace_command.resolve_single_rev(ui, args.value_of("from").unwrap())?;
    let to_commit = workspace_command.resolve_single_rev(ui, args.value_of("to").unwrap())?;
    workspace_command.check_rewriteable(&to_commit)?;
    let repo = workspace_command.repo();
    let tree_id;
    if args.is_present("interactive") {
        if args.is_present("paths") {
            return Err(UserError(
                "restore with --interactive and path is not yet supported".to_string(),
            ));
        }
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
        tree_id =
            workspace_command.edit_diff(&from_commit.tree(), &to_commit.tree(), &instructions)?;
    } else if args.is_present("paths") {
        let matcher = matcher_from_values(
            ui,
            workspace_command.workspace_root(),
            args.values_of("paths"),
        )?;
        let mut tree_builder = repo.store().tree_builder(to_commit.tree().id().clone());
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
        tree_id = from_commit.tree().id().clone();
    }
    if &tree_id == to_commit.tree().id() {
        ui.write("Nothing changed.\n")?;
    } else {
        let mut tx = workspace_command
            .start_transaction(&format!("restore into commit {}", to_commit.id().hex()));
        let mut_repo = tx.mut_repo();
        let new_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &to_commit)
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

fn cmd_edit(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_revision_arg(ui, args)?;
    workspace_command.check_rewriteable(&commit)?;
    let repo = workspace_command.repo();
    let base_tree = merge_commit_trees(repo.as_repo_ref(), &commit.parents());
    let instructions = format!(
        "\
You are editing changes in: {}

The diff initially shows the commit's changes.

Adjust the right side until it shows the contents you want. If you
don't make any changes, then the operation will be aborted.",
        short_commit_description(&commit)
    );
    let tree_id = workspace_command.edit_diff(&base_tree, &commit.tree(), &instructions)?;
    if &tree_id == commit.tree().id() {
        ui.write("Nothing changed.\n")?;
    } else {
        let mut tx =
            workspace_command.start_transaction(&format!("edit commit {}", commit.id().hex()));
        let mut_repo = tx.mut_repo();
        let new_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
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

fn cmd_split(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?.rebase_descendants(false);
    let commit = workspace_command.resolve_revision_arg(ui, args)?;
    workspace_command.check_rewriteable(&commit)?;
    let repo = workspace_command.repo();
    let base_tree = merge_commit_trees(repo.as_repo_ref(), &commit.parents());
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
    let tree_id = workspace_command.edit_diff(&base_tree, &commit.tree(), &instructions)?;
    if &tree_id == commit.tree().id() {
        ui.write("Nothing changed.\n")?;
    } else {
        let mut tx =
            workspace_command.start_transaction(&format!("split commit {}", commit.id().hex()));
        let mut_repo = tx.mut_repo();
        let first_description = edit_description(
            repo,
            &("JJ: Enter commit description for the first part.\n".to_string()
                + commit.description()),
        );
        let first_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
            .set_tree(tree_id)
            .set_description(first_description)
            .write_to_repo(mut_repo);
        let second_description = edit_description(
            repo,
            &("JJ: Enter commit description for the second part.\n".to_string()
                + commit.description()),
        );
        let second_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
            .set_parents(vec![first_commit.id().clone()])
            .set_tree(commit.tree().id().clone())
            .generate_new_change_id()
            .set_description(second_description)
            .write_to_repo(mut_repo);
        let mut rebaser = DescendantRebaser::new(
            ui.settings(),
            mut_repo,
            hashmap! { commit.id().clone() => hashset!{second_commit.id().clone()} },
            hashset! {},
        );
        rebaser.rebase_all();
        let num_rebased = rebaser.rebased().len();
        if num_rebased > 0 {
            writeln!(ui, "Rebased {} descendant commits", num_rebased)?;
        }
        ui.write("First part: ")?;
        ui.write_commit_summary(
            mut_repo.as_repo_ref(),
            &workspace_command.workspace_id(),
            &first_commit,
        )?;
        ui.write("\nSecond part: ")?;
        ui.write_commit_summary(
            mut_repo.as_repo_ref(),
            &workspace_command.workspace_id(),
            &second_commit,
        )?;
        ui.write("\n")?;
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_merge(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let revision_args = args.values_of("revisions").unwrap();
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
        let commit = workspace_command.resolve_single_rev(ui, revision_arg)?;
        parent_ids.push(commit.id().clone());
        commits.push(commit);
    }
    let repo = workspace_command.repo();
    let description = if args.is_present("message") {
        args.value_of("message").unwrap().to_string()
    } else {
        edit_description(
            repo,
            "\n\nJJ: Enter commit description for the merge commit.\n",
        )
    };
    let merged_tree = merge_commit_trees(repo.as_repo_ref(), &commits);
    let mut tx = workspace_command.start_transaction("merge commits");
    CommitBuilder::for_new_commit(ui.settings(), repo.store(), merged_tree.id().clone())
        .set_parents(parent_ids)
        .set_description(description)
        .set_open(false)
        .write_to_repo(tx.mut_repo());
    workspace_command.finish_transaction(ui, tx)?;

    Ok(())
}

fn cmd_rebase(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?.rebase_descendants(false);
    let mut new_parents = vec![];
    for revision_str in args.values_of("destination").unwrap() {
        let destination = workspace_command.resolve_single_rev(ui, revision_str)?;
        new_parents.push(destination);
    }
    // TODO: Unless we want to allow both --revision and --source, is it better to
    // replace   --source by --rebase-descendants?
    let old_commit;
    let rebase_descendants;
    if let Some(source_str) = args.value_of("source") {
        rebase_descendants = true;
        old_commit = workspace_command.resolve_single_rev(ui, source_str)?;
    } else {
        rebase_descendants = false;
        old_commit =
            workspace_command.resolve_single_rev(ui, args.value_of("revision").unwrap_or("@"))?;
    }
    workspace_command.check_rewriteable(&old_commit)?;
    for parent in &new_parents {
        if workspace_command
            .repo
            .index()
            .is_ancestor(old_commit.id(), parent.id())
        {
            return Err(CommandError::UserError(format!(
                "Cannot rebase {} onto descendant {}",
                short_commit_hash(old_commit.id()),
                short_commit_hash(parent.id())
            )));
        }
    }

    if rebase_descendants {
        let mut tx = workspace_command.start_transaction(&format!(
            "rebase commit {} and descendants",
            old_commit.id().hex()
        ));
        rebase_commit(ui.settings(), tx.mut_repo(), &old_commit, &new_parents);
        let num_rebased = tx.mut_repo().rebase_descendants(ui.settings()) + 1;
        writeln!(ui, "Rebased {} commits", num_rebased)?;
        workspace_command.finish_transaction(ui, tx)?;
    } else {
        let mut tx = workspace_command
            .start_transaction(&format!("rebase commit {}", old_commit.id().hex()));
        rebase_commit(ui.settings(), tx.mut_repo(), &old_commit, &new_parents);
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
        num_rebased_descendants += tx.mut_repo().rebase_descendants(ui.settings());
        if num_rebased_descendants > 0 {
            writeln!(
                ui,
                "Also rebased {} descendant commits onto parent of rebased commit",
                num_rebased_descendants
            )?;
        }
        workspace_command.finish_transaction(ui, tx)?;
    }

    Ok(())
}

fn cmd_backout(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit_to_back_out = workspace_command.resolve_revision_arg(ui, args)?;
    let mut parents = vec![];
    for revision_str in args.values_of("destination").unwrap() {
        let destination = workspace_command.resolve_single_rev(ui, revision_str)?;
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

fn cmd_branch(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?.rebase_descendants(false);
    let branch_name = args.value_of("name").unwrap();
    if args.is_present("delete") {
        if workspace_command
            .repo()
            .view()
            .get_local_branch(branch_name)
            .is_none()
        {
            return Err(CommandError::UserError("No such branch".to_string()));
        }
        let mut tx = workspace_command.start_transaction(&format!("delete branch {}", branch_name));
        tx.mut_repo().remove_local_branch(branch_name);
        workspace_command.finish_transaction(ui, tx)?;
    } else if args.is_present("forget") {
        if workspace_command
            .repo()
            .view()
            .get_local_branch(branch_name)
            .is_none()
        {
            return Err(CommandError::UserError("No such branch".to_string()));
        }
        let mut tx = workspace_command.start_transaction(&format!("forget branch {}", branch_name));
        tx.mut_repo().remove_branch(branch_name);
        workspace_command.finish_transaction(ui, tx)?;
    } else {
        let target_commit = workspace_command.resolve_revision_arg(ui, args)?;
        if !args.is_present("allow-backwards")
            && !is_fast_forward(
                workspace_command.repo().as_repo_ref(),
                branch_name,
                target_commit.id(),
            )
        {
            return Err(CommandError::UserError(
                "Use --allow-backwards to allow moving a branch backwards or sideways".to_string(),
            ));
        }
        let mut tx = workspace_command.start_transaction(&format!(
            "point branch {} to commit {}",
            branch_name,
            target_commit.id().hex()
        ));
        tx.mut_repo().set_local_branch(
            branch_name.to_string(),
            RefTarget::Normal(target_commit.id().clone()),
        );
        workspace_command.finish_transaction(ui, tx)?;
    }

    Ok(())
}

fn cmd_branches(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &ArgMatches,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
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

fn cmd_debug(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    if let Some(completion_matches) = args.subcommand_matches("completion") {
        let mut app = command.app.clone();
        let mut buf = vec![];
        let shell = if completion_matches.is_present("zsh") {
            clap_complete::Shell::Zsh
        } else if completion_matches.is_present("fish") {
            clap_complete::Shell::Fish
        } else {
            clap_complete::Shell::Bash
        };
        clap_complete::generate(shell, &mut app, "jj", &mut buf);
        ui.stdout_formatter().write_all(&buf)?;
    } else if let Some(_mangen_matches) = args.subcommand_matches("mangen") {
        let mut buf = vec![];
        let man = clap_mangen::Man::new(command.app.clone());
        man.render(&mut buf)?;
        ui.stdout_formatter().write_all(&buf)?;
    } else if let Some(resolve_matches) = args.subcommand_matches("resolverev") {
        let mut workspace_command = command.workspace_helper(ui)?;
        let commit = workspace_command.resolve_revision_arg(ui, resolve_matches)?;
        writeln!(ui, "{}", commit.id().hex())?;
    } else if let Some(_wc_matches) = args.subcommand_matches("workingcopy") {
        let workspace_command = command.workspace_helper(ui)?;
        let wc = workspace_command.working_copy();
        writeln!(ui, "Current operation: {:?}", wc.operation_id())?;
        writeln!(ui, "Current tree: {:?}", wc.current_tree_id())?;
        for (file, state) in wc.file_states().iter() {
            writeln!(
                ui,
                "{:?} {:13?} {:10?} {:?}",
                state.file_type, state.size, state.mtime.0, file
            )?;
        }
    } else if let Some(template_matches) = args.subcommand_matches("template") {
        let parse = TemplateParser::parse(
            crate::template_parser::Rule::template,
            template_matches.value_of("template").unwrap(),
        );
        writeln!(ui, "{:?}", parse)?;
    } else if let Some(_reindex_matches) = args.subcommand_matches("index") {
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
    } else if let Some(_reindex_matches) = args.subcommand_matches("reindex") {
        let mut workspace_command = command.workspace_helper(ui)?;
        let mut_repo = Arc::get_mut(workspace_command.repo_mut()).unwrap();
        let index = mut_repo.reindex();
        writeln!(ui, "Finished indexing {:?} commits.", index.num_commits())?;
    } else {
        panic!("unhandled command: {:#?}", command.root_args());
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

fn cmd_bench(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    if let Some(command_matches) = args.subcommand_matches("commonancestors") {
        let mut workspace_command = command.workspace_helper(ui)?;
        let revision1_str = command_matches.value_of("revision1").unwrap();
        let commit1 = workspace_command.resolve_single_rev(ui, revision1_str)?;
        let revision2_str = command_matches.value_of("revision2").unwrap();
        let commit2 = workspace_command.resolve_single_rev(ui, revision2_str)?;
        let index = workspace_command.repo().index();
        let routine = || index.common_ancestors(&[commit1.id().clone()], &[commit2.id().clone()]);
        run_bench(
            ui,
            &format!("commonancestors-{}-{}", revision1_str, revision2_str),
            routine,
        )?;
    } else if let Some(command_matches) = args.subcommand_matches("isancestor") {
        let mut workspace_command = command.workspace_helper(ui)?;
        let ancestor_str = command_matches.value_of("ancestor").unwrap();
        let ancestor_commit = workspace_command.resolve_single_rev(ui, ancestor_str)?;
        let descendants_str = command_matches.value_of("descendant").unwrap();
        let descendant_commit = workspace_command.resolve_single_rev(ui, descendants_str)?;
        let index = workspace_command.repo().index();
        let routine = || index.is_ancestor(ancestor_commit.id(), descendant_commit.id());
        run_bench(
            ui,
            &format!("isancestor-{}-{}", ancestor_str, descendants_str),
            routine,
        )?;
    } else if let Some(command_matches) = args.subcommand_matches("walkrevs") {
        let mut workspace_command = command.workspace_helper(ui)?;
        let unwanted_str = command_matches.value_of("unwanted").unwrap();
        let unwanted_commit = workspace_command.resolve_single_rev(ui, unwanted_str)?;
        let wanted_str = command_matches.value_of("wanted");
        let wanted_commit = workspace_command.resolve_single_rev(ui, wanted_str.unwrap())?;
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
            &format!("walkrevs-{}-{}", unwanted_str, wanted_str.unwrap()),
            routine,
        )?;
    } else if let Some(command_matches) = args.subcommand_matches("resolveprefix") {
        let workspace_command = command.workspace_helper(ui)?;
        let prefix =
            HexPrefix::new(command_matches.value_of("prefix").unwrap().to_string()).unwrap();
        let index = workspace_command.repo().index();
        let routine = || index.resolve_prefix(&prefix);
        run_bench(ui, &format!("resolveprefix-{}", prefix.hex()), routine)?;
    } else {
        panic!("unhandled command: {:#?}", command.root_args());
    };
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
    _args: &ArgMatches,
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
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let bad_op = resolve_single_op(repo, args.value_of("operation").unwrap())?;
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
    let bad_repo = repo.loader().load_at(&bad_op);
    let parent_repo = repo.loader().load_at(&parent_ops[0]);
    tx.mut_repo().merge(&bad_repo, &parent_repo);
    workspace_command.finish_transaction(ui, tx)?;

    Ok(())
}
fn cmd_op_restore(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let target_op = resolve_single_op(repo, args.value_of("operation").unwrap())?;
    let mut tx = workspace_command
        .start_transaction(&format!("restore to operation {}", target_op.id().hex()));
    tx.mut_repo().set_view(target_op.view().take_store_view());
    workspace_command.finish_transaction(ui, tx)?;

    Ok(())
}

fn cmd_operation(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    if let Some(command_matches) = args.subcommand_matches("log") {
        cmd_op_log(ui, command, command_matches)?;
    } else if let Some(command_matches) = args.subcommand_matches("undo") {
        cmd_op_undo(ui, command, command_matches)?;
    } else if let Some(command_matches) = args.subcommand_matches("restore") {
        cmd_op_restore(ui, command, command_matches)?;
    } else {
        panic!("unhandled command: {:#?}", command.root_args());
    }
    Ok(())
}

fn cmd_undo(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    cmd_op_undo(ui, command, args)
}

fn cmd_workspace(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    if let Some(command_matches) = args.subcommand_matches("add") {
        cmd_workspace_add(ui, command, command_matches)?;
    } else if let Some(command_matches) = args.subcommand_matches("forget") {
        cmd_workspace_forget(ui, command, command_matches)?;
    } else if let Some(command_matches) = args.subcommand_matches("list") {
        cmd_workspace_list(ui, command, command_matches)?;
    } else {
        panic!("unhandled command: {:#?}", command.root_args());
    }
    Ok(())
}

fn cmd_workspace_add(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let old_workspace_command = command.workspace_helper(ui)?;
    let destination_str = args.value_of("destination").unwrap();
    let destination_path = ui.cwd().join(destination_str);
    if destination_path.exists() {
        return Err(CommandError::UserError(
            "Workspace already exists".to_string(),
        ));
    } else {
        fs::create_dir(&destination_path).unwrap();
    }
    let name = if let Some(name) = args.value_of("name") {
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
        destination_path.display()
    )?;

    let mut new_workspace_command = WorkspaceCommandHelper::for_loaded_repo(
        ui,
        new_workspace,
        command.string_args.clone(),
        &command.root_args,
        repo,
    )?;
    let mut tx = new_workspace_command
        .start_transaction(&format!("Initial checkout in workspace {}", &name));
    // Check out a parent of the checkout of the current workspace, or the root if
    // there is no checkout in the current workspace.
    let new_checkout_commit = if let Some(old_checkout_id) = new_workspace_command
        .repo()
        .view()
        .get_checkout(&new_workspace_command.workspace_id())
    {
        new_workspace_command
            .repo()
            .store()
            .get_commit(old_checkout_id)
            .unwrap()
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
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let workspace_id = if let Some(workspace_str) = args.value_of("workspace") {
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
    _args: &ArgMatches,
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

fn get_git_repo(store: &Store) -> Result<git2::Repository, CommandError> {
    match store.git_repo() {
        None => Err(CommandError::UserError(
            "The repo is not backed by a git repo".to_string(),
        )),
        Some(git_repo) => Ok(git_repo),
    }
}

fn cmd_git_remote(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    if let Some(command_matches) = args.subcommand_matches("add") {
        cmd_git_remote_add(ui, command, command_matches)?;
    } else if let Some(command_matches) = args.subcommand_matches("remove") {
        cmd_git_remote_remove(ui, command, command_matches)?;
    } else {
        panic!("unhandled command: {:#?}", command.root_args());
    }
    Ok(())
}

fn cmd_git_remote_add(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    let remote_name = args.value_of("remote").unwrap();
    let url = args.value_of("url").unwrap();
    if git_repo.find_remote(remote_name).is_ok() {
        return Err(CommandError::UserError("Remote already exists".to_string()));
    }
    git_repo
        .remote(remote_name, url)
        .map_err(|err| CommandError::UserError(err.to_string()))?;
    Ok(())
}

fn cmd_git_remote_remove(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    let remote_name = args.value_of("remote").unwrap();
    if git_repo.find_remote(remote_name).is_err() {
        return Err(CommandError::UserError("Remote doesn't exists".to_string()));
    }
    git_repo
        .remote_delete(remote_name)
        .map_err(|err| CommandError::UserError(err.to_string()))?;
    let mut branches_to_delete = vec![];
    for (branch, target) in repo.view().branches() {
        if target.remote_targets.contains_key(remote_name) {
            branches_to_delete.push(branch.clone());
        }
    }
    if !branches_to_delete.is_empty() {
        let mut tx =
            workspace_command.start_transaction(&format!("remove git remote {}", remote_name));
        for branch in branches_to_delete {
            tx.mut_repo().remove_remote_branch(&branch, remote_name);
        }
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_git_fetch(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    let remote_name = args.value_of("remote").unwrap();
    let mut tx =
        workspace_command.start_transaction(&format!("fetch from git remote {}", remote_name));
    git::fetch(tx.mut_repo(), &git_repo, remote_name)
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

fn cmd_git_clone(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    if command.root_args.occurrences_of("repository") > 0 {
        return Err(CommandError::UserError(
            "'--repository' cannot be used with 'git clone'".to_string(),
        ));
    }
    let source = args.value_of("source").unwrap();
    let wc_path_str = args
        .value_of("destination")
        .or_else(|| clone_destination_for_source(source))
        .ok_or_else(|| {
            CommandError::UserError(
                "No destination specified and wasn't able to guess it".to_string(),
            )
        })?;
    let wc_path = ui.cwd().join(wc_path_str);
    if wc_path.exists() {
        assert!(wc_path.is_dir());
    } else {
        fs::create_dir(&wc_path).unwrap();
    }

    let (workspace, repo) = Workspace::init_internal_git(ui.settings(), wc_path.clone())?;
    let git_repo = get_git_repo(repo.store())?;
    writeln!(ui, "Fetching into new repo in {:?}", wc_path)?;
    let mut workspace_command = command.for_loaded_repo(ui, workspace, repo)?;
    let remote_name = "origin";
    git_repo.remote(remote_name, source).unwrap();
    let mut tx = workspace_command.start_transaction("fetch from git remote into empty repo");
    let maybe_default_branch =
        git::fetch(tx.mut_repo(), &git_repo, remote_name).map_err(|err| match err {
            GitFetchError::NoSuchRemote(_) => {
                panic!("should't happen as we just created the git remote")
            }
            GitFetchError::InternalGitError(err) => {
                CommandError::UserError(format!("Fetch failed: {:?}", err))
            }
        })?;
    if let Some(default_branch) = maybe_default_branch {
        let default_branch_target = tx
            .mut_repo()
            .view()
            .get_remote_branch(&default_branch, "origin");
        if let Some(RefTarget::Normal(commit_id)) = default_branch_target {
            if let Ok(commit) = workspace_command.repo().store().get_commit(&commit_id) {
                tx.mut_repo()
                    .check_out(workspace_command.workspace_id(), ui.settings(), &commit);
            }
        }
    }
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_git_push(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ArgMatches,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let remote_name = args.value_of("remote").unwrap();

    let mut branch_updates = HashMap::new();
    if let Some(branch_name) = args.value_of("branch") {
        let maybe_branch_target = repo.view().get_branch(branch_name);
        if maybe_branch_target.is_none() {
            return Err(CommandError::UserError(format!(
                "Branch {} doesn't exist",
                branch_name
            )));
        }
        let branch_target = maybe_branch_target.unwrap();
        let push_action = classify_branch_push_action(branch_target, remote_name);

        match push_action {
            BranchPushAction::AlreadyMatches => {
                writeln!(
                    ui,
                    "Branch {}@{} already matches {}",
                    branch_name, remote_name, branch_name
                )?;
                return Ok(());
            }
            BranchPushAction::LocalConflicted => {
                return Err(CommandError::UserError(format!(
                    "Branch {} is conflicted",
                    branch_name
                )));
            }
            BranchPushAction::RemoteConflicted => {
                return Err(CommandError::UserError(format!(
                    "Branch {}@{} is conflicted",
                    branch_name, remote_name
                )));
            }
            BranchPushAction::Update(update) => {
                if let Some(new_target) = &update.new_target {
                    let new_target_commit = repo.store().get_commit(new_target)?;
                    if new_target_commit.is_open() {
                        return Err(CommandError::UserError(
                            "Won't push open commit".to_string(),
                        ));
                    }
                }
                branch_updates.insert(branch_name, update);
            }
        }
    } else {
        // TODO: Is it useful to warn about conflicted branches?
        for (branch_name, branch_target) in repo.view().branches() {
            let push_action = classify_branch_push_action(branch_target, remote_name);
            match push_action {
                BranchPushAction::AlreadyMatches => {}
                BranchPushAction::LocalConflicted => {}
                BranchPushAction::RemoteConflicted => {}
                BranchPushAction::Update(update) => {
                    branch_updates.insert(branch_name, update);
                }
            }
        }
    }

    if branch_updates.is_empty() {
        writeln!(ui, "Nothing changed.")?;
        return Ok(());
    }

    let mut ref_updates = vec![];
    let mut new_heads = vec![];
    for (branch_name, update) in branch_updates {
        let qualified_name = format!("refs/heads/{}", branch_name);
        if let Some(new_target) = update.new_target {
            new_heads.push(new_target.clone());
            let force = match update.old_target {
                None => false,
                Some(old_target) => !repo.index().is_ancestor(&old_target, &new_target),
            };
            ref_updates.push(GitRefUpdate {
                qualified_name,
                force,
                new_target: Some(new_target),
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
        if let Some(old_head) = branch_target.remote_targets.get(remote_name) {
            old_heads.extend(old_head.adds());
        }
    }
    for index_entry in repo.index().walk_revs(&new_heads, &old_heads) {
        let commit = repo.store().get_commit(&index_entry.commit_id())?;
        if commit.tree().has_conflict() {
            return Err(UserError(format!(
                "Won't push commit {} since it has conflicts",
                short_commit_hash(commit.id())
            )));
        }
    }

    let git_repo = get_git_repo(repo.store())?;
    git::push_updates(&git_repo, remote_name, &ref_updates)
        .map_err(|err| CommandError::UserError(err.to_string()))?;
    let mut tx = workspace_command.start_transaction("import git refs");
    git::import_refs(tx.mut_repo(), &git_repo)?;
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_git_import(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &ArgMatches,
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
    _args: &ArgMatches,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    git::export_refs(repo, &git_repo)?;
    Ok(())
}

fn cmd_git(ui: &mut Ui, command: &CommandHelper, args: &ArgMatches) -> Result<(), CommandError> {
    if let Some(command_matches) = args.subcommand_matches("remote") {
        cmd_git_remote(ui, command, command_matches)?;
    } else if let Some(command_matches) = args.subcommand_matches("fetch") {
        cmd_git_fetch(ui, command, command_matches)?;
    } else if let Some(command_matches) = args.subcommand_matches("clone") {
        cmd_git_clone(ui, command, command_matches)?;
    } else if let Some(command_matches) = args.subcommand_matches("push") {
        cmd_git_push(ui, command, command_matches)?;
    } else if let Some(command_matches) = args.subcommand_matches("import") {
        cmd_git_import(ui, command, command_matches)?;
    } else if let Some(command_matches) = args.subcommand_matches("export") {
        cmd_git_export(ui, command, command_matches)?;
    } else {
        panic!("unhandled command: {:#?}", command.root_args());
    }
    Ok(())
}

fn resolve_alias(ui: &mut Ui, args: Vec<String>) -> Vec<String> {
    if args.len() >= 2 {
        let command_name = args[1].clone();
        if let Ok(alias_definition) = ui
            .settings()
            .config()
            .get_array(&format!("alias.{}", command_name))
        {
            let mut resolved_args = vec![args[0].clone()];
            for arg in alias_definition {
                match arg.into_str() {
                    Ok(string_arg) => resolved_args.push(string_arg),
                    Err(err) => {
                        ui.write_error(&format!(
                            "Warning: Ignoring bad alias definition: {:?}\n",
                            err
                        ))
                        .unwrap();
                        return args;
                    }
                }
            }
            resolved_args.extend_from_slice(&args[2..]);
            return resolved_args;
        }
    }
    args
}

pub fn dispatch<I, T>(mut ui: Ui, args: I) -> i32
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
            ui.write_error("Error: Non-utf8 argument\n").unwrap();
            return 1;
        }
    }
    let string_args = resolve_alias(&mut ui, string_args);
    let app = get_app();
    let matches = app.clone().get_matches_from(&string_args);
    let command_helper = CommandHelper::new(app, string_args, matches.clone());
    let result = if let Some(sub_args) = command_helper.root_args.subcommand_matches("init") {
        cmd_init(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("checkout") {
        cmd_checkout(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("untrack") {
        cmd_untrack(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("files") {
        cmd_files(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("diff") {
        cmd_diff(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("show") {
        cmd_show(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("status") {
        cmd_status(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("log") {
        cmd_log(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("obslog") {
        cmd_obslog(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("describe") {
        cmd_describe(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("close") {
        cmd_close(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("open") {
        cmd_open(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("duplicate") {
        cmd_duplicate(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("abandon") {
        cmd_abandon(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("new") {
        cmd_new(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("move") {
        cmd_move(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("squash") {
        cmd_squash(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("unsquash") {
        cmd_unsquash(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("restore") {
        cmd_restore(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("edit") {
        cmd_edit(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("split") {
        cmd_split(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("merge") {
        cmd_merge(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("rebase") {
        cmd_rebase(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("backout") {
        cmd_backout(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("branch") {
        cmd_branch(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("branches") {
        cmd_branches(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("operation") {
        cmd_operation(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("undo") {
        cmd_undo(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("workspace") {
        cmd_workspace(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("git") {
        cmd_git(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("bench") {
        cmd_bench(&mut ui, &command_helper, sub_args)
    } else if let Some(sub_args) = matches.subcommand_matches("debug") {
        cmd_debug(&mut ui, &command_helper, sub_args)
    } else {
        panic!("unhandled command: {:#?}", matches);
    };
    match result {
        Ok(()) => 0,
        Err(CommandError::UserError(message)) => {
            ui.write_error(&format!("Error: {}\n", message)).unwrap();
            1
        }
        Err(CommandError::BrokenPipe) => 2,
        Err(CommandError::InternalError(message)) => {
            ui.write_error(&format!("Internal error: {}\n", message))
                .unwrap();
            255
        }
    }
}
