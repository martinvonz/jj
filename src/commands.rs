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
extern crate config;

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::fmt::Debug;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;
use std::{fs, io};

use clap::{crate_version, App, Arg, ArgMatches, SubCommand};
use criterion::Criterion;
use itertools::Itertools;
use jujutsu_lib::commit::Commit;
use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::dag_walk::topo_order_reverse;
use jujutsu_lib::diff::DiffHunk;
use jujutsu_lib::evolution::{
    DivergenceResolution, DivergenceResolver, OrphanResolution, OrphanResolver,
};
use jujutsu_lib::files::DiffLine;
use jujutsu_lib::git::GitFetchError;
use jujutsu_lib::index::HexPrefix;
use jujutsu_lib::matchers::{EverythingMatcher, FilesMatcher, Matcher};
use jujutsu_lib::op_heads_store::OpHeadsStore;
use jujutsu_lib::op_store::{OpStore, OpStoreError, OperationId, RefTarget};
use jujutsu_lib::operation::Operation;
use jujutsu_lib::repo::{
    MutableRepo, ReadonlyRepo, RepoInitError, RepoLoadError, RepoLoader, RepoRef,
};
use jujutsu_lib::revset::{RevsetError, RevsetExpression, RevsetParseError};
use jujutsu_lib::revset_graph_iterator::RevsetGraphEdgeType;
use jujutsu_lib::rewrite::{back_out_commit, merge_commit_trees, rebase_commit};
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::store::{CommitId, StoreError, Timestamp, TreeValue};
use jujutsu_lib::store_wrapper::StoreWrapper;
use jujutsu_lib::transaction::Transaction;
use jujutsu_lib::tree::{Diff, DiffSummary};
use jujutsu_lib::working_copy::{CheckoutStats, WorkingCopy};
use jujutsu_lib::{conflicts, files, git, revset};
use pest::Parser;

use self::chrono::{FixedOffset, TimeZone, Utc};
use crate::commands::CommandError::UserError;
use crate::diff_edit::DiffEditError;
use crate::formatter::Formatter;
use crate::graphlog::{AsciiGraphDrawer, Edge};
use crate::template_parser::TemplateParser;
use crate::templater::Template;
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

impl From<StoreError> for CommandError {
    fn from(err: StoreError) -> Self {
        CommandError::UserError(format!("Unexpected error from store: {}", err))
    }
}

impl From<RepoInitError> for CommandError {
    fn from(_: RepoInitError) -> Self {
        CommandError::UserError("The target repo already exists".to_string())
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

fn get_repo(ui: &Ui, matches: &ArgMatches) -> Result<Arc<ReadonlyRepo>, CommandError> {
    let wc_path_str = matches.value_of("repository").unwrap();
    let wc_path = ui.cwd().join(wc_path_str);
    let loader = match RepoLoader::init(ui.settings(), wc_path) {
        Ok(loader) => loader,
        Err(RepoLoadError::NoRepoHere(wc_path)) => {
            let mut message = format!("There is no jj repo in \"{}\"", wc_path_str);
            let git_dir = wc_path.join(".git");
            if git_dir.is_dir() {
                // TODO: Make this hint separate from the error, so the caller can format
                // it differently.
                let git_dir_str = PathBuf::from(wc_path_str)
                    .join(".git")
                    .to_str()
                    .unwrap()
                    .to_owned();
                message += &format!("\nIt looks like this is a git repo. You can create a jj repo backed by it by running this:\n\
                                     jj init --git-store={} <path to new jj repo>", git_dir_str);
            }
            return Err(CommandError::UserError(message));
        }
    };
    let op_str = matches.value_of("at_op").unwrap();
    if op_str == "@" {
        Ok(loader.load_at_head())
    } else {
        let op = resolve_single_op_from_store(loader.op_store(), loader.op_heads_store(), op_str)?;
        Ok(loader.load_at(&op))
    }
}

struct CommandHelper<'args> {
    string_args: Vec<String>,
    root_matches: ArgMatches<'args>,
}

impl<'args> CommandHelper<'args> {
    fn new(string_args: Vec<String>, root_matches: ArgMatches<'args>) -> Self {
        Self {
            string_args,
            root_matches,
        }
    }

    fn root_matches(&self) -> &ArgMatches {
        &self.root_matches
    }

    fn repo_helper(&self, ui: &Ui) -> Result<RepoCommandHelper, CommandError> {
        RepoCommandHelper::new(ui, self.string_args.clone(), &self.root_matches)
    }
}

// Provides utilities for writing a command that works on a repo (like most
// commands do).
struct RepoCommandHelper {
    string_args: Vec<String>,
    settings: UserSettings,
    repo: Arc<ReadonlyRepo>,
    may_update_working_copy: bool,
    working_copy_committed: bool,
    // Whether to evolve orphans when the transaction
    // finishes. This should generally be true for commands that rewrite commits.
    evolve_orphans: bool,
    // Whether the checkout should be updated to an appropriate successor when the transaction
    // finishes. This should generally be true for commands that rewrite commits.
    auto_update_checkout: bool,
    // Whether branches should be updated to appropriate successors when the transaction
    // finishes. This should generally be true for commands that rewrite commits.
    auto_update_branches: bool,
}

impl RepoCommandHelper {
    fn new(
        ui: &Ui,
        string_args: Vec<String>,
        root_matches: &ArgMatches,
    ) -> Result<Self, CommandError> {
        let repo = get_repo(ui, root_matches)?;
        let may_update_working_copy = root_matches.value_of("at_op").unwrap() == "@";
        Ok(RepoCommandHelper {
            string_args,
            settings: ui.settings().clone(),
            repo,
            may_update_working_copy,
            working_copy_committed: false,
            evolve_orphans: true,
            auto_update_checkout: true,
            auto_update_branches: true,
        })
    }

    fn evolve_orphans(mut self, value: bool) -> Self {
        self.evolve_orphans = value;
        self
    }

    fn auto_update_checkout(mut self, value: bool) -> Self {
        self.auto_update_checkout = value;
        self
    }

    fn auto_update_branches(mut self, value: bool) -> Self {
        self.auto_update_branches = value;
        self
    }

    fn repo(&self) -> &Arc<ReadonlyRepo> {
        &self.repo
    }

    fn repo_mut(&mut self) -> &mut Arc<ReadonlyRepo> {
        &mut self.repo
    }

    fn resolve_revision_arg(
        &mut self,
        ui: &mut Ui,
        command_matches: &ArgMatches,
    ) -> Result<Commit, CommandError> {
        self.resolve_single_rev(ui, command_matches.value_of("revision").unwrap())
    }

