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

use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::Arc;

use clap::{crate_version, App, Arg, ArgMatches, SubCommand};

use criterion::Criterion;

use pest::Parser;

use jujube_lib::commit::Commit;
use jujube_lib::commit_builder::CommitBuilder;
use jujube_lib::conflicts;
use jujube_lib::dag_walk::{topo_order_reverse, walk_ancestors};
use jujube_lib::evolution::evolve;
use jujube_lib::evolution::EvolveListener;
use jujube_lib::files;
use jujube_lib::files::DiffLine;
use jujube_lib::git;
use jujube_lib::op_store::{OpStore, OpStoreError, OperationId};
use jujube_lib::repo::{ReadonlyRepo, RepoLoadError};
use jujube_lib::repo_path::RepoPath;
use jujube_lib::rewrite::{back_out_commit, merge_commit_trees, rebase_commit};
use jujube_lib::settings::UserSettings;
use jujube_lib::store::{CommitId, Timestamp};
use jujube_lib::store::{StoreError, TreeValue};
use jujube_lib::tree::Tree;
use jujube_lib::trees::TreeValueDiff;
use jujube_lib::working_copy::{CheckoutStats, WorkingCopy};

use self::chrono::{FixedOffset, TimeZone, Utc};
use crate::commands::CommandError::UserError;
use crate::diff_edit::DiffEditError;
use crate::graphlog::{AsciiGraphDrawer, Edge};
use crate::styler::{ColorStyler, Styler};
use crate::template_parser::TemplateParser;
use crate::templater::Template;
use crate::ui::Ui;
use jujube_lib::git::GitFetchError;
use jujube_lib::index::{HexPrefix, PrefixResolution};
use jujube_lib::operation::Operation;
use jujube_lib::store_wrapper::StoreWrapper;
use jujube_lib::transaction::Transaction;
use jujube_lib::view::merge_views;
use std::fmt::Debug;
use std::time::Instant;

