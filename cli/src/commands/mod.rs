// Copyright 2020 The Jujutsu Authors
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

#[cfg(feature = "bench")]
mod bench;
mod branch;
mod debug;
mod git;
mod operation;

use std::collections::{BTreeMap, HashSet};
use std::fmt::Debug;
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::{fmt, fs, io};

use clap::builder::NonEmptyStringValueParser;
use clap::parser::ValueSource;
use clap::{ArgGroup, Command, CommandFactory, FromArgMatches, Subcommand};
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use jj_lib::backend::{CommitId, ObjectId, TreeValue};
use jj_lib::commit::Commit;
use jj_lib::dag_walk::topo_order_reverse;
use jj_lib::git_backend::GitBackend;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::merge::Merge;
use jj_lib::merged_tree::{MergedTree, MergedTreeBuilder};
use jj_lib::op_store::WorkspaceId;
use jj_lib::repo::{ReadonlyRepo, Repo};
use jj_lib::repo_path::RepoPath;
use jj_lib::revset::{RevsetExpression, RevsetFilterPredicate, RevsetIteratorExt};
use jj_lib::revset_graph::{
    ReverseRevsetGraphIterator, RevsetGraphEdgeType, TopoGroupedRevsetGraphIterator,
};
use jj_lib::rewrite::{back_out_commit, merge_commit_trees, rebase_commit, DescendantRebaser};
use jj_lib::settings::UserSettings;
use jj_lib::working_copy::SnapshotOptions;
use jj_lib::workspace::Workspace;
use jj_lib::{conflicts, file_util, revset};
use maplit::{hashmap, hashset};
use tracing::instrument;

use crate::cli_util::{
    self, check_stale_working_copy, get_new_config_file_path, print_checkout_stats,
    print_git_import_stats, resolve_multiple_nonempty_revsets,
    resolve_multiple_nonempty_revsets_default_single, run_ui_editor, serialize_config_value,
    short_commit_hash, user_error, user_error_with_hint, write_config_value_to_file, Args,
    CommandError, CommandHelper, LogContentFormat, RevisionArg, WorkspaceCommandHelper,
};
use crate::config::{AnnotatedValue, ConfigSource};
use crate::diff_util::{self, DiffFormat, DiffFormatArgs};
use crate::formatter::{Formatter, PlainTextFormatter};
use crate::graphlog::{get_graphlog, Edge};
use crate::text_util;
use crate::ui::Ui;

#[derive(clap::Parser, Clone, Debug)]
enum Commands {
    Abandon(AbandonArgs),
    Backout(BackoutArgs),
    #[cfg(feature = "bench")]
    #[command(subcommand)]
    Bench(bench::BenchCommands),
    #[command(subcommand)]
    Branch(branch::BranchSubcommand),
    #[command(alias = "print")]
    Cat(CatArgs),
    Checkout(CheckoutArgs),
    Chmod(ChmodArgs),
    Commit(CommitArgs),
    #[command(subcommand)]
    Config(ConfigSubcommand),
    #[command(subcommand)]
    Debug(debug::DebugCommands),
    Describe(DescribeArgs),
    Diff(DiffArgs),
    Diffedit(DiffeditArgs),
    Duplicate(DuplicateArgs),
    Edit(EditArgs),
    Files(FilesArgs),
    #[command(subcommand)]
    Git(git::GitCommands),
    Init(InitArgs),
    Interdiff(InterdiffArgs),
    Log(LogArgs),
    /// Merge work from multiple branches
    ///
    /// Unlike most other VCSs, `jj merge` does not implicitly include the
    /// working copy revision's parent as one of the parents of the merge;
    /// you need to explicitly list all revisions that should become parents
    /// of the merge.
    ///
    /// This is the same as `jj new`, except that it requires at least two
    /// arguments.
    Merge(NewArgs),
    Move(MoveArgs),
    New(NewArgs),
    Next(NextArgs),
    Obslog(ObslogArgs),
    #[command(subcommand)]
    #[command(visible_alias = "op")]
    Operation(operation::OperationCommands),
    Prev(PrevArgs),
    Rebase(RebaseArgs),
    Resolve(ResolveArgs),
    Restore(RestoreArgs),
    #[command(hide = true)]
    // TODO: Flesh out.
    Run(RunArgs),
    Show(ShowArgs),
    #[command(subcommand)]
    Sparse(SparseArgs),
    Split(SplitArgs),
    Squash(SquashArgs),
    Status(StatusArgs),
    #[command(subcommand)]
    Util(UtilCommands),
    /// Undo an operation (shortcut for `jj op undo`)
    Undo(operation::OperationUndoArgs),
    Unsquash(UnsquashArgs),
    Untrack(UntrackArgs),
    Version(VersionArgs),
    #[command(subcommand)]
    Workspace(WorkspaceCommands),
}

/// Display version information
#[derive(clap::Args, Clone, Debug)]
struct VersionArgs {}

/// Create a new repo in the given directory
///
/// If the given directory does not exist, it will be created. If no directory
/// is given, the current directory is used.
#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("backend").args(&["git", "git_repo"])))]
struct InitArgs {
    /// The destination directory
    #[arg(default_value = ".", value_hint = clap::ValueHint::DirPath)]
    destination: String,
    /// Use the Git backend, creating a jj repo backed by a Git repo
    #[arg(long)]
    git: bool,
    /// Path to a git repo the jj repo will be backed by
    #[arg(long, value_hint = clap::ValueHint::DirPath)]
    git_repo: Option<String>,
}

#[derive(clap::Args, Clone, Debug)]
#[command(group = clap::ArgGroup::new("config_level").multiple(false).required(true))]
struct ConfigArgs {
    /// Target the user-level config
    #[arg(long, group = "config_level")]
    user: bool,

    /// Target the repo-level config
    #[arg(long, group = "config_level")]
    repo: bool,
}

impl ConfigArgs {
    fn get_source_kind(&self) -> ConfigSource {
        if self.user {
            ConfigSource::User
        } else if self.repo {
            ConfigSource::Repo
        } else {
            // Shouldn't be reachable unless clap ArgGroup is broken.
            panic!("No config_level provided");
        }
    }
}

/// Manage config options
///
/// Operates on jj configuration, which comes from the config file and
/// environment variables.
///
/// For file locations, supported config options, and other details about jj
/// config, see https://github.com/martinvonz/jj/blob/main/docs/config.md.
#[derive(clap::Subcommand, Clone, Debug)]
enum ConfigSubcommand {
    #[command(visible_alias("l"))]
    List(ConfigListArgs),
    #[command(visible_alias("g"))]
    Get(ConfigGetArgs),
    #[command(visible_alias("s"))]
    Set(ConfigSetArgs),
    #[command(visible_alias("e"))]
    Edit(ConfigEditArgs),
}

/// List variables set in config file, along with their values.
#[derive(clap::Args, Clone, Debug)]
struct ConfigListArgs {
    /// An optional name of a specific config option to look up.
    #[arg(value_parser = NonEmptyStringValueParser::new())]
    pub name: Option<String>,
    /// Whether to explicitly include built-in default values in the list.
    #[arg(long)]
    pub include_defaults: bool,
    // TODO(#1047): Support --show-origin using LayeredConfigs.
    // TODO(#1047): Support ConfigArgs (--user or --repo).
}

/// Get the value of a given config option.
///
/// Unlike `jj config list`, the result of `jj config get` is printed without
/// extra formatting and therefore is usable in scripting. For example:
///
/// $ jj config list user.name
/// user.name="Martin von Zweigbergk"
/// $ jj config get user.name
/// Martin von Zweigbergk
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
struct ConfigGetArgs {
    #[arg(required = true)]
    name: String,
}

/// Update config file to set the given option to a given value.
#[derive(clap::Args, Clone, Debug)]
struct ConfigSetArgs {
    #[arg(required = true)]
    name: String,
    #[arg(required = true)]
    value: String,
    #[clap(flatten)]
    config_args: ConfigArgs,
}

/// Start an editor on a jj config file.
#[derive(clap::Args, Clone, Debug)]
struct ConfigEditArgs {
    #[clap(flatten)]
    pub config_args: ConfigArgs,
}

/// Create a new, empty change and edit it in the working copy
///
/// For more information, see
/// https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
#[command(visible_aliases = &["co"])]
struct CheckoutArgs {
    /// The revision to update to
    revision: RevisionArg,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
    /// The change description to use
    #[arg(long = "message", short, value_name = "MESSAGE")]
    message_paragraphs: Vec<String>,
}