    fn resolve_single_rev(
        &mut self,
        ui: &mut Ui,
        revision_str: &str,
    ) -> Result<Commit, CommandError> {
        let revset_expression = self.parse_revset(ui, revision_str)?;
        let revset = revset_expression.evaluate(self.repo.as_repo_ref())?;
        let mut iter = revset.iter();
        match iter.next() {
            None => Err(CommandError::UserError(format!(
                "Revset \"{}\" didn't resolve to any revisions",
                revision_str
            ))),
            Some(entry) => {
                let commit = self.repo.store().get_commit(&entry.commit_id())?;
                if iter.next().is_some() {
                    return Err(CommandError::UserError(format!(
                        "Revset \"{}\" resolved to more than one revision",
                        revision_str
                    )));
                } else {
                    Ok(commit)
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
        let revset = revset_expression.evaluate(self.repo.as_repo_ref())?;
        Ok(revset
            .iter()
            .map(|entry| self.repo.store().get_commit(&entry.commit_id()).unwrap())
            .collect())
    }

    fn parse_revset(
        &mut self,
        ui: &mut Ui,
        revision_str: &str,
    ) -> Result<RevsetExpression, CommandError> {
        let expression = revset::parse(revision_str)?;
        // If the revset is exactly "@", then we need to commit the working copy. If
        // it's another symbol, then we don't. If it's more complex, then we do
        // (just to be safe). TODO: Maybe make this smarter. How do we generally
        // figure out if a revset needs to commit the working copy? For example,
        // ":@" should perhaps not result in a new working copy commit, but
        // "::@" should. "foo::" is probably also should, since we would
        // otherwise need to evaluate the revset and see if "foo::" includes the
        // parent of the current checkout. Other interesting cases include some kind of
        // reference pointing to the working copy commit. If it's a
        // type of reference that would get updated when the commit gets rewritten, then
        // we probably should create a new working copy commit.
        let mentions_checkout = match &expression {
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
        if self.may_update_working_copy {
            let repo = self.repo.clone();
            let wc = repo.working_copy_locked();
            let locked_wc = wc.write_tree();
            let old_commit = locked_wc.old_commit();
            // Check if the current checkout has changed on disk after we read it. It's fine
            // if it has, but we'll need to reload the repo so the new commit is
            // in the index and view.
            let old_checkout = repo.view().checkout();
            if old_checkout != old_commit.id() {
                // TODO: This is not enough. The new commit is not necessarily still in the
                // view when we reload.
                self.repo = repo.reload();
            }
            let new_tree_id = locked_wc.new_tree_id();
            if new_tree_id != *old_commit.tree().id() {
                let mut tx = self.repo.start_transaction("commit working copy");
                let mut_repo = tx.mut_repo();
                let commit =
                    CommitBuilder::for_rewrite_from(&self.settings, self.repo.store(), &old_commit)
                        .set_tree(new_tree_id)
                        .write_to_repo(mut_repo);
                mut_repo.set_checkout(commit.id().clone());

                // Update branches pointing to the old checkout
                let mut branches_to_update = HashSet::new();
                for (branch_name, branch_target) in mut_repo.view().branches() {
                    match &branch_target.local_target {
                        None => {
                            // nothing to do (a deleted branch doesn't need
                            // updating)
                        }
                        Some(RefTarget::Normal(current_target)) => {
                            if current_target == old_checkout {
                                branches_to_update.insert(branch_name.clone());
                            }
                        }
                        Some(RefTarget::Conflict { adds, .. }) => {
                            for current_target in adds {
                                if current_target == old_checkout {
                                    writeln!(ui, "Branch {}'s target was rewritten, but not updating it since it's conflicted", branch_name)?;
                                }
                            }
                        }
                    }
                }
                for branch_name in branches_to_update {
                    mut_repo.set_local_branch(branch_name, RefTarget::Normal(commit.id().clone()));
                }

                // Evolve descendants (though it currently evolves all commits)
                let evolve_result = evolve_orphans(&self.settings, mut_repo)?;
                if evolve_result.num_resolved > 0 {
                    writeln!(
                        ui,
                        "Rebased {} descendant commits onto updated working copy",
                        evolve_result.num_resolved
                    )?;
                }
                if evolve_result.num_failed > 0 {
                    writeln!(
                        ui,
                        "Failed to rebase {} descendant commits onto updated working copy (run `jj evolve`)",
                        evolve_result.num_failed
                    )?;
                }

                self.repo = tx.commit();
                locked_wc.finish(commit);
            } else {
                locked_wc.discard();
            }
            self.working_copy_committed = true;
        }
        Ok(())
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
                format!("'{}'", arg.replace("'", "\\'"))
            }
        };
        let quoted_strings = self.string_args.iter().map(shell_escape).collect_vec();
        tx.set_tag("args".to_string(), quoted_strings.join(" "));
        tx
    }

    fn finish_transaction(
        &mut self,
        ui: &mut Ui,
        mut tx: Transaction,
    ) -> Result<Option<CheckoutStats>, CommandError> {
        let mut_repo = tx.mut_repo();
        if self.evolve_orphans {
            let evolve_result = evolve_orphans(ui.settings(), mut_repo)?;
            if evolve_result.num_resolved > 0 {
                writeln!(
                    ui,
                    "Rebased {} descendant commits",
                    evolve_result.num_resolved
                )?;
            }
            if evolve_result.num_failed > 0 {
                writeln!(
                    ui,
                    "Failed to rebase {} descendant commits (run `jj evolve`)",
                    evolve_result.num_failed
                )?;
            }
        }
        if self.auto_update_checkout {
            update_checkout_after_rewrite(ui, mut_repo)?;
        }
        if self.auto_update_branches {
            update_branches_after_rewrite(ui, mut_repo)?;
        }
        self.repo = tx.commit();
        update_working_copy(ui, &self.repo, &self.repo.working_copy_locked())
    }
}

fn rev_arg<'a, 'b>() -> Arg<'a, 'b> {
    Arg::with_name("revision")
        .long("revision")
        .short("r")
        .takes_value(true)
        .default_value("@")
}

fn paths_arg<'a, 'b>() -> Arg<'a, 'b> {
    Arg::with_name("paths").index(1).multiple(true)
}

fn message_arg<'a, 'b>() -> Arg<'a, 'b> {
    Arg::with_name("message")
        .long("message")
        .short("m")
        .takes_value(true)
}

fn op_arg<'a, 'b>() -> Arg<'a, 'b> {
    Arg::with_name("operation")
        .long("operation")
        .alias("op")
        .short("o")
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
        let op_id = OperationId(binary_op_id);
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
            "No operation id matching \"{}\"",
            op_str
        )))
    } else if matches.len() == 1 {
        Ok(matches.pop().unwrap())
    } else {
        Err(CommandError::UserError(format!(
            "Operation id prefix \"{}\" is ambiguous",
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
        // TODO: Add support for matching directories (and probably globs and other
        // formats)
        let mut paths = HashSet::new();
        for value in values {
            paths.insert(ui.parse_file_path(wc_path, value)?);
        }
        Ok(Box::new(FilesMatcher::new(paths)))
    } else {
        Ok(Box::new(EverythingMatcher))
    }
}

fn update_working_copy(
    ui: &mut Ui,
    repo: &Arc<ReadonlyRepo>,
    wc: &WorkingCopy,
) -> Result<Option<CheckoutStats>, CommandError> {
    let old_commit = wc.current_commit();
    let new_commit = repo.store().get_commit(repo.view().checkout()).unwrap();
    if old_commit == new_commit {
        return Ok(None);
    }
    // TODO: CheckoutError::ConcurrentCheckout should probably just result in a
    // warning for most commands (but be an error for the checkout command)
    let stats = wc.check_out(new_commit.clone()).map_err(|err| {
        CommandError::InternalError(format!(
            "Failed to check out commit {}: {}",
            new_commit.id().hex(),
            err
        ))
    })?;
    ui.write("Working copy now at: ")?;
    ui.write_commit_summary(repo.as_repo_ref(), &new_commit)?;
    ui.write("\n")?;
    Ok(Some(stats))
}

struct OrphanEvolutionResult {
    num_resolved: i32,
    num_failed: i32,
}

fn evolve_orphans(
    settings: &UserSettings,
    mut_repo: &mut MutableRepo,
) -> Result<OrphanEvolutionResult, CommandError> {
    let mut orphan_resolver = OrphanResolver::new(settings, mut_repo);
    let mut num_resolved = 0;
    let mut num_failed = 0;
    while let Some(resolution) = orphan_resolver.resolve_next(mut_repo) {
        match resolution {
            OrphanResolution::Resolved { .. } => {
                num_resolved += 1;
            }
            _ => {
                num_failed += 1;
            }
        }
    }
    Ok(OrphanEvolutionResult {
        num_resolved,
        num_failed,
    })
}

fn update_checkout_after_rewrite(ui: &mut Ui, mut_repo: &mut MutableRepo) -> io::Result<()> {
    // TODO: Perhaps this method should be in MutableRepo.
    let new_checkout_candidates = mut_repo
        .evolution()
        .new_parent(mut_repo.as_repo_ref(), mut_repo.view().checkout());
    if new_checkout_candidates.is_empty() {
        return Ok(());
    }
    // Filter out heads that already existed.
    // TODO: Filter out *commits* that already existed (so we get updated to an
    // appropriate new non-head)
    let old_heads = mut_repo.base_repo().view().heads().clone();
    let new_checkout_candidates: HashSet<_> = new_checkout_candidates
        .difference(&old_heads)
        .cloned()
        .collect();
    if new_checkout_candidates.is_empty() {
        return Ok(());
    }
    if new_checkout_candidates.len() > 1 {
        ui.write(
            "There are several candidates for updating the checkout to -- picking arbitrarily\n",
        )?;
    }
    let new_checkout = new_checkout_candidates.iter().min().unwrap();
    let new_commit = mut_repo.store().get_commit(new_checkout).unwrap();
    mut_repo.check_out(ui.settings(), &new_commit);
    Ok(())
}

fn update_branches_after_rewrite(ui: &mut Ui, mut_repo: &mut MutableRepo) -> io::Result<()> {
    // TODO: Perhaps this method should be in MutableRepo.
    let new_evolution = mut_repo.evolution();
    let base_repo = mut_repo.base_repo();
    let old_evolution = base_repo.evolution();
    let mut updates = HashMap::new();
    for (branch_name, branch_target) in mut_repo.view().branches() {
        match &branch_target.local_target {
            None => {
                // nothing to do (a deleted branch doesn't need updating)
            }
            Some(RefTarget::Normal(current_target)) => {
                if new_evolution.is_obsolete(current_target)
                    && !old_evolution.is_obsolete(current_target)
                {
                    let new_targets =
                        new_evolution.new_parent(mut_repo.as_repo_ref(), current_target);
                    if new_targets.len() == 1 {
                        updates.insert(
                            branch_name.clone(),
                            RefTarget::Normal(new_targets.iter().next().unwrap().clone()),
                        );
                    } else {
                        writeln!(
                            ui,
                            "Branch {}'s target was obsoleted, but the new target is unclear",
                            branch_name
                        )?;
                    }
                }
            }
            Some(RefTarget::Conflict { adds, .. }) => {
                for current_target in adds {
                    if new_evolution.is_obsolete(current_target)
                        && !old_evolution.is_obsolete(current_target)
                    {
                        writeln!(ui, "Branch {}'s target was obsoleted, but not updating it since it's conflicted", branch_name )?;
                    }
                }
            }
        }
    }
    for (branch_name, new_local_target) in updates {
        mut_repo.set_local_branch(branch_name, new_local_target);
    }

    Ok(())
}

fn get_app<'a, 'b>() -> App<'a, 'b> {
    let init_command = SubCommand::with_name("init")
        .about("Initialize a repo")
        .arg(Arg::with_name("destination").index(1).default_value("."))
        .arg(Arg::with_name("git").long("git"))
        .arg(
            Arg::with_name("git-store")
                .long("git-store")
                .takes_value(true)
                .help("path to a .git backing store"),
        );
    let checkout_command = SubCommand::with_name("checkout")
        .alias("co")
        .about("Update the working copy to another commit")
        .arg(Arg::with_name("revision").index(1).required(true));
    let files_command = SubCommand::with_name("files")
        .about("List files")
        .arg(rev_arg());
    let diff_command = SubCommand::with_name("diff")
        .about("Show modified files")
        .arg(
            Arg::with_name("summary")
                .long("summary")
                .short("s")
                .help("show only the diff type (modified/added/removed)"),
        )
        .arg(
            Arg::with_name("revision")
                .long("revision")
                .short("r")
                .takes_value(true),
        )
        .arg(Arg::with_name("from").long("from").takes_value(true))
        .arg(Arg::with_name("to").long("to").takes_value(true))
        .arg(paths_arg());
    let status_command = SubCommand::with_name("status")
        .alias("st")
        .about("Show repo status");
    let log_command = SubCommand::with_name("log")
        .about("Show commit history")
        .arg(
            Arg::with_name("template")
                .long("template")
                .short("T")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("revisions")
                .long("revisions")
                .short("r")
                .takes_value(true)
                .default_value(",,non_obsolete_heads()"),
        )
        .arg(Arg::with_name("no-graph").long("no-graph"));
    let obslog_command = SubCommand::with_name("obslog")
        .about("Show how a commit has evolved")
        .arg(rev_arg())
        .arg(
            Arg::with_name("template")
                .long("template")
                .short("T")
                .takes_value(true),
        )
        .arg(Arg::with_name("no-graph").long("no-graph"));
    let describe_command = SubCommand::with_name("describe")
        .about("Edit the commit description")
        .arg(Arg::with_name("revision").index(1).default_value("@"))
        .arg(message_arg())
        .arg(Arg::with_name("stdin").long("stdin"));
    let close_command = SubCommand::with_name("close")
        .about("Mark a commit closed, making new work go into a new commit")
        .arg(Arg::with_name("revision").index(1).default_value("@"))
        .arg(message_arg());
    let open_command = SubCommand::with_name("open")
        .about("Mark a commit open, making new work be added to it")
        .arg(Arg::with_name("revision").index(1).default_value("@"));
    let duplicate_command = SubCommand::with_name("duplicate")
        .about("Create a copy of the commit with a new change id")
        .arg(Arg::with_name("revision").index(1).default_value("@"));
    let prune_command = SubCommand::with_name("prune")
        .about("Mark a commit pruned, making descendants rebase onto its parent")
        .arg(Arg::with_name("revision").index(1).default_value("@"));
    let new_command = SubCommand::with_name("new")
        .about("Create a new, empty commit")
        .arg(Arg::with_name("revision").index(1).default_value("@"));
    let squash_command = SubCommand::with_name("squash")
        .about("Move changes from a commit into its parent")
        .arg(rev_arg())
        .arg(Arg::with_name("interactive").long("interactive").short("i"));
    let unsquash_command = SubCommand::with_name("unsquash")
        .about("Move changes from a commit's parent into the commit")
        .arg(rev_arg())
        .arg(Arg::with_name("interactive").long("interactive").short("i"));
    let discard_command = SubCommand::with_name("discard")
        .about("Discard a commit (and its descendants)")
        .arg(Arg::with_name("revision").index(1).default_value("@"));
    let restore_command = SubCommand::with_name("restore")
        .about("Restore paths from another revision")
        .arg(
            Arg::with_name("from")
                .long("from")
                .takes_value(true)
                .default_value(":@"),
        )
        .arg(
            Arg::with_name("to")
                .long("to")
                .takes_value(true)
                .default_value("@"),
        )
        .arg(Arg::with_name("interactive").long("interactive").short("i"))
        .arg(paths_arg());
    let edit_command = SubCommand::with_name("edit")
        .about("Edit the content changes in a revision")
        .arg(rev_arg());
    let split_command = SubCommand::with_name("split")
        .about("Split a revision in two")
        .arg(rev_arg());
    let merge_command = SubCommand::with_name("merge")
        .about("Merge work from multiple branches")
        .arg(
            Arg::with_name("revisions")
                .index(1)
                .required(true)
                .multiple(true),
        )
        .arg(message_arg());
    let rebase_command = SubCommand::with_name("rebase")
        .about("Move a commit to a different parent")
        .arg(rev_arg())
        .arg(
            Arg::with_name("destination")
                .long("destination")
                .short("d")
                .takes_value(true)
                .required(true)
                .multiple(true),
        );
    let backout_command = SubCommand::with_name("backout")
        .about("Apply the reverse of a commit on top of another commit")
        .arg(rev_arg())
        .arg(
            Arg::with_name("destination")
                .long("destination")
                .short("d")
                .takes_value(true)
                .default_value("@")
                .multiple(true),
        );
    let branch_command = SubCommand::with_name("branch")
        .about("Update or delete branch")
        .arg(rev_arg())
        .arg(Arg::with_name("allow-backwards").long("allow-backwards"))
        .arg(Arg::with_name("delete").long("delete"))
        .arg(Arg::with_name("name").index(1).required(true));
    let branches_command = SubCommand::with_name("branches").about("List branches");
    let evolve_command =
        SubCommand::with_name("evolve").about("Resolve problems with the repo's meta-history");
    let operation_command = SubCommand::with_name("operation")
        .alias("op")
        .about("Commands for working with the operation log")
        .setting(clap::AppSettings::SubcommandRequiredElseHelp)
        .subcommand(SubCommand::with_name("log").about("Show the operation log"))
        .subcommand(
            SubCommand::with_name("undo")
                .about("Undo an operation")
                .arg(op_arg()),
        )
        .subcommand(
            SubCommand::with_name("restore")
                .about("restore to the state at an operation")
                .arg(op_arg()),
        );
    let git_command = SubCommand::with_name("git")
        .about("Commands for working with the underlying git repo")
        .setting(clap::AppSettings::SubcommandRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("fetch")
                .about("Fetch from a git remote")
                .arg(
                    Arg::with_name("remote")
                        .long("remote")
                        .takes_value(true)
                        .default_value("origin"),
                ),
        )
        .subcommand(
            SubCommand::with_name("clone")
                .about("Create a new repo backed by a clone of a git repo")
                .arg(Arg::with_name("source").index(1).required(true))
                .arg(Arg::with_name("destination").index(2).required(true)),
        )
        .subcommand(
            SubCommand::with_name("push")
                .about("Push a branch to a git remote")
                .arg(
                    Arg::with_name("branch")
                        .long("branch")
                        .takes_value(true)
                        .required(true),
                )
                .arg(
                    Arg::with_name("remote")
                        .long("remote")
                        .takes_value(true)
                        .default_value("origin"),
                ),
        )
        .subcommand(
            SubCommand::with_name("refresh")
                .about("Update repo with changes made in underlying git repo"),
        );
    let bench_command = SubCommand::with_name("bench")
        .about("Commands for benchmarking internal operations")
        .setting(clap::AppSettings::SubcommandRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("commonancestors")
                .about("Find the common ancestor(s) of a set of commits")
                .arg(Arg::with_name("revision1").index(1).required(true))
                .arg(Arg::with_name("revision2").index(2).required(true)),
        )
        .subcommand(
            SubCommand::with_name("isancestor")
                .about("Checks if the first commit is an ancestor of the second commit")
                .arg(Arg::with_name("ancestor").index(1).required(true))
                .arg(Arg::with_name("descendant").index(2).required(true)),
        )
        .subcommand(
            SubCommand::with_name("walkrevs")
                .about("Walk revisions that are ancestors of the second argument but not ancestors of the first")
                .arg(Arg::with_name("unwanted").index(1).required(true))
                .arg(Arg::with_name("wanted").index(2).required(true)),
        )
        .subcommand(
            SubCommand::with_name("resolveprefix")
                .about("Resolve a commit id prefix")
                .arg(Arg::with_name("prefix").index(1).required(true)),
        );
    let debug_command = SubCommand::with_name("debug")
        .about("Low-level commands not intended for users")
        .setting(clap::AppSettings::SubcommandRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("resolverev")
                .about("Resolve a revision identifier to its full id")
                .arg(rev_arg()),
        )
        .subcommand(
            SubCommand::with_name("workingcopy")
                .about("Show information about the working copy state"),
        )
        .subcommand(
            SubCommand::with_name("writeworkingcopy")
                .about("Write a tree from the working copy state"),
        )
        .subcommand(
            SubCommand::with_name("template")
                .about("Parse a template")
                .arg(Arg::with_name("template").index(1).required(true)),
        )
        .subcommand(SubCommand::with_name("index").about("Show commit index stats"))
        .subcommand(SubCommand::with_name("reindex").about("Rebuild commit index"));
    App::new("Jujutsu")
        .global_setting(clap::AppSettings::ColoredHelp)
        .setting(clap::AppSettings::SubcommandRequiredElseHelp)
        .version(crate_version!())
        .author("Martin von Zweigbergk <martinvonz@google.com>")
        .about("An experimental VCS")
        .arg(
            Arg::with_name("repository")
                .long("repository")
                .short("R")
                .global(true)
                .takes_value(true)
                .default_value("."),
        )
        .arg(
            Arg::with_name("at_op")
                .long("at-operation")
                .alias("at-op")
                .global(true)
                .takes_value(true)
                .default_value("@"),
        )
        .subcommand(init_command)
        .subcommand(checkout_command)
        .subcommand(files_command)
        .subcommand(diff_command)
        .subcommand(status_command)
        .subcommand(log_command)
        .subcommand(obslog_command)
        .subcommand(describe_command)
        .subcommand(close_command)
        .subcommand(open_command)
        .subcommand(duplicate_command)
        .subcommand(prune_command)
        .subcommand(new_command)
        .subcommand(squash_command)
        .subcommand(unsquash_command)
        .subcommand(discard_command)
        .subcommand(restore_command)
        .subcommand(edit_command)
        .subcommand(split_command)
        .subcommand(merge_command)
        .subcommand(rebase_command)
        .subcommand(backout_command)
        .subcommand(branch_command)
        .subcommand(branches_command)
        .subcommand(evolve_command)
        .subcommand(operation_command)
        .subcommand(git_command)
        .subcommand(bench_command)
        .subcommand(debug_command)
}

fn short_commit_description(commit: &Commit) -> String {
    let first_line = commit.description().split('\n').next().unwrap();
    format!("{} ({})", &commit.id().hex()[0..12], first_line)
}

fn cmd_init(
    ui: &mut Ui,
    _command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    if sub_matches.is_present("git") && sub_matches.is_present("git-store") {
        return Err(CommandError::UserError(String::from(
            "--git cannot be used with --git-store",
        )));
    }
    let wc_path_str = sub_matches.value_of("destination").unwrap();
    let wc_path = ui.cwd().join(wc_path_str);
    if wc_path.exists() {
        assert!(wc_path.is_dir());
    } else {
        fs::create_dir(&wc_path).unwrap();
    }

    let repo = if let Some(git_store_str) = sub_matches.value_of("git-store") {
        let git_store_path = ui.cwd().join(git_store_str);
        let repo = ReadonlyRepo::init_external_git(ui.settings(), wc_path, git_store_path)?;
        let git_repo = repo.store().git_repo().unwrap();
        let mut tx = repo.start_transaction("import git refs");
        git::import_refs(tx.mut_repo(), &git_repo).unwrap();
        // TODO: Check out a recent commit. Maybe one with the highest generation
        // number.
        tx.commit()
    } else if sub_matches.is_present("git") {
        ReadonlyRepo::init_internal_git(ui.settings(), wc_path)?
    } else {
        ReadonlyRepo::init_local(ui.settings(), wc_path)?
    };
    writeln!(
        ui,
        "Initialized repo in \"{}\"",
        repo.working_copy_path().display()
    )?;
    Ok(())
}

fn cmd_checkout(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command
        .repo_helper(ui)?
        .auto_update_checkout(false)
        .auto_update_branches(false);
    let new_commit = repo_command.resolve_revision_arg(ui, sub_matches)?;
    repo_command.commit_working_copy(ui)?;
    let mut tx =
        repo_command.start_transaction(&format!("check out commit {}", new_commit.id().hex()));
    tx.mut_repo().check_out(ui.settings(), &new_commit);
    let stats = repo_command.finish_transaction(ui, tx)?;
    match stats {
        None => ui.write("Already on that commit\n")?,
        Some(stats) => writeln!(
            ui,
            "added {} files, modified {} files, removed {} files",
            stats.added_files, stats.updated_files, stats.removed_files
        )?,
    }
    Ok(())
}

fn cmd_files(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let commit = repo_command.resolve_revision_arg(ui, sub_matches)?;
    for (name, _value) in commit.tree().entries() {
        writeln!(
            ui,
            "{}",
            &ui.format_file_path(repo_command.repo().working_copy_path(), &name)
        )?;
    }
    Ok(())
}

fn print_diff(left: &[u8], right: &[u8], formatter: &mut dyn Formatter) -> io::Result<()> {
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
                        print_diff_line(formatter, line)?;
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
                print_diff_line(formatter, line)?;
            }
            context.clear();
            print_diff_line(formatter, &diff_line)?;
            context_before = false;
            skipped_context = false;
        }
    }
    if !context_before {
        for line in &context {
            print_diff_line(formatter, line)?;
        }
    }

    Ok(())
}