enum CommandError {
    UserError(String),
    InternalError(String),
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

impl From<RepoLoadError> for CommandError {
    fn from(err: RepoLoadError) -> Self {
        CommandError::UserError(format!("Failed to load repo: {}", err))
    }
}

fn get_repo(ui: &Ui, matches: &ArgMatches) -> Result<Arc<ReadonlyRepo>, CommandError> {
    let wc_path_str = matches.value_of("repository").unwrap();
    let wc_path = ui.cwd().join(wc_path_str);
    let loader = ReadonlyRepo::loader(ui.settings(), wc_path)?;
    if let Some(op_str) = matches.value_of("at_op") {
        let op = resolve_single_op_from_store(loader.op_store(), op_str)?;
        Ok(loader.load_at(&op)?)
    } else {
        Ok(loader.load_at_head()?)
    }
}

fn resolve_commit_id_prefix(
    repo: &ReadonlyRepo,
    prefix: &HexPrefix,
) -> Result<CommitId, CommandError> {
    match repo.index().resolve_prefix(prefix) {
        PrefixResolution::NoMatch => Err(CommandError::UserError(String::from("No such commit"))),
        PrefixResolution::AmbiguousMatch => {
            Err(CommandError::UserError(String::from("Ambiguous prefix")))
        }
        PrefixResolution::SingleMatch(id) => Ok(id),
    }
}

fn resolve_revision_arg(
    ui: &Ui,
    repo: &mut ReadonlyRepo,
    matches: &ArgMatches,
) -> Result<Commit, CommandError> {
    resolve_single_rev(ui, repo, matches.value_of("revision").unwrap())
}

fn resolve_single_rev(
    ui: &Ui,
    repo: &mut ReadonlyRepo,
    revision_str: &str,
) -> Result<Commit, CommandError> {
    if revision_str == "@" {
        let owned_wc = repo.working_copy().clone();
        let wc = owned_wc.lock().unwrap();
        // TODO: Avoid committing every time this function is called.
        Ok(wc.commit(ui.settings(), repo))
    } else if revision_str == "@^" {
        let commit = repo.store().get_commit(repo.view().checkout()).unwrap();
        assert!(commit.is_open());
        let parents = commit.parents();
        Ok(parents[0].clone())
    } else if revision_str == "root" {
        Ok(repo.store().root_commit())
    } else if revision_str.starts_with("desc(") && revision_str.ends_with(')') {
        let needle = revision_str[5..revision_str.len() - 1].to_string();
        let mut matches = vec![];
        let heads: HashSet<Commit> = repo
            .view()
            .heads()
            .iter()
            .map(|commit_id| repo.store().get_commit(commit_id).unwrap())
            .collect();
        let heads = skip_uninteresting_heads(repo, heads);
        for commit in walk_ancestors(heads) {
            if commit.description().contains(&needle) {
                matches.push(commit);
            }
        }
        matches
            .pop()
            .ok_or_else(|| CommandError::UserError(String::from("No matching commit")))
    } else {
        if let Ok(binary_commit_id) = hex::decode(revision_str) {
            let commit_id = CommitId(binary_commit_id);
            match repo.store().get_commit(&commit_id) {
                Ok(commit) => return Ok(commit),
                Err(StoreError::NotFound) => {} // fall through
                Err(err) => {
                    return Err(CommandError::InternalError(format!(
                        "Failed to read commit: {}",
                        err
                    )))
                }
            }
        }
        let id = resolve_commit_id_prefix(repo, &HexPrefix::new(revision_str.to_owned()))?;
        Ok(repo.store().get_commit(&id).unwrap())
    }
}

fn rev_arg<'a, 'b>() -> Arg<'a, 'b> {
    Arg::with_name("revision")
        .long("revision")
        .short("r")
        .takes_value(true)
        .default_value("@")
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
    let view = repo.view();
    if op_str == "@" {
        Ok(view.as_view_ref().base_op())
    } else {
        resolve_single_op_from_store(&repo.op_store(), op_str)
    }
}

fn resolve_single_op_from_store(
    op_store: &Arc<dyn OpStore>,
    op_str: &str,
) -> Result<Operation, CommandError> {
    if let Ok(binary_op_id) = hex::decode(op_str) {
        let op_id = OperationId(binary_op_id);
        match op_store.read_operation(&op_id) {
            Ok(operation) => Ok(Operation::new(op_store.clone(), op_id, operation)),
            Err(OpStoreError::NotFound) => Err(CommandError::UserError(format!(
                "Operation id not found: {}",
                op_str
            ))),
            Err(err) => Err(CommandError::InternalError(format!(
                "Failed to read commit: {:?}",
                err
            ))),
        }
    } else {
        Err(CommandError::UserError(format!(
            "Invalid operation id: {}",
            op_str
        )))
    }
}

fn update_working_copy(
    ui: &mut Ui,
    repo: &mut ReadonlyRepo,
    wc: &WorkingCopy,
) -> Result<Option<CheckoutStats>, CommandError> {
    repo.reload();
    let old_commit = wc.current_commit();
    let new_commit = repo.store().get_commit(repo.view().checkout()).unwrap();
    if old_commit == new_commit {
        return Ok(None);
    }
    ui.write("leaving: ");
    ui.write_commit_summary(repo.as_repo_ref(), &old_commit);
    ui.write("\n");
    // TODO: CheckoutError::ConcurrentCheckout should probably just result in a
    // warning for most commands (but be an error for the checkout command)
    let stats = wc.check_out(new_commit.clone()).map_err(|err| {
        CommandError::InternalError(format!(
            "Failed to check out commit {}: {}",
            new_commit.id().hex(),
            err
        ))
    })?;
    ui.write("now at: ");
    ui.write_commit_summary(repo.as_repo_ref(), &new_commit);
    ui.write("\n");
    Ok(Some(stats))
}

fn update_checkout_after_rewrite(ui: &mut Ui, tx: &mut Transaction) {
    // TODO: Perhaps this method should be in Transaction.
    let new_checkout_candidates = tx.evolution().new_parent(tx.view().checkout());
    if new_checkout_candidates.is_empty() {
        return;
    }
    // Filter out heads that already existed.
    // TODO: Filter out *commits* that already existed (so we get updated to an
    // appropriate new non-head)
    let old_heads = tx.base_repo().view().heads().clone();
    let new_checkout_candidates: HashSet<_> = new_checkout_candidates
        .difference(&old_heads)
        .cloned()
        .collect();
    if new_checkout_candidates.is_empty() {
        return;
    }
    if new_checkout_candidates.len() > 1 {
        ui.write(
            "There are several candidates for updating the checkout to -- picking arbitrarily\n",
        );
    }
    let new_checkout = new_checkout_candidates.iter().min().unwrap();
    let new_commit = tx.store().get_commit(new_checkout).unwrap();
    tx.check_out(ui.settings(), &new_commit);
}

fn get_app<'a, 'b>() -> App<'a, 'b> {
    let init_command = SubCommand::with_name("init")
        .about("initialize a repo")
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
        .about("update the working copy to another commit")
        .arg(Arg::with_name("revision").index(1).required(true));
    let files_command = SubCommand::with_name("files")
        .about("list files")
        .arg(rev_arg());
    let diff_command = SubCommand::with_name("diff")
        .about("show modified files")
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
        .arg(Arg::with_name("to").long("to").takes_value(true));
    let status_command = SubCommand::with_name("status")
        .alias("st")
        .about("show repo status");
    let log_command = SubCommand::with_name("log")
        .about("show commit history")
        .arg(
            Arg::with_name("template")
                .long("template")
                .short("T")
                .takes_value(true),
        )
        .arg(Arg::with_name("all").long("all"))
        .arg(Arg::with_name("no-graph").long("no-graph"));
    let obslog_command = SubCommand::with_name("obslog")
        .about("show how a commit has evolved")
        .arg(rev_arg())
        .arg(
            Arg::with_name("template")
                .long("template")
                .short("T")
                .takes_value(true),
        )
        .arg(Arg::with_name("no-graph").long("no-graph"));
    let describe_command = SubCommand::with_name("describe")
        .about("edit the commit description")
        .arg(rev_arg())
        .arg(message_arg())
        .arg(Arg::with_name("stdin").long("stdin"));
    let close_command = SubCommand::with_name("close")
        .about("mark a commit closed, making new work go into a new commit")
        .arg(rev_arg())
        .arg(message_arg());
    let open_command = SubCommand::with_name("open")
        .about("mark a commit open, making new work be added to it")
        .arg(rev_arg());
    let duplicate_command = SubCommand::with_name("duplicate")
        .about("create a copy of the commit with a new change id")
        .arg(rev_arg());
    let prune_command = SubCommand::with_name("prune")
        .about("create an empty successor of a commit")
        .arg(rev_arg());
    let new_command = SubCommand::with_name("new")
        .about("create a new, empty commit")
        .arg(rev_arg());
    let squash_command = SubCommand::with_name("squash")
        .about("squash a commit into its parent")
        .arg(rev_arg());
    let discard_command = SubCommand::with_name("discard")
        .about("discard a commit (and its descendants)")
        .arg(rev_arg());
    let restore_command = SubCommand::with_name("restore")
        .about("restore paths from another revision")
        .arg(
            Arg::with_name("source")
                .long("source")
                .short("s")
                .takes_value(true)
                .default_value("@^"),
        )
        .arg(
            Arg::with_name("destination")
                .long("destination")
                .short("d")
                .takes_value(true)
                .default_value("@"),
        )
        .arg(Arg::with_name("interactive").long("interactive").short("i"))
        .arg(Arg::with_name("paths").index(1).multiple(true));
    let edit_command = SubCommand::with_name("edit")
        .about("edit the content changes in a revision")
        .arg(rev_arg());
    let split_command = SubCommand::with_name("split")
        .about("split a revision in two")
        .arg(rev_arg());
    let merge_command = SubCommand::with_name("merge")
        .about("merge work from multiple branches")
        .arg(
            Arg::with_name("revisions")
                .index(1)
                .required(true)
                .multiple(true),
        )
        .arg(message_arg());
    let rebase_command = SubCommand::with_name("rebase")
        .about("move a commit to a different parent")
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
        .about("apply the reverse of a commit on top of another commit")
        .arg(rev_arg())
        .arg(
            Arg::with_name("destination")
                .long("destination")
                .short("d")
                .takes_value(true)
                .default_value("@")
                .multiple(true),
        );
    let evolve_command =
        SubCommand::with_name("evolve").about("resolve problems with the repo's meta-history");
    let operation_command = SubCommand::with_name("operation")
        .alias("op")
        .about("commands for working with the operation log")
        .setting(clap::AppSettings::SubcommandRequiredElseHelp)
        .subcommand(SubCommand::with_name("log").about("show the operation log"))
        .subcommand(
            SubCommand::with_name("undo")
                .about("undo an operation")
                .arg(op_arg()),
        )
        .subcommand(
            SubCommand::with_name("restore")
                .about("restore to the state at an operation")
                .arg(op_arg()),
        );
    let git_command = SubCommand::with_name("git")
        .about("commands for working with the underlying git repo")
        .setting(clap::AppSettings::SubcommandRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("fetch")
                .about("fetch from a git remote")
                .arg(
                    Arg::with_name("remote")
                        .long("remote")
                        .takes_value(true)
                        .default_value("origin"),
                ),
        )
        .subcommand(
            SubCommand::with_name("clone")
                .about("create a new repo backed by a clone of a git repo")
                .arg(Arg::with_name("source").index(1).required(true))
                .arg(Arg::with_name("destination").index(2).required(true)),
        )
        .subcommand(
            SubCommand::with_name("push")
                .about("push a revision to a git remote branch")
                .arg(
                    Arg::with_name("revision")
                        .long("revision")
                        .short("r")
                        .takes_value(true)
                        .default_value("@^"),
                )
                .arg(
                    Arg::with_name("remote")
                        .long("remote")
                        .takes_value(true)
                        .default_value("origin"),
                )
                .arg(
                    Arg::with_name("branch")
                        .long("branch")
                        .takes_value(true)
                        .required(true),
                ),
        )
        .subcommand(
            SubCommand::with_name("refresh")
                .about("update repo with changes made in underlying git repo"),
        );
    let bench_command = SubCommand::with_name("bench")
        .about("commands for benchmarking internal operations")
        .setting(clap::AppSettings::SubcommandRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("commonancestors")
                .about("finds the common ancestor(s) of a set of commits")
                .arg(Arg::with_name("revision1").index(1).required(true))
                .arg(Arg::with_name("revision2").index(2).required(true)),
        )
        .subcommand(
            SubCommand::with_name("isancestor")
                .about("checks if the first commit is an ancestor of the second commit")
                .arg(Arg::with_name("ancestor").index(1).required(true))
                .arg(Arg::with_name("descendant").index(2).required(true)),
        )
        .subcommand(
            SubCommand::with_name("walkrevs")
                .about("walks revisions that are ancestors of the second argument but not ancestors of the first")
                .arg(Arg::with_name("unwanted").index(1).required(true))
                .arg(Arg::with_name("wanted").index(2).required(true)),
        )
        .subcommand(
            SubCommand::with_name("resolveprefix")
                .about("resolve a commit id prefix")
                .arg(Arg::with_name("prefix").index(1).required(true)),
        );
    let debug_command = SubCommand::with_name("debug")
        .about("low-level commands not intended for users")
        .setting(clap::AppSettings::SubcommandRequiredElseHelp)
        .subcommand(
            SubCommand::with_name("resolverev")
                .about("resolves a revision identifier to its full id")
                .arg(rev_arg()),
        )
        .subcommand(
            SubCommand::with_name("workingcopy")
                .about("show information about the working copy state"),
        )
        .subcommand(
            SubCommand::with_name("writeworkingcopy")
                .about("write a tree from the working copy state"),
        )
        .subcommand(
            SubCommand::with_name("template")
                .about("parse a template")
                .arg(Arg::with_name("template").index(1).required(true)),
        )
        .subcommand(SubCommand::with_name("index").about("show commit index stats"))
        .subcommand(SubCommand::with_name("reindex").about("rebuild commit index"));
    App::new("Jujube")
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
                .takes_value(true),
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
        .subcommand(discard_command)
        .subcommand(restore_command)
        .subcommand(edit_command)
        .subcommand(split_command)
        .subcommand(merge_command)
        .subcommand(rebase_command)
        .subcommand(backout_command)
        .subcommand(evolve_command)
        .subcommand(operation_command)
        .subcommand(git_command)
        .subcommand(bench_command)
        .subcommand(debug_command)
}

fn cmd_init(
    ui: &mut Ui,
    _matches: &ArgMatches,
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

    let repo;
    if let Some(git_store_str) = sub_matches.value_of("git-store") {
        let git_store_path = ui.cwd().join(git_store_str);
        repo = ReadonlyRepo::init_external_git(ui.settings(), wc_path, git_store_path);
        let git_repo = repo.store().git_repo().unwrap();
        let mut tx = repo.start_transaction("import git refs");
        git::import_refs(&mut tx, &git_repo).unwrap();
        // TODO: Check out a recent commit. Maybe one with the highest generation
        // number.
        tx.commit();
    } else if sub_matches.is_present("git") {
        repo = ReadonlyRepo::init_internal_git(ui.settings(), wc_path);
    } else {
        repo = ReadonlyRepo::init_local(ui.settings(), wc_path);
    }
    writeln!(ui, "Initialized repo in {:?}", repo.working_copy_path());
    Ok(())
}

fn cmd_checkout(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let new_commit = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    let wc = owned_wc.lock().unwrap();
    wc.commit(ui.settings(), mut_repo);
    let mut tx = repo.start_transaction(&format!("check out commit {}", new_commit.id().hex()));
    tx.check_out(ui.settings(), &new_commit);
    tx.commit();
    let stats = update_working_copy(ui, Arc::get_mut(&mut repo).unwrap(), &wc)?;
    match stats {
        None => ui.write("already on that commit\n"),
        Some(stats) => writeln!(
            ui,
            "added {} files, modified {} files, removed {} files",
            stats.added_files, stats.updated_files, stats.removed_files
        ),
    }
    Ok(())
}

fn cmd_files(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let commit = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    for (name, _value) in commit.tree().entries() {
        writeln!(ui, "{}", name.to_internal_string());
    }
    Ok(())
}

fn print_diff(left: &[u8], right: &[u8], styler: &mut dyn Styler) {
    let num_context_lines = 3;
    let mut context = VecDeque::new();
    // Have we printed "..." for any skipped context?
    let mut skipped_context = false;
    // Are the lines in `context` to be printed before the next modified line?
    let mut context_before = true;
    files::diff(left, right, &mut |diff_line| {
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
                        print_diff_line(styler, line);
                    }
                    context.clear();
                    context_before = true;
                }
                if !skipped_context {
                    styler.write_bytes(b"    ...\n");
                    skipped_context = true;
                }
            }
        } else {
            if context_before {
                for line in &context {
                    print_diff_line(styler, line);
                }
            }
            context.clear();
            print_diff_line(styler, diff_line);
            context_before = false;
            skipped_context = false;
        }
    });
    if !context_before {
        for line in &context {
            print_diff_line(styler, line);
        }
    }
}