/// Stop tracking specified paths in the working copy
#[derive(clap::Args, Clone, Debug)]
struct UntrackArgs {
    /// Paths to untrack
    #[arg(required = true, value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

/// List files in a revision
#[derive(clap::Args, Clone, Debug)]
struct FilesArgs {
    /// The revision to list files in
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Only list files matching these prefixes (instead of all files)
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

/// Print contents of a file in a revision
#[derive(clap::Args, Clone, Debug)]
struct CatArgs {
    /// The revision to get the file contents from
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// The file to print
    #[arg(value_hint = clap::ValueHint::FilePath)]
    path: String,
}

/// Show changes in a revision
///
/// With the `-r` option, which is the default, shows the changes compared to
/// the parent revision. If there are several parent revisions (i.e., the given
/// revision is a merge), then they will be merged and the changes from the
/// result to the given revision will be shown.
///
/// With the `--from` and/or `--to` options, shows the difference from/to the
/// given revisions. If either is left out, it defaults to the working-copy
/// commit. For example, `jj diff --from main` shows the changes from "main"
/// (perhaps a branch name) to the working-copy commit.
#[derive(clap::Args, Clone, Debug)]
struct DiffArgs {
    /// Show changes in this revision, compared to its parent(s)
    #[arg(long, short)]
    revision: Option<RevisionArg>,
    /// Show changes from this revision
    #[arg(long, conflicts_with = "revision")]
    from: Option<RevisionArg>,
    /// Show changes to this revision
    #[arg(long, conflicts_with = "revision")]
    to: Option<RevisionArg>,
    /// Restrict the diff to these paths
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
    #[command(flatten)]
    format: DiffFormatArgs,
}

/// Show commit description and changes in a revision
#[derive(clap::Args, Clone, Debug)]
struct ShowArgs {
    /// Show changes in this revision, compared to its parent(s)
    #[arg(default_value = "@")]
    revision: RevisionArg,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
    #[command(flatten)]
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
#[command(visible_alias = "st")]
struct StatusArgs {}

/// Show commit history
#[derive(clap::Args, Clone, Debug)]
struct LogArgs {
    /// Which revisions to show. Defaults to the `revsets.log` setting, or
    /// `@ | ancestors(immutable_heads().., 2) | heads(immutable_heads())` if
    /// it is not set.
    #[arg(long, short)]
    revisions: Vec<RevisionArg>,
    /// Show commits modifying the given paths
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
    /// Show revisions in the opposite order (older revisions first)
    #[arg(long)]
    reversed: bool,
    /// Limit number of revisions to show
    ///
    /// Applied after revisions are filtered and reordered.
    #[arg(long, short)]
    limit: Option<usize>,
    /// Don't show the graph, show a flat list of revisions
    #[arg(long)]
    no_graph: bool,
    /// Render each revision using the given template
    ///
    /// For the syntax, see https://github.com/martinvonz/jj/blob/main/docs/templates.md
    #[arg(long, short = 'T')]
    template: Option<String>,
    /// Show patch
    #[arg(long, short = 'p')]
    patch: bool,
    #[command(flatten)]
    diff_format: DiffFormatArgs,
}

/// Show how a change has evolved
///
/// Show how a change has evolved as it's been updated, rebased, etc.
#[derive(clap::Args, Clone, Debug)]
struct ObslogArgs {
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Limit number of revisions to show
    #[arg(long, short)]
    limit: Option<usize>,
    /// Don't show the graph, show a flat list of revisions
    #[arg(long)]
    no_graph: bool,
    /// Render each revision using the given template
    ///
    /// For the syntax, see https://github.com/martinvonz/jj/blob/main/docs/templates.md
    #[arg(long, short = 'T')]
    template: Option<String>,
    /// Show patch compared to the previous version of this change
    ///
    /// If the previous version has different parents, it will be temporarily
    /// rebased to the parents of the new version, so the diff is not
    /// contaminated by unrelated changes.
    #[arg(long, short = 'p')]
    patch: bool,
    #[command(flatten)]
    diff_format: DiffFormatArgs,
}

/// Compare the changes of two commits
///
/// This excludes changes from other commits by temporarily rebasing `--from`
/// onto `--to`'s parents. If you wish to compare the same change across
/// versions, consider `jj obslog -p` instead.
#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("to_diff").args(&["from", "to"]).multiple(true).required(true)))]
struct InterdiffArgs {
    /// Show changes from this revision
    #[arg(long)]
    from: Option<RevisionArg>,
    /// Show changes to this revision
    #[arg(long)]
    to: Option<RevisionArg>,
    /// Restrict the diff to these paths
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
    #[command(flatten)]
    format: DiffFormatArgs,
}

/// Update the change description or other metadata
///
/// Starts an editor to let you edit the description of a change. The editor
/// will be $EDITOR, or `pico` if that's not defined (`Notepad` on Windows).
#[derive(clap::Args, Clone, Debug)]
#[command(visible_aliases = &["desc"])]
struct DescribeArgs {
    /// The revision whose description to edit
    #[arg(default_value = "@")]
    revision: RevisionArg,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
    /// The change description to use (don't open editor)
    #[arg(long = "message", short, value_name = "MESSAGE")]
    message_paragraphs: Vec<String>,
    /// Read the change description from stdin
    #[arg(long)]
    stdin: bool,
    /// Don't open an editor
    ///
    /// This is mainly useful in combination with e.g. `--reset-author`.
    #[arg(long)]
    no_edit: bool,
    /// Reset the author to the configured user
    ///
    /// This resets the author name, email, and timestamp.
    #[arg(long)]
    reset_author: bool,
}

/// Update the description and create a new change on top.
#[derive(clap::Args, Clone, Debug)]
#[command(visible_aliases=&["ci"])]
struct CommitArgs {
    /// Interactively choose which changes to include in the first commit
    #[arg(short, long)]
    interactive: bool,
    /// The change description to use (don't open editor)
    #[arg(long = "message", short, value_name = "MESSAGE")]
    message_paragraphs: Vec<String>,
    /// Put these paths in the first commit
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

/// Create a new change with the same content as an existing one
#[derive(clap::Args, Clone, Debug)]
struct DuplicateArgs {
    /// The revision(s) to duplicate
    #[arg(default_value = "@")]
    revisions: Vec<RevisionArg>,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
}

/// Abandon a revision
///
/// Abandon a revision, rebasing descendants onto its parent(s). The behavior is
/// similar to `jj restore --changes-in`; the difference is that `jj abandon`
/// gives you a new change, while `jj restore` updates the existing change.
#[derive(clap::Args, Clone, Debug)]
struct AbandonArgs {
    /// The revision(s) to abandon
    #[arg(default_value = "@")]
    revisions: Vec<RevisionArg>,
    /// Do not print every abandoned commit on a separate line
    #[arg(long, short)]
    summary: bool,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
}

/// Edit a commit in the working copy
///
/// Puts the contents of a commit in the working copy for editing. Any changes
/// you make in the working copy will update (amend) the commit.
#[derive(clap::Args, Clone, Debug)]
struct EditArgs {
    /// The commit to edit
    revision: RevisionArg,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
}

/// Create a new, empty change and edit it in the working copy
///
/// Note that you can create a merge commit by specifying multiple revisions as
/// argument. For example, `jj new main @` will create a new commit with the
/// `main` branch and the working copy as parents.
///
/// For more information, see
/// https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("order").args(&["insert_after", "insert_before"])))]
struct NewArgs {
    /// Parent(s) of the new change
    #[arg(default_value = "@")]
    revisions: Vec<RevisionArg>,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
    /// The change description to use
    #[arg(long = "message", short, value_name = "MESSAGE")]
    message_paragraphs: Vec<String>,
    /// Deprecated. Please prefix the revset with `all:` instead.
    #[arg(long, short = 'L', hide = true)]
    allow_large_revsets: bool,
    /// Insert the new change between the target commit(s) and their children
    #[arg(long, short = 'A', visible_alias = "after")]
    insert_after: bool,
    /// Insert the new change between the target commit(s) and their parents
    #[arg(long, short = 'B', visible_alias = "before")]
    insert_before: bool,
}

/// Move the current working copy commit to the next child revision in the
/// repository.
///
///
/// The command moves you to the next child in a linear fashion.
///
///
/// D      D @
/// |      |/
/// C @ => C
/// |/     |
/// B      B
///
///
/// If `--edit` is passed, it will move you directly to the child
/// revision.
///
///
/// D    D
/// |    |
/// C    C
/// |    |
/// B => @
/// |    |
/// @    A
// TODO(#2126): Handle multiple child revisions properly.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
struct NextArgs {
    /// How many revisions to move forward. By default advances to the next
    /// child.
    #[arg(default_value = "1")]
    amount: u64,
    /// Instead of creating a new working-copy commit on top of the target
    /// commit (like `jj new`), edit the target commit directly (like `jj
    /// edit`).
    #[arg(long)]
    edit: bool,
}

/// Move the working copy commit to the parent of the current revision.
///
///
/// The command moves you to the parent in a linear fashion.
///
///
/// D @  D
/// |/   |
/// A => A @
/// |    | /
/// B    B
///
///
/// If `--edit` is passed, it will move the working copy commit
/// directly to the parent.
///
///
/// D @  D
/// |/   |
/// C => @
/// |    |
/// B    B
/// |    |
/// A    A
// TODO(#2126): Handle multiple parents, e.g merges.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
struct PrevArgs {
    /// How many revisions to move backward. By default moves to the parent.
    #[arg(default_value = "1")]
    amount: u64,
    /// Edit the parent directly, instead of moving the working-copy commit.
    #[arg(long)]
    edit: bool,
}

/// Move changes from one revision into another
///
/// Use `--interactive` to move only part of the source revision into the
/// destination. The selected changes (or all the changes in the source revision
/// if not using `--interactive`) will be moved into the destination. The
/// changes will be removed from the source. If that means that the source is
/// now empty compared to its parent, it will be abandoned. Without
/// `--interactive`, the source change will always be empty.
///
/// If the source became empty and both the source and destination had a
/// non-empty description, you will be asked for the combined description. If
/// either was empty, then the other one will be used.
#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("to_move").args(&["from", "to"]).multiple(true).required(true)))]
struct MoveArgs {
    /// Move part of this change into the destination
    #[arg(long)]
    from: Option<RevisionArg>,
    /// Move part of the source into this change
    #[arg(long)]
    to: Option<RevisionArg>,
    /// Interactively choose which parts to move
    #[arg(long, short)]
    interactive: bool,
    /// Move only changes to these paths (instead of all paths)
    #[arg(conflicts_with = "interactive", value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

/// Move changes from a revision into its parent
///
/// After moving the changes into the parent, the child revision will have the
/// same content state as before. If that means that the change is now empty
/// compared to its parent, it will be abandoned.
/// Without `--interactive`, the child change will always be empty.
///
/// If the source became empty and both the source and destination had a
/// non-empty description, you will be asked for the combined description. If
/// either was empty, then the other one will be used.
#[derive(clap::Args, Clone, Debug)]
#[command(visible_alias = "amend")]
struct SquashArgs {
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// The description to use for squashed revision (don't open editor)
    #[arg(long = "message", short, value_name = "MESSAGE")]
    message_paragraphs: Vec<String>,
    /// Interactively choose which parts to squash
    #[arg(long, short)]
    interactive: bool,
    /// Move only changes to these paths (instead of all paths)
    #[arg(conflicts_with = "interactive", value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

/// Move changes from a revision's parent into the revision
///
/// After moving the changes out of the parent, the child revision will have the
/// same content state as before. If moving the change out of the parent change
/// made it empty compared to its parent, it will be abandoned. Without
/// `--interactive`, the parent change will always become empty.
///
/// If the source became empty and both the source and destination had a
/// non-empty description, you will be asked for the combined description. If
/// either was empty, then the other one will be used.
#[derive(clap::Args, Clone, Debug)]
#[command(visible_alias = "unamend")]
struct UnsquashArgs {
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Interactively choose which parts to unsquash
    // TODO: It doesn't make much sense to run this without -i. We should make that
    // the default.
    #[arg(long, short)]
    interactive: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
enum ChmodMode {
    /// Make a path non-executable (alias: normal)
    // We use short names for enum values so that errors say that the possible values are `n, x`.
    #[value(name = "n", alias("normal"))]
    Normal,
    /// Make a path executable (alias: executable)
    #[value(name = "x", alias("executable"))]
    Executable,
}

/// Sets or removes the executable bit for paths in the repo
///
/// Unlike the POSIX `chmod`, `jj chmod` also works on Windows, on conflicted
/// files, and on arbitrary revisions.
#[derive(clap::Args, Clone, Debug)]
struct ChmodArgs {
    mode: ChmodMode,
    /// The revision to update
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Paths to change the executable bit for
    #[arg(required = true, value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

/// Resolve a conflicted file with an external merge tool
///
/// Only conflicts that can be resolved with a 3-way merge are supported. See
/// docs for merge tool configuration instructions.
///
/// Note that conflicts can also be resolved without using this command. You may
/// edit the conflict markers in the conflicted file directly with a text
/// editor.
//  TODOs:
//   - `jj resolve --editor` to resolve a conflict in the default text editor. Should work for
//     conflicts with 3+ adds. Useful to resolve conflicts in a commit other than the current one.
//   - A way to help split commits with conflicts that are too complicated (more than two sides)
//     into commits with simpler conflicts. In case of a tree with many merges, we could for example
//     point to existing commits with simpler conflicts where resolving those conflicts would help
//     simplify the present one.
#[derive(clap::Args, Clone, Debug)]
struct ResolveArgs {
    #[arg(long, short, default_value = "@")]
    revision: String,
    /// Instead of resolving one conflict, list all the conflicts
    // TODO: Also have a `--summary` option. `--list` currently acts like
    // `diff --summary`, but should be more verbose.
    #[arg(long, short)]
    list: bool,
    /// Do not print the list of remaining conflicts (if any) after resolving a
    /// conflict
    #[arg(long, short, conflicts_with = "list")]
    quiet: bool,
    /// Restrict to these paths when searching for a conflict to resolve. We
    /// will attempt to resolve the first conflict we can find. You can use
    /// the `--list` argument to find paths to use here.
    // TODO: Find the conflict we can resolve even if it's not the first one.
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

/// Restore paths from another revision
///
/// That means that the paths get the same content in the destination (`--to`)
/// as they had in the source (`--from`). This is typically used for undoing
/// changes to some paths in the working copy (`jj restore <paths>`).
///
/// If only one of `--from` or `--to` is specified, the other one defaults to
/// the working copy.
///
/// When neither `--from` nor `--to` is specified, the command restores into the
/// working copy from its parent(s). `jj restore` without arguments is similar
/// to `jj abandon`, except that it leaves an empty revision with its
/// description and other metadata preserved.
///
/// See `jj diffedit` if you'd like to restore portions of files rather than
/// entire files.
#[derive(clap::Args, Clone, Debug)]
struct RestoreArgs {
    /// Restore only these paths (instead of all paths)
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
    /// Revision to restore from (source)
    #[arg(long)]
    from: Option<RevisionArg>,
    /// Revision to restore into (destination)
    #[arg(long)]
    to: Option<RevisionArg>,
    /// Undo the changes in a revision as compared to the merge of its parents.
    ///
    /// This undoes the changes that can be seen with `jj diff -r REVISION`. If
    /// `REVISION` only has a single parent, this option is equivalent to `jj
    ///  restore --to REVISION --from REVISION-`.
    ///
    /// The default behavior of `jj restore` is equivalent to `jj restore
    /// --changes-in @`.
    #[arg(long, short, value_name="REVISION", conflicts_with_all=["to", "from"])]
    changes_in: Option<RevisionArg>,
    /// Prints an error. DO NOT USE.
    ///
    /// If we followed the pattern of `jj diff` and `jj diffedit`, we would use
    /// `--revision` instead of `--changes-in` However, that would make it
    /// likely that someone unfamiliar with this pattern would use `-r` when
    /// they wanted `--from`. This would make a different revision empty, and
    /// the user might not even realize something went wrong.
    #[arg(long, short, hide = true)]
    revision: Option<RevisionArg>,
}

/// Run a command across a set of revisions.
///
///
/// All recorded state will be persisted in the `.jj` directory, so occasionally
/// a `jj run --clean` is needed to clean up disk space.
///
/// # Example
///
/// # Run pre-commit on your local work
/// $ jj run 'pre-commit.py .github/pre-commit.yaml' -r (main..@) -j 4
///
/// This allows pre-commit integration and other funny stuff.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
struct RunArgs {
    /// The command to run across all selected revisions.
    #[arg(long, short, alias = "x")]
    command: String,
    /// The revisions to change.
    #[arg(long, short, default_value = "@")]
    revisions: Vec<RevisionArg>,
}

/// Touch up the content changes in a revision with a diff editor
///
/// With the `-r` option, which is the default, starts a diff editor (`meld` by
/// default) on the changes in the revision.
///
/// With the `--from` and/or `--to` options, starts a diff editor comparing the
/// "from" revision to the "to" revision.
///
/// Edit the right side of the diff until it looks the way you want. Once you
/// close the editor, the revision specified with `-r` or `--to` will be
/// updated. Descendants will be rebased on top as usual, which may result in
/// conflicts.
///
/// See `jj restore` if you want to move entire files from one revision to
/// another. See `jj squash -i` or `jj unsquash -i` if you instead want to move
/// changes into or out of the parent revision.
#[derive(clap::Args, Clone, Debug)]
struct DiffeditArgs {
    /// The revision to touch up. Defaults to @ if neither --to nor --from are
    /// specified.
    #[arg(long, short)]
    revision: Option<RevisionArg>,
    /// Show changes from this revision. Defaults to @ if --to is specified.
    #[arg(long, conflicts_with = "revision")]
    from: Option<RevisionArg>,
    /// Edit changes in this revision. Defaults to @ if --from is specified.
    #[arg(long, conflicts_with = "revision")]
    to: Option<RevisionArg>,
}

/// Split a revision in two
///
/// Starts a diff editor (`meld` by default) on the changes in the revision.
/// Edit the right side of the diff until it has the content you want in the
/// first revision. Once you close the editor, your edited content will replace
/// the previous revision. The remaining changes will be put in a new revision
/// on top.
///
/// If the change you split had a description, you will be asked to enter a
/// change description for each commit. If the change did not have a
/// description, the second part will not get a description, and you will be
/// asked for a description only for the first part.
#[derive(clap::Args, Clone, Debug)]
struct SplitArgs {
    /// Interactively choose which parts to split. This is the default if no
    /// paths are provided.
    #[arg(long, short)]
    interactive: bool,
    /// The revision to split
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// Put these paths in the first commit
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

/// Move revisions to different parent(s)
///
/// There are three different ways of specifying which revisions to rebase:
/// `-b` to rebase a whole branch, `-s` to rebase a revision and its
/// descendants, and `-r` to rebase a single commit. If none of them is
/// specified, it defaults to `-b @`.
///
/// With `-s`, the command rebases the specified revision and its descendants
/// onto the destination. For example, `jj rebase -s M -d O` would transform
/// your history like this (letters followed by an apostrophe are post-rebase
/// versions):
///
/// O           N'
/// |           |
/// | N         M'
/// | |         |
/// | M         O
/// | |    =>   |
/// | | L       | L
/// | |/        | |
/// | K         | K
/// |/          |/
/// J           J
///
/// With `-b`, the command rebases the whole "branch" containing the specified
/// revision. A "branch" is the set of commits that includes:
///
/// * the specified revision and ancestors that are not also ancestors of the
///   destination
/// * all descendants of those commits
///
/// In other words, `jj rebase -b X -d Y` rebases commits in the revset
/// `(Y..X)::` (which is equivalent to `jj rebase -s 'roots(Y..X)' -d Y` for a
/// single root). For example, either `jj rebase -b L -d O` or `jj rebase -b M
/// -d O` would transform your history like this (because `L` and `M` are on the
/// same "branch", relative to the destination):
///
/// O           N'
/// |           |
/// | N         M'
/// | |         |
/// | M         | L'
/// | |    =>   |/
/// | | L       K'
/// | |/        |
/// | K         O
/// |/          |
/// J           J
///
/// With `-r`, the command rebases only the specified revision onto the
/// destination. Any "hole" left behind will be filled by rebasing descendants
/// onto the specified revision's parent(s). For example, `jj rebase -r K -d M`
/// would transform your history like this:
///
/// M          K'
/// |          |
/// | L        M
/// | |   =>   |
/// | K        | L'
/// |/         |/
/// J          J
///
/// Note that you can create a merge commit by repeating the `-d` argument.
/// For example, if you realize that commit L actually depends on commit M in
/// order to work (in addition to its current parent K), you can run `jj rebase
/// -s L -d K -d M`:
///
/// M          L'
/// |          |\
/// | L        M |
/// | |   =>   | |
/// | K        | K
/// |/         |/
/// J          J
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
#[command(group(ArgGroup::new("to_rebase").args(&["branch", "source", "revision"])))]
struct RebaseArgs {
    /// Rebase the whole branch relative to destination's ancestors (can be
    /// repeated)
    ///
    /// `jj rebase -b=br -d=dst` is equivalent to `jj rebase '-s=roots(dst..br)'
    /// -d=dst`.
    ///
    /// If none of `-b`, `-s`, or `-r` is provided, then the default is `-b @`.
    #[arg(long, short)]
    branch: Vec<RevisionArg>,