fn print_diff_line(formatter: &mut dyn Formatter, diff_line: &DiffLine) -> io::Result<()> {
    if diff_line.has_left_content {
        formatter.add_label(String::from("left"))?;
        formatter.write_bytes(format!("{:>4}", diff_line.left_line_number).as_bytes())?;
        formatter.remove_label()?;
        formatter.write_bytes(b" ")?;
    } else {
        formatter.write_bytes(b"     ")?;
    }
    if diff_line.has_right_content {
        formatter.add_label(String::from("right"))?;
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
                    formatter.add_label(String::from("left"))?;
                    formatter.write_bytes(before)?;
                    formatter.remove_label()?;
                }
                if !after.is_empty() {
                    formatter.add_label(String::from("right"))?;
                    formatter.write_bytes(after)?;
                    formatter.remove_label()?;
                }
            }
        }
    }

    Ok(())
}

fn cmd_diff(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    if sub_matches.is_present("revision")
        && (sub_matches.is_present("from") || sub_matches.is_present("to"))
    {
        return Err(CommandError::UserError(String::from(
            "--revision cannot be used with --from or --to",
        )));
    }
    let mut repo_command = command.repo_helper(ui)?;
    let from_tree;
    let to_tree;
    if sub_matches.is_present("from") || sub_matches.is_present("to") {
        let from =
            repo_command.resolve_single_rev(ui, sub_matches.value_of("from").unwrap_or("@"))?;
        from_tree = from.tree();
        let to = repo_command.resolve_single_rev(ui, sub_matches.value_of("to").unwrap_or("@"))?;
        to_tree = to.tree();
    } else {
        let commit =
            repo_command.resolve_single_rev(ui, sub_matches.value_of("revision").unwrap_or("@"))?;
        let parents = commit.parents();
        from_tree = merge_commit_trees(repo_command.repo().as_repo_ref(), &parents);
        to_tree = commit.tree()
    }
    let repo = repo_command.repo();
    let matcher =
        matcher_from_values(ui, repo.working_copy_path(), sub_matches.values_of("paths"))?;
    if sub_matches.is_present("summary") {
        let summary = from_tree.diff_summary(&to_tree, matcher.as_ref());
        show_diff_summary(ui, repo.working_copy_path(), &summary)?;
    } else {
        let mut formatter = ui.stdout_formatter();
        formatter.add_label(String::from("diff"))?;
        for (path, diff) in from_tree.diff(&to_tree, matcher.as_ref()) {
            let ui_path = ui.format_file_path(repo.working_copy_path(), &path);
            match diff {
                Diff::Added(TreeValue::Normal {
                    id,
                    executable: false,
                }) => {
                    formatter.add_label(String::from("header"))?;
                    formatter.write_str(&format!("added file {}:\n", ui_path))?;
                    formatter.remove_label()?;
                    let mut file_reader = repo.store().read_file(&path, &id).unwrap();
                    formatter.write_from_reader(&mut file_reader)?;
                }
                Diff::Modified(
                    TreeValue::Normal {
                        id: id_left,
                        executable: left_executable,
                    },
                    TreeValue::Normal {
                        id: id_right,
                        executable: right_executable,
                    },
                ) if left_executable == right_executable => {
                    formatter.add_label(String::from("header"))?;
                    if left_executable {
                        formatter.write_str(&format!("modified executable file {}:\n", ui_path))?;
                    } else {
                        formatter.write_str(&format!("modified file {}:\n", ui_path))?;
                    }
                    formatter.remove_label()?;

                    let mut file_reader_left = repo.store().read_file(&path, &id_left).unwrap();
                    let mut buffer_left = vec![];
                    file_reader_left.read_to_end(&mut buffer_left).unwrap();
                    let mut file_reader_right = repo.store().read_file(&path, &id_right).unwrap();
                    let mut buffer_right = vec![];
                    file_reader_right.read_to_end(&mut buffer_right).unwrap();

                    print_diff(
                        buffer_left.as_slice(),
                        buffer_right.as_slice(),
                        formatter.as_mut(),
                    )?;
                }
                Diff::Modified(
                    TreeValue::Conflict(id_left),
                    TreeValue::Normal {
                        id: id_right,
                        executable: false,
                    },
                ) => {
                    formatter.add_label(String::from("header"))?;
                    formatter.write_str(&format!("resolved conflict in file {}:\n", ui_path))?;
                    formatter.remove_label()?;

                    let conflict_left = repo.store().read_conflict(&id_left).unwrap();
                    let mut buffer_left = vec![];
                    conflicts::materialize_conflict(
                        repo.store(),
                        &path,
                        &conflict_left,
                        &mut buffer_left,
                    );
                    let mut file_reader_right = repo.store().read_file(&path, &id_right).unwrap();
                    let mut buffer_right = vec![];
                    file_reader_right.read_to_end(&mut buffer_right).unwrap();

                    print_diff(
                        buffer_left.as_slice(),
                        buffer_right.as_slice(),
                        formatter.as_mut(),
                    )?;
                }
                Diff::Modified(
                    TreeValue::Normal {
                        id: id_left,
                        executable: false,
                    },
                    TreeValue::Conflict(id_right),
                ) => {
                    formatter.add_label(String::from("header"))?;
                    formatter.write_str(&format!("new conflict in file {}:\n", ui_path))?;
                    formatter.remove_label()?;
                    let mut file_reader_left = repo.store().read_file(&path, &id_left).unwrap();
                    let mut buffer_left = vec![];
                    file_reader_left.read_to_end(&mut buffer_left).unwrap();
                    let conflict_right = repo.store().read_conflict(&id_right).unwrap();
                    let mut buffer_right = vec![];
                    conflicts::materialize_conflict(
                        repo.store(),
                        &path,
                        &conflict_right,
                        &mut buffer_right,
                    );

                    print_diff(
                        buffer_left.as_slice(),
                        buffer_right.as_slice(),
                        formatter.as_mut(),
                    )?;
                }
                Diff::Removed(TreeValue::Normal {
                    id,
                    executable: false,
                }) => {
                    formatter.add_label(String::from("header"))?;
                    formatter.write_str(&format!("removed file {}:\n", ui_path))?;
                    formatter.remove_label()?;

                    let mut file_reader = repo.store().read_file(&path, &id).unwrap();
                    formatter.write_from_reader(&mut file_reader)?;
                }
                other => {
                    writeln!(
                        formatter,
                        "unhandled diff case in path {:?}: {:?}",
                        path, other
                    )?;
                }
            }
        }
        formatter.remove_label()?;
    }
    Ok(())
}