fn print_diff_line(styler: &mut dyn Styler, diff_line: &DiffLine) {
    if diff_line.has_left_content {
        styler.add_label(String::from("left"));
        styler.write_bytes(format!("{:>4}", diff_line.left_line_number).as_bytes());
        styler.remove_label();
        styler.write_bytes(b" ");
    } else {
        styler.write_bytes(b"     ");
    }
    if diff_line.has_right_content {
        styler.add_label(String::from("right"));
        styler.write_bytes(format!("{:>4}", diff_line.right_line_number).as_bytes());
        styler.remove_label();
        styler.write_bytes(b": ");
    } else {
        styler.write_bytes(b"    : ");
    }
    for hunk in &diff_line.hunks {
        match hunk {
            files::DiffHunk::Unmodified(data) => {
                styler.write_bytes(data);
            }
            files::DiffHunk::Removed(data) => {
                styler.add_label(String::from("left"));
                styler.write_bytes(data);
                styler.remove_label();
            }
            files::DiffHunk::Added(data) => {
                styler.add_label(String::from("right"));
                styler.write_bytes(data);
                styler.remove_label();
            }
        }
    }
}

fn cmd_diff(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    if sub_matches.is_present("revision")
        && (sub_matches.is_present("from") || sub_matches.is_present("to"))
    {
        return Err(CommandError::UserError(String::from(
            "--revision cannot be used with --from or --to",
        )));
    }
    let mut repo = get_repo(ui, &matches)?;
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    if sub_matches.is_present("from") || sub_matches.is_present("to") {}
    let from_tree;
    let to_tree;
    if sub_matches.is_present("from") || sub_matches.is_present("to") {
        from_tree =
            resolve_single_rev(ui, mut_repo, sub_matches.value_of("from").unwrap_or("@"))?.tree();
        to_tree =
            resolve_single_rev(ui, mut_repo, sub_matches.value_of("to").unwrap_or("@"))?.tree();
    } else {
        let commit = resolve_single_rev(
            ui,
            mut_repo,
            sub_matches.value_of("revision").unwrap_or("@"),
        )?;
        let parents = commit.parents();
        from_tree = merge_commit_trees(repo.as_repo_ref(), &parents);
        to_tree = commit.tree()
    }
    if sub_matches.is_present("summary") {
        show_diff_summary(ui, &from_tree, &to_tree);
    } else {
        let mut styler = ui.styler();
        styler.add_label(String::from("diff"));
        from_tree.diff(&to_tree, &mut |path, diff| match diff {
            TreeValueDiff::Added(TreeValue::Normal {
                id,
                executable: false,
            }) => {
                styler.add_label(String::from("header"));
                styler.write_str(&format!("added file {}:\n", path.to_internal_string()));
                styler.remove_label();

                let mut file_reader = repo.store().read_file(path, id).unwrap();
                styler.write_from_reader(&mut file_reader);
            }
            TreeValueDiff::Modified(
                TreeValue::Normal {
                    id: id_left,
                    executable: false,
                },
                TreeValue::Normal {
                    id: id_right,
                    executable: false,
                },
            ) => {
                styler.add_label(String::from("header"));
                styler.write_str(&format!("modified file {}:\n", path.to_internal_string()));
                styler.remove_label();

                let mut file_reader_left = repo.store().read_file(path, id_left).unwrap();
                let mut buffer_left = vec![];
                file_reader_left.read_to_end(&mut buffer_left).unwrap();
                let mut file_reader_right = repo.store().read_file(path, id_right).unwrap();
                let mut buffer_right = vec![];
                file_reader_right.read_to_end(&mut buffer_right).unwrap();

                print_diff(
                    buffer_left.as_slice(),
                    buffer_right.as_slice(),
                    styler.as_mut(),
                );
            }
            TreeValueDiff::Modified(
                TreeValue::Conflict(id_left),
                TreeValue::Normal {
                    id: id_right,
                    executable: false,
                },
            ) => {
                styler.add_label(String::from("header"));
                styler.write_str(&format!(
                    "resolved conflict in file {}:\n",
                    path.to_internal_string()
                ));
                styler.remove_label();

                let conflict_left = repo.store().read_conflict(id_left).unwrap();
                let mut buffer_left = vec![];
                conflicts::materialize_conflict(
                    repo.store(),
                    &path.to_repo_path(),
                    &conflict_left,
                    &mut buffer_left,
                );
                let mut file_reader_right = repo.store().read_file(path, id_right).unwrap();
                let mut buffer_right = vec![];
                file_reader_right.read_to_end(&mut buffer_right).unwrap();

                print_diff(
                    buffer_left.as_slice(),
                    buffer_right.as_slice(),
                    styler.as_mut(),
                );
            }
            TreeValueDiff::Modified(
                TreeValue::Normal {
                    id: id_left,
                    executable: false,
                },
                TreeValue::Conflict(id_right),
            ) => {
                styler.add_label(String::from("header"));
                styler.write_str(&format!(
                    "new conflict in file {}:\n",
                    path.to_internal_string()
                ));
                styler.remove_label();

                let mut file_reader_left = repo.store().read_file(path, id_left).unwrap();
                let mut buffer_left = vec![];
                file_reader_left.read_to_end(&mut buffer_left).unwrap();
                let conflict_right = repo.store().read_conflict(id_right).unwrap();
                let mut buffer_right = vec![];
                conflicts::materialize_conflict(
                    repo.store(),
                    &path.to_repo_path(),
                    &conflict_right,
                    &mut buffer_right,
                );

                print_diff(
                    buffer_left.as_slice(),
                    buffer_right.as_slice(),
                    styler.as_mut(),
                );
            }
            TreeValueDiff::Removed(TreeValue::Normal {
                id,
                executable: false,
            }) => {
                styler.add_label(String::from("header"));
                styler.write_str(&format!("removed file {}:\n", path.to_internal_string()));
                styler.remove_label();

                let mut file_reader = repo.store().read_file(path, id).unwrap();
                styler.write_from_reader(&mut file_reader);
            }
            other => {
                writeln!(
                    styler,
                    "unhandled diff case in path {:?}: {:?}",
                    path, other
                )
                .unwrap();
            }
        });
        styler.remove_label();
    }
    Ok(())
}