    /// Rebase specified revision(s) together their tree of descendants (can be
    /// repeated)
    ///
    /// Each specified revision will become a direct child of the destination
    /// revision(s), even if some of the source revisions are descendants
    /// of others.
    ///
    /// If none of `-b`, `-s`, or `-r` is provided, then the default is `-b @`.
    #[arg(long, short)]
    source: Vec<RevisionArg>,
    /// Rebase only this revision, rebasing descendants onto this revision's
    /// parent(s)
    ///
    /// If none of `-b`, `-s`, or `-r` is provided, then the default is `-b @`.
    #[arg(long, short)]
    revision: Option<RevisionArg>,
    /// The revision(s) to rebase onto (can be repeated to create a merge
    /// commit)
    #[arg(long, short, required = true)]
    destination: Vec<RevisionArg>,
    /// Deprecated. Please prefix the revset with `all:` instead.
    #[arg(long, short = 'L', hide = true)]
    allow_large_revsets: bool,
}

/// Apply the reverse of a revision on top of another revision
#[derive(clap::Args, Clone, Debug)]
struct BackoutArgs {
    /// The revision to apply the reverse of
    #[arg(long, short, default_value = "@")]
    revision: RevisionArg,
    /// The revision to apply the reverse changes on top of
    // TODO: It seems better to default this to `@-`. Maybe the working
    // copy should be rebased on top?
    #[arg(long, short, default_value = "@")]
    destination: Vec<RevisionArg>,
}

/// Commands for working with workspaces
///
/// Workspaces let you add additional working copies attached to the same repo.
/// A common use case is so you can run a slow build or test in one workspace
/// while you're continuing to write code in another workspace.
///
/// Each workspace has its own working-copy commit. When you have more than one
/// workspace attached to a repo, they are indicated by `@<workspace name>` in
/// `jj log`.
#[derive(Subcommand, Clone, Debug)]
enum WorkspaceCommands {
    Add(WorkspaceAddArgs),
    Forget(WorkspaceForgetArgs),
    List(WorkspaceListArgs),
    Root(WorkspaceRootArgs),
    UpdateStale(WorkspaceUpdateStaleArgs),
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
    #[arg(long)]
    name: Option<String>,
    /// The revision that the workspace should be created at; a new working copy
    /// commit will be created on top of it.
    #[arg(long, short)]
    revision: Option<RevisionArg>,
}

/// Stop tracking a workspace's working-copy commit in the repo
///
/// The workspace will not be touched on disk. It can be deleted from disk
/// before or after running this command.
#[derive(clap::Args, Clone, Debug)]
struct WorkspaceForgetArgs {
    /// Names of the workspaces to forget. By default, forgets only the current
    /// workspace.
    workspaces: Vec<String>,
}

/// List workspaces
#[derive(clap::Args, Clone, Debug)]
struct WorkspaceListArgs {}

/// Show the current workspace root directory
#[derive(clap::Args, Clone, Debug)]
struct WorkspaceRootArgs {}

/// Update a workspace that has become stale
///
/// For information about stale working copies, see
/// https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
struct WorkspaceUpdateStaleArgs {}

/// Manage which paths from the working-copy commit are present in the working
/// copy
#[derive(Subcommand, Clone, Debug)]
enum SparseArgs {
    List(SparseListArgs),
    Set(SparseSetArgs),
}

/// List the patterns that are currently present in the working copy
///
/// By default, a newly cloned or initialized repo will have have a pattern
/// matching all files from the repo root. That pattern is rendered as `.` (a
/// single period).
#[derive(clap::Args, Clone, Debug)]
struct SparseListArgs {}

/// Update the patterns that are present in the working copy
///
/// For example, if all you need is the `README.md` and the `lib/`
/// directory, use `jj sparse set --clear --add README.md --add lib`.
/// If you no longer need the `lib` directory, use `jj sparse set --remove lib`.
#[derive(clap::Args, Clone, Debug)]
struct SparseSetArgs {
    /// Patterns to add to the working copy
    #[arg(long, value_hint = clap::ValueHint::AnyPath)]
    add: Vec<String>,
    /// Patterns to remove from the working copy
    #[arg(long, conflicts_with = "clear", value_hint = clap::ValueHint::AnyPath)]
    remove: Vec<String>,
    /// Include no files in the working copy (combine with --add)
    #[arg(long)]
    clear: bool,
    /// Edit patterns with $EDITOR
    #[arg(long)]
    edit: bool,
    /// Include all files in the working copy
    #[arg(long, conflicts_with_all = &["add", "remove", "clear"])]
    reset: bool,
}

/// Infrequently used commands such as for generating shell completions
#[derive(Subcommand, Clone, Debug)]
enum UtilCommands {
    Completion(UtilCompletionArgs),
    Mangen(UtilMangenArgs),
    ConfigSchema(UtilConfigSchemaArgs),
}

/// Print a command-line-completion script
#[derive(clap::Args, Clone, Debug)]
struct UtilCompletionArgs {
    /// Print a completion script for Bash
    ///
    /// Apply it by running this:
    ///
    /// source <(jj util completion)
    #[arg(long, verbatim_doc_comment)]
    bash: bool,
    /// Print a completion script for Fish
    ///
    /// Apply it by running this:
    ///
    /// jj util completion --fish | source
    #[arg(long, verbatim_doc_comment)]
    fish: bool,
    /// Print a completion script for Zsh
    ///
    /// Apply it by running this:
    ///
    /// autoload -U compinit
    /// compinit
    /// source <(jj util completion --zsh)
    #[arg(long, verbatim_doc_comment)]
    zsh: bool,
}

/// Print a ROFF (manpage)
#[derive(clap::Args, Clone, Debug)]
struct UtilMangenArgs {}

/// Print the JSON schema for the jj TOML config format.
#[derive(clap::Args, Clone, Debug)]
struct UtilConfigSchemaArgs {}

#[instrument(skip_all)]
fn cmd_version(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &VersionArgs,
) -> Result<(), CommandError> {
    write!(ui.stdout(), "{}", command.app().render_version())?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_init(ui: &mut Ui, command: &CommandHelper, args: &InitArgs) -> Result<(), CommandError> {
    if command.global_args().repository.is_some() {
        return Err(user_error("'--repository' cannot be used with 'init'"));
    }
    let wc_path = command.cwd().join(&args.destination);
    match fs::create_dir(&wc_path) {
        Ok(()) => {}
        Err(_) if wc_path.is_dir() => {}
        Err(e) => return Err(user_error(format!("Failed to create workspace: {e}"))),
    }
    let wc_path = wc_path
        .canonicalize()
        .map_err(|e| user_error(format!("Failed to create workspace: {e}")))?; // raced?

    if let Some(git_store_str) = &args.git_repo {
        let mut git_store_path = command.cwd().join(git_store_str);
        git_store_path = git_store_path
            .canonicalize()
            .map_err(|_| user_error(format!("{} doesn't exist", git_store_path.display())))?;
        if !git_store_path.ends_with(".git") {
            git_store_path.push(".git");
            // Undo if .git doesn't exist - likely a bare repo.
            if !git_store_path.exists() {
                git_store_path.pop();
            }
        }
        let (workspace, repo) =
            Workspace::init_external_git(command.settings(), &wc_path, &git_store_path)?;
        let git_repo = repo
            .store()
            .backend_impl()
            .downcast_ref::<GitBackend>()
            .unwrap()
            .open_git_repo()?;
        let mut workspace_command = command.for_loaded_repo(ui, workspace, repo)?;
        workspace_command.snapshot(ui)?;
        if workspace_command.working_copy_shared_with_git() {
            git::add_to_git_exclude(ui, &git_repo)?;
        } else {
            let mut tx = workspace_command.start_transaction("import git refs");
            let stats = jj_lib::git::import_some_refs(
                tx.mut_repo(),
                &git_repo,
                &command.settings().git_settings(),
                |ref_name| !jj_lib::git::is_reserved_git_remote_ref(ref_name),
            )?;
            print_git_import_stats(ui, &stats)?;
            if let Some(git_head_id) = tx.mut_repo().view().git_head().as_normal().cloned() {
                let git_head_commit = tx.mut_repo().store().get_commit(&git_head_id)?;
                tx.check_out(&git_head_commit)?;
            }
            if tx.mut_repo().has_changes() {
                tx.finish(ui)?;
            }
        }
    } else if args.git {
        Workspace::init_internal_git(command.settings(), &wc_path)?;
    } else {
        if !command.settings().allow_native_backend() {
            return Err(user_error_with_hint(
                "The native backend is disallowed by default.",
                "Did you mean to pass `--git`?
Set `ui.allow-init-native` to allow initializing a repo with the native backend.",
            ));
        }
        Workspace::init_local(command.settings(), &wc_path)?;
    };
    let cwd = command.cwd().canonicalize().unwrap();
    let relative_wc_path = file_util::relative_path(&cwd, &wc_path);
    writeln!(
        ui.stderr(),
        "Initialized repo in \"{}\"",
        relative_wc_path.display()
    )?;
    if args.git && wc_path.join(".git").exists() {
        writeln!(ui.warning(), "Empty repo created.")?;
        writeln!(
            ui.hint(),
            "Hint: To create a repo backed by the existing Git repo, run `jj init --git-repo={}` \
             instead.",
            relative_wc_path.display()
        )?;
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_config(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &ConfigSubcommand,
) -> Result<(), CommandError> {
    match subcommand {
        ConfigSubcommand::List(sub_args) => cmd_config_list(ui, command, sub_args),
        ConfigSubcommand::Get(sub_args) => cmd_config_get(ui, command, sub_args),
        ConfigSubcommand::Set(sub_args) => cmd_config_set(ui, command, sub_args),
        ConfigSubcommand::Edit(sub_args) => cmd_config_edit(ui, command, sub_args),
    }
}

#[instrument(skip_all)]
fn cmd_config_list(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigListArgs,
) -> Result<(), CommandError> {
    ui.request_pager();
    let name_path = args
        .name
        .as_ref()
        .map_or(vec![], |name| name.split('.').collect_vec());
    let values = command.resolved_config_values(&name_path)?;
    let mut wrote_values = false;
    for AnnotatedValue {
        path,
        value,
        source,
        is_overridden,
    } in &values
    {
        // Remove overridden values.
        // TODO(#1047): Allow printing overridden values via `--include-overridden`.
        if *is_overridden {
            continue;
        }
        // Skip built-ins if not included.
        if !args.include_defaults && *source == ConfigSource::Default {
            continue;
        }
        writeln!(
            ui.stdout(),
            "{}={}",
            path.join("."),
            serialize_config_value(value)
        )?;
        wrote_values = true;
    }
    if !wrote_values {
        // Note to stderr explaining why output is empty.
        if let Some(name) = &args.name {
            writeln!(ui.warning(), "No matching config key for {name}")?;
        } else {
            writeln!(ui.warning(), "No config to list")?;
        }
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_config_get(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigGetArgs,
) -> Result<(), CommandError> {
    let value = command
        .settings()
        .config()
        .get_string(&args.name)
        .map_err(|err| match err {
            config::ConfigError::Type {
                origin,
                unexpected,
                expected,
                key,
            } => {
                let expected = format!("a value convertible to {expected}");
                // Copied from `impl fmt::Display for ConfigError`. We can't use
                // the `Display` impl directly because `expected` is required to
                // be a `'static str`.
                let mut buf = String::new();
                use std::fmt::Write;
                write!(buf, "invalid type: {unexpected}, expected {expected}").unwrap();
                if let Some(key) = key {
                    write!(buf, " for key `{key}`").unwrap();
                }
                if let Some(origin) = origin {
                    write!(buf, " in {origin}").unwrap();
                }
                CommandError::ConfigError(buf.to_string())
            }
            err => err.into(),
        })?;
    writeln!(ui.stdout(), "{value}")?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_config_set(
    _ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigSetArgs,
) -> Result<(), CommandError> {
    let config_path = get_new_config_file_path(&args.config_args.get_source_kind(), command)?;
    if config_path.is_dir() {
        return Err(user_error(format!(
            "Can't set config in path {path} (dirs not supported)",
            path = config_path.display()
        )));
    }
    write_config_value_to_file(&args.name, &args.value, &config_path)
}

#[instrument(skip_all)]
fn cmd_config_edit(
    _ui: &mut Ui,
    command: &CommandHelper,
    args: &ConfigEditArgs,
) -> Result<(), CommandError> {
    let config_path = get_new_config_file_path(&args.config_args.get_source_kind(), command)?;
    run_ui_editor(command.settings(), &config_path)
}

#[instrument(skip_all)]
fn cmd_checkout(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &CheckoutArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let target = workspace_command.resolve_single_rev(&args.revision, ui)?;
    let mut tx =
        workspace_command.start_transaction(&format!("check out commit {}", target.id().hex()));
    let commit_builder = tx
        .mut_repo()
        .new_commit(
            command.settings(),
            vec![target.id().clone()],
            target.tree_id().clone(),
        )
        .set_description(cli_util::join_message_paragraphs(&args.message_paragraphs));
    let new_commit = commit_builder.write()?;
    tx.edit(&new_commit).unwrap();
    tx.finish(ui)?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_untrack(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &UntrackArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let store = workspace_command.repo().store().clone();
    let matcher = workspace_command.matcher_from_values(&args.paths)?;

    let mut tx = workspace_command
        .start_transaction("untrack paths")
        .into_inner();
    let base_ignores = workspace_command.base_ignores();
    let (mut locked_ws, wc_commit) = workspace_command.start_working_copy_mutation()?;
    // Create a new tree without the unwanted files
    let mut tree_builder = MergedTreeBuilder::new(wc_commit.tree_id().clone());
    let wc_tree = wc_commit.tree()?;
    for (path, _value) in wc_tree.entries_matching(matcher.as_ref()) {
        tree_builder.set_or_remove(path, Merge::absent());
    }
    let new_tree_id = tree_builder.write_tree(&store)?;
    let new_tree = store.get_root_tree(&new_tree_id)?;
    // Reset the working copy to the new tree
    locked_ws.locked_wc().reset(&new_tree)?;
    // Commit the working copy again so we can inform the user if paths couldn't be
    // untracked because they're not ignored.
    let wc_tree_id = locked_ws.locked_wc().snapshot(SnapshotOptions {
        base_ignores,
        fsmonitor_kind: command.settings().fsmonitor_kind()?,
        progress: None,
        max_new_file_size: command.settings().max_new_file_size()?,
    })?;
    if wc_tree_id != new_tree_id {
        let wc_tree = store.get_root_tree(&wc_tree_id)?;
        let added_back = wc_tree.entries_matching(matcher.as_ref()).collect_vec();
        if !added_back.is_empty() {
            drop(locked_ws);
            let path = &added_back[0].0;
            let ui_path = workspace_command.format_file_path(path);
            let message = if added_back.len() > 1 {
                format!(
                    "'{}' and {} other files are not ignored.",
                    ui_path,
                    added_back.len() - 1
                )
            } else {
                format!("'{ui_path}' is not ignored.")
            };
            return Err(user_error_with_hint(
                message,
                "Files that are not ignored will be added back by the next command.
Make sure they're ignored, then try again.",
            ));
        } else {
            // This means there were some concurrent changes made in the working copy. We
            // don't want to mix those in, so reset the working copy again.
            locked_ws.locked_wc().reset(&new_tree)?;
        }
    }
    tx.mut_repo()
        .rewrite_commit(command.settings(), &wc_commit)
        .set_tree_id(new_tree_id)
        .write()?;
    let num_rebased = tx.mut_repo().rebase_descendants(command.settings())?;
    if num_rebased > 0 {
        writeln!(ui.stderr(), "Rebased {num_rebased} descendant commits")?;
    }
    let repo = tx.commit();
    locked_ws.finish(repo.op_id().clone())?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_files(ui: &mut Ui, command: &CommandHelper, args: &FilesArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    let tree = commit.tree()?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    ui.request_pager();
    for (name, _value) in tree.entries_matching(matcher.as_ref()) {
        writeln!(
            ui.stdout(),
            "{}",
            &workspace_command.format_file_path(&name)
        )?;
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_cat(ui: &mut Ui, command: &CommandHelper, args: &CatArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    let tree = commit.tree()?;
    let path = workspace_command.parse_file_path(&args.path)?;
    let repo = workspace_command.repo();
    match tree.path_value(&path).into_resolved() {
        Ok(None) => {
            return Err(user_error("No such path"));
        }
        Ok(Some(TreeValue::File { id, .. })) => {
            let mut contents = repo.store().read_file(&path, &id)?;
            ui.request_pager();
            std::io::copy(&mut contents, &mut ui.stdout_formatter().as_mut())?;
        }
        Err(conflict) => {
            let mut contents = vec![];
            conflicts::materialize(&conflict, repo.store(), &path, &mut contents).unwrap();
            ui.request_pager();
            ui.stdout_formatter().write_all(&contents)?;
        }
        _ => {
            return Err(user_error("Path exists but is not a file"));
        }
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_diff(ui: &mut Ui, command: &CommandHelper, args: &DiffArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let from_tree;
    let to_tree;
    if args.from.is_some() || args.to.is_some() {
        let from = workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"), ui)?;
        from_tree = from.tree()?;
        let to = workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"), ui)?;
        to_tree = to.tree()?;
    } else {
        let commit =
            workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"), ui)?;
        let parents = commit.parents();
        from_tree = merge_commit_trees(workspace_command.repo().as_ref(), &parents)?;
        to_tree = commit.tree()?
    }
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let diff_formats = diff_util::diff_formats_for(command.settings(), &args.format)?;
    ui.request_pager();
    diff_util::show_diff(
        ui,
        ui.stdout_formatter().as_mut(),
        &workspace_command,
        &from_tree,
        &to_tree,
        matcher.as_ref(),
        &diff_formats,
    )?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_show(ui: &mut Ui, command: &CommandHelper, args: &ShowArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    let template_string = command.settings().config().get_string("templates.show")?;
    let template = workspace_command.parse_commit_template(&template_string)?;
    let diff_formats = diff_util::diff_formats_for(command.settings(), &args.format)?;
    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();
    template.format(&commit, formatter)?;
    diff_util::show_patch(
        ui,
        formatter,
        &workspace_command,
        &commit,
        &EverythingMatcher,
        &diff_formats,
    )?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_status(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &StatusArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let maybe_wc_commit = workspace_command
        .get_wc_commit_id()
        .map(|id| repo.store().get_commit(id))
        .transpose()?;
    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();

    if let Some(wc_commit) = &maybe_wc_commit {
        let parent_tree = merge_commit_trees(repo.as_ref(), &wc_commit.parents())?;
        let tree = wc_commit.tree()?;
        if tree.id() == parent_tree.id() {
            formatter.write_str("The working copy is clean\n")?;
        } else {
            formatter.write_str("Working copy changes:\n")?;
            diff_util::show_diff_summary(
                formatter,
                &workspace_command,
                parent_tree.diff(&tree, &EverythingMatcher),
            )?;
        }

        let conflicts = wc_commit.tree()?.conflicts().collect_vec();
        if !conflicts.is_empty() {
            writeln!(
                formatter.labeled("conflict"),
                "There are unresolved conflicts at these paths:"
            )?;
            print_conflicted_paths(&conflicts, formatter, &workspace_command)?
        }

        formatter.write_str("Working copy : ")?;
        formatter.with_label("working_copy", |fmt| {
            workspace_command.write_commit_summary(fmt, wc_commit)
        })?;
        formatter.write_str("\n")?;
        for parent in wc_commit.parents() {
            formatter.write_str("Parent commit: ")?;
            workspace_command.write_commit_summary(formatter, &parent)?;
            formatter.write_str("\n")?;
        }
    } else {
        formatter.write_str("No working copy\n")?;
    }

    let conflicted_local_branches = repo
        .view()
        .local_branches()
        .filter(|(_, target)| target.has_conflict())
        .map(|(branch_name, _)| branch_name)
        .collect_vec();
    let conflicted_remote_branches = repo
        .view()
        .all_remote_branches()
        .filter(|(_, remote_ref)| remote_ref.target.has_conflict())
        .map(|(full_name, _)| full_name)
        .collect_vec();
    if !conflicted_local_branches.is_empty() {
        writeln!(
            formatter.labeled("conflict"),
            "These branches have conflicts:"
        )?;
        for branch_name in conflicted_local_branches {
            write!(formatter, "  ")?;
            write!(formatter.labeled("branch"), "{branch_name}")?;
            writeln!(formatter)?;
        }
        writeln!(
            formatter,
            "  Use `jj branch list` to see details. Use `jj branch set <name> -r <rev>` to \
             resolve."
        )?;
    }
    if !conflicted_remote_branches.is_empty() {
        writeln!(
            formatter.labeled("conflict"),
            "These remote branches have conflicts:"
        )?;
        for (branch_name, remote_name) in conflicted_remote_branches {
            write!(formatter, "  ")?;
            write!(formatter.labeled("branch"), "{branch_name}@{remote_name}")?;
            writeln!(formatter)?;
        }
        writeln!(
            formatter,
            "  Use `jj branch list` to see details. Use `jj git fetch` to resolve."
        )?;
    }

    Ok(())
}

#[instrument(skip_all)]
fn cmd_log(ui: &mut Ui, command: &CommandHelper, args: &LogArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;

    let revset_expression = {
        let mut expression = if args.revisions.is_empty() {
            workspace_command.parse_revset(&command.settings().default_revset(), Some(ui))?
        } else {
            let expressions: Vec<_> = args
                .revisions
                .iter()
                .map(|revision_str| workspace_command.parse_revset(revision_str, Some(ui)))
                .try_collect()?;
            RevsetExpression::union_all(&expressions)
        };
        if !args.paths.is_empty() {
            let repo_paths: Vec<_> = args
                .paths
                .iter()
                .map(|path_arg| workspace_command.parse_file_path(path_arg))
                .try_collect()?;
            expression = expression.intersection(&RevsetExpression::filter(
                RevsetFilterPredicate::File(Some(repo_paths)),
            ));
        }
        revset::optimize(expression)
    };
    let repo = workspace_command.repo();
    let wc_commit_id = workspace_command.get_wc_commit_id();
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let revset = workspace_command.evaluate_revset(revset_expression)?;

    let store = repo.store();
    let diff_formats =
        diff_util::diff_formats_for_log(command.settings(), &args.diff_format, args.patch)?;

    let template_string = match &args.template {
        Some(value) => value.to_string(),
        None => command.settings().config().get_string("templates.log")?,
    };
    let template = workspace_command.parse_commit_template(&template_string)?;
    let with_content_format = LogContentFormat::new(ui, command.settings())?;

    {
        ui.request_pager();
        let mut formatter = ui.stdout_formatter();
        let formatter = formatter.as_mut();

        if !args.no_graph {
            let mut graph = get_graphlog(command.settings(), formatter.raw());
            let default_node_symbol = graph.default_node_symbol().to_owned();
            let forward_iter = TopoGroupedRevsetGraphIterator::new(revset.iter_graph());
            let iter: Box<dyn Iterator<Item = _>> = if args.reversed {
                Box::new(ReverseRevsetGraphIterator::new(forward_iter))
            } else {
                Box::new(forward_iter)
            };
            for (commit_id, edges) in iter.take(args.limit.unwrap_or(usize::MAX)) {
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
                let commit = store.get_commit(&commit_id)?;
                with_content_format.write_graph_text(
                    ui.new_formatter(&mut buffer).as_mut(),
                    |formatter| template.format(&commit, formatter),
                    || graph.width(&commit_id, &graphlog_edges),
                )?;
                if !buffer.ends_with(b"\n") {
                    buffer.push(b'\n');
                }
                if !diff_formats.is_empty() {
                    let mut formatter = ui.new_formatter(&mut buffer);
                    diff_util::show_patch(
                        ui,
                        formatter.as_mut(),
                        &workspace_command,
                        &commit,
                        matcher.as_ref(),
                        &diff_formats,
                    )?;
                }
                let node_symbol = if Some(&commit_id) == wc_commit_id {
                    "@"
                } else {
                    &default_node_symbol
                };

                graph.add_node(
                    &commit_id,
                    &graphlog_edges,
                    node_symbol,
                    &String::from_utf8_lossy(&buffer),
                )?;
            }
        } else {
            let iter: Box<dyn Iterator<Item = CommitId>> = if args.reversed {
                Box::new(revset.iter().reversed())
            } else {
                Box::new(revset.iter())
            };
            for commit_or_error in iter.commits(store).take(args.limit.unwrap_or(usize::MAX)) {
                let commit = commit_or_error?;
                with_content_format
                    .write(formatter, |formatter| template.format(&commit, formatter))?;
                if !diff_formats.is_empty() {
                    diff_util::show_patch(
                        ui,
                        formatter,
                        &workspace_command,
                        &commit,
                        matcher.as_ref(),
                        &diff_formats,
                    )?;
                }
            }
        }
    }

    // Check to see if the user might have specified a path when they intended
    // to specify a revset.
    if let ([], [only_path]) = (args.revisions.as_slice(), args.paths.as_slice()) {
        if only_path == "." && workspace_command.parse_file_path(only_path)?.is_root() {
            // For users of e.g. Mercurial, where `.` indicates the current commit.
            writeln!(
                ui.warning(),
                "warning: The argument {only_path:?} is being interpreted as a path, but this is \
                 often not useful because all non-empty commits touch '.'.  If you meant to show \
                 the working copy commit, pass -r '@' instead."
            )?;
        } else if revset.is_empty()
            && revset::parse(only_path, &workspace_command.revset_parse_context()).is_ok()
        {
            writeln!(
                ui.warning(),
                "warning: The argument {only_path:?} is being interpreted as a path. To specify a \
                 revset, pass -r {only_path:?} instead."
            )?;
        }
    }

    Ok(())
}

#[instrument(skip_all)]
fn cmd_obslog(ui: &mut Ui, command: &CommandHelper, args: &ObslogArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;

    let start_commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    let wc_commit_id = workspace_command.get_wc_commit_id();

    let diff_formats =
        diff_util::diff_formats_for_log(command.settings(), &args.diff_format, args.patch)?;

    let template_string = match &args.template {
        Some(value) => value.to_string(),
        None => command.settings().config().get_string("templates.log")?,
    };
    let template = workspace_command.parse_commit_template(&template_string)?;
    let with_content_format = LogContentFormat::new(ui, command.settings())?;

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();
    formatter.push_label("log")?;

    let mut commits = topo_order_reverse(
        vec![start_commit],
        |commit: &Commit| commit.id().clone(),
        |commit: &Commit| commit.predecessors(),
    );
    if let Some(n) = args.limit {
        commits.truncate(n);
    }
    if !args.no_graph {
        let mut graph = get_graphlog(command.settings(), formatter.raw());
        let default_node_symbol = graph.default_node_symbol().to_owned();
        for commit in commits {
            let mut edges = vec![];
            for predecessor in &commit.predecessors() {
                edges.push(Edge::direct(predecessor.id().clone()));
            }
            let mut buffer = vec![];
            with_content_format.write_graph_text(
                ui.new_formatter(&mut buffer).as_mut(),
                |formatter| template.format(&commit, formatter),
                || graph.width(commit.id(), &edges),
            )?;
            if !buffer.ends_with(b"\n") {
                buffer.push(b'\n');
            }
            if !diff_formats.is_empty() {
                let mut formatter = ui.new_formatter(&mut buffer);
                show_predecessor_patch(
                    ui,
                    formatter.as_mut(),
                    &workspace_command,
                    &commit,
                    &diff_formats,
                )?;
            }
            let node_symbol = if Some(commit.id()) == wc_commit_id {
                "@"
            } else {
                &default_node_symbol
            };
            graph.add_node(
                commit.id(),
                &edges,
                node_symbol,
                &String::from_utf8_lossy(&buffer),
            )?;
        }
    } else {
        for commit in commits {
            with_content_format
                .write(formatter, |formatter| template.format(&commit, formatter))?;
            if !diff_formats.is_empty() {
                show_predecessor_patch(ui, formatter, &workspace_command, &commit, &diff_formats)?;
            }
        }
    }

    Ok(())
}

fn show_predecessor_patch(
    ui: &Ui,
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    commit: &Commit,
    diff_formats: &[DiffFormat],
) -> Result<(), CommandError> {
    let predecessors = commit.predecessors();
    let predecessor = match predecessors.first() {
        Some(predecessor) => predecessor,
        None => return Ok(()),
    };
    let predecessor_tree = rebase_to_dest_parent(workspace_command, predecessor, commit)?;
    let tree = commit.tree()?;
    diff_util::show_diff(
        ui,
        formatter,
        workspace_command,
        &predecessor_tree,
        &tree,
        &EverythingMatcher,
        diff_formats,
    )
}

#[instrument(skip_all)]
fn cmd_interdiff(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &InterdiffArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let from = workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"), ui)?;
    let to = workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"), ui)?;

    let from_tree = rebase_to_dest_parent(&workspace_command, &from, &to)?;
    let to_tree = to.tree()?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let diff_formats = diff_util::diff_formats_for(command.settings(), &args.format)?;
    ui.request_pager();
    diff_util::show_diff(
        ui,
        ui.stdout_formatter().as_mut(),
        &workspace_command,
        &from_tree,
        &to_tree,
        matcher.as_ref(),
        &diff_formats,
    )
}

fn rebase_to_dest_parent(
    workspace_command: &WorkspaceCommandHelper,
    source: &Commit,
    destination: &Commit,
) -> Result<MergedTree, CommandError> {
    if source.parent_ids() == destination.parent_ids() {
        Ok(source.tree()?)
    } else {
        let destination_parent_tree =
            merge_commit_trees(workspace_command.repo().as_ref(), &destination.parents())?;
        let source_parent_tree =
            merge_commit_trees(workspace_command.repo().as_ref(), &source.parents())?;
        let source_tree = source.tree()?;
        let rebased_tree = destination_parent_tree.merge(&source_parent_tree, &source_tree)?;
        Ok(rebased_tree)
    }
}

fn edit_description(
    repo: &ReadonlyRepo,
    description: &str,
    settings: &UserSettings,
) -> Result<String, CommandError> {
    let description_file_path = (|| -> Result<_, io::Error> {
        let mut file = tempfile::Builder::new()
            .prefix("editor-")
            .suffix(".jjdescription")
            .tempfile_in(repo.repo_path())?;
        file.write_all(description.as_bytes())?;
        file.write_all(b"\nJJ: Lines starting with \"JJ: \" (like this one) will be removed.\n")?;
        let (_, path) = file.keep().map_err(|e| e.error)?;
        Ok(path)
    })()
    .map_err(|e| {
        user_error(format!(
            r#"Failed to create description file in "{path}": {e}"#,
            path = repo.repo_path().display()
        ))
    })?;

    run_ui_editor(settings, &description_file_path)?;

    let description = fs::read_to_string(&description_file_path).map_err(|e| {
        user_error(format!(
            r#"Failed to read description file "{path}": {e}"#,
            path = description_file_path.display()
        ))
    })?;
    // Delete the file only if everything went well.
    // TODO: Tell the user the name of the file we left behind.
    std::fs::remove_file(description_file_path).ok();
    // Normalize line ending, remove leading and trailing blank lines.
    let description = description
        .lines()
        .filter(|line| !line.starts_with("JJ: "))
        .join("\n");
    Ok(text_util::complete_newline(description.trim_matches('\n')))
}

fn edit_sparse(
    workspace_root: &Path,
    repo_path: &Path,
    sparse: &[RepoPath],
    settings: &UserSettings,
) -> Result<Vec<RepoPath>, CommandError> {
    let file = (|| -> Result<_, io::Error> {
        let mut file = tempfile::Builder::new()
            .prefix("editor-")
            .suffix(".jjsparse")
            .tempfile_in(repo_path)?;
        for sparse_path in sparse {
            let workspace_relative_sparse_path =
                file_util::relative_path(workspace_root, &sparse_path.to_fs_path(workspace_root));
            file.write_all(
                workspace_relative_sparse_path
                    .to_str()
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!(
                                "stored sparse path is not valid utf-8: {}",
                                workspace_relative_sparse_path.display()
                            ),
                        )
                    })?
                    .as_bytes(),
            )?;
            file.write_all(b"\n")?;
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    })()
    .map_err(|e| {
        user_error(format!(
            r#"Failed to create sparse patterns file in "{path}": {e}"#,
            path = repo_path.display()
        ))
    })?;
    let file_path = file.path().to_owned();

    run_ui_editor(settings, &file_path)?;

    // Read and parse patterns.
    io::BufReader::new(file)
        .lines()
        .filter(|line| {
            line.as_ref()
                .map(|line| !line.starts_with("JJ: ") && !line.trim().is_empty())
                .unwrap_or(true)
        })
        .map(|line| {
            let line = line.map_err(|e| {
                user_error(format!(
                    r#"Failed to read sparse patterns file "{path}": {e}"#,
                    path = file_path.display()
                ))
            })?;
            Ok::<_, CommandError>(RepoPath::parse_fs_path(
                workspace_root,
                workspace_root,
                line.trim(),
            )?)
        })
        .try_collect()
}

#[instrument(skip_all)]
fn cmd_describe(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DescribeArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    workspace_command.check_rewritable([&commit])?;
    let description = if args.stdin {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer).unwrap();
        buffer
    } else if !args.message_paragraphs.is_empty() {
        cli_util::join_message_paragraphs(&args.message_paragraphs)
    } else if args.no_edit {
        commit.description().to_owned()
    } else {
        let template =
            description_template_for_commit(ui, command.settings(), &workspace_command, &commit)?;
        edit_description(workspace_command.repo(), &template, command.settings())?
    };
    if description == *commit.description() && !args.reset_author {
        writeln!(ui.stderr(), "Nothing changed.")?;
    } else {
        let mut tx =
            workspace_command.start_transaction(&format!("describe commit {}", commit.id().hex()));
        let mut commit_builder = tx
            .mut_repo()
            .rewrite_commit(command.settings(), &commit)
            .set_description(description);
        if args.reset_author {
            let new_author = commit_builder.committer().clone();
            commit_builder = commit_builder.set_author(new_author);
        }
        commit_builder.write()?;
        tx.finish(ui)?;
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_commit(ui: &mut Ui, command: &CommandHelper, args: &CommitArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let commit_id = workspace_command
        .get_wc_commit_id()
        .ok_or_else(|| user_error("This command requires a working copy"))?;
    let commit = workspace_command.repo().store().get_commit(commit_id)?;
    let template =
        description_template_for_commit(ui, command.settings(), &workspace_command, &commit)?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let mut tx = workspace_command.start_transaction(&format!("commit {}", commit.id().hex()));
    let base_tree = merge_commit_trees(tx.repo(), &commit.parents())?;
    let instructions = format!(
        "\
You are splitting the working-copy commit: {}

The diff initially shows all changes. Adjust the right side until it shows the
contents you want for the first commit. The remainder will be included in the
new working-copy commit.
",
        tx.format_commit_summary(&commit)
    );
    let tree_id = tx.select_diff(
        ui,
        &base_tree,
        &commit.tree()?,
        matcher.as_ref(),
        &instructions,
        args.interactive,
    )?;
    let middle_tree = tx.repo().store().get_root_tree(&tree_id)?;
    if !args.paths.is_empty() && middle_tree.id() == base_tree.id() {
        writeln!(
            ui.warning(),
            "The given paths do not match any file: {}",
            args.paths.join(" ")
        )?;
    }

    let description = if !args.message_paragraphs.is_empty() {
        cli_util::join_message_paragraphs(&args.message_paragraphs)
    } else {
        edit_description(tx.base_repo(), &template, command.settings())?
    };

    let new_commit = tx
        .mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .set_tree_id(tree_id)
        .set_description(description)
        .write()?;
    let workspace_ids = tx
        .mut_repo()
        .view()
        .workspaces_for_wc_commit_id(commit.id());
    if !workspace_ids.is_empty() {
        let new_wc_commit = tx
            .mut_repo()
            .new_commit(
                command.settings(),
                vec![new_commit.id().clone()],
                commit.tree_id().clone(),
            )
            .write()?;
        for workspace_id in workspace_ids {
            tx.mut_repo().edit(workspace_id, &new_wc_commit).unwrap();
        }
    }
    tx.finish(ui)?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_duplicate(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DuplicateArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let to_duplicate: IndexSet<Commit> =
        resolve_multiple_nonempty_revsets(&args.revisions, &workspace_command, ui)?;
    if to_duplicate
        .iter()
        .any(|commit| commit.id() == workspace_command.repo().store().root_commit_id())
    {
        return Err(user_error("Cannot duplicate the root commit"));
    }
    let mut duplicated_old_to_new: IndexMap<Commit, Commit> = IndexMap::new();

    let mut tx = workspace_command
        .start_transaction(&format!("duplicating {} commit(s)", to_duplicate.len()));
    let base_repo = tx.base_repo().clone();
    let store = base_repo.store();
    let mut_repo = tx.mut_repo();

    for original_commit_id in base_repo
        .index()
        .topo_order(&mut to_duplicate.iter().map(|c| c.id()))
        .into_iter()
    {
        // Topological order ensures that any parents of `original_commit` are
        // either not in `to_duplicate` or were already duplicated.
        let original_commit = store.get_commit(&original_commit_id).unwrap();
        let new_parents = original_commit
            .parents()
            .iter()
            .map(|parent| {
                if let Some(duplicated_parent) = duplicated_old_to_new.get(parent) {
                    duplicated_parent
                } else {
                    parent
                }
                .id()
                .clone()
            })
            .collect();
        let new_commit = mut_repo
            .rewrite_commit(command.settings(), &original_commit)
            .generate_new_change_id()
            .set_parents(new_parents)
            .write()?;
        duplicated_old_to_new.insert(original_commit, new_commit);
    }

    for (old, new) in duplicated_old_to_new.iter() {
        write!(
            ui.stderr(),
            "Duplicated {} as ",
            short_commit_hash(old.id())
        )?;
        tx.write_commit_summary(ui.stderr_formatter().as_mut(), new)?;
        writeln!(ui.stderr())?;
    }
    tx.finish(ui)?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_abandon(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &AbandonArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let to_abandon = resolve_multiple_nonempty_revsets(&args.revisions, &workspace_command, ui)?;
    workspace_command.check_rewritable(to_abandon.iter())?;
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
    for commit in &to_abandon {
        tx.mut_repo().record_abandoned_commit(commit.id().clone());
    }
    let num_rebased = tx.mut_repo().rebase_descendants(command.settings())?;

    if to_abandon.len() == 1 {
        write!(ui.stderr(), "Abandoned commit ")?;
        tx.base_workspace_helper()
            .write_commit_summary(ui.stderr_formatter().as_mut(), &to_abandon[0])?;
        writeln!(ui.stderr())?;
    } else if !args.summary {
        writeln!(ui.stderr(), "Abandoned the following commits:")?;
        for commit in to_abandon {
            write!(ui.stderr(), "  ")?;
            tx.base_workspace_helper()
                .write_commit_summary(ui.stderr_formatter().as_mut(), &commit)?;
            writeln!(ui.stderr())?;
        }
    } else {
        writeln!(ui.stderr(), "Abandoned {} commits.", &to_abandon.len())?;
    }
    if num_rebased > 0 {
        writeln!(
            ui.stderr(),
            "Rebased {num_rebased} descendant commits onto parents of abandoned commits"
        )?;
    }
    tx.finish(ui)?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_edit(ui: &mut Ui, command: &CommandHelper, args: &EditArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let new_commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    workspace_command.check_rewritable([&new_commit])?;
    if workspace_command.get_wc_commit_id() == Some(new_commit.id()) {
        writeln!(ui.stderr(), "Already editing that commit")?;
    } else {
        let mut tx =
            workspace_command.start_transaction(&format!("edit commit {}", new_commit.id().hex()));
        tx.edit(&new_commit)?;
        tx.finish(ui)?;
    }
    Ok(())
}

/// Resolves revsets into revisions to rebase onto. These revisions don't have
/// to be rewriteable.
fn resolve_destination_revs(
    workspace_command: &WorkspaceCommandHelper,
    ui: &mut Ui,
    revisions: &[RevisionArg],
) -> Result<IndexSet<Commit>, CommandError> {
    let commits =
        resolve_multiple_nonempty_revsets_default_single(workspace_command, ui, revisions)?;
    let root_commit_id = workspace_command.repo().store().root_commit_id();
    if commits.len() >= 2 && commits.iter().any(|c| c.id() == root_commit_id) {
        Err(user_error("Cannot merge with root revision"))
    } else {
        Ok(commits)
    }
}

#[instrument(skip_all)]
fn cmd_new(ui: &mut Ui, command: &CommandHelper, args: &NewArgs) -> Result<(), CommandError> {
    if args.allow_large_revsets {
        return Err(user_error(
            "--allow-large-revsets has been deprecated.
Please use `jj new 'all:x|y'` instead of `jj new --allow-large-revsets x y`.",
        ));
    }
    let mut workspace_command = command.workspace_helper(ui)?;
    assert!(
        !args.revisions.is_empty(),
        "expected a non-empty list from clap"
    );
    let target_commits = resolve_destination_revs(&workspace_command, ui, &args.revisions)?
        .into_iter()
        .collect_vec();
    let target_ids = target_commits.iter().map(|c| c.id().clone()).collect_vec();
    let mut tx = workspace_command.start_transaction("new empty commit");
    let mut num_rebased = 0;
    let new_commit;
    if args.insert_before {
        // Instead of having the new commit as a child of the changes given on the
        // command line, add it between the changes' parents and the changes.
        // The parents of the new commit will be the parents of the target commits
        // which are not descendants of other target commits.
        let root_commit = tx.repo().store().root_commit();
        if target_ids.contains(root_commit.id()) {
            return Err(user_error("Cannot insert a commit before the root commit"));
        }
        let new_children = RevsetExpression::commits(target_ids.clone());
        let new_parents = new_children.parents();
        if let Some(commit_id) = new_children
            .dag_range_to(&new_parents)
            .resolve(tx.repo())?
            .evaluate(tx.repo())?
            .iter()
            .next()
        {
            return Err(user_error(format!(
                "Refusing to create a loop: commit {} would be both an ancestor and a descendant \
                 of the new commit",
                short_commit_hash(&commit_id),
            )));
        }
        let mut new_parents_commits: Vec<Commit> = new_parents
            .resolve(tx.repo())?
            .evaluate(tx.repo())?
            .iter()
            .commits(tx.repo().store())
            .try_collect()?;
        // The git backend does not support creating merge commits involving the root
        // commit.
        if new_parents_commits.len() > 1 {
            new_parents_commits.retain(|c| c != &root_commit);
        }
        let merged_tree = merge_commit_trees(tx.repo(), &new_parents_commits)?;
        let new_parents_commit_id = new_parents_commits.iter().map(|c| c.id().clone()).collect();
        new_commit = tx
            .mut_repo()
            .new_commit(command.settings(), new_parents_commit_id, merged_tree.id())
            .set_description(cli_util::join_message_paragraphs(&args.message_paragraphs))
            .write()?;
        num_rebased = target_ids.len();
        for child_commit in target_commits {
            rebase_commit(
                command.settings(),
                tx.mut_repo(),
                &child_commit,
                &[new_commit.clone()],
            )?;
        }
    } else {
        let merged_tree = merge_commit_trees(tx.repo(), &target_commits)?;
        new_commit = tx
            .mut_repo()
            .new_commit(command.settings(), target_ids.clone(), merged_tree.id())
            .set_description(cli_util::join_message_paragraphs(&args.message_paragraphs))
            .write()?;
        if args.insert_after {
            // Each child of the targets will be rebased: its set of parents will be updated
            // so that the targets are replaced by the new commit.
            let old_parents = RevsetExpression::commits(target_ids);
            // Exclude children that are ancestors of the new commit
            let to_rebase = old_parents.children().minus(&old_parents.ancestors());
            let commits_to_rebase: Vec<Commit> = to_rebase
                .resolve(tx.base_repo().as_ref())?
                .evaluate(tx.base_repo().as_ref())?
                .iter()
                .commits(tx.base_repo().store())
                .try_collect()?;
            num_rebased = commits_to_rebase.len();
            for child_commit in commits_to_rebase {
                let commit_parents =
                    RevsetExpression::commits(child_commit.parent_ids().to_owned());
                let new_parents = commit_parents.minus(&old_parents);
                let mut new_parent_commits: Vec<Commit> = new_parents
                    .resolve(tx.base_repo().as_ref())?
                    .evaluate(tx.base_repo().as_ref())?
                    .iter()
                    .commits(tx.base_repo().store())
                    .try_collect()?;
                new_parent_commits.push(new_commit.clone());
                rebase_commit(
                    command.settings(),
                    tx.mut_repo(),
                    &child_commit,
                    &new_parent_commits,
                )?;
            }
        }
    }
    num_rebased += tx.mut_repo().rebase_descendants(command.settings())?;
    if num_rebased > 0 {
        writeln!(ui.stderr(), "Rebased {num_rebased} descendant commits")?;
    }
    tx.edit(&new_commit).unwrap();
    tx.finish(ui)?;
    Ok(())
}

fn cmd_next(ui: &mut Ui, command: &CommandHelper, args: &NextArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let edit = args.edit;
    let amount = args.amount;
    let current_wc_id = workspace_command
        .get_wc_commit_id()
        .ok_or_else(|| user_error("This command requires a working copy"))?;
    let current_wc = workspace_command.repo().store().get_commit(current_wc_id)?;
    let current_short = short_commit_hash(current_wc.id());
    // If we're editing, start at the working-copy commit.
    // Otherwise start from our direct parent.
    let start_id = if edit {
        current_wc_id
    } else {
        match current_wc.parent_ids() {
            [parent_id] => parent_id,
            _ => return Err(user_error("Cannot run `jj next` on a merge commit")),
        }
    };
    let descendant_expression = RevsetExpression::commit(start_id.clone()).descendants_at(amount);
    let target_expression = if edit {
        descendant_expression
    } else {
        descendant_expression.minus(&RevsetExpression::commit(current_wc_id.clone()).descendants())
    };
    let targets: Vec<Commit> = target_expression
        .resolve(workspace_command.repo().as_ref())?
        .evaluate(workspace_command.repo().as_ref())?
        .iter()
        .commits(workspace_command.repo().store())
        .take(2)
        .try_collect()?;
    let target = match targets.as_slice() {
        [target] => target,
        [] => {
            // We found no descendant.
            return Err(user_error(format!(
                "No descendant found {amount} commit{} forward",
                if amount > 1 { "s" } else { "" }
            )));
        }
        _ => {
            // TODO(#2126) We currently cannot deal with multiple children, which result
            // from branches. Prompt the user for resolution.
            return Err(user_error("Ambiguous target commit"));
        }
    };
    let target_short = short_commit_hash(target.id());
    // We're editing, just move to the target commit.
    if edit {
        // We're editing, the target must be rewritable.
        workspace_command.check_rewritable([target])?;
        let mut tx = workspace_command
            .start_transaction(&format!("next: {current_short} -> editing {target_short}"));
        tx.edit(target)?;
        tx.finish(ui)?;
        return Ok(());
    }
    let mut tx =
        workspace_command.start_transaction(&format!("next: {current_short} -> {target_short}"));
    // Move the working-copy commit to the new parent.
    tx.check_out(target)?;
    tx.finish(ui)?;
    Ok(())
}

fn cmd_prev(ui: &mut Ui, command: &CommandHelper, args: &PrevArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let edit = args.edit;
    let amount = args.amount;
    let current_wc_id = workspace_command
        .get_wc_commit_id()
        .ok_or_else(|| user_error("This command requires a working copy"))?;
    let current_wc = workspace_command.repo().store().get_commit(current_wc_id)?;
    let current_short = short_commit_hash(current_wc.id());
    let start_id = if edit {
        current_wc_id
    } else {
        match current_wc.parent_ids() {
            [parent_id] => parent_id,
            _ => return Err(user_error("Cannot run `jj prev` on a merge commit")),
        }
    };
    let ancestor_expression = RevsetExpression::commit(start_id.clone()).ancestors_at(amount);
    let target_revset = if edit {
        ancestor_expression
    } else {
        // Jujutsu will always create a new commit for prev, even where Mercurial cannot
        // and fails. The decision and all discussion around it are available
        // here: https://github.com/martinvonz/jj/pull/1200#discussion_r1298623933
        //
        // If users ever request erroring out, add `.ancestors()` to the revset below.
        ancestor_expression.minus(&RevsetExpression::commit(current_wc_id.clone()))
    };
    let targets: Vec<_> = target_revset
        .resolve(workspace_command.repo().as_ref())?
        .evaluate(workspace_command.repo().as_ref())?
        .iter()
        .commits(workspace_command.repo().store())
        .take(2)
        .try_collect()?;
    let target = match targets.as_slice() {
        [target] => target,
        [] => {
            return Err(user_error(format!(
                "No ancestor found {amount} commit{} back",
                if amount > 1 { "s" } else { "" }
            )))
        }
        _ => return Err(user_error("Ambiguous target commit")),
    };
    // Generate a short commit hash, to make it readable in the op log.
    let target_short = short_commit_hash(target.id());
    // If we're editing, just move to the revision directly.
    if edit {
        // The target must be rewritable if we're editing.
        workspace_command.check_rewritable([target])?;
        let mut tx = workspace_command
            .start_transaction(&format!("prev: {current_short} -> editing {target_short}"));
        tx.edit(target)?;
        tx.finish(ui)?;
        return Ok(());
    }
    let mut tx =
        workspace_command.start_transaction(&format!("prev: {current_short} -> {target_short}"));
    tx.check_out(target)?;
    tx.finish(ui)?;
    Ok(())
}

fn combine_messages(
    repo: &ReadonlyRepo,
    source: &Commit,
    destination: &Commit,
    settings: &UserSettings,
    abandon_source: bool,
) -> Result<String, CommandError> {
    let description = if abandon_source {
        if source.description().is_empty() {
            destination.description().to_string()
        } else if destination.description().is_empty() {
            source.description().to_string()
        } else {
            let combined = "JJ: Enter a description for the combined commit.\n".to_string()
                + "JJ: Description from the destination commit:\n"
                + destination.description()
                + "\nJJ: Description from the source commit:\n"
                + source.description();
            edit_description(repo, &combined, settings)?
        }
    } else {
        destination.description().to_string()
    };
    Ok(description)
}

#[instrument(skip_all)]
fn cmd_move(ui: &mut Ui, command: &CommandHelper, args: &MoveArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let source = workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"), ui)?;
    let mut destination =
        workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"), ui)?;
    if source.id() == destination.id() {
        return Err(user_error("Source and destination cannot be the same."));
    }
    workspace_command.check_rewritable([&source, &destination])?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let mut tx = workspace_command.start_transaction(&format!(
        "move changes from {} to {}",
        source.id().hex(),
        destination.id().hex()
    ));
    let parent_tree = merge_commit_trees(tx.repo(), &source.parents())?;
    let source_tree = source.tree()?;
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
        tx.format_commit_summary(&source),
        tx.format_commit_summary(&destination)
    );
    let new_parent_tree_id = tx.select_diff(
        ui,
        &parent_tree,
        &source_tree,
        matcher.as_ref(),
        &instructions,
        args.interactive,
    )?;
    if args.interactive && new_parent_tree_id == parent_tree.id() {
        return Err(user_error("No changes to move"));
    }
    let new_parent_tree = tx.repo().store().get_root_tree(&new_parent_tree_id)?;
    // Apply the reverse of the selected changes onto the source
    let new_source_tree = source_tree.merge(&new_parent_tree, &parent_tree)?;
    let abandon_source = new_source_tree.id() == parent_tree.id();
    if abandon_source {
        tx.mut_repo().record_abandoned_commit(source.id().clone());
    } else {
        tx.mut_repo()
            .rewrite_commit(command.settings(), &source)
            .set_tree_id(new_source_tree.id().clone())
            .write()?;
    }
    if tx.repo().index().is_ancestor(source.id(), destination.id()) {
        // If we're moving changes to a descendant, first rebase descendants onto the
        // rewritten source. Otherwise it will likely already have the content
        // changes we're moving, so applying them will have no effect and the
        // changes will disappear.
        let mut rebaser = tx.mut_repo().create_descendant_rebaser(command.settings());
        rebaser.rebase_all()?;
        let rebased_destination_id = rebaser.rebased().get(destination.id()).unwrap().clone();
        destination = tx.mut_repo().store().get_commit(&rebased_destination_id)?;
    }
    // Apply the selected changes onto the destination
    let destination_tree = destination.tree()?;
    let new_destination_tree = destination_tree.merge(&parent_tree, &new_parent_tree)?;
    let description = combine_messages(
        tx.base_repo(),
        &source,
        &destination,
        command.settings(),
        abandon_source,
    )?;
    tx.mut_repo()
        .rewrite_commit(command.settings(), &destination)
        .set_tree_id(new_destination_tree.id().clone())
        .set_description(description)
        .write()?;
    tx.finish(ui)?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_squash(ui: &mut Ui, command: &CommandHelper, args: &SquashArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    workspace_command.check_rewritable([&commit])?;
    let parents = commit.parents();
    if parents.len() != 1 {
        return Err(user_error("Cannot squash merge commits"));
    }
    let parent = &parents[0];
    workspace_command.check_rewritable(&parents[..1])?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
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
        tx.format_commit_summary(&commit),
        tx.format_commit_summary(parent)
    );
    let parent_tree = parent.tree()?;
    let tree = commit.tree()?;
    let new_parent_tree_id = tx.select_diff(
        ui,
        &parent_tree,
        &tree,
        matcher.as_ref(),
        &instructions,
        args.interactive,
    )?;
    if &new_parent_tree_id == parent.tree_id() {
        if args.interactive {
            return Err(user_error("No changes selected"));
        }

        if let [only_path] = &args.paths[..] {
            let (_, matches) = command.matches().subcommand().unwrap();
            if matches.value_source("revision").unwrap() == ValueSource::DefaultValue
                && revset::parse(
                    only_path,
                    &tx.base_workspace_helper().revset_parse_context(),
                )
                .is_ok()
            {
                writeln!(
                    ui.warning(),
                    "warning: The argument {only_path:?} is being interpreted as a path. To \
                     specify a revset, pass -r {only_path:?} instead."
                )?;
            }
        }
    }
    // Abandon the child if the parent now has all the content from the child
    // (always the case in the non-interactive case).
    let abandon_child = &new_parent_tree_id == commit.tree_id();
    let description = if !args.message_paragraphs.is_empty() {
        cli_util::join_message_paragraphs(&args.message_paragraphs)
    } else {
        combine_messages(
            tx.base_repo(),
            &commit,
            parent,
            command.settings(),
            abandon_child,
        )?
    };
    let mut_repo = tx.mut_repo();
    let new_parent = mut_repo
        .rewrite_commit(command.settings(), parent)
        .set_tree_id(new_parent_tree_id)
        .set_predecessors(vec![parent.id().clone(), commit.id().clone()])
        .set_description(description)
        .write()?;
    if abandon_child {
        mut_repo.record_abandoned_commit(commit.id().clone());
    } else {
        // Commit the remainder on top of the new parent commit.
        mut_repo
            .rewrite_commit(command.settings(), &commit)
            .set_parents(vec![new_parent.id().clone()])
            .write()?;
    }
    tx.finish(ui)?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_unsquash(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &UnsquashArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    workspace_command.check_rewritable([&commit])?;
    let parents = commit.parents();
    if parents.len() != 1 {
        return Err(user_error("Cannot unsquash merge commits"));
    }
    let parent = &parents[0];
    workspace_command.check_rewritable(&parents[..1])?;
    let mut tx =
        workspace_command.start_transaction(&format!("unsquash commit {}", commit.id().hex()));
    let parent_base_tree = merge_commit_trees(tx.repo(), &parent.parents())?;
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
            tx.format_commit_summary(parent),
            tx.format_commit_summary(&commit)
        );
        let parent_tree = parent.tree()?;
        new_parent_tree_id = tx.edit_diff(
            ui,
            &parent_base_tree,
            &parent_tree,
            &EverythingMatcher,
            &instructions,
        )?;
        if new_parent_tree_id == parent_base_tree.id() {
            return Err(user_error("No changes selected"));
        }
    } else {
        new_parent_tree_id = parent_base_tree.id().clone();
    }
    // Abandon the parent if it is now empty (always the case in the non-interactive
    // case).
    if new_parent_tree_id == parent_base_tree.id() {
        tx.mut_repo().record_abandoned_commit(parent.id().clone());
        let description =
            combine_messages(tx.base_repo(), parent, &commit, command.settings(), true)?;
        // Commit the new child on top of the parent's parents.
        tx.mut_repo()
            .rewrite_commit(command.settings(), &commit)
            .set_parents(parent.parent_ids().to_vec())
            .set_description(description)
            .write()?;
    } else {
        let new_parent = tx
            .mut_repo()
            .rewrite_commit(command.settings(), parent)
            .set_tree_id(new_parent_tree_id)
            .set_predecessors(vec![parent.id().clone(), commit.id().clone()])
            .write()?;
        // Commit the new child on top of the new parent.
        tx.mut_repo()
            .rewrite_commit(command.settings(), &commit)
            .set_parents(vec![new_parent.id().clone()])
            .write()?;
    }
    tx.finish(ui)?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_chmod(ui: &mut Ui, command: &CommandHelper, args: &ChmodArgs) -> Result<(), CommandError> {
    let executable_bit = match args.mode {
        ChmodMode::Executable => true,
        ChmodMode::Normal => false,
    };

    let mut workspace_command = command.workspace_helper(ui)?;
    let repo_paths: Vec<_> = args
        .paths
        .iter()
        .map(|path| workspace_command.parse_file_path(path))
        .try_collect()?;
    let commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    workspace_command.check_rewritable([&commit])?;

    let mut tx = workspace_command.start_transaction(&format!(
        "make paths {} in commit {}",
        if executable_bit {
            "executable"
        } else {
            "non-executable"
        },
        commit.id().hex(),
    ));
    let tree = commit.tree()?;
    let store = tree.store();
    let mut tree_builder = MergedTreeBuilder::new(commit.tree_id().clone());
    for repo_path in repo_paths {
        let user_error_with_path = |msg: &str| {
            user_error(format!(
                "{msg} at '{}'.",
                tx.base_workspace_helper().format_file_path(&repo_path)
            ))
        };
        let tree_value = tree.path_value(&repo_path);
        if tree_value.is_absent() {
            return Err(user_error_with_path("No such path"));
        }
        let all_files = tree_value
            .adds()
            .iter()
            .flatten()
            .all(|tree_value| matches!(tree_value, TreeValue::File { .. }));
        if !all_files {
            let message = if tree_value.is_resolved() {
                "Found neither a file nor a conflict"
            } else {
                "Some of the sides of the conflict are not files"
            };
            return Err(user_error_with_path(message));
        }
        let new_tree_value = tree_value.map(|value| match value {
            Some(TreeValue::File { id, executable: _ }) => Some(TreeValue::File {
                id: id.clone(),
                executable: executable_bit,
            }),
            Some(TreeValue::Conflict(_)) => {
                panic!("Conflict sides must not themselves be conflicts")
            }
            value => value.clone(),
        });
        tree_builder.set_or_remove(repo_path, new_tree_value);
    }

    let new_tree_id = tree_builder.write_tree(store)?;
    tx.mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .set_tree_id(new_tree_id)
        .write()?;
    tx.finish(ui)
}

#[instrument(skip_all)]
fn cmd_resolve(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ResolveArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    let tree = commit.tree()?;
    let conflicts = tree
        .conflicts()
        .filter(|path| matcher.matches(&path.0))
        .collect_vec();
    if conflicts.is_empty() {
        return Err(CommandError::CliError(format!(
            "No conflicts found {}",
            if args.paths.is_empty() {
                "at this revision"
            } else {
                "at the given path(s)"
            }
        )));
    }
    if args.list {
        return print_conflicted_paths(
            &conflicts,
            ui.stdout_formatter().as_mut(),
            &workspace_command,
        );
    };

    let (repo_path, _) = conflicts.get(0).unwrap();
    workspace_command.check_rewritable([&commit])?;
    let mut tx = workspace_command.start_transaction(&format!(
        "Resolve conflicts in commit {}",
        commit.id().hex()
    ));
    let new_tree_id = tx.run_mergetool(ui, &tree, repo_path)?;
    let new_commit = tx
        .mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .set_tree_id(new_tree_id)
        .write()?;
    tx.finish(ui)?;

    if !args.quiet {
        let new_tree = new_commit.tree()?;
        let new_conflicts = new_tree.conflicts().collect_vec();
        if !new_conflicts.is_empty() {
            writeln!(
                ui.stderr(),
                "After this operation, some files at this revision still have conflicts:"
            )?;
            print_conflicted_paths(
                &new_conflicts,
                ui.stderr_formatter().as_mut(),
                &workspace_command,
            )?;
        }
    };
    Ok(())
}

#[instrument(skip_all)]
fn print_conflicted_paths(
    conflicts: &[(RepoPath, Merge<Option<TreeValue>>)],
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
        let n_adds = conflict.adds().iter().flatten().count();
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
        for term in itertools::chain(conflict.removes().iter(), conflict.adds().iter()).flatten() {
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

        write!(formatter, "{formatted_path} ",)?;
        formatter.with_label("conflict_description", |formatter| {
            let print_pair = |formatter: &mut dyn Formatter, (text, label): &(String, &str)| {
                formatter.with_label(label, |fmt| fmt.write_str(text))
            };
            print_pair(
                formatter,
                &(
                    format!("{sides}-sided"),
                    if sides > 2 { "difficult" } else { "normal" },
                ),
            )?;
            formatter.write_str(" conflict")?;

            if !seen_objects.is_empty() {
                formatter.write_str(" including ")?;
                let seen_objects = seen_objects.into_iter().collect_vec();
                match &seen_objects[..] {
                    [] => unreachable!(),
                    [only] => print_pair(formatter, only)?,
                    [first, middle @ .., last] => {
                        print_pair(formatter, first)?;
                        for pair in middle {
                            formatter.write_str(", ")?;
                            print_pair(formatter, pair)?;
                        }
                        formatter.write_str(" and ")?;
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

#[instrument(skip_all)]
fn cmd_restore(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &RestoreArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let (from_tree, to_commit);
    if args.revision.is_some() {
        return Err(user_error(
            "`jj restore` does not have a `--revision`/`-r` option. If you'd like to modify\nthe \
             *current* revision, use `--from`. If you'd like to modify a *different* \
             revision,\nuse `--to` or `--changes-in`.",
        ));
    }
    if args.from.is_some() || args.to.is_some() {
        to_commit = workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"), ui)?;
        from_tree = workspace_command
            .resolve_single_rev(args.from.as_deref().unwrap_or("@"), ui)?
            .tree()?;
    } else {
        to_commit =
            workspace_command.resolve_single_rev(args.changes_in.as_deref().unwrap_or("@"), ui)?;
        from_tree = merge_commit_trees(workspace_command.repo().as_ref(), &to_commit.parents())?;
    }
    workspace_command.check_rewritable([&to_commit])?;

    let new_tree_id = if args.paths.is_empty() {
        from_tree.id().clone()
    } else {
        let matcher = workspace_command.matcher_from_values(&args.paths)?;
        let mut tree_builder = MergedTreeBuilder::new(to_commit.tree_id().clone());
        let to_tree = to_commit.tree()?;
        for (repo_path, before, _after) in from_tree.diff(&to_tree, matcher.as_ref()) {
            tree_builder.set_or_remove(repo_path, before);
        }
        tree_builder.write_tree(workspace_command.repo().store())?
    };
    if &new_tree_id == to_commit.tree_id() {
        writeln!(ui.stderr(), "Nothing changed.")?;
    } else {
        let mut tx = workspace_command
            .start_transaction(&format!("restore into commit {}", to_commit.id().hex()));
        let mut_repo = tx.mut_repo();
        let new_commit = mut_repo
            .rewrite_commit(command.settings(), &to_commit)
            .set_tree_id(new_tree_id)
            .write()?;
        write!(ui.stderr(), "Created ")?;
        tx.write_commit_summary(ui.stderr_formatter().as_mut(), &new_commit)?;
        writeln!(ui.stderr())?;
        tx.finish(ui)?;
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_diffedit(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &DiffeditArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let (target_commit, base_commits, diff_description);
    if args.from.is_some() || args.to.is_some() {
        target_commit =
            workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"), ui)?;
        base_commits =
            vec![workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"), ui)?];
        diff_description = format!(
            "The diff initially shows the commit's changes relative to:\n{}",
            workspace_command.format_commit_summary(&base_commits[0])
        );
    } else {
        target_commit =
            workspace_command.resolve_single_rev(args.revision.as_deref().unwrap_or("@"), ui)?;
        base_commits = target_commit.parents();
        diff_description = "The diff initially shows the commit's changes.".to_string();
    };
    workspace_command.check_rewritable([&target_commit])?;

    let mut tx =
        workspace_command.start_transaction(&format!("edit commit {}", target_commit.id().hex()));
    let instructions = format!(
        "\
You are editing changes in: {}

{diff_description}

Adjust the right side until it shows the contents you want. If you
don't make any changes, then the operation will be aborted.",
        tx.format_commit_summary(&target_commit),
    );
    let base_tree = merge_commit_trees(tx.repo(), base_commits.as_slice())?;
    let tree = target_commit.tree()?;
    let tree_id = tx.edit_diff(ui, &base_tree, &tree, &EverythingMatcher, &instructions)?;
    if tree_id == *target_commit.tree_id() {
        writeln!(ui.stderr(), "Nothing changed.")?;
    } else {
        let mut_repo = tx.mut_repo();
        let new_commit = mut_repo
            .rewrite_commit(command.settings(), &target_commit)
            .set_tree_id(tree_id)
            .write()?;
        write!(ui.stderr(), "Created ")?;
        tx.write_commit_summary(ui.stderr_formatter().as_mut(), &new_commit)?;
        writeln!(ui.stderr())?;
        tx.finish(ui)?;
    }
    Ok(())
}

fn description_template_for_commit(
    ui: &Ui,
    settings: &UserSettings,
    workspace_command: &WorkspaceCommandHelper,
    commit: &Commit,
) -> Result<String, CommandError> {
    let mut diff_summary_bytes = Vec::new();
    diff_util::show_patch(
        ui,
        &mut PlainTextFormatter::new(&mut diff_summary_bytes),
        workspace_command,
        commit,
        &EverythingMatcher,
        &[DiffFormat::Summary],
    )?;
    let description = if commit.description().is_empty() {
        settings.default_description()
    } else {
        commit.description().to_owned()
    };
    if diff_summary_bytes.is_empty() {
        Ok(description)
    } else {
        Ok(description + "\n" + &diff_summary_to_description(&diff_summary_bytes))
    }
}

fn description_template_for_cmd_split(
    ui: &Ui,
    settings: &UserSettings,
    workspace_command: &WorkspaceCommandHelper,
    intro: &str,
    overall_commit_description: &str,
    from_tree: &MergedTree,
    to_tree: &MergedTree,
) -> Result<String, CommandError> {
    let mut diff_summary_bytes = Vec::new();
    diff_util::show_diff(
        ui,
        &mut PlainTextFormatter::new(&mut diff_summary_bytes),
        workspace_command,
        from_tree,
        to_tree,
        &EverythingMatcher,
        &[DiffFormat::Summary],
    )?;
    let description = if overall_commit_description.is_empty() {
        settings.default_description()
    } else {
        overall_commit_description.to_owned()
    };
    Ok(format!("JJ: {intro}\n{description}\n") + &diff_summary_to_description(&diff_summary_bytes))
}

fn diff_summary_to_description(bytes: &[u8]) -> String {
    let text = std::str::from_utf8(bytes).expect(
        "Summary diffs and repo paths must always be valid UTF8.",
        // Double-check this assumption for diffs that include file content.
    );
    "JJ: This commit contains the following changes:\n".to_owned()
        + &textwrap::indent(text, "JJ:     ")
}

#[instrument(skip_all)]
fn cmd_split(ui: &mut Ui, command: &CommandHelper, args: &SplitArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision, ui)?;
    workspace_command.check_rewritable([&commit])?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let mut tx =
        workspace_command.start_transaction(&format!("split commit {}", commit.id().hex()));
    let end_tree = commit.tree()?;
    let base_tree = merge_commit_trees(tx.repo(), &commit.parents())?;
    let interactive = args.interactive || args.paths.is_empty();
    let instructions = format!(
        "\
You are splitting a commit in two: {}

The diff initially shows the changes in the commit you're splitting.

Adjust the right side until it shows the contents you want for the first
(parent) commit. The remainder will be in the second commit. If you
don't make any changes, then the operation will be aborted.
",
        tx.format_commit_summary(&commit)
    );
    let tree_id = tx.select_diff(
        ui,
        &base_tree,
        &end_tree,
        matcher.as_ref(),
        &instructions,
        interactive,
    )?;
    if &tree_id == commit.tree_id() && interactive {
        writeln!(ui.stderr(), "Nothing changed.")?;
        return Ok(());
    }
    let middle_tree = tx.repo().store().get_root_tree(&tree_id)?;
    if middle_tree.id() == base_tree.id() {
        writeln!(
            ui.warning(),
            "The given paths do not match any file: {}",
            args.paths.join(" ")
        )?;
    }

    let first_template = description_template_for_cmd_split(
        ui,
        command.settings(),
        tx.base_workspace_helper(),
        "Enter commit description for the first part (parent).",
        commit.description(),
        &base_tree,
        &middle_tree,
    )?;
    let first_description = edit_description(tx.base_repo(), &first_template, command.settings())?;
    let first_commit = tx
        .mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .set_tree_id(tree_id)
        .set_description(first_description)
        .write()?;
    let second_description = if commit.description().is_empty() {
        // If there was no description before, don't ask for one for the second commit.
        "".to_string()
    } else {
        let second_template = description_template_for_cmd_split(
            ui,
            command.settings(),
            tx.base_workspace_helper(),
            "Enter commit description for the second part (child).",
            commit.description(),
            &middle_tree,
            &end_tree,
        )?;
        edit_description(tx.base_repo(), &second_template, command.settings())?
    };
    let second_commit = tx
        .mut_repo()
        .rewrite_commit(command.settings(), &commit)
        .set_parents(vec![first_commit.id().clone()])
        .set_tree_id(commit.tree_id().clone())
        .generate_new_change_id()
        .set_description(second_description)
        .write()?;
    let mut rebaser = DescendantRebaser::new(
        command.settings(),
        tx.mut_repo(),
        hashmap! { commit.id().clone() => hashset!{second_commit.id().clone()} },
        hashset! {},
    );
    rebaser.rebase_all()?;
    let num_rebased = rebaser.rebased().len();
    if num_rebased > 0 {
        writeln!(ui.stderr(), "Rebased {num_rebased} descendant commits")?;
    }
    write!(ui.stderr(), "First part: ")?;
    tx.write_commit_summary(ui.stderr_formatter().as_mut(), &first_commit)?;
    write!(ui.stderr(), "\nSecond part: ")?;
    tx.write_commit_summary(ui.stderr_formatter().as_mut(), &second_commit)?;
    writeln!(ui.stderr())?;
    tx.finish(ui)?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_merge(ui: &mut Ui, command: &CommandHelper, args: &NewArgs) -> Result<(), CommandError> {
    if args.revisions.len() < 2 {
        return Err(CommandError::CliError(String::from(
            "Merge requires at least two revisions",
        )));
    }
    cmd_new(ui, command, args)
}

// TODO: Move to run.rs
fn cmd_run(_ui: &mut Ui, _command: &CommandHelper, _args: &RunArgs) -> Result<(), CommandError> {
    Err(user_error("This is a stub, do not use"))
}

#[instrument(skip_all)]
fn cmd_rebase(ui: &mut Ui, command: &CommandHelper, args: &RebaseArgs) -> Result<(), CommandError> {
    if args.allow_large_revsets {
        return Err(user_error(
            "--allow-large-revsets has been deprecated.
Please use `jj rebase -d 'all:x|y'` instead of `jj rebase --allow-large-revsets -d x -d y`.",
        ));
    }
    let mut workspace_command = command.workspace_helper(ui)?;
    let new_parents = resolve_destination_revs(&workspace_command, ui, &args.destination)?
        .into_iter()
        .collect_vec();
    if let Some(rev_str) = &args.revision {
        rebase_revision(
            ui,
            command.settings(),
            &mut workspace_command,
            &new_parents,
            rev_str,
        )?;
    } else if !args.source.is_empty() {
        let source_commits =
            resolve_multiple_nonempty_revsets_default_single(&workspace_command, ui, &args.source)?;
        rebase_descendants(
            ui,
            command.settings(),
            &mut workspace_command,
            &new_parents,
            &source_commits,
        )?;
    } else {
        let branch_commits = if args.branch.is_empty() {
            IndexSet::from([workspace_command.resolve_single_rev("@", ui)?])
        } else {
            resolve_multiple_nonempty_revsets_default_single(&workspace_command, ui, &args.branch)?
        };
        rebase_branch(
            ui,
            command.settings(),
            &mut workspace_command,
            &new_parents,
            &branch_commits,
        )?;
    }
    Ok(())
}

fn rebase_branch(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parents: &[Commit],
    branch_commits: &IndexSet<Commit>,
) -> Result<(), CommandError> {
    let parent_ids = new_parents
        .iter()
        .map(|commit| commit.id().clone())
        .collect_vec();
    let branch_commit_ids = branch_commits
        .iter()
        .map(|commit| commit.id().clone())
        .collect_vec();
    let roots_expression = RevsetExpression::commits(parent_ids)
        .range(&RevsetExpression::commits(branch_commit_ids))
        .roots();
    let root_commits: IndexSet<_> = roots_expression
        .resolve(workspace_command.repo().as_ref())
        .unwrap()
        .evaluate(workspace_command.repo().as_ref())
        .unwrap()
        .iter()
        .commits(workspace_command.repo().store())
        .try_collect()?;
    rebase_descendants(ui, settings, workspace_command, new_parents, &root_commits)
}

fn rebase_descendants(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parents: &[Commit],
    old_commits: &IndexSet<Commit>,
) -> Result<(), CommandError> {
    workspace_command.check_rewritable(old_commits)?;
    for old_commit in old_commits.iter() {
        check_rebase_destinations(workspace_command.repo(), new_parents, old_commit)?;
    }
    let tx_message = if old_commits.len() == 1 {
        format!(
            "rebase commit {} and descendants",
            old_commits.first().unwrap().id().hex()
        )
    } else {
        format!("rebase {} commits and their descendants", old_commits.len())
    };
    let mut tx = workspace_command.start_transaction(&tx_message);
    // `rebase_descendants` takes care of sorting in reverse topological order, so
    // no need to do it here.
    for old_commit in old_commits {
        rebase_commit(settings, tx.mut_repo(), old_commit, new_parents)?;
    }
    let num_rebased = old_commits.len() + tx.mut_repo().rebase_descendants(settings)?;
    writeln!(ui.stderr(), "Rebased {num_rebased} commits")?;
    tx.finish(ui)?;
    Ok(())
}

fn rebase_revision(
    ui: &mut Ui,
    settings: &UserSettings,
    workspace_command: &mut WorkspaceCommandHelper,
    new_parents: &[Commit],
    rev_str: &str,
) -> Result<(), CommandError> {
    let old_commit = workspace_command.resolve_single_rev(rev_str, ui)?;
    workspace_command.check_rewritable([&old_commit])?;
    check_rebase_destinations(workspace_command.repo(), new_parents, &old_commit)?;
    let children_expression = RevsetExpression::commit(old_commit.id().clone()).children();
    let child_commits: Vec<_> = children_expression
        .resolve(workspace_command.repo().as_ref())
        .unwrap()
        .evaluate(workspace_command.repo().as_ref())
        .unwrap()
        .iter()
        .commits(workspace_command.repo().store())
        .try_collect()?;

    let mut tx =
        workspace_command.start_transaction(&format!("rebase commit {}", old_commit.id().hex()));
    rebase_commit(settings, tx.mut_repo(), &old_commit, new_parents)?;
    // Manually rebase children because we don't want to rebase them onto the
    // rewritten commit. (But we still want to record the commit as rewritten so
    // branches and the working copy get updated to the rewritten commit.)
    let mut num_rebased_descendants = 0;
    for child_commit in &child_commits {
        let new_child_parent_ids: Vec<CommitId> = child_commit
            .parents()
            .iter()
            .flat_map(|c| {
                if c == &old_commit {
                    old_commit
                        .parents()
                        .iter()
                        .map(|c| c.id().clone())
                        .collect()
                } else {
                    [c.id().clone()].to_vec()
                }
            })
            .collect();

        // Some of the new parents may be ancestors of others as in
        // `test_rebase_single_revision`.
        let new_child_parents_expression = RevsetExpression::commits(new_child_parent_ids.clone())
            .minus(
                &RevsetExpression::commits(new_child_parent_ids.clone())
                    .parents()
                    .ancestors(),
            );
        let new_child_parents: Vec<Commit> = new_child_parents_expression
            .resolve(tx.base_repo().as_ref())
            .unwrap()
            .evaluate(tx.base_repo().as_ref())
            .unwrap()
            .iter()
            .commits(tx.base_repo().store())
            .try_collect()?;

        rebase_commit(settings, tx.mut_repo(), child_commit, &new_child_parents)?;
        num_rebased_descendants += 1;
    }
    num_rebased_descendants += tx.mut_repo().rebase_descendants(settings)?;
    if num_rebased_descendants > 0 {
        writeln!(
            ui.stderr(),
            "Also rebased {num_rebased_descendants} descendant commits onto parent of rebased \
             commit"
        )?;
    }
    tx.finish(ui)?;
    Ok(())
}

fn check_rebase_destinations(
    repo: &Arc<ReadonlyRepo>,
    new_parents: &[Commit],
    commit: &Commit,
) -> Result<(), CommandError> {
    for parent in new_parents {
        if repo.index().is_ancestor(commit.id(), parent.id()) {
            return Err(user_error(format!(
                "Cannot rebase {} onto descendant {}",
                short_commit_hash(commit.id()),
                short_commit_hash(parent.id())
            )));
        }
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_backout(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BackoutArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let commit_to_back_out = workspace_command.resolve_single_rev(&args.revision, ui)?;
    let mut parents = vec![];
    for revision_str in &args.destination {
        let destination = workspace_command.resolve_single_rev(revision_str, ui)?;
        parents.push(destination);
    }
    let mut tx = workspace_command.start_transaction(&format!(
        "back out commit {}",
        commit_to_back_out.id().hex()
    ));
    back_out_commit(
        command.settings(),
        tx.mut_repo(),
        &commit_to_back_out,
        &parents,
    )?;
    tx.finish(ui)?;

    Ok(())
}

fn make_branch_term(branch_names: &[impl fmt::Display]) -> String {
    match branch_names {
        [branch_name] => format!("branch {}", branch_name),
        branch_names => format!("branches {}", branch_names.iter().join(", ")),
    }
}

#[instrument(skip_all)]
fn cmd_util(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &UtilCommands,
) -> Result<(), CommandError> {
    match subcommand {
        UtilCommands::Completion(completion_matches) => {
            let mut app = command.app().clone();
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
        UtilCommands::Mangen(_mangen_matches) => {
            let mut buf = vec![];
            let man = clap_mangen::Man::new(command.app().clone());
            man.render(&mut buf)?;
            ui.stdout_formatter().write_all(&buf)?;
        }
        UtilCommands::ConfigSchema(_config_schema_matches) => {
            // TODO(#879): Consider generating entire schema dynamically vs. static file.
            let buf = include_bytes!("../config-schema.json");
            ui.stdout_formatter().write_all(buf)?;
        }
    }
    Ok(())
}

#[instrument(skip_all)]
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
        WorkspaceCommands::Root(command_matches) => {
            cmd_workspace_root(ui, command, command_matches)
        }
        WorkspaceCommands::UpdateStale(command_matches) => {
            cmd_workspace_update_stale(ui, command, command_matches)
        }
    }
}

#[instrument(skip_all)]
fn cmd_workspace_add(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &WorkspaceAddArgs,
) -> Result<(), CommandError> {
    let old_workspace_command = command.workspace_helper(ui)?;
    let destination_path = command.cwd().join(&args.destination);
    if destination_path.exists() {
        return Err(user_error("Workspace already exists"));
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
    if repo.view().get_wc_commit_id(&workspace_id).is_some() {
        return Err(user_error(format!(
            "Workspace named '{name}' already exists"
        )));
    }
    let (new_workspace, repo) = Workspace::init_workspace_with_existing_repo(
        command.settings(),
        &destination_path,
        repo,
        workspace_id,
    )?;
    writeln!(
        ui.stderr(),
        "Created workspace in \"{}\"",
        file_util::relative_path(old_workspace_command.workspace_root(), &destination_path)
            .display()
    )?;

    let mut new_workspace_command = WorkspaceCommandHelper::new(ui, command, new_workspace, repo)?;
    let mut tx = new_workspace_command.start_transaction(&format!(
        "Create initial working-copy commit in workspace {}",
        &name
    ));

    let new_wc_commit = if let Some(specific_rev) = &args.revision {
        old_workspace_command.resolve_single_rev(specific_rev, ui)?
    } else {
        // Check out a parent of the current workspace's working-copy commit, or the
        // root if there is no working-copy commit in the current workspace.
        if let Some(old_wc_commit_id) = tx
            .base_repo()
            .view()
            .get_wc_commit_id(old_workspace_command.workspace_id())
        {
            tx.repo().store().get_commit(old_wc_commit_id)?.parents()[0].clone()
        } else {
            tx.repo().store().root_commit()
        }
    };

    tx.check_out(&new_wc_commit)?;
    tx.finish(ui)?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_workspace_forget(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &WorkspaceForgetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let len = args.workspaces.len();

    let mut wss = Vec::new();
    let description = match len {
        // NOTE (aseipp): if there's only 1-or-0 arguments, shortcut. this is
        // mostly so the oplog description can look good: it removes the need,
        // in the case of more-than-1 argument, to handle pluralization of the
        // nouns in the description
        0 | 1 => {
            let ws = match len == 0 {
                true => workspace_command.workspace_id().to_owned(),
                false => WorkspaceId::new(args.workspaces[0].to_string()),
            };
            wss.push(ws.clone());
            format!("forget workspace {}", ws.as_str())
        }
        _ => {
            args.workspaces
                .iter()
                .map(|ws| WorkspaceId::new(ws.to_string()))
                .for_each(|ws| wss.push(ws));

            format!("forget workspaces {}", args.workspaces.join(", "))
        }
    };

    for ws in &wss {
        if workspace_command
            .repo()
            .view()
            .get_wc_commit_id(ws)
            .is_none()
        {
            return Err(user_error(format!("No such workspace: {}", ws.as_str())));
        }
    }

    // bundle every workspace forget into a single transaction, so that e.g.
    // undo correctly restores all of them at once.
    let mut tx = workspace_command.start_transaction(&description);
    wss.iter().for_each(|ws| tx.mut_repo().remove_wc_commit(ws));
    tx.finish(ui)?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_workspace_list(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &WorkspaceListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    for (workspace_id, wc_commit_id) in repo.view().wc_commit_ids().iter().sorted() {
        write!(ui.stdout(), "{}: ", workspace_id.as_str())?;
        let commit = repo.store().get_commit(wc_commit_id)?;
        workspace_command.write_commit_summary(ui.stdout_formatter().as_mut(), &commit)?;
        writeln!(ui.stdout())?;
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_workspace_root(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &WorkspaceRootArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let root = workspace_command
        .workspace_root()
        .to_str()
        .ok_or_else(|| user_error("The workspace root is not valid UTF-8"))?;
    writeln!(ui.stdout(), "{root}")?;
    Ok(())
}

#[instrument(skip_all)]
fn cmd_workspace_update_stale(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &WorkspaceUpdateStaleArgs,
) -> Result<(), CommandError> {
    // Snapshot the current working copy on top of the last known working-copy
    // operation, then merge the concurrent operations. The wc_commit_id of the
    // merged repo wouldn't change because the old one wins, but it's probably
    // fine if we picked the new wc_commit_id.
    let known_wc_commit = {
        let mut workspace_command = command.for_stale_working_copy(ui)?;
        workspace_command.snapshot(ui)?;
        let wc_commit_id = workspace_command.get_wc_commit_id().unwrap();
        workspace_command.repo().store().get_commit(wc_commit_id)?
    };
    let mut workspace_command = command.workspace_helper_no_snapshot(ui)?;

    let repo = workspace_command.repo().clone();
    let (mut locked_ws, desired_wc_commit) =
        workspace_command.unchecked_start_working_copy_mutation()?;
    match check_stale_working_copy(locked_ws.locked_wc(), &desired_wc_commit, &repo) {
        Ok(_) => {
            writeln!(
                ui.stderr(),
                "Nothing to do (the working copy is not stale)."
            )?;
        }
        Err(_) => {
            // The same check as start_working_copy_mutation(), but with the stale
            // working-copy commit.
            if known_wc_commit.tree_id() != locked_ws.locked_wc().old_tree_id() {
                return Err(user_error("Concurrent working copy operation. Try again."));
            }
            let desired_tree = desired_wc_commit.tree()?;
            let stats = locked_ws
                .locked_wc()
                .check_out(&desired_tree)
                .map_err(|err| {
                    CommandError::InternalError(format!(
                        "Failed to check out commit {}: {}",
                        desired_wc_commit.id().hex(),
                        err
                    ))
                })?;
            locked_ws.finish(repo.op_id().clone())?;
            write!(ui.stderr(), "Working copy now at: ")?;
            ui.stderr_formatter().with_label("working_copy", |fmt| {
                workspace_command.write_commit_summary(fmt, &desired_wc_commit)
            })?;
            writeln!(ui.stderr())?;
            print_checkout_stats(ui, stats, &desired_wc_commit)?;
        }
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_sparse(ui: &mut Ui, command: &CommandHelper, args: &SparseArgs) -> Result<(), CommandError> {
    match args {
        SparseArgs::List(sub_args) => cmd_sparse_list(ui, command, sub_args),
        SparseArgs::Set(sub_args) => cmd_sparse_set(ui, command, sub_args),
    }
}

#[instrument(skip_all)]
fn cmd_sparse_list(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &SparseListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    for path in workspace_command.working_copy().sparse_patterns()? {
        let ui_path = workspace_command.format_file_path(path);
        writeln!(ui.stdout(), "{ui_path}")?;
    }
    Ok(())
}

#[instrument(skip_all)]
fn cmd_sparse_set(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &SparseSetArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let paths_to_add: Vec<_> = args
        .add
        .iter()
        .map(|v| workspace_command.parse_file_path(v))
        .try_collect()?;
    let paths_to_remove: Vec<_> = args
        .remove
        .iter()
        .map(|v| workspace_command.parse_file_path(v))
        .try_collect()?;
    // Determine inputs of `edit` operation now, since `workspace_command` is
    // inaccessible while the working copy is locked.
    let edit_inputs = args.edit.then(|| {
        (
            workspace_command.repo().clone(),
            workspace_command.workspace_root().clone(),
        )
    });
    let (mut locked_ws, wc_commit) = workspace_command.start_working_copy_mutation()?;
    let mut new_patterns = HashSet::new();
    if args.reset {
        new_patterns.insert(RepoPath::root());
    } else {
        if !args.clear {
            new_patterns.extend(locked_ws.locked_wc().sparse_patterns()?.iter().cloned());
            for path in paths_to_remove {
                new_patterns.remove(&path);
            }
        }
        for path in paths_to_add {
            new_patterns.insert(path);
        }
    }
    let mut new_patterns = new_patterns.into_iter().collect_vec();
    new_patterns.sort();
    if let Some((repo, workspace_root)) = edit_inputs {
        new_patterns = edit_sparse(
            &workspace_root,
            repo.repo_path(),
            &new_patterns,
            command.settings(),
        )?;
        new_patterns.sort();
    }
    let stats = locked_ws
        .locked_wc()
        .set_sparse_patterns(new_patterns)
        .map_err(|err| {
            CommandError::InternalError(format!("Failed to update working copy paths: {err}"))
        })?;
    let operation_id = locked_ws.locked_wc().old_operation_id().clone();
    locked_ws.finish(operation_id)?;
    print_checkout_stats(ui, stats, &wc_commit)?;

    Ok(())
}

pub fn default_app() -> Command {
    Commands::augment_subcommands(Args::command())
}

#[instrument(skip_all)]
pub fn run_command(ui: &mut Ui, command_helper: &CommandHelper) -> Result<(), CommandError> {
    let derived_subcommands: Commands =
        Commands::from_arg_matches(command_helper.matches()).unwrap();
    match &derived_subcommands {
        Commands::Version(sub_args) => cmd_version(ui, command_helper, sub_args),
        Commands::Init(sub_args) => cmd_init(ui, command_helper, sub_args),
        Commands::Config(sub_args) => cmd_config(ui, command_helper, sub_args),
        Commands::Checkout(sub_args) => cmd_checkout(ui, command_helper, sub_args),
        Commands::Untrack(sub_args) => cmd_untrack(ui, command_helper, sub_args),
        Commands::Files(sub_args) => cmd_files(ui, command_helper, sub_args),
        Commands::Cat(sub_args) => cmd_cat(ui, command_helper, sub_args),
        Commands::Diff(sub_args) => cmd_diff(ui, command_helper, sub_args),
        Commands::Show(sub_args) => cmd_show(ui, command_helper, sub_args),
        Commands::Status(sub_args) => cmd_status(ui, command_helper, sub_args),
        Commands::Log(sub_args) => cmd_log(ui, command_helper, sub_args),
        Commands::Interdiff(sub_args) => cmd_interdiff(ui, command_helper, sub_args),
        Commands::Obslog(sub_args) => cmd_obslog(ui, command_helper, sub_args),
        Commands::Describe(sub_args) => cmd_describe(ui, command_helper, sub_args),
        Commands::Commit(sub_args) => cmd_commit(ui, command_helper, sub_args),
        Commands::Duplicate(sub_args) => cmd_duplicate(ui, command_helper, sub_args),
        Commands::Abandon(sub_args) => cmd_abandon(ui, command_helper, sub_args),
        Commands::Edit(sub_args) => cmd_edit(ui, command_helper, sub_args),
        Commands::Next(sub_args) => cmd_next(ui, command_helper, sub_args),
        Commands::Prev(sub_args) => cmd_prev(ui, command_helper, sub_args),
        Commands::New(sub_args) => cmd_new(ui, command_helper, sub_args),
        Commands::Move(sub_args) => cmd_move(ui, command_helper, sub_args),
        Commands::Squash(sub_args) => cmd_squash(ui, command_helper, sub_args),
        Commands::Unsquash(sub_args) => cmd_unsquash(ui, command_helper, sub_args),
        Commands::Restore(sub_args) => cmd_restore(ui, command_helper, sub_args),
        Commands::Run(sub_args) => cmd_run(ui, command_helper, sub_args),
        Commands::Diffedit(sub_args) => cmd_diffedit(ui, command_helper, sub_args),
        Commands::Split(sub_args) => cmd_split(ui, command_helper, sub_args),
        Commands::Merge(sub_args) => cmd_merge(ui, command_helper, sub_args),
        Commands::Rebase(sub_args) => cmd_rebase(ui, command_helper, sub_args),
        Commands::Backout(sub_args) => cmd_backout(ui, command_helper, sub_args),
        Commands::Resolve(sub_args) => cmd_resolve(ui, command_helper, sub_args),
        Commands::Branch(sub_args) => branch::cmd_branch(ui, command_helper, sub_args),
        Commands::Undo(sub_args) => operation::cmd_op_undo(ui, command_helper, sub_args),
        Commands::Operation(sub_args) => operation::cmd_operation(ui, command_helper, sub_args),
        Commands::Workspace(sub_args) => cmd_workspace(ui, command_helper, sub_args),
        Commands::Sparse(sub_args) => cmd_sparse(ui, command_helper, sub_args),
        Commands::Chmod(sub_args) => cmd_chmod(ui, command_helper, sub_args),
        Commands::Git(sub_args) => git::cmd_git(ui, command_helper, sub_args),
        Commands::Util(sub_args) => cmd_util(ui, command_helper, sub_args),
        #[cfg(feature = "bench")]
        Commands::Bench(sub_args) => bench::cmd_bench(ui, command_helper, sub_args),
        Commands::Debug(sub_args) => debug::cmd_debug(ui, command_helper, sub_args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_app() {
        default_app().debug_assert();
    }
}