fn show_diff_summary(ui: &mut Ui, wc_path: &Path, summary: &DiffSummary) -> io::Result<()> {
    for file in &summary.modified {
        writeln!(ui, "M {}", ui.format_file_path(wc_path, file))?;
    }
    for file in &summary.added {
        writeln!(ui, "A {}", ui.format_file_path(wc_path, file))?;
    }
    for file in &summary.removed {
        writeln!(ui, "R {}", ui.format_file_path(wc_path, file))?;
    }
    Ok(())
}

fn cmd_status(
    ui: &mut Ui,
    command: &CommandHelper,
    _sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    repo_command.maybe_commit_working_copy(ui)?;
    let repo = repo_command.repo();
    let commit = repo.store().get_commit(repo.view().checkout()).unwrap();
    ui.write("Parent commit: ")?;
    ui.write_commit_summary(repo.as_repo_ref(), &commit.parents()[0])?;
    ui.write("\n")?;
    ui.write("Working copy : ")?;
    ui.write_commit_summary(repo.as_repo_ref(), &commit)?;
    ui.write("\n")?;
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
        writeln!(ui, "  Use `jj branches` to see details. Use `jj branch <name> -r <rev>` to resolve.")?;
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
        writeln!(ui, "  Use `jj branches` to see details. Use `jj git pull` to resolve.")?;
    }
    let summary = commit.parents()[0]
        .tree()
        .diff_summary(&commit.tree(), &EverythingMatcher);
    if summary.is_empty() {
        ui.write("The working copy is clean\n")?;
    } else {
        ui.write("Working copy changes:\n")?;
        show_diff_summary(ui, repo.working_copy_path(), &summary)?;
    }
    Ok(())
}