fn show_diff_summary(ui: &mut Ui, from: &Tree, to: &Tree) {
    let summary = from.diff_summary(&to);
    for file in summary.modified {
        writeln!(ui, "M {}", file.to_internal_string());
    }
    for file in summary.added {
        writeln!(ui, "A {}", file.to_internal_string());
    }
    for file in summary.removed {
        writeln!(ui, "R {}", file.to_internal_string());
    }
}

fn cmd_status(
    ui: &mut Ui,
    matches: &ArgMatches,
    _sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let wc = owned_wc.lock().unwrap();
    let commit = wc.commit(ui.settings(), mut_repo);
    ui.write("Working copy : ");
    ui.write_commit_summary(repo.as_repo_ref(), &commit);
    ui.write("\n");
    ui.write("Parent commit: ");
    ui.write_commit_summary(repo.as_repo_ref(), &commit.parents()[0]);
    ui.write("\n");
    ui.write("Diff summary:\n");
    show_diff_summary(ui, &commit.parents()[0].tree(), &commit.tree());
    Ok(())
}

fn log_template(settings: &UserSettings) -> String {
    let default_template = r#"
            label(if(open, "open"),
            "commit: " commit_id "\n"
            "change: " change_id "\n"
            "author: " author.name() " <" author.email() ">\n"
            "committer: " committer.name() " <" committer.email() ">\n"
            "git refs: " git_refs "\n"
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
            if(current_checkout, "<-- ")
            label(if(open, "open"),
            commit_id.short()
            " " change_id.short()
            " " author.email()
            " " committer.email()
            " " git_refs
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

fn skip_uninteresting_heads(repo: &ReadonlyRepo, heads: HashSet<Commit>) -> HashSet<Commit> {
    let checkout_id = repo.view().checkout().clone();
    let mut result = HashSet::new();
    let mut work: Vec<_> = heads.into_iter().collect();
    let evolution = repo.evolution();
    while !work.is_empty() {
        let commit = work.pop().unwrap();
        if result.contains(&commit) {
            continue;
        }
        if (!commit.is_pruned() && !evolution.is_obsolete(commit.id()))
            || commit.id() == &checkout_id
        {
            result.insert(commit);
        } else {
            for parent in commit.parents() {
                work.push(parent);
            }
        }
    }
    result
}

fn cmd_log(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();

    let use_graph = !sub_matches.is_present("no-graph");
    if use_graph {
        // Commit so the latest working copy is reflected in the visible heads
        owned_wc.lock().unwrap().commit(ui.settings(), mut_repo);
    }

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

    let mut styler = ui.styler();
    let mut styler = styler.as_mut();
    styler.add_label(String::from("log"));

    let mut heads: HashSet<_> = repo
        .view()
        .heads()
        .iter()
        .map(|id| repo.store().get_commit(id).unwrap())
        .collect();
    if !sub_matches.is_present("all") {
        heads = skip_uninteresting_heads(&repo, heads);
    };
    let mut heads: Vec<_> = heads.into_iter().collect();
    heads.sort_by_key(|commit| commit.committer().timestamp.clone());
    heads.reverse();

    let commits = topo_order_reverse(
        heads,
        Box::new(|commit: &Commit| commit.id().clone()),
        Box::new(|commit: &Commit| commit.parents()),
    );
    if use_graph {
        let mut graph = AsciiGraphDrawer::new(&mut styler);
        for commit in commits {
            let mut edges = vec![];
            for parent in commit.parents() {
                edges.push(Edge::direct(parent.id().clone()));
            }
            let mut buffer = vec![];
            // TODO: only use color if requested
            {
                let writer = Box::new(&mut buffer);
                let mut styler = ColorStyler::new(writer, ui.settings());
                template.format(&commit, &mut styler);
            }
            if !buffer.ends_with(b"\n") {
                buffer.push(b'\n');
            }
            graph.add_node(commit.id(), &edges, b"o", &buffer);
        }
    } else {
        for commit in commits {
            template.format(&commit, styler);
        }
    }

    Ok(())
}

fn cmd_obslog(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;

    let use_graph = !sub_matches.is_present("no-graph");
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let start_commit = resolve_revision_arg(ui, mut_repo, sub_matches)?;

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

    let mut styler = ui.styler();
    let mut styler = styler.as_mut();
    styler.add_label(String::from("log"));

    let commits = topo_order_reverse(
        vec![start_commit],
        Box::new(|commit: &Commit| commit.id().clone()),
        Box::new(|commit: &Commit| commit.predecessors()),
    );
    if use_graph {
        let mut graph = AsciiGraphDrawer::new(&mut styler);
        for commit in commits {
            let mut edges = vec![];
            for predecessor in commit.predecessors() {
                edges.push(Edge::direct(predecessor.id().clone()));
            }
            let mut buffer = vec![];
            // TODO: only use color if requested
            {
                let writer = Box::new(&mut buffer);
                let mut styler = ColorStyler::new(writer, ui.settings());
                template.format(&commit, &mut styler);
            }
            if !buffer.ends_with(b"\n") {
                buffer.push(b'\n');
            }
            graph.add_node(commit.id(), &edges, b"o", &buffer);
        }
    } else {
        for commit in commits {
            template.format(&commit, styler);
        }
    }

    Ok(())
}

fn edit_description(repo: &ReadonlyRepo, description: &str) -> String {
    // TODO: Where should this file live? The current location prevents two
    // concurrent `jj describe` calls.
    let description_file_path = repo.repo_path().join("description");
    {
        let mut description_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&description_file_path)
            .unwrap_or_else(|_| panic!("failed to open {:?} for write", &description_file_path));
        description_file.write_all(description.as_bytes()).unwrap();
    }

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "pico".to_string());
    // Handle things like `EDITOR=emacs -nw`
    let args: Vec<_> = editor.split(' ').collect();
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
    String::from_utf8(buf).unwrap()
}

fn cmd_describe(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let commit = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    let description;
    if sub_matches.is_present("stdin") {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer).unwrap();
        description = buffer;
    } else if sub_matches.is_present("message") {
        description = sub_matches.value_of("message").unwrap().to_owned()
    } else {
        description = edit_description(&repo, commit.description());
    }
    let mut tx = repo.start_transaction(&format!("describe commit {}", commit.id().hex()));
    CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
        .set_description(description)
        .write_to_transaction(&mut tx);
    update_checkout_after_rewrite(ui, &mut tx);
    tx.commit();
    update_working_copy(
        ui,
        Arc::get_mut(&mut repo).unwrap(),
        &owned_wc.lock().unwrap(),
    )?;
    Ok(())
}

fn cmd_open(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let commit = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    let mut tx = repo.start_transaction(&format!("open commit {}", commit.id().hex()));
    CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
        .set_open(true)
        .write_to_transaction(&mut tx);
    update_checkout_after_rewrite(ui, &mut tx);
    tx.commit();
    update_working_copy(
        ui,
        Arc::get_mut(&mut repo).unwrap(),
        &owned_wc.lock().unwrap(),
    )?;
    Ok(())
}

fn cmd_close(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let commit = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    let mut commit_builder =
        CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit).set_open(false);
    let description;
    if sub_matches.is_present("message") {
        description = sub_matches.value_of("message").unwrap().to_string();
    } else if commit.description().is_empty() {
        description = edit_description(&repo, "");
    } else {
        description = commit.description().to_string();
    }
    commit_builder = commit_builder.set_description(description);
    let mut tx = repo.start_transaction(&format!("close commit {}", commit.id().hex()));
    commit_builder.write_to_transaction(&mut tx);
    update_checkout_after_rewrite(ui, &mut tx);
    tx.commit();
    update_working_copy(
        ui,
        Arc::get_mut(&mut repo).unwrap(),
        &owned_wc.lock().unwrap(),
    )?;
    Ok(())
}

fn cmd_duplicate(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let predecessor = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    let mut tx = repo.start_transaction(&format!("duplicate commit {}", predecessor.id().hex()));
    let new_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &predecessor)
        .generate_new_change_id()
        .write_to_transaction(&mut tx);
    ui.write("created: ");
    ui.write_commit_summary(tx.as_repo_ref(), &new_commit);
    ui.write("\n");
    tx.commit();
    Ok(())
}

fn cmd_prune(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let predecessor = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    if predecessor.id() == repo.store().root_commit_id() {
        return Err(CommandError::UserError(String::from(
            "Cannot prune the root commit",
        )));
    }
    let mut tx = repo.start_transaction(&format!("prune commit {}", predecessor.id().hex()));
    CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &predecessor)
        .set_pruned(true)
        .write_to_transaction(&mut tx);
    update_checkout_after_rewrite(ui, &mut tx);
    tx.commit();
    update_working_copy(
        ui,
        Arc::get_mut(&mut repo).unwrap(),
        &owned_wc.lock().unwrap(),
    )?;
    Ok(())
}

fn cmd_new(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let parent = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    let commit_builder = CommitBuilder::for_open_commit(
        ui.settings(),
        repo.store(),
        parent.id().clone(),
        parent.tree().id().clone(),
    );
    let mut tx = repo.start_transaction("new empty commit");
    let new_commit = commit_builder.write_to_transaction(&mut tx);
    if tx.view().checkout() == parent.id() {
        tx.check_out(ui.settings(), &new_commit);
    }
    tx.commit();
    update_working_copy(
        ui,
        Arc::get_mut(&mut repo).unwrap(),
        &owned_wc.lock().unwrap(),
    )?;
    Ok(())
}

fn cmd_squash(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let commit = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    let parents = commit.parents();
    if parents.len() != 1 {
        return Err(CommandError::UserError(String::from(
            "Cannot squash merge commits",
        )));
    }
    let parent = &parents[0];
    if parent.id() == repo.store().root_commit_id() {
        return Err(CommandError::UserError(String::from(
            "Cannot squash into the root commit",
        )));
    }
    let mut tx = repo.start_transaction(&format!("squash commit {}", commit.id().hex()));
    let squashed_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &parent)
        .set_tree(commit.tree().id().clone())
        .set_predecessors(vec![parent.id().clone(), commit.id().clone()])
        .write_to_transaction(&mut tx);
    // Commit the remainder on top of the new commit (always empty in the
    // non-interactive case), so the squashed-in commit becomes obsolete, and so
    // descendants evolve correctly.
    CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
        .set_parents(vec![squashed_commit.id().clone()])
        .set_pruned(true)
        .write_to_transaction(&mut tx);
    update_checkout_after_rewrite(ui, &mut tx);
    tx.commit();
    update_working_copy(
        ui,
        Arc::get_mut(&mut repo).unwrap(),
        &owned_wc.lock().unwrap(),
    )?;
    Ok(())
}

fn cmd_discard(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let commit = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    let mut tx = repo.start_transaction(&format!("discard commit {}", commit.id().hex()));
    tx.remove_head(&commit);
    for parent in commit.parents() {
        tx.add_head(&parent);
    }
    // TODO: also remove descendants
    tx.commit();
    // TODO: check out parent/ancestor if the current commit got hidden
    Ok(())
}