fn log_template(settings: &UserSettings) -> String {
    let default_template = r#"
            label(if(open, "open"),
            "commit: " commit_id "\n"
            "change: " change_id "\n"
            "author: " author.name() " <" author.email() "> " author.timestamp() "\n"
            "committer: " committer.name() " <" committer.email() "> "  committer.timestamp() "\n"
            "branches: " branches "\n"
            "tags: " tags "\n"
            "open: " open "\n"
            "pruned: " pruned "\n"
            "obsolete: " obsolete "\n"
            "orphan: " orphan "\n"
            "divergent: " divergent "\n"
            "has conflict: " conflict "\n"
            description "\n"
            )"#;
    settings
        .config()
        .get_str("template.log")
        .unwrap_or_else(|_| String::from(default_template))
}

fn graph_log_template(settings: &UserSettings) -> String {
    // TODO: define a method on boolean values, so we can get auto-coloring
    //       with e.g. `obsolete.then("obsolete")`
    let default_template = r#"
            label(if(open, "open"),
            commit_id.short()
            " " change_id.short()
            " " author.email()
            " " label("timestamp", author.timestamp())
            " " branches
            " " tags
            if(pruned, label("pruned", " pruned"))
            if(obsolete, label("obsolete", " obsolete"))
            if(orphan, label("orphan", " orphan"))
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

fn cmd_log(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;

    let revset_expression =
        repo_command.parse_revset(ui, sub_matches.value_of("revisions").unwrap())?;
    let repo = repo_command.repo();
    let checkout_id = repo.view().checkout().clone();
    let revset = revset_expression.evaluate(repo.as_repo_ref())?;
    let store = repo.store();

    let use_graph = !sub_matches.is_present("no-graph");
    let template_string = match sub_matches.value_of("template") {
        Some(value) => value.to_string(),
        None => {
            if use_graph {
                graph_log_template(ui.settings())
            } else {
                log_template(ui.settings())
            }
        }
    };
    let template =
        crate::template_parser::parse_commit_template(repo.as_repo_ref(), &template_string);

    let mut formatter = ui.stdout_formatter();
    let mut formatter = formatter.as_mut();
    formatter.add_label(String::from("log"))?;

    if use_graph {
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
            {
                let writer = Box::new(&mut buffer);
                let mut formatter = ui.new_formatter(writer);
                let commit = store.get_commit(&index_entry.commit_id()).unwrap();
                template.format(&commit, formatter.as_mut())?;
            }
            if !buffer.ends_with(b"\n") {
                buffer.push(b'\n');
            }
            let node_symbol = if index_entry.commit_id() == checkout_id {
                b"@"
            } else {
                b"o"
            };
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

fn cmd_obslog(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;

    let use_graph = !sub_matches.is_present("no-graph");
    let start_commit = repo_command.resolve_revision_arg(ui, sub_matches)?;
    let checkout_id = repo_command.repo().view().checkout().clone();

    let template_string = match sub_matches.value_of("template") {
        Some(value) => value.to_string(),
        None => {
            if use_graph {
                graph_log_template(ui.settings())
            } else {
                log_template(ui.settings())
            }
        }
    };
    let template = crate::template_parser::parse_commit_template(
        repo_command.repo().as_repo_ref(),
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
    if use_graph {
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
            let node_symbol = if commit.id() == &checkout_id {
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
    let exit_status = Command::new(args[0])
        .args(editor_args)
        .arg(&description_file_path)
        .status()
        .expect("failed to run editor");
    if !exit_status.success() {
        panic!("failed to run editor");
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
    lines.join("")
}

fn cmd_describe(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let commit = repo_command.resolve_revision_arg(ui, sub_matches)?;
    repo_command.check_rewriteable(&commit)?;
    let repo = repo_command.repo();
    let description;
    if sub_matches.is_present("stdin") {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer).unwrap();
        description = buffer;
    } else if sub_matches.is_present("message") {
        description = sub_matches.value_of("message").unwrap().to_owned()
    } else {
        description = edit_description(repo, commit.description());
    }
    if description == *commit.description() {
        ui.write("Nothing changed.\n")?;
    } else {
        let mut tx =
            repo_command.start_transaction(&format!("describe commit {}", commit.id().hex()));
        CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
            .set_description(description)
            .write_to_repo(tx.mut_repo());
        repo_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_open(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let commit = repo_command.resolve_revision_arg(ui, sub_matches)?;
    repo_command.check_rewriteable(&commit)?;
    let repo = repo_command.repo();
    let mut tx = repo_command.start_transaction(&format!("open commit {}", commit.id().hex()));
    CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
        .set_open(true)
        .write_to_repo(tx.mut_repo());
    repo_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_close(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let commit = repo_command.resolve_revision_arg(ui, sub_matches)?;
    repo_command.check_rewriteable(&commit)?;
    let repo = repo_command.repo();
    let mut commit_builder =
        CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit).set_open(false);
    let description;
    if sub_matches.is_present("message") {
        description = sub_matches.value_of("message").unwrap().to_string();
    } else if commit.description().is_empty() {
        description = edit_description(repo, "\n\nJJ: Enter commit description.\n");
    } else {
        description = commit.description().to_string();
    }
    commit_builder = commit_builder.set_description(description);
    let mut tx = repo_command.start_transaction(&format!("close commit {}", commit.id().hex()));
    commit_builder.write_to_repo(tx.mut_repo());
    repo_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_duplicate(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let predecessor = repo_command.resolve_revision_arg(ui, sub_matches)?;
    let repo = repo_command.repo();
    let mut tx =
        repo_command.start_transaction(&format!("duplicate commit {}", predecessor.id().hex()));
    let mut_repo = tx.mut_repo();
    let new_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &predecessor)
        .generate_new_change_id()
        .write_to_repo(mut_repo);
    ui.write("Created: ")?;
    ui.write_commit_summary(mut_repo.as_repo_ref(), &new_commit)?;
    ui.write("\n")?;
    tx.commit();
    Ok(())
}

fn cmd_prune(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let predecessors =
        repo_command.resolve_revset(ui, sub_matches.value_of("revision").unwrap())?;
    repo_command.check_non_empty(&predecessors)?;
    for predecessor in &predecessors {
        repo_command.check_rewriteable(predecessor)?;
    }
    let repo = repo_command.repo();
    let transaction_description = if predecessors.len() == 1 {
        format!("prune commit {}", predecessors[0].id().hex())
    } else {
        format!(
            "prune commit {} and {} more",
            predecessors[0].id().hex(),
            predecessors.len() - 1
        )
    };
    let mut tx = repo_command.start_transaction(&transaction_description);
    for predecessor in predecessors {
        CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &predecessor)
            .set_pruned(true)
            .write_to_repo(tx.mut_repo());
    }
    repo_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_new(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let parent = repo_command.resolve_revision_arg(ui, sub_matches)?;
    let repo = repo_command.repo();
    let commit_builder = CommitBuilder::for_open_commit(
        ui.settings(),
        repo.store(),
        parent.id().clone(),
        parent.tree().id().clone(),
    );
    let mut tx = repo_command.start_transaction("new empty commit");
    let mut_repo = tx.mut_repo();
    let new_commit = commit_builder.write_to_repo(mut_repo);
    if mut_repo.view().checkout() == parent.id() {
        mut_repo.check_out(ui.settings(), &new_commit);
    }
    repo_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_squash(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let commit = repo_command.resolve_revision_arg(ui, sub_matches)?;
    repo_command.check_rewriteable(&commit)?;
    let repo = repo_command.repo();
    let parents = commit.parents();
    if parents.len() != 1 {
        return Err(CommandError::UserError(String::from(
            "Cannot squash merge commits",
        )));
    }
    let parent = &parents[0];
    repo_command.check_rewriteable(parent)?;
    let mut tx = repo_command.start_transaction(&format!("squash commit {}", commit.id().hex()));
    let mut_repo = tx.mut_repo();
    let new_parent_tree_id;
    if sub_matches.is_present("interactive") {
        let instructions = format!(
            "You are moving changes from: {}\n\
            into its parent: {}\n\n\
            
            The left side of the diff shows the contents of the parent commit. The\n\
            right side initially shows the contents of the commit you're moving\n\
            changes from.\n\n\
            
            Adjust the right side until the diff shows the changes you want to move\n\
            to the destination. If you don't make any changes, then all the changes\n\
            from the source will be moved into the parent.\n",
            short_commit_description(&commit),
            short_commit_description(parent)
        );
        new_parent_tree_id =
            crate::diff_edit::edit_diff(ui, &parent.tree(), &commit.tree(), &instructions)?;
        if &new_parent_tree_id == parent.tree().id() {
            return Err(CommandError::UserError(String::from("No changes selected")));
        }
    } else {
        new_parent_tree_id = commit.tree().id().clone();
    }
    // Prune the child if the parent now has all the content from the child (always
    // the case in the non-interactive case).
    let prune_child = &new_parent_tree_id == commit.tree().id();
    let new_parent = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), parent)
        .set_tree(new_parent_tree_id)
        .set_predecessors(vec![parent.id().clone(), commit.id().clone()])
        .write_to_repo(mut_repo);
    // Commit the remainder on top of the new parent commit.
    CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
        .set_parents(vec![new_parent.id().clone()])
        .set_pruned(prune_child)
        .write_to_repo(mut_repo);
    repo_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_unsquash(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let commit = repo_command.resolve_revision_arg(ui, sub_matches)?;
    repo_command.check_rewriteable(&commit)?;
    let repo = repo_command.repo();
    let parents = commit.parents();
    if parents.len() != 1 {
        return Err(CommandError::UserError(String::from(
            "Cannot unsquash merge commits",
        )));
    }
    let parent = &parents[0];
    repo_command.check_rewriteable(parent)?;
    let mut tx = repo_command.start_transaction(&format!("unsquash commit {}", commit.id().hex()));
    let mut_repo = tx.mut_repo();
    let parent_base_tree = merge_commit_trees(repo.as_repo_ref(), &parent.parents());
    let new_parent_tree_id;
    if sub_matches.is_present("interactive") {
        let instructions = format!(
            "You are moving changes from: {}\n\
            into its child: {}\n\n\
    
            The diff initially shows the parent commit's changes.\n\n\
            
            Adjust the right side until it shows the contents you want to keep in\n\
            the parent commit. The changes you edited out will be moved into the\n\
            child commit. If you don't make any changes, then the operation will be\n\
            aborted.\n",
            short_commit_description(parent),
            short_commit_description(&commit)
        );
        new_parent_tree_id =
            crate::diff_edit::edit_diff(ui, &parent_base_tree, &parent.tree(), &instructions)?;
        if &new_parent_tree_id == parent_base_tree.id() {
            return Err(CommandError::UserError(String::from("No changes selected")));
        }
    } else {
        new_parent_tree_id = parent_base_tree.id().clone();
    }
    // Prune the parent if it is now empty (always the case in the non-interactive
    // case).
    let prune_parent = &new_parent_tree_id == parent_base_tree.id();
    let new_parent = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), parent)
        .set_tree(new_parent_tree_id)
        .set_predecessors(vec![parent.id().clone(), commit.id().clone()])
        .set_pruned(prune_parent)
        .write_to_repo(mut_repo);
    // Commit the new child on top of the new parent.
    CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
        .set_parents(vec![new_parent.id().clone()])
        .write_to_repo(mut_repo);
    repo_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_discard(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let commit = repo_command.resolve_revision_arg(ui, sub_matches)?;
    let mut tx = repo_command.start_transaction(&format!("discard commit {}", commit.id().hex()));
    let mut_repo = tx.mut_repo();
    mut_repo.remove_head(&commit);
    for parent in commit.parents() {
        mut_repo.add_head(&parent);
    }
    // TODO: also remove descendants
    tx.commit();
    // TODO: check out parent/ancestor if the current commit got hidden
    Ok(())
}

fn cmd_restore(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let from_commit = repo_command.resolve_single_rev(ui, sub_matches.value_of("from").unwrap())?;
    let to_commit = repo_command.resolve_single_rev(ui, sub_matches.value_of("to").unwrap())?;
    repo_command.check_rewriteable(&to_commit)?;
    let repo = repo_command.repo();
    let tree_id;
    if sub_matches.is_present("interactive") {
        if sub_matches.is_present("paths") {
            return Err(UserError(
                "restore with --interactive and path is not yet supported".to_string(),
            ));
        }
        let instructions = format!(
            "You are restoring state from: {}\n\
            into: {}\n\n\
    
            The left side of the diff shows the contents of the commit you're\n\
            restoring from. The right side initially shows the contents of the\n\
            commit you're restoring into.\n\n\
            
            Adjust the right side until it has the changes you wanted from the left\n\
            side. If you don't make any changes, then the operation will be aborted.\n",
            short_commit_description(&from_commit),
            short_commit_description(&to_commit)
        );
        tree_id =
            crate::diff_edit::edit_diff(ui, &from_commit.tree(), &to_commit.tree(), &instructions)?;
    } else if sub_matches.is_present("paths") {
        let matcher =
            matcher_from_values(ui, repo.working_copy_path(), sub_matches.values_of("paths"))?;
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
        let mut tx = repo_command
            .start_transaction(&format!("restore into commit {}", to_commit.id().hex()));
        let mut_repo = tx.mut_repo();
        let new_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &to_commit)
            .set_tree(tree_id)
            .write_to_repo(mut_repo);
        ui.write("Created ")?;
        ui.write_commit_summary(mut_repo.as_repo_ref(), &new_commit)?;
        ui.write("\n")?;
        repo_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_edit(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let commit = repo_command.resolve_revision_arg(ui, sub_matches)?;
    repo_command.check_rewriteable(&commit)?;
    let repo = repo_command.repo();
    let base_tree = merge_commit_trees(repo.as_repo_ref(), &commit.parents());
    let instructions = format!(
        "You are editing changes in: {}\n\n\
        
        The diff initially shows the commit's changes.\n\n\
        
        Adjust the right side until it shows the contents you want. If you\n\
        don't make any changes, then the operation will be aborted.\n",
        short_commit_description(&commit)
    );
    let tree_id = crate::diff_edit::edit_diff(ui, &base_tree, &commit.tree(), &instructions)?;
    if &tree_id == commit.tree().id() {
        ui.write("Nothing changed.\n")?;
    } else {
        let mut tx = repo_command.start_transaction(&format!("edit commit {}", commit.id().hex()));
        let mut_repo = tx.mut_repo();
        let new_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
            .set_tree(tree_id)
            .write_to_repo(mut_repo);
        ui.write("Created ")?;
        ui.write_commit_summary(mut_repo.as_repo_ref(), &new_commit)?;
        ui.write("\n")?;
        repo_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_split(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let commit = repo_command.resolve_revision_arg(ui, sub_matches)?;
    repo_command.check_rewriteable(&commit)?;
    let repo = repo_command.repo();
    let base_tree = merge_commit_trees(repo.as_repo_ref(), &commit.parents());
    let instructions = format!(
        "You are splitting a commit in two: {}\n\n\
        
        The diff initially shows the changes in the commit you're splitting.\n\n\
        
        Adjust the right side until it shows the contents you want for the first\n\
        commit. The remainder will be in the second commit. If you don't make\n\
        any changes, then the operation will be aborted.\n",
        short_commit_description(&commit)
    );
    let tree_id = crate::diff_edit::edit_diff(ui, &base_tree, &commit.tree(), &instructions)?;
    if &tree_id == commit.tree().id() {
        ui.write("Nothing changed.\n")?;
    } else {
        let mut tx = repo_command.start_transaction(&format!("split commit {}", commit.id().hex()));
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
        ui.write("First part: ")?;
        ui.write_commit_summary(mut_repo.as_repo_ref(), &first_commit)?;
        ui.write("\nSecond part: ")?;
        ui.write_commit_summary(mut_repo.as_repo_ref(), &second_commit)?;
        ui.write("\n")?;
        repo_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_merge(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let revision_args = sub_matches.values_of("revisions").unwrap();
    if revision_args.len() < 2 {
        return Err(CommandError::UserError(String::from(
            "Merge requires at least two revisions",
        )));
    }
    let mut commits = vec![];
    let mut parent_ids = vec![];
    for revision_arg in revision_args {
        let commit = repo_command.resolve_single_rev(ui, revision_arg)?;
        parent_ids.push(commit.id().clone());
        commits.push(commit);
    }
    let repo = repo_command.repo();
    let description;
    if sub_matches.is_present("message") {
        description = sub_matches.value_of("message").unwrap().to_string();
    } else {
        description = edit_description(
            repo,
            "\n\nJJ: Enter commit description for the merge commit.\n",
        );
    }
    let merged_tree = merge_commit_trees(repo.as_repo_ref(), &commits);
    let mut tx = repo_command.start_transaction("merge commits");
    CommitBuilder::for_new_commit(ui.settings(), repo.store(), merged_tree.id().clone())
        .set_parents(parent_ids)
        .set_description(description)
        .set_open(false)
        .write_to_repo(tx.mut_repo());
    repo_command.finish_transaction(ui, tx)?;

    Ok(())
}

fn cmd_rebase(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let commit_to_rebase = repo_command.resolve_revision_arg(ui, sub_matches)?;
    repo_command.check_rewriteable(&commit_to_rebase)?;
    let mut parents = vec![];
    for revision_str in sub_matches.values_of("destination").unwrap() {
        let destination = repo_command.resolve_single_rev(ui, revision_str)?;
        parents.push(destination);
    }
    let mut tx =
        repo_command.start_transaction(&format!("rebase commit {}", commit_to_rebase.id().hex()));
    rebase_commit(ui.settings(), tx.mut_repo(), &commit_to_rebase, &parents);
    repo_command.finish_transaction(ui, tx)?;

    Ok(())
}

fn cmd_backout(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let commit_to_back_out = repo_command.resolve_revision_arg(ui, sub_matches)?;
    let mut parents = vec![];
    for revision_str in sub_matches.values_of("destination").unwrap() {
        let destination = repo_command.resolve_single_rev(ui, revision_str)?;
        parents.push(destination);
    }
    let mut tx = repo_command.start_transaction(&format!(
        "back out commit {}",
        commit_to_back_out.id().hex()
    ));
    back_out_commit(ui.settings(), tx.mut_repo(), &commit_to_back_out, &parents);
    repo_command.finish_transaction(ui, tx)?;

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
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command
        .repo_helper(ui)?
        .auto_update_checkout(false)
        .auto_update_branches(false);
    let branch_name = sub_matches.value_of("name").unwrap();
    if sub_matches.is_present("delete") {
        let mut tx = repo_command.start_transaction(&format!("delete branch {}", branch_name));
        tx.mut_repo().remove_local_branch(branch_name);
        repo_command.finish_transaction(ui, tx)?;
    } else {
        let target_commit = repo_command.resolve_revision_arg(ui, sub_matches)?;
        if !sub_matches.is_present("allow-backwards")
            && !is_fast_forward(
                repo_command.repo().as_repo_ref(),
                branch_name,
                target_commit.id(),
            )
        {
            return Err(CommandError::UserError(
                "Use --allow-backwards to allow moving a branch backwards or sideways".to_string(),
            ));
        }
        let mut tx = repo_command.start_transaction(&format!(
            "point branch {} to commit {}",
            branch_name,
            target_commit.id().hex()
        ));
        tx.mut_repo().set_local_branch(
            branch_name.to_string(),
            RefTarget::Normal(target_commit.id().clone()),
        );
        repo_command.finish_transaction(ui, tx)?;
    }

    Ok(())
}

fn cmd_branches(
    ui: &mut Ui,
    command: &CommandHelper,
    _sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let repo_command = command.repo_helper(ui)?;
    let repo = repo_command.repo();

    let print_branch_target =
        |ui: &mut Ui, target: Option<&RefTarget>| -> Result<(), CommandError> {
            match target {
                Some(RefTarget::Normal(id)) => {
                    write!(ui, ": ")?;
                    let commit = repo.store().get_commit(id)?;
                    ui.write_commit_summary(repo.as_repo_ref(), &commit)?;
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
                        ui.write_commit_summary(repo.as_repo_ref(), &commit)?;
                        writeln!(ui)?;
                    }
                    for id in adds {
                        let commit = repo.store().get_commit(id)?;
                        write!(ui, "  + ")?;
                        ui.write_commit_summary(repo.as_repo_ref(), &commit)?;
                        writeln!(ui)?;
                    }
                }
                None => {
                    writeln!(ui, " (deleted)")?;
                }
            }
            Ok(())
        };

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
            ui.stdout_formatter().add_label("branch".to_string())?;
            write!(ui, "{}@{}", name, remote)?;
            ui.stdout_formatter().remove_label()?;
            print_branch_target(ui, Some(remote_target))?;
            // TODO: Display information about remote branches, but probably
            // only those that have different targets than the local
            // branch. Maybe indicate how much the remotes are
            // ahead/behind/ diverged.
        }
    }

    Ok(())
}

fn cmd_evolve<'s>(
    ui: &mut Ui<'s>,
    command: &CommandHelper,
    _sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?.evolve_orphans(false);

    // TODO: This clone is unnecessary. Maybe ui.write() etc should not require a
    // mutable borrow? But the mutable borrow might be useful for making sure we
    // have only one Ui instance we write to across threads?
    let user_settings = ui.settings().clone();
    let mut tx = repo_command.start_transaction("evolve");
    let mut_repo = tx.mut_repo();
    let mut divergence_resolver = DivergenceResolver::new(&user_settings, mut_repo);
    while let Some(resolution) = divergence_resolver.resolve_next(mut_repo) {
        match resolution {
            DivergenceResolution::Resolved {
                divergents,
                resolved,
            } => {
                ui.write("Resolving divergent commits:\n").unwrap();
                for source in divergents {
                    ui.write("  ")?;
                    ui.write_commit_summary(mut_repo.as_repo_ref(), &source)?;
                    ui.write("\n")?;
                }
                ui.write("Resolved as: ")?;
                ui.write_commit_summary(mut_repo.as_repo_ref(), &resolved)?;
                ui.write("\n")?;
            }
            DivergenceResolution::NoCommonPredecessor { commit1, commit2 } => {
                ui.write("Skipping divergent commits with no common predecessor:\n")?;
                ui.write("  ")?;
                ui.write_commit_summary(mut_repo.as_repo_ref(), &commit1)?;
                ui.write("\n")?;
                ui.write("  ")?;
                ui.write_commit_summary(mut_repo.as_repo_ref(), &commit2)?;
                ui.write("\n")?;
            }
        }
    }

    let mut orphan_resolver = OrphanResolver::new(&user_settings, mut_repo);
    while let Some(resolution) = orphan_resolver.resolve_next(mut_repo) {
        match resolution {
            OrphanResolution::Resolved { orphan, new_commit } => {
                ui.write("Resolving orphan: ")?;
                ui.write_commit_summary(mut_repo.as_repo_ref(), &orphan)?;
                ui.write("\n")?;
                ui.write("Resolved as: ")?;
                ui.write_commit_summary(mut_repo.as_repo_ref(), &new_commit)?;
                ui.write("\n")?;
            }
            OrphanResolution::AmbiguousTarget { orphan } => {
                ui.write("Skipping orphan with ambiguous new parents: ")?;
                ui.write_commit_summary(mut_repo.as_repo_ref(), &orphan)?;
                ui.write("\n")?;
            }
        }
    }

    repo_command.finish_transaction(ui, tx)?;

    Ok(())
}

fn cmd_debug(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    if let Some(resolve_matches) = sub_matches.subcommand_matches("resolverev") {
        let mut repo_command = command.repo_helper(ui)?;
        let commit = repo_command.resolve_revision_arg(ui, resolve_matches)?;
        writeln!(ui, "{}", commit.id().hex())?;
    } else if let Some(_wc_matches) = sub_matches.subcommand_matches("workingcopy") {
        let repo_command = command.repo_helper(ui)?;
        let wc = repo_command.repo().working_copy_locked();
        writeln!(ui, "Current commit: {:?}", wc.current_commit_id())?;
        writeln!(ui, "Current tree: {:?}", wc.current_tree_id())?;
        for (file, state) in wc.file_states().iter() {
            writeln!(
                ui,
                "{:?} {:13?} {:10?} {:?}",
                state.file_type, state.size, state.mtime.0, file
            )?;
        }
    } else if let Some(template_matches) = sub_matches.subcommand_matches("template") {
        let parse = TemplateParser::parse(
            crate::template_parser::Rule::template,
            template_matches.value_of("template").unwrap(),
        );
        writeln!(ui, "{:?}", parse)?;
    } else if let Some(_reindex_matches) = sub_matches.subcommand_matches("index") {
        let repo_command = command.repo_helper(ui)?;
        let stats = repo_command.repo().index().stats();
        writeln!(ui, "Number of commits: {}", stats.num_commits)?;
        writeln!(ui, "Number of merges: {}", stats.num_merges)?;
        writeln!(ui, "Max generation number: {}", stats.max_generation_number)?;
        writeln!(ui, "Number of heads: {}", stats.num_heads)?;
        writeln!(ui, "Number of pruned commits: {}", stats.num_pruned_commits)?;
        writeln!(ui, "Number of changes: {}", stats.num_changes)?;
        writeln!(ui, "Stats per level:")?;
        for (i, level) in stats.levels.iter().enumerate() {
            writeln!(ui, "  Level {}:", i)?;
            writeln!(ui, "    Number of commits: {}", level.num_commits)?;
            writeln!(ui, "    Name: {}", level.name.as_ref().unwrap())?;
        }
    } else if let Some(_reindex_matches) = sub_matches.subcommand_matches("reindex") {
        let mut repo_command = command.repo_helper(ui)?;
        let mut_repo = Arc::get_mut(repo_command.repo_mut()).unwrap();
        let index = mut_repo.reindex();
        writeln!(ui, "Finished indexing {:?} commits.", index.num_commits())?;
    } else {
        panic!("unhandled command: {:#?}", command.root_matches());
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
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    if let Some(command_matches) = sub_matches.subcommand_matches("commonancestors") {
        let mut repo_command = command.repo_helper(ui)?;
        let revision1_str = command_matches.value_of("revision1").unwrap();
        let commit1 = repo_command.resolve_single_rev(ui, revision1_str)?;
        let revision2_str = command_matches.value_of("revision2").unwrap();
        let commit2 = repo_command.resolve_single_rev(ui, revision2_str)?;
        let index = repo_command.repo().index();
        let routine = || index.common_ancestors(&[commit1.id().clone()], &[commit2.id().clone()]);
        run_bench(
            ui,
            &format!("commonancestors-{}-{}", revision1_str, revision2_str),
            routine,
        )?;
    } else if let Some(command_matches) = sub_matches.subcommand_matches("isancestor") {
        let mut repo_command = command.repo_helper(ui)?;
        let ancestor_str = command_matches.value_of("ancestor").unwrap();
        let ancestor_commit = repo_command.resolve_single_rev(ui, ancestor_str)?;
        let descendants_str = command_matches.value_of("descendant").unwrap();
        let descendant_commit = repo_command.resolve_single_rev(ui, descendants_str)?;
        let index = repo_command.repo().index();
        let routine = || index.is_ancestor(ancestor_commit.id(), descendant_commit.id());
        run_bench(
            ui,
            &format!("isancestor-{}-{}", ancestor_str, descendants_str),
            routine,
        )?;
    } else if let Some(command_matches) = sub_matches.subcommand_matches("walkrevs") {
        let mut repo_command = command.repo_helper(ui)?;
        let unwanted_str = command_matches.value_of("unwanted").unwrap();
        let unwanted_commit = repo_command.resolve_single_rev(ui, unwanted_str)?;
        let wanted_str = command_matches.value_of("wanted");
        let wanted_commit = repo_command.resolve_single_rev(ui, wanted_str.unwrap())?;
        let index = repo_command.repo().index();
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
    } else if let Some(command_matches) = sub_matches.subcommand_matches("resolveprefix") {
        let repo_command = command.repo_helper(ui)?;
        let prefix =
            HexPrefix::new(command_matches.value_of("prefix").unwrap().to_string()).unwrap();
        let index = repo_command.repo().index();
        let routine = || index.resolve_prefix(&prefix);
        run_bench(ui, &format!("resolveprefix-{}", prefix.hex()), routine)?;
    } else {
        panic!("unhandled command: {:#?}", command.root_matches());
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
    _op_matches: &ArgMatches,
    _cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let repo_command = command.repo_helper(ui)?;
    let repo = repo_command.repo();
    let head_op = repo.operation().clone();
    let head_op_id = head_op.id().clone();
    let mut formatter = ui.stdout_formatter();
    let mut formatter = formatter.as_mut();
    struct OpTemplate;
    impl Template<Operation> for OpTemplate {
        fn format(&self, op: &Operation, formatter: &mut dyn Formatter) -> io::Result<()> {
            // TODO: why can't this label be applied outside of the template?
            formatter.add_label("op-log".to_string())?;
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
                formatter.write_str(&format!("\n{}: {}", key, value))?;
            }
            formatter.remove_label()?;

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
        let mut buffer = vec![];
        {
            let writer = Box::new(&mut buffer);
            let mut formatter = ui.new_formatter(writer);
            template.format(&op, formatter.as_mut())?;
        }
        if !buffer.ends_with(b"\n") {
            buffer.push(b'\n');
        }
        let node_symbol = if op.id() == &head_op_id { b"@" } else { b"o" };
        graph.add_node(op.id(), &edges, node_symbol, &buffer)?;
    }

    Ok(())
}

fn cmd_op_undo(
    ui: &mut Ui,
    command: &CommandHelper,
    _op_matches: &ArgMatches,
    _cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let repo = repo_command.repo();
    let bad_op = resolve_single_op(repo, _cmd_matches.value_of("operation").unwrap())?;
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

    let mut tx = repo_command.start_transaction(&format!("undo operation {}", bad_op.id().hex()));
    let bad_repo = repo.loader().load_at(&bad_op);
    let parent_repo = repo.loader().load_at(&parent_ops[0]);
    tx.mut_repo().merge(&bad_repo, &parent_repo);
    repo_command.finish_transaction(ui, tx)?;

    Ok(())
}
fn cmd_op_restore(
    ui: &mut Ui,
    command: &CommandHelper,
    _op_matches: &ArgMatches,
    _cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo_command = command.repo_helper(ui)?;
    let repo = repo_command.repo();
    let target_op = resolve_single_op(repo, _cmd_matches.value_of("operation").unwrap())?;
    let mut tx =
        repo_command.start_transaction(&format!("restore to operation {}", target_op.id().hex()));
    tx.mut_repo().set_view(target_op.view().take_store_view());
    repo_command.finish_transaction(ui, tx)?;

    Ok(())
}

fn cmd_operation(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    if let Some(command_matches) = sub_matches.subcommand_matches("log") {
        cmd_op_log(ui, command, sub_matches, command_matches)?;
    } else if let Some(command_matches) = sub_matches.subcommand_matches("undo") {
        cmd_op_undo(ui, command, sub_matches, command_matches)?;
    } else if let Some(command_matches) = sub_matches.subcommand_matches("restore") {
        cmd_op_restore(ui, command, sub_matches, command_matches)?;
    } else {
        panic!("unhandled command: {:#?}", command.root_matches());
    }
    Ok(())
}

fn get_git_repo(store: &StoreWrapper) -> Result<git2::Repository, CommandError> {
    match store.git_repo() {
        None => Err(CommandError::UserError(
            "The repo is not backed by a git repo".to_string(),
        )),
        Some(git_repo) => Ok(git_repo),
    }
}

fn cmd_git_fetch(
    ui: &mut Ui,
    command: &CommandHelper,
    _git_matches: &ArgMatches,
    cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let repo_command = command.repo_helper(ui)?;
    let repo = repo_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    let remote_name = cmd_matches.value_of("remote").unwrap();
    let mut tx = repo_command.start_transaction(&format!("fetch from git remote {}", remote_name));
    git::fetch(tx.mut_repo(), &git_repo, remote_name)
        .map_err(|err| CommandError::UserError(err.to_string()))?;
    tx.commit();
    Ok(())
}

fn cmd_git_clone(
    ui: &mut Ui,
    _command: &CommandHelper,
    _git_matches: &ArgMatches,
    cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let source = cmd_matches.value_of("source").unwrap();
    let wc_path_str = cmd_matches.value_of("destination").unwrap();
    let wc_path = ui.cwd().join(wc_path_str);
    if wc_path.exists() {
        assert!(wc_path.is_dir());
    } else {
        fs::create_dir(&wc_path).unwrap();
    }

    let repo = ReadonlyRepo::init_internal_git(ui.settings(), wc_path)?;
    let git_repo = get_git_repo(repo.store())?;
    writeln!(
        ui,
        "Fetching into new repo in {:?}",
        repo.working_copy_path()
    )?;
    let remote_name = "origin";
    git_repo.remote(remote_name, source).unwrap();
    let mut tx = repo.start_transaction("fetch from git remote into empty repo");
    git::fetch(tx.mut_repo(), &git_repo, remote_name).map_err(|err| match err {
        GitFetchError::NoSuchRemote(_) => {
            panic!("should't happen as we just created the git remote")
        }
        GitFetchError::InternalGitError(err) => {
            CommandError::UserError(format!("Fetch failed: {:?}", err))
        }
    })?;
    tx.commit();
    Ok(())
}

fn cmd_git_push(
    ui: &mut Ui,
    command: &CommandHelper,
    _git_matches: &ArgMatches,
    cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let repo_command = command.repo_helper(ui)?;
    let repo = repo_command.repo();
    let branch_name = cmd_matches.value_of("branch").unwrap();
    let remote_name = cmd_matches.value_of("remote").unwrap();

    let maybe_branch_target = repo.view().get_branch(branch_name);
    if maybe_branch_target.is_none() {
        return Err(CommandError::UserError(format!(
            "Branch {} doesn't exist",
            branch_name
        )));
    }

    let branch_target = maybe_branch_target.unwrap();
    let maybe_remote_target = branch_target.remote_targets.get(remote_name);
    if branch_target.local_target.as_ref() == maybe_remote_target {
        writeln!(
            ui,
            "Branch {}@{} already matches {}",
            branch_name, remote_name, branch_name
        )?;
        return Ok(());
    }

    let git_repo = get_git_repo(repo.store())?;
    if let Some(new_target) = &branch_target.local_target {
        match new_target {
            RefTarget::Conflict { .. } => {
                return Err(CommandError::UserError(format!(
                    "Branch {} is conflicted",
                    branch_name
                )));
            }
            RefTarget::Normal(new_target_id) => {
                let new_target_commit = repo.store().get_commit(new_target_id)?;
                if new_target_commit.is_open() {
                    return Err(CommandError::UserError(
                        "Won't push open commit".to_string(),
                    ));
                }
                let force = match maybe_remote_target {
                    None => false,
                    Some(RefTarget::Conflict { .. }) => {
                        return Err(CommandError::UserError(format!(
                            "Branch {}@{} is conflicted",
                            branch_name, remote_name
                        )));
                    }
                    Some(RefTarget::Normal(old_target_id)) => {
                        !repo.index().is_ancestor(old_target_id, new_target_id)
                    }
                };

                git::push_commit(
                    &git_repo,
                    &new_target_commit,
                    remote_name,
                    branch_name,
                    force,
                )
                .map_err(|err| CommandError::UserError(err.to_string()))?;
            }
        }
    } else {
        git::delete_remote_branch(&git_repo, remote_name, branch_name)
            .map_err(|err| CommandError::UserError(err.to_string()))?;
    }
    let mut tx = repo_command.start_transaction("import git refs");
    git::import_refs(tx.mut_repo(), &git_repo)
        .map_err(|err| CommandError::UserError(err.to_string()))?;
    tx.commit();
    Ok(())
}

fn cmd_git_refresh(
    ui: &mut Ui,
    command: &CommandHelper,
    _git_matches: &ArgMatches,
    _cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let repo_command = command.repo_helper(ui)?;
    let repo = repo_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    let mut tx = repo_command.start_transaction("import git refs");
    git::import_refs(tx.mut_repo(), &git_repo)
        .map_err(|err| CommandError::UserError(err.to_string()))?;
    tx.commit();
    Ok(())
}

fn cmd_git(
    ui: &mut Ui,
    command: &CommandHelper,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    if let Some(command_matches) = sub_matches.subcommand_matches("fetch") {
        cmd_git_fetch(ui, command, sub_matches, command_matches)?;
    } else if let Some(command_matches) = sub_matches.subcommand_matches("clone") {
        cmd_git_clone(ui, command, sub_matches, command_matches)?;
    } else if let Some(command_matches) = sub_matches.subcommand_matches("push") {
        cmd_git_push(ui, command, sub_matches, command_matches)?;
    } else if let Some(command_matches) = sub_matches.subcommand_matches("refresh") {
        cmd_git_refresh(ui, command, sub_matches, command_matches)?;
    } else {
        panic!("unhandled command: {:#?}", command.root_matches());
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
    let matches = get_app().get_matches_from(&string_args);
    let command_helper = CommandHelper::new(string_args, matches.clone());
    let result = if let Some(sub_matches) = command_helper.root_matches.subcommand_matches("init") {
        cmd_init(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("checkout") {
        cmd_checkout(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("files") {
        cmd_files(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("diff") {
        cmd_diff(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("status") {
        cmd_status(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("log") {
        cmd_log(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("obslog") {
        cmd_obslog(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("describe") {
        cmd_describe(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("close") {
        cmd_close(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("open") {
        cmd_open(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("duplicate") {
        cmd_duplicate(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("prune") {
        cmd_prune(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("new") {
        cmd_new(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("squash") {
        cmd_squash(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("unsquash") {
        cmd_unsquash(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("discard") {
        cmd_discard(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("restore") {
        cmd_restore(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("edit") {
        cmd_edit(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("split") {
        cmd_split(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("merge") {
        cmd_merge(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("rebase") {
        cmd_rebase(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("backout") {
        cmd_backout(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("branch") {
        cmd_branch(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("branches") {
        cmd_branches(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("evolve") {
        cmd_evolve(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("operation") {
        cmd_operation(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("git") {
        cmd_git(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("bench") {
        cmd_bench(&mut ui, &command_helper, sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("debug") {
        cmd_debug(&mut ui, &command_helper, sub_matches)
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