fn cmd_restore(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let source_commit = resolve_single_rev(ui, mut_repo, sub_matches.value_of("source").unwrap())?;
    let destination_commit =
        resolve_single_rev(ui, mut_repo, sub_matches.value_of("destination").unwrap())?;
    let tree_id;
    if sub_matches.is_present("interactive") {
        if sub_matches.is_present("paths") {
            return Err(UserError(
                "restore with --interactive and path is not yet supported".to_string(),
            ));
        }
        tree_id = crate::diff_edit::edit_diff(&source_commit.tree(), &destination_commit.tree())?;
    } else if sub_matches.is_present("paths") {
        let paths = sub_matches.values_of("paths").unwrap();
        let mut tree_builder = repo
            .store()
            .tree_builder(destination_commit.tree().id().clone());
        for path in paths {
            let repo_path = RepoPath::from(path);
            match source_commit.tree().path_value(&repo_path) {
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
        tree_id = source_commit.tree().id().clone();
    }
    if &tree_id == destination_commit.tree().id() {
        ui.write("Nothing changed.\n");
    } else {
        let mut tx = repo.start_transaction(&format!(
            "restore into commit {}",
            destination_commit.id().hex()
        ));
        let new_commit =
            CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &destination_commit)
                .set_tree(tree_id)
                .write_to_transaction(&mut tx);
        ui.write("Created ");
        ui.write_commit_summary(tx.as_repo_ref(), &new_commit);
        ui.write("\n");
        update_checkout_after_rewrite(ui, &mut tx);
        tx.commit();
        update_working_copy(
            ui,
            Arc::get_mut(&mut repo).unwrap(),
            &owned_wc.lock().unwrap(),
        )?;
    }
    Ok(())
}

fn cmd_edit(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let commit = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    let base_tree = merge_commit_trees(repo.as_repo_ref(), &commit.parents());
    let tree_id = crate::diff_edit::edit_diff(&base_tree, &commit.tree())?;
    if &tree_id == commit.tree().id() {
        ui.write("Nothing changed.\n");
    } else {
        let mut tx = repo.start_transaction(&format!("edit commit {}", commit.id().hex()));
        let new_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
            .set_tree(tree_id)
            .write_to_transaction(&mut tx);
        ui.write("Created ");
        ui.write_commit_summary(tx.as_repo_ref(), &new_commit);
        ui.write("\n");
        update_checkout_after_rewrite(ui, &mut tx);
        tx.commit();
        update_working_copy(
            ui,
            Arc::get_mut(&mut repo).unwrap(),
            &owned_wc.lock().unwrap(),
        )?;
    }
    Ok(())
}

fn cmd_split(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let commit = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    let base_tree = merge_commit_trees(repo.as_repo_ref(), &commit.parents());
    let tree_id = crate::diff_edit::edit_diff(&base_tree, &commit.tree())?;
    if &tree_id == commit.tree().id() {
        ui.write("Nothing changed.\n");
    } else {
        let mut tx = repo.start_transaction(&format!("split commit {}", commit.id().hex()));
        // TODO: Add a header or footer to the decription where we describe to the user
        // that this is the first commit
        let first_description = edit_description(&repo, commit.description());
        let first_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
            .set_tree(tree_id)
            .set_description(first_description)
            .write_to_transaction(&mut tx);
        let second_description = edit_description(&repo, commit.description());
        let second_commit = CommitBuilder::for_rewrite_from(ui.settings(), repo.store(), &commit)
            .set_parents(vec![first_commit.id().clone()])
            .set_tree(commit.tree().id().clone())
            .generate_new_change_id()
            .set_description(second_description)
            .write_to_transaction(&mut tx);
        ui.write("First part: ");
        ui.write_commit_summary(tx.as_repo_ref(), &first_commit);
        ui.write("Second part: ");
        ui.write_commit_summary(tx.as_repo_ref(), &second_commit);
        ui.write("\n");
        update_checkout_after_rewrite(ui, &mut tx);
        tx.commit();
        update_working_copy(
            ui,
            Arc::get_mut(&mut repo).unwrap(),
            &owned_wc.lock().unwrap(),
        )?;
    }
    Ok(())
}

fn cmd_merge(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let revision_args = sub_matches.values_of("revisions").unwrap();
    if revision_args.len() < 2 {
        return Err(CommandError::UserError(String::from(
            "Merge requires at least two revisions",
        )));
    }
    let mut commits = vec![];
    let mut parent_ids = vec![];
    for revision_arg in revision_args {
        let commit = resolve_single_rev(ui, mut_repo, revision_arg)?;
        parent_ids.push(commit.id().clone());
        commits.push(commit);
    }
    let description;
    if sub_matches.is_present("message") {
        description = sub_matches.value_of("message").unwrap().to_string();
    } else {
        description = edit_description(&repo, "");
    }
    let merged_tree = merge_commit_trees(repo.as_repo_ref(), &commits);
    let mut tx = repo.start_transaction("merge commits");
    CommitBuilder::for_new_commit(ui.settings(), repo.store(), merged_tree.id().clone())
        .set_parents(parent_ids)
        .set_description(description)
        .set_open(false)
        .write_to_transaction(&mut tx);
    update_checkout_after_rewrite(ui, &mut tx);
    tx.commit();
    update_working_copy(
        ui,
        Arc::get_mut(&mut repo).unwrap(),
        &owned_wc.lock().unwrap(),
    )?;

    Ok(())
}

fn cmd_rebase(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let commit_to_rebase = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    let mut parents = vec![];
    for revision_str in sub_matches.values_of("destination").unwrap() {
        parents.push(resolve_single_rev(ui, mut_repo, revision_str)?);
    }
    let mut tx = repo.start_transaction(&format!("rebase commit {}", commit_to_rebase.id().hex()));
    rebase_commit(ui.settings(), &mut tx, &commit_to_rebase, &parents);
    update_checkout_after_rewrite(ui, &mut tx);
    tx.commit();
    update_working_copy(
        ui,
        Arc::get_mut(&mut repo).unwrap(),
        &owned_wc.lock().unwrap(),
    )?;

    Ok(())
}

fn cmd_backout(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let commit_to_back_out = resolve_revision_arg(ui, mut_repo, sub_matches)?;
    let mut parents = vec![];
    for revision_str in sub_matches.values_of("destination").unwrap() {
        parents.push(resolve_single_rev(ui, mut_repo, revision_str)?);
    }
    let mut tx = repo.start_transaction(&format!(
        "back out commit {}",
        commit_to_back_out.id().hex()
    ));
    back_out_commit(ui.settings(), &mut tx, &commit_to_back_out, &parents);
    update_checkout_after_rewrite(ui, &mut tx);
    tx.commit();
    update_working_copy(
        ui,
        Arc::get_mut(&mut repo).unwrap(),
        &owned_wc.lock().unwrap(),
    )?;

    Ok(())
}

fn cmd_evolve<'s>(
    ui: &mut Ui<'s>,
    matches: &ArgMatches,
    _sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();

    struct Listener<'a, 's> {
        ui: &'a mut Ui<'s>,
    }

    impl<'a, 's> EvolveListener for Listener<'a, 's> {
        fn orphan_evolved(&mut self, tx: &mut Transaction, orphan: &Commit, new_commit: &Commit) {
            self.ui.write("Resolving orphan: ");
            self.ui.write_commit_summary(tx.as_repo_ref(), &orphan);
            self.ui.write("\n");
            self.ui.write("Resolved as: ");
            self.ui.write_commit_summary(tx.as_repo_ref(), &new_commit);
            self.ui.write("\n");
        }

        fn orphan_target_ambiguous(&mut self, tx: &mut Transaction, orphan: &Commit) {
            self.ui
                .write("Skipping orphan with ambiguous new parents: ");
            self.ui.write_commit_summary(tx.as_repo_ref(), &orphan);
            self.ui.write("\n");
        }

        fn divergent_resolved(
            &mut self,
            tx: &mut Transaction,
            sources: &[Commit],
            resolved: &Commit,
        ) {
            self.ui.write("Resolving divergent commits:\n");
            for source in sources {
                self.ui.write("  ");
                self.ui.write_commit_summary(tx.as_repo_ref(), &source);
                self.ui.write("\n");
            }
            self.ui.write("Resolved as: ");
            self.ui.write_commit_summary(tx.as_repo_ref(), &resolved);
            self.ui.write("\n");
        }

        fn divergent_no_common_predecessor(
            &mut self,
            tx: &mut Transaction,
            commit1: &Commit,
            commit2: &Commit,
        ) {
            self.ui
                .write("Skipping divergent commits with no common predecessor:\n");
            self.ui.write("  ");
            self.ui.write_commit_summary(tx.as_repo_ref(), &commit1);
            self.ui.write("\n");
            self.ui.write("  ");
            self.ui.write_commit_summary(tx.as_repo_ref(), &commit2);
            self.ui.write("\n");
        }
    }

    // TODO: This clone is unnecessary. Maybe ui.write() etc should not require a
    // mutable borrow? But the mutable borrow might be useful for making sure we
    // have only one Ui instance we write to across threads?
    let user_settings = ui.settings().clone();
    let mut listener = Listener { ui };
    let mut tx = repo.start_transaction("evolve");
    evolve(&user_settings, &mut tx, &mut listener);
    update_checkout_after_rewrite(ui, &mut tx);
    tx.commit();
    update_working_copy(
        ui,
        Arc::get_mut(&mut repo).unwrap(),
        &owned_wc.lock().unwrap(),
    )?;

    Ok(())
}

fn cmd_debug(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    if let Some(resolve_matches) = sub_matches.subcommand_matches("resolverev") {
        let mut repo = get_repo(ui, &matches)?;
        let mut_repo = Arc::get_mut(&mut repo).unwrap();
        let commit = resolve_revision_arg(ui, mut_repo, resolve_matches)?;
        writeln!(ui, "{}", commit.id().hex());
    } else if let Some(_wc_matches) = sub_matches.subcommand_matches("workingcopy") {
        let repo = get_repo(ui, &matches)?;
        let wc = repo.working_copy_locked();
        writeln!(ui, "Current commit: {:?}", wc.current_commit_id());
        writeln!(ui, "Current tree: {:?}", wc.current_tree_id());
        for (file, state) in wc.file_states().iter() {
            writeln!(
                ui,
                "{:?} {:13?} {:10?} {:?}",
                state.file_type, state.size, state.mtime.0, file
            );
        }
    } else if let Some(_wc_matches) = sub_matches.subcommand_matches("writeworkingcopy") {
        let mut repo = get_repo(ui, &matches)?;
        let owned_wc = repo.working_copy().clone();
        let wc = owned_wc.lock().unwrap();
        let mut_repo = Arc::get_mut(&mut repo).unwrap();
        let old_commit_id = wc.current_commit_id();
        let new_commit_id = wc.commit(ui.settings(), mut_repo).id().clone();
        writeln!(ui, "old commit {:?}", old_commit_id);
        writeln!(ui, "new commit {:?}", new_commit_id);
    } else if let Some(template_matches) = sub_matches.subcommand_matches("template") {
        let parse = TemplateParser::parse(
            crate::template_parser::Rule::template,
            template_matches.value_of("template").unwrap(),
        );
        writeln!(ui, "{:?}", parse);
    } else if let Some(_reindex_matches) = sub_matches.subcommand_matches("index") {
        let repo = get_repo(ui, &matches)?;
        let stats = repo.index().stats();
        writeln!(ui, "Number of commits: {}", stats.num_commits);
        writeln!(ui, "Number of merges: {}", stats.num_merges);
        writeln!(ui, "Max generation number: {}", stats.max_generation_number);
        writeln!(ui, "Number of heads: {}", stats.num_heads);
        writeln!(ui, "Number of pruned commits: {}", stats.num_pruned_commits);
        writeln!(ui, "Number of changes: {}", stats.num_changes);
        writeln!(ui, "Stats per level:");
        for (i, level) in stats.levels.iter().enumerate() {
            writeln!(ui, "  Level {}:", i);
            writeln!(ui, "    Number of commits: {}", level.num_commits);
            writeln!(ui, "    Name: {}", level.name.as_ref().unwrap());
        }
    } else if let Some(_reindex_matches) = sub_matches.subcommand_matches("reindex") {
        let mut repo = get_repo(ui, &matches)?;
        let mut_repo = Arc::get_mut(&mut repo).unwrap();
        let index = mut_repo.reindex();
        writeln!(ui, "Finished indexing {:?} commits.", index.num_commits());
    } else {
        panic!("unhandled command: {:#?}", matches);
    }
    Ok(())
}

fn run_bench<R, O>(ui: &mut Ui, id: &str, mut routine: R)
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
    );
    criterion.bench_function(id, |bencher: &mut criterion::Bencher| {
        bencher.iter(routine);
    });
}

fn cmd_bench(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    if let Some(command_matches) = sub_matches.subcommand_matches("commonancestors") {
        let mut repo = get_repo(ui, &matches)?;
        let mut_repo = Arc::get_mut(&mut repo).unwrap();
        let commit1 =
            resolve_single_rev(ui, mut_repo, command_matches.value_of("revision1").unwrap())?;
        let commit2 =
            resolve_single_rev(ui, mut_repo, command_matches.value_of("revision2").unwrap())?;
        let routine = || {
            repo.index()
                .common_ancestors(&[commit1.id().clone()], &[commit2.id().clone()])
        };
        run_bench(ui, "commonancestors", routine);
    } else if let Some(command_matches) = sub_matches.subcommand_matches("isancestor") {
        let mut repo = get_repo(ui, &matches)?;
        let mut_repo = Arc::get_mut(&mut repo).unwrap();
        let ancestor_commit =
            resolve_single_rev(ui, mut_repo, command_matches.value_of("ancestor").unwrap())?;
        let descendant_commit = resolve_single_rev(
            ui,
            mut_repo,
            command_matches.value_of("descendant").unwrap(),
        )?;
        let index = repo.index();
        let routine = || index.is_ancestor(ancestor_commit.id(), descendant_commit.id());
        run_bench(ui, "isancestor", routine);
    } else if let Some(command_matches) = sub_matches.subcommand_matches("walkrevs") {
        let mut repo = get_repo(ui, &matches)?;
        let mut_repo = Arc::get_mut(&mut repo).unwrap();
        let unwanted_commit =
            resolve_single_rev(ui, mut_repo, command_matches.value_of("unwanted").unwrap())?;
        let wanted_commit =
            resolve_single_rev(ui, mut_repo, command_matches.value_of("wanted").unwrap())?;
        let index = repo.index();
        let routine = || {
            index
                .walk_revs(
                    &[wanted_commit.id().clone()],
                    &[unwanted_commit.id().clone()],
                )
                .count()
        };
        run_bench(ui, "walkrevs", routine);
    } else if let Some(command_matches) = sub_matches.subcommand_matches("resolveprefix") {
        let repo = get_repo(ui, &matches)?;
        let prefix = HexPrefix::new(command_matches.value_of("prefix").unwrap().to_string());
        let index = repo.index();
        let routine = || index.resolve_prefix(&prefix);
        run_bench(ui, "resolveprefix", routine);
    } else {
        panic!("unhandled command: {:#?}", matches);
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
    matches: &ArgMatches,
    _op_matches: &ArgMatches,
    _cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let repo = get_repo(ui, &matches)?;
    let view = repo.view();
    let head_op = view.as_view_ref().base_op();
    let mut styler = ui.styler();
    let mut styler = styler.as_mut();
    struct OpTemplate;
    impl Template<Operation> for OpTemplate {
        fn format(&self, op: &Operation, styler: &mut dyn Styler) {
            // TODO: why can't this label be applied outside of the template?
            styler.add_label("op-log".to_string());
            // TODO: Make this templated
            styler.add_label("id".to_string());
            // TODO: support lookup by op-id prefix, so we don't need to print the full hash
            // here
            styler.write_str(&op.id().hex());
            styler.remove_label();
            styler.write_str(" ");
            let metadata = &op.store_operation().metadata;
            styler.add_label("user".to_string());
            styler.write_str(&format!("{}@{}", metadata.username, metadata.hostname));
            styler.remove_label();
            styler.write_str(" ");
            styler.add_label("time".to_string());
            styler.write_str(&format!(
                "{} - {}",
                format_timestamp(&metadata.start_time),
                format_timestamp(&metadata.end_time)
            ));
            styler.remove_label();
            styler.write_str("\n");
            styler.add_label("description".to_string());
            styler.write_str(&metadata.description);
            styler.remove_label();

            styler.remove_label();
        }
    }
    let template = OpTemplate;

    let mut graph = AsciiGraphDrawer::new(&mut styler);
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
        // TODO: only use color if requested
        {
            let writer = Box::new(&mut buffer);
            let mut styler = ColorStyler::new(writer, ui.settings());
            template.format(&op, &mut styler);
        }
        if !buffer.ends_with(b"\n") {
            buffer.push(b'\n');
        }
        graph.add_node(op.id(), &edges, b"o", &buffer);
    }

    Ok(())
}

fn cmd_op_undo(
    ui: &mut Ui,
    matches: &ArgMatches,
    _op_matches: &ArgMatches,
    _cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let bad_op = resolve_single_op(&repo, _cmd_matches.value_of("operation").unwrap())?;
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

    let fixed_view = {
        let view = repo.view();
        let parent_view = parent_ops[0].view();
        let bad_view = bad_op.view();
        let current_view = view.as_view_ref().base_op().view();
        merge_views(
            repo.store(),
            current_view.store_view(),
            bad_view.store_view(),
            parent_view.store_view(),
        )
    };

    let mut tx = repo.start_transaction(&format!("undo operation {}", bad_op.id().hex()));
    tx.set_view(fixed_view);
    tx.commit();
    update_working_copy(
        ui,
        Arc::get_mut(&mut repo).unwrap(),
        &owned_wc.lock().unwrap(),
    )?;

    Ok(())
}
fn cmd_op_restore(
    ui: &mut Ui,
    matches: &ArgMatches,
    _op_matches: &ArgMatches,
    _cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let owned_wc = repo.working_copy().clone();
    let target_op = resolve_single_op(&repo, _cmd_matches.value_of("operation").unwrap())?;
    let mut tx = repo.start_transaction(&format!("restore to operation {}", target_op.id().hex()));
    tx.set_view(target_op.view().take_store_view());
    tx.commit();
    update_working_copy(
        ui,
        Arc::get_mut(&mut repo).unwrap(),
        &owned_wc.lock().unwrap(),
    )?;

    Ok(())
}

fn cmd_operation(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    if let Some(command_matches) = sub_matches.subcommand_matches("log") {
        cmd_op_log(ui, matches, sub_matches, command_matches)?;
    } else if let Some(command_matches) = sub_matches.subcommand_matches("undo") {
        cmd_op_undo(ui, matches, sub_matches, command_matches)?;
    } else if let Some(command_matches) = sub_matches.subcommand_matches("restore") {
        cmd_op_restore(ui, matches, sub_matches, command_matches)?;
    } else {
        panic!("unhandled command: {:#?}", matches);
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
    matches: &ArgMatches,
    _git_matches: &ArgMatches,
    cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let repo = get_repo(ui, &matches)?;
    let git_repo = get_git_repo(repo.store())?;
    let remote_name = cmd_matches.value_of("remote").unwrap();
    let mut tx = repo.start_transaction(&format!("fetch from git remote {}", remote_name));
    git::fetch(&mut tx, &git_repo, remote_name)
        .map_err(|err| CommandError::UserError(err.to_string()))?;
    tx.commit();
    Ok(())
}

fn cmd_git_clone(
    ui: &mut Ui,
    _matches: &ArgMatches,
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

    let repo = ReadonlyRepo::init_internal_git(ui.settings(), wc_path);
    let git_repo = get_git_repo(repo.store())?;
    writeln!(
        ui,
        "Fetching into new repo in {:?}",
        repo.working_copy_path()
    );
    let remote_name = "origin";
    git_repo.remote(remote_name, source).unwrap();
    let mut tx = repo.start_transaction("fetch from git remote into empty repo");
    git::fetch(&mut tx, &git_repo, remote_name).map_err(|err| match err {
        GitFetchError::NoSuchRemote(_) => {
            panic!("should't happen as we just created the git remote")
        }
        GitFetchError::InternalGitError(err) => {
            CommandError::UserError(format!("Fetch failed: {:?}", err))
        }
    })?;
    tx.commit();
    writeln!(ui, "Done");
    Ok(())
}

fn cmd_git_push(
    ui: &mut Ui,
    matches: &ArgMatches,
    _git_matches: &ArgMatches,
    cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let mut repo = get_repo(ui, &matches)?;
    let git_repo = get_git_repo(repo.store())?;
    let mut_repo = Arc::get_mut(&mut repo).unwrap();
    let commit = resolve_revision_arg(ui, mut_repo, cmd_matches)?;
    let remote_name = cmd_matches.value_of("remote").unwrap();
    let branch_name = cmd_matches.value_of("branch").unwrap();
    git::push_commit(&git_repo, &commit, remote_name, branch_name)
        .map_err(|err| CommandError::UserError(err.to_string()))?;
    let mut tx = repo.start_transaction("import git refs");
    git::import_refs(&mut tx, &git_repo).map_err(|err| CommandError::UserError(err.to_string()))?;
    tx.commit();
    Ok(())
}

fn cmd_git_refresh(
    ui: &mut Ui,
    matches: &ArgMatches,
    _git_matches: &ArgMatches,
    _cmd_matches: &ArgMatches,
) -> Result<(), CommandError> {
    let repo = get_repo(ui, &matches)?;
    let git_repo = get_git_repo(repo.store())?;
    let mut tx = repo.start_transaction("import git refs");
    git::import_refs(&mut tx, &git_repo).map_err(|err| CommandError::UserError(err.to_string()))?;
    tx.commit();
    Ok(())
}

fn cmd_git(
    ui: &mut Ui,
    matches: &ArgMatches,
    sub_matches: &ArgMatches,
) -> Result<(), CommandError> {
    if let Some(command_matches) = sub_matches.subcommand_matches("fetch") {
        cmd_git_fetch(ui, matches, sub_matches, command_matches)?;
    } else if let Some(command_matches) = sub_matches.subcommand_matches("clone") {
        cmd_git_clone(ui, matches, sub_matches, command_matches)?;
    } else if let Some(command_matches) = sub_matches.subcommand_matches("push") {
        cmd_git_push(ui, matches, sub_matches, command_matches)?;
    } else if let Some(command_matches) = sub_matches.subcommand_matches("refresh") {
        cmd_git_refresh(ui, matches, sub_matches, command_matches)?;
    } else {
        panic!("unhandled command: {:#?}", matches);
    }
    Ok(())
}

pub fn dispatch<I, T>(mut ui: Ui, args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = get_app().get_matches_from(args);
    let result = if let Some(sub_matches) = matches.subcommand_matches("init") {
        cmd_init(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("checkout") {
        cmd_checkout(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("files") {
        cmd_files(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("diff") {
        cmd_diff(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("status") {
        cmd_status(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("log") {
        cmd_log(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("obslog") {
        cmd_obslog(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("describe") {
        cmd_describe(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("close") {
        cmd_close(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("open") {
        cmd_open(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("duplicate") {
        cmd_duplicate(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("prune") {
        cmd_prune(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("new") {
        cmd_new(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("squash") {
        cmd_squash(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("discard") {
        cmd_discard(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("restore") {
        cmd_restore(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("edit") {
        cmd_edit(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("split") {
        cmd_split(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("merge") {
        cmd_merge(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("rebase") {
        cmd_rebase(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("backout") {
        cmd_backout(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("evolve") {
        cmd_evolve(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("operation") {
        cmd_operation(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("git") {
        cmd_git(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("bench") {
        cmd_bench(&mut ui, &matches, &sub_matches)
    } else if let Some(sub_matches) = matches.subcommand_matches("debug") {
        cmd_debug(&mut ui, &matches, &sub_matches)
    } else {
        panic!("unhandled command: {:#?}", matches);
    };
    match result {
        Ok(()) => 0,
        Err(CommandError::UserError(message)) => {
            ui.write_error(format!("Error: {}\n", message).as_str());
            1
        }
        Err(CommandError::InternalError(message)) => {
            ui.write_error(format!("Internal error: {}\n", message).as_str());
            255
        }
    }
}
