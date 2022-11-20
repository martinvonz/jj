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

use std::collections::{HashSet, VecDeque};
use std::fmt::Debug;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use std::{fs, io};

use chrono::{FixedOffset, LocalResult, TimeZone, Utc};
use clap::{ArgGroup, ArgMatches, CommandFactory, FromArgMatches, Subcommand};
use itertools::Itertools;
use jujutsu_lib::backend::{BackendError, CommitId, Timestamp, TreeValue};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::commit_builder::CommitBuilder;
use jujutsu_lib::dag_walk::topo_order_reverse;
use jujutsu_lib::diff::{Diff, DiffHunk};
use jujutsu_lib::files::DiffLine;
use jujutsu_lib::git::{GitFetchError, GitRefUpdate};
use jujutsu_lib::index::IndexEntry;
use jujutsu_lib::matchers::{EverythingMatcher, Matcher};
use jujutsu_lib::op_store::{BranchTarget, RefTarget, WorkspaceId};
use jujutsu_lib::operation::Operation;
use jujutsu_lib::refs::{classify_branch_push_action, BranchPushAction, BranchPushUpdate};
use jujutsu_lib::repo::{ReadonlyRepo, RepoRef};
use jujutsu_lib::repo_path::RepoPath;
use jujutsu_lib::revset::RevsetExpression;
use jujutsu_lib::revset_graph_iterator::{RevsetGraphEdge, RevsetGraphEdgeType};
use jujutsu_lib::rewrite::{back_out_commit, merge_commit_trees, rebase_commit, DescendantRebaser};
use jujutsu_lib::settings::UserSettings;
use jujutsu_lib::store::Store;
use jujutsu_lib::tree::{merge_trees, Tree, TreeDiffIterator};
use jujutsu_lib::view::View;
use jujutsu_lib::workspace::Workspace;
use jujutsu_lib::{conflicts, diff, file_util, files, git, revset, tree};
use maplit::{hashmap, hashset};
use pest::Parser;

use crate::cli_util::{
    print_checkout_stats, resolve_base_revs, short_commit_description, short_commit_hash,
    user_error, user_error_with_hint, write_commit_summary, Args, CommandError, CommandHelper,
    WorkspaceCommandHelper,
};
use crate::formatter::{Formatter, PlainTextFormatter};
use crate::graphlog::{AsciiGraphDrawer, Edge};
use crate::progress::Progress;
use crate::template_parser::TemplateParser;
use crate::templater::Template;
use crate::ui::Ui;

#[derive(clap::Parser, Clone, Debug)]
enum Commands {
    Version(VersionArgs),
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
    Interdiff(InterdiffArgs),
    Describe(DescribeArgs),
    Commit(CommitArgs),
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
    Rebase(RebaseArgs),
    Backout(BackoutArgs),
    #[command(subcommand)]
    Branch(BranchSubcommand),
    /// Undo an operation (shortcut for `jj op undo`)
    Undo(OperationUndoArgs),
    #[command(subcommand)]
    #[command(visible_alias = "op")]
    Operation(OperationCommands),
    #[command(subcommand)]
    Workspace(WorkspaceCommands),
    Sparse(SparseArgs),
    #[command(subcommand)]
    Git(GitCommands),
    #[command(subcommand)]
    Debug(DebugCommands),
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

/// Create a new, empty change and edit it in the working copy
///
/// For more information, see
/// https://github.com/martinvonz/jj/blob/main/docs/working-copy.md.
#[derive(clap::Args, Clone, Debug)]
#[command(visible_aliases = &["co", "update", "up"])]
struct CheckoutArgs {
    /// The revision to update to
    revision: String,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
    /// The change description to use
    #[arg(long, short, default_value = "")]
    message: String,
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
    revision: String,
    /// Only list files matching these prefixes (instead of all files)
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

/// Print contents of a file in a revision
#[derive(clap::Args, Clone, Debug)]
struct PrintArgs {
    /// The revision to get the file contents from
    #[arg(long, short, default_value = "@")]
    revision: String,
    /// The file to print
    #[arg(value_hint = clap::ValueHint::FilePath)]
    path: String,
}

#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("format").args(&["summary", "git", "color_words"])))]
struct DiffFormatArgs {
    /// For each path, show only whether it was modified, added, or removed
    #[arg(long, short)]
    summary: bool,
    /// Show a Git-format diff
    #[arg(long)]
    git: bool,
    /// Show a word-level diff with changes indicated only by color
    #[arg(long)]
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
/// given revisions. If either is left out, it defaults to the working-copy
/// commit. For example, `jj diff --from main` shows the changes from "main"
/// (perhaps a branch name) to the working-copy commit.
#[derive(clap::Args, Clone, Debug)]
struct DiffArgs {
    /// Show changes in this revision, compared to its parent(s)
    #[arg(long, short)]
    revision: Option<String>,
    /// Show changes from this revision
    #[arg(long, conflicts_with = "revision")]
    from: Option<String>,
    /// Show changes to this revision
    #[arg(long, conflicts_with = "revision")]
    to: Option<String>,
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
    revision: String,
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
    /// Which revisions to show. Defaults to the `ui.default-revset` setting,
    /// or `@ | (remote_branches() | tags()).. | ((remote_branches() |
    /// tags())..)-` if it is not set.
    #[arg(long, short)]
    revisions: Option<String>,
    /// Show commits modifying the given paths
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
    /// Show revisions in the opposite order (older revisions first)
    #[arg(long)]
    reversed: bool,
    /// Don't show the graph, show a flat list of revisions
    #[arg(long)]
    no_graph: bool,
    /// Render each revision using the given template (the syntax is not yet
    /// documented and is likely to change)
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
    revision: String,
    /// Don't show the graph, show a flat list of revisions
    #[arg(long)]
    no_graph: bool,
    /// Render each revision using the given template (the syntax is not yet
    /// documented and is likely to change)
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
    from: Option<String>,
    /// Show changes to this revision
    #[arg(long)]
    to: Option<String>,
    /// Restrict the diff to these paths
    #[arg(value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
    #[command(flatten)]
    format: DiffFormatArgs,
}

/// Edit the change description
///
/// Starts an editor to let you edit the description of a change. The editor
/// will be $EDITOR, or `pico` if that's not defined.
#[derive(clap::Args, Clone, Debug)]
struct DescribeArgs {
    /// The revision whose description to edit
    #[arg(default_value = "@")]
    revision: String,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
    /// The change description to use (don't open editor)
    #[arg(long, short)]
    message: Option<String>,
    /// Read the change description from stdin
    #[arg(long)]
    stdin: bool,
}

/// Update the description and create a new change on top.
#[derive(clap::Args, Clone, Debug)]
#[command(hide = true)]
struct CommitArgs {
    /// The change description to use (don't open editor)
    #[arg(long, short)]
    message: Option<String>,
}

/// Create a new change with the same content as an existing one
#[derive(clap::Args, Clone, Debug)]
struct DuplicateArgs {
    /// The revision to duplicate
    #[arg(default_value = "@")]
    revision: String,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
}

/// Abandon a revision
///
/// Abandon a revision, rebasing descendants onto its parent(s). The behavior is
/// similar to `jj restore`; the difference is that `jj abandon` gives you a new
/// change, while `jj restore` updates the existing change.
#[derive(clap::Args, Clone, Debug)]
#[command(visible_alias = "hide")]
struct AbandonArgs {
    /// The revision(s) to abandon
    #[arg(default_value = "@")]
    revisions: Vec<String>,
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
    revision: String,
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
struct NewArgs {
    /// Parent(s) of the new change
    #[arg(default_value = "@")]
    revisions: Vec<String>,
    /// Ignored (but lets you pass `-r` for consistency with other commands)
    #[arg(short = 'r', hide = true)]
    unused_revision: bool,
    /// The change description to use
    #[arg(long, short, default_value = "")]
    message: String,
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
    from: Option<String>,
    /// Move part of the source into this change
    #[arg(long)]
    to: Option<String>,
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
    revision: String,
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
    revision: String,
    /// Interactively choose which parts to unsquash
    // TODO: It doesn't make much sense to run this without -i. We should make that
    // the default.
    #[arg(long, short)]
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
#[derive(clap::Args, Clone, Debug)]
struct RestoreArgs {
    /// Revision to restore from (source)
    #[arg(long)]
    from: Option<String>,
    /// Revision to restore into (destination)
    #[arg(long)]
    to: Option<String>,
    /// Interactively choose which parts to restore
    #[arg(long, short)]
    interactive: bool,
    /// Restore only these paths (instead of all paths)
    #[arg(conflicts_with = "interactive", value_hint = clap::ValueHint::AnyPath)]
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
    #[arg(long, short, default_value = "@")]
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
    #[arg(long, short, default_value = "@")]
    revision: String,
    /// Put these paths in the first commit and don't run the diff editor
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
///
/// Note that you can create a merge commit by repeating the `-d` argument.
/// For example, if you realize that commit C actually depends on commit D in
/// order to work (in addition to its current parent B), you can run `jj rebase
/// -s C -d B -d D`:
///
/// D          C'
/// |          |\
/// | C        D |
/// | |   =>   | |
/// | B        | B
/// |/         |/
/// A          A
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
#[command(group(ArgGroup::new("to_rebase").args(&["branch", "source", "revision"])))]
struct RebaseArgs {
    /// Rebase the whole branch (relative to destination's ancestors)
    #[arg(long, short)]
    branch: Option<String>,
    /// Rebase this revision and its descendants
    #[arg(long, short)]
    source: Option<String>,
    /// Rebase only this revision, rebasing descendants onto this revision's
    /// parent(s)
    #[arg(long, short)]
    revision: Option<String>,
    /// The revision(s) to rebase onto
    #[arg(long, short, required = true)]
    destination: Vec<String>,
}

/// Apply the reverse of a revision on top of another revision
#[derive(clap::Args, Clone, Debug)]
struct BackoutArgs {
    /// The revision to apply the reverse of
    #[arg(long, short, default_value = "@")]
    revision: String,
    /// The revision to apply the reverse changes on top of
    // TODO: It seems better to default this to `@-`. Maybe the working
    // copy should be rebased on top?
    #[arg(long, short, default_value = "@")]
    destination: Vec<String>,
}

/// Manage branches.
///
/// For information about branches, see
/// https://github.com/martinvonz/jj/blob/main/docs/branches.md.
#[derive(clap::Subcommand, Clone, Debug)]
enum BranchSubcommand {
    /// Create a new branch.
    #[command(visible_alias("c"))]
    Create {
        /// The branch's target revision.
        #[arg(long, short)]
        revision: Option<String>,

        /// The branches to create.
        #[arg(required = true)]
        names: Vec<String>,
    },

    /// Delete an existing branch and propagate the deletion to remotes on the
    /// next push.
    #[command(visible_alias("d"))]
    Delete {
        /// The branches to delete.
        #[arg(required = true)]
        names: Vec<String>,
    },

    /// Forget everything about a branch, including its local and remote
    /// targets.
    ///
    /// A forgotten branch will not impact remotes on future pushes. It will be
    /// recreated on future pulls if it still exists in the remote.
    #[command(visible_alias("f"))]
    Forget {
        /// The branches to delete.
        #[arg(required = true)]
        names: Vec<String>,
    },

    /// List branches and their targets
    ///
    /// A remote branch will be included only if its target is different from
    /// the local target. For a conflicted branch (both local and remote), old
    /// target revisions are preceded by a "-" and new target revisions are
    /// preceded by a "+". For information about branches, see
    /// https://github.com/martinvonz/jj/blob/main/docs/branches.md.
    #[command(visible_alias("l"))]
    List,

    /// Update a given branch to point to a certain commit.
    #[command(visible_alias("s"))]
    Set {
        /// The branch's target revision.
        #[arg(long, short)]
        revision: Option<String>,

        /// Allow moving the branch backwards or sideways.
        #[arg(long, short = 'B')]
        allow_backwards: bool,

        /// The branches to update.
        #[arg(required = true)]
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
    #[arg(default_value = "@")]
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
    #[arg(long)]
    name: Option<String>,
}

/// Stop tracking a workspace's working-copy commit in the repo
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

/// Manage which paths from the working-copy commit are present in the working
/// copy
#[derive(clap::Args, Clone, Debug)]
struct SparseArgs {
    /// Patterns to add to the working copy
    #[arg(long, value_hint = clap::ValueHint::AnyPath)]
    add: Vec<String>,
    /// Patterns to remove from the working copy
    #[arg(long, conflicts_with = "clear", value_hint = clap::ValueHint::AnyPath)]
    remove: Vec<String>,
    /// Include no files in the working copy (combine with --add)
    #[arg(long)]
    clear: bool,
    /// Include all files in the working copy
    #[arg(long, conflicts_with_all = &["add", "remove", "clear"])]
    reset: bool,
    /// List patterns
    #[arg(long, conflicts_with_all = &["add", "remove", "clear", "reset"])]
    list: bool,
}

/// Commands for working with the underlying Git repo
///
/// For a comparison with Git, including a table of commands, see
/// https://github.com/martinvonz/jj/blob/main/docs/git-comparison.md.
#[derive(Subcommand, Clone, Debug)]
enum GitCommands {
    #[command(subcommand)]
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
    Rename(GitRemoteRenameArgs),
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

/// Rename a Git remote
#[derive(clap::Args, Clone, Debug)]
struct GitRemoteRenameArgs {
    /// The name of an existing remote
    old: String,
    /// The desired name for `old`
    new: String,
}

/// List Git remotes
#[derive(clap::Args, Clone, Debug)]
struct GitRemoteListArgs {}

/// Fetch from a Git remote
#[derive(clap::Args, Clone, Debug)]
struct GitFetchArgs {
    /// The remote to fetch from (only named remotes are supported)
    #[arg(long, default_value = "origin")]
    remote: String,
}

/// Create a new repo backed by a clone of a Git repo
///
/// The Git repo will be a bare git repo stored inside the `.jj/` directory.
#[derive(clap::Args, Clone, Debug)]
struct GitCloneArgs {
    /// URL or path of the Git repo to clone
    #[arg(value_hint = clap::ValueHint::DirPath)]
    source: String,
    /// The directory to write the Jujutsu repo to
    #[arg(value_hint = clap::ValueHint::DirPath)]
    destination: Option<String>,
}

/// Push to a Git remote
///
/// By default, pushes any branches pointing to `@`, or `@-` if no branches
/// point to `@`. Use `--branch` to push a specific branch. Use `--all` to push
/// all branches. Use `--change` to generate a branch name based on a specific
/// commit's change ID.
#[derive(clap::Args, Clone, Debug)]
#[command(group(ArgGroup::new("what").args(&["branch", "all", "change"])))]
struct GitPushArgs {
    /// The remote to push to (only named remotes are supported)
    #[arg(long, default_value = "origin")]
    remote: String,
    /// Push only this branch
    #[arg(long)]
    branch: Option<String>,
    /// Push all branches
    #[arg(long)]
    all: bool,
    /// Push this commit by creating a branch based on its change ID
    #[arg(long)]
    change: Option<String>,
    /// Only display what will change on the remote
    #[arg(long)]
    dry_run: bool,
}

/// Update repo with changes made in the underlying Git repo
#[derive(clap::Args, Clone, Debug)]
struct GitImportArgs {}

/// Update the underlying Git repo with changes made in the repo
#[derive(clap::Args, Clone, Debug)]
struct GitExportArgs {}

/// Low-level commands not intended for users
#[derive(Subcommand, Clone, Debug)]
#[command(hide = true)]
enum DebugCommands {
    Completion(DebugCompletionArgs),
    Mangen(DebugMangenArgs),
    #[command(name = "resolverev")]
    ResolveRev(DebugResolveRevArgs),
    #[command(name = "workingcopy")]
    WorkingCopy(DebugWorkingCopyArgs),
    Template(DebugTemplateArgs),
    Index(DebugIndexArgs),
    #[command(name = "reindex")]
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
    #[arg(long, verbatim_doc_comment)]
    bash: bool,
    /// Print a completion script for Fish
    ///
    /// Apply it by running this:
    ///
    /// autoload -U compinit
    /// compinit
    /// source <(jj debug completion --zsh | sed '$d')  # remove the last line
    /// compdef _jj jj
    #[arg(long, verbatim_doc_comment)]
    fish: bool,
    /// Print a completion script for Zsh
    ///
    /// Apply it by running this:
    ///
    /// jj debug completion --fish | source
    #[arg(long, verbatim_doc_comment)]
    zsh: bool,
}

/// Print a ROFF (manpage)
#[derive(clap::Args, Clone, Debug)]
struct DebugMangenArgs {}

/// Resolve a revision identifier to its full ID
#[derive(clap::Args, Clone, Debug)]
struct DebugResolveRevArgs {
    #[arg(long, short, default_value = "@")]
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
    #[arg(default_value = "@")]
    operation: String,
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

fn cmd_version(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &VersionArgs,
) -> Result<(), CommandError> {
    ui.write(&command.app().render_version())?;
    Ok(())
}

fn cmd_init(ui: &mut Ui, command: &CommandHelper, args: &InitArgs) -> Result<(), CommandError> {
    if command.global_args().repository.is_some() {
        return Err(user_error("'--repository' cannot be used with 'init'"));
    }
    let wc_path = ui.cwd().join(&args.destination);
    match fs::create_dir(&wc_path) {
        Ok(()) => {}
        Err(_) if wc_path.is_dir() => {}
        Err(e) => return Err(user_error(format!("Failed to create workspace: {e}"))),
    }
    let wc_path = wc_path
        .canonicalize()
        .map_err(|e| user_error(format!("Failed to create workspace: {e}")))?; // raced?

    if let Some(git_store_str) = &args.git_repo {
        let mut git_store_path = ui.cwd().join(git_store_str);
        git_store_path = git_store_path
            .canonicalize()
            .map_err(|_| user_error(format!("{} doesn't exist", git_store_path.display())))?;
        if !git_store_path.ends_with(".git") {
            git_store_path = git_store_path.join(".git");
        }
        // If the git repo is inside the workspace, use a relative path to it so the
        // whole workspace can be moved without breaking.
        if let Ok(relative_path) = git_store_path.strip_prefix(&wc_path) {
            git_store_path = PathBuf::from("..")
                .join("..")
                .join("..")
                .join(relative_path);
        }
        let (workspace, repo) =
            Workspace::init_external_git(ui.settings(), &wc_path, &git_store_path)?;
        let git_repo = repo.store().git_repo().unwrap();
        let mut workspace_command = command.for_loaded_repo(ui, workspace, repo)?;
        workspace_command.snapshot(ui)?;
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
        Workspace::init_internal_git(ui.settings(), &wc_path)?;
    } else {
        if !ui.settings().allow_native_backend() {
            return Err(user_error_with_hint(
                "The native backend is disallowed by default.",
                "Did you mean to pass `--git`?
Set `ui.allow-init-native` to allow initializing a repo with the native backend.",
            ));
        }
        Workspace::init_local(ui.settings(), &wc_path)?;
    };
    let cwd = ui.cwd().canonicalize().unwrap();
    let relative_wc_path = file_util::relative_path(&cwd, &wc_path);
    writeln!(ui, "Initialized repo in \"{}\"", relative_wc_path.display())?;
    if args.git && wc_path.join(".git").exists() {
        ui.write_warn("Empty repo created.\n")?;
        ui.write_hint(format!(
            "Hint: To create a repo backed by the existing Git repo, run `jj init --git-repo={}` \
             instead.\n",
            relative_wc_path.display()
        ))?;
    }
    Ok(())
}

fn cmd_checkout(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &CheckoutArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let target = workspace_command.resolve_single_rev(&args.revision)?;
    let workspace_id = workspace_command.workspace_id();
    let mut tx =
        workspace_command.start_transaction(&format!("check out commit {}", target.id().hex()));
    let commit_builder = CommitBuilder::for_new_commit(
        ui.settings(),
        vec![target.id().clone()],
        target.tree_id().clone(),
    )
    .set_description(args.message.clone());
    let new_commit = commit_builder.write_to_repo(tx.mut_repo());
    tx.mut_repo().edit(workspace_id, &new_commit).unwrap();
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn cmd_untrack(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &UntrackArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let store = workspace_command.repo().store().clone();
    let matcher = workspace_command.matcher_from_values(&args.paths)?;

    let mut tx = workspace_command.start_transaction("untrack paths");
    let base_ignores = workspace_command.base_ignores();
    let (mut locked_working_copy, wc_commit) = workspace_command.start_working_copy_mutation()?;
    // Create a new tree without the unwanted files
    let mut tree_builder = store.tree_builder(wc_commit.tree_id().clone());
    for (path, _value) in wc_commit.tree().entries_matching(matcher.as_ref()) {
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
            let message = if added_back.len() > 1 {
                format!(
                    "'{}' and {} other files are not ignored.",
                    ui_path,
                    added_back.len() - 1
                )
            } else {
                format!("'{}' is not ignored.", ui_path)
            };
            return Err(user_error_with_hint(
                message,
                "Files that are not ignored will be added back by the next command.
Make sure they're ignored, then try again.",
            ));
        } else {
            // This means there were some concurrent changes made in the working copy. We
            // don't want to mix those in, so reset the working copy again.
            locked_working_copy.reset(&new_tree)?;
        }
    }
    CommitBuilder::for_rewrite_from(ui.settings(), &wc_commit)
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
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    for (name, _value) in commit.tree().entries_matching(matcher.as_ref()) {
        writeln!(ui, "{}", &workspace_command.format_file_path(&name))?;
    }
    Ok(())
}

fn cmd_print(ui: &mut Ui, command: &CommandHelper, args: &PrintArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let commit = workspace_command.resolve_single_rev(&args.revision)?;
    let path = workspace_command.parse_file_path(&args.path)?;
    let repo = workspace_command.repo();
    match commit.tree().path_value(&path) {
        None => {
            return Err(user_error("No such path"));
        }
        Some(TreeValue::File { id, .. }) => {
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
            return Err(user_error("Path exists but is not a file"));
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
        formatter.with_label("removed", |formatter| {
            formatter.write_bytes(format!("{:>4}", diff_line.left_line_number).as_bytes())
        })?;
        formatter.write_bytes(b" ")?;
    } else {
        formatter.write_bytes(b"     ")?;
    }
    if diff_line.has_right_content {
        formatter.with_label("added", |formatter| {
            formatter.write_bytes(format!("{:>4}", diff_line.right_line_number).as_bytes())
        })?;
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
                    formatter.with_label("removed", |formatter| formatter.write_bytes(before))?;
                }
                if !after.is_empty() {
                    formatter.with_label("added", |formatter| formatter.write_bytes(after))?;
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
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
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
    let template = crate::template_parser::parse_commit_template(
        workspace_command.repo().as_repo_ref(),
        &workspace_command.workspace_id(),
        template_string,
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

fn diff_as_bytes(
    workspace_command: &WorkspaceCommandHelper,
    tree_diff: TreeDiffIterator,
    format: DiffFormat,
) -> Result<Vec<u8>, CommandError> {
    let mut diff_bytes: Vec<u8> = vec![];
    let mut formatter = PlainTextFormatter::new(&mut diff_bytes);
    show_diff(&mut formatter, workspace_command, tree_diff, format)?;
    Ok(diff_bytes)
}

fn diff_content(
    repo: &Arc<ReadonlyRepo>,
    path: &RepoPath,
    value: &TreeValue,
) -> Result<Vec<u8>, CommandError> {
    match value {
        TreeValue::File { id, .. } => {
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
        TreeValue::File { executable, .. } => {
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
    formatter.add_label("diff")?;
    for (path, diff) in tree_diff {
        let ui_path = workspace_command.format_file_path(&path);
        match diff {
            tree::Diff::Added(right_value) => {
                let right_content = diff_content(repo, &path, &right_value)?;
                let description = basic_diff_file_type(&right_value);
                formatter.with_label("header", |formatter| {
                    formatter.write_str(&format!("Added {} {}:\n", description, ui_path))
                })?;
                show_color_words_diff_hunks(&[], &right_content, formatter)?;
            }
            tree::Diff::Modified(left_value, right_value) => {
                let left_content = diff_content(repo, &path, &left_value)?;
                let right_content = diff_content(repo, &path, &right_value)?;
                let description = match (left_value, right_value) {
                    (
                        TreeValue::File {
                            executable: left_executable,
                            ..
                        },
                        TreeValue::File {
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
                formatter.with_label("header", |formatter| {
                    formatter.write_str(&format!("{} {}:\n", description, ui_path))
                })?;
                show_color_words_diff_hunks(&left_content, &right_content, formatter)?;
            }
            tree::Diff::Removed(left_value) => {
                let left_content = diff_content(repo, &path, &left_value)?;
                let description = basic_diff_file_type(&left_value);
                formatter.with_label("header", |formatter| {
                    formatter.write_str(&format!("Removed {} {}:\n", description, ui_path))
                })?;
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
        TreeValue::File { id, executable } => {
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
        formatter.with_label("hunk_header", |formatter| {
            writeln!(
                formatter,
                "@@ -{},{} +{},{} @@",
                hunk.left_line_range.start,
                hunk.left_line_range.len(),
                hunk.right_line_range.start,
                hunk.right_line_range.len()
            )
        })?;
        for (line_type, content) in hunk.lines {
            match line_type {
                DiffLineType::Context => {
                    formatter.with_label("context", |formatter| {
                        formatter.write_str(" ")?;
                        formatter.write_all(content)
                    })?;
                }
                DiffLineType::Removed => {
                    formatter.with_label("removed", |formatter| {
                        formatter.write_str("-")?;
                        formatter.write_all(content)
                    })?;
                }
                DiffLineType::Added => {
                    formatter.with_label("added", |formatter| {
                        formatter.write_str("+")?;
                        formatter.write_all(content)
                    })?;
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
    formatter.add_label("diff")?;
    for (path, diff) in tree_diff {
        let path_string = path.to_internal_file_string();
        match diff {
            tree::Diff::Added(right_value) => {
                let right_part = git_diff_part(repo, &path, &right_value)?;
                formatter.with_label("file_header", |formatter| {
                    writeln!(formatter, "diff --git a/{} b/{}", path_string, path_string)?;
                    writeln!(formatter, "new file mode {}", &right_part.mode)?;
                    writeln!(formatter, "index 0000000000..{}", &right_part.hash)?;
                    writeln!(formatter, "--- /dev/null")?;
                    writeln!(formatter, "+++ b/{}", path_string)
                })?;
                show_unified_diff_hunks(formatter, &[], &right_part.content)?;
            }
            tree::Diff::Modified(left_value, right_value) => {
                let left_part = git_diff_part(repo, &path, &left_value)?;
                let right_part = git_diff_part(repo, &path, &right_value)?;
                formatter.with_label("file_header", |formatter| {
                    writeln!(formatter, "diff --git a/{} b/{}", path_string, path_string)?;
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
                    Ok(())
                })?;
                show_unified_diff_hunks(formatter, &left_part.content, &right_part.content)?;
            }
            tree::Diff::Removed(left_value) => {
                let left_part = git_diff_part(repo, &path, &left_value)?;
                formatter.with_label("file_header", |formatter| {
                    writeln!(formatter, "diff --git a/{} b/{}", path_string, path_string)?;
                    writeln!(formatter, "deleted file mode {}", &left_part.mode)?;
                    writeln!(formatter, "index {}..0000000000", &left_part.hash)?;
                    writeln!(formatter, "--- a/{}", path_string)?;
                    writeln!(formatter, "+++ /dev/null")
                })?;
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
    formatter.with_label("diff", |formatter| {
        for (repo_path, diff) in tree_diff {
            match diff {
                tree::Diff::Modified(_, _) => {
                    formatter.with_label("modified", |formatter| {
                        writeln!(
                            formatter,
                            "M {}",
                            workspace_command.format_file_path(&repo_path)
                        )
                    })?;
                }
                tree::Diff::Added(_) => {
                    formatter.with_label("added", |formatter| {
                        writeln!(
                            formatter,
                            "A {}",
                            workspace_command.format_file_path(&repo_path)
                        )
                    })?;
                }
                tree::Diff::Removed(_) => {
                    formatter.with_label("removed", |formatter| {
                        writeln!(
                            formatter,
                            "R {}",
                            workspace_command.format_file_path(&repo_path)
                        )
                    })?;
                }
            }
        }
        Ok(())
    })
}

fn cmd_status(
    ui: &mut Ui,
    command: &CommandHelper,
    _args: &StatusArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let maybe_checkout_id = repo
        .view()
        .get_wc_commit_id(&workspace_command.workspace_id());
    let maybe_checkout = maybe_checkout_id
        .map(|id| repo.store().get_commit(id))
        .transpose()?;
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();
    if let Some(wc_commit) = &maybe_checkout {
        formatter.write_str("Parent commit: ")?;
        let workspace_id = workspace_command.workspace_id();
        write_commit_summary(
            formatter,
            repo.as_repo_ref(),
            &workspace_id,
            &wc_commit.parents()[0],
            ui.settings(),
        )?;
        formatter.write_str("\n")?;
        formatter.write_str("Working copy : ")?;
        write_commit_summary(
            formatter,
            repo.as_repo_ref(),
            &workspace_id,
            wc_commit,
            ui.settings(),
        )?;
        formatter.write_str("\n")?;
    } else {
        formatter.write_str("No working copy\n")?;
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
        formatter.with_label("conflict", |formatter| {
            writeln!(formatter, "These branches have conflicts:")
        })?;
        for branch_name in conflicted_local_branches {
            write!(formatter, "  ")?;
            formatter.with_label("branch", |formatter| write!(formatter, "{}", branch_name))?;
            writeln!(formatter)?;
        }
        writeln!(
            formatter,
            "  Use `jj branch list` to see details. Use `jj branch set <name> -r <rev>` to \
             resolve."
        )?;
    }
    if !conflicted_remote_branches.is_empty() {
        formatter.with_label("conflict", |formatter| {
            writeln!(formatter, "These remote branches have conflicts:")
        })?;
        for (branch_name, remote_name) in conflicted_remote_branches {
            write!(formatter, "  ")?;
            formatter.with_label("branch", |formatter| {
                write!(formatter, "{}@{}", branch_name, remote_name)
            })?;
            writeln!(formatter)?;
        }
        writeln!(
            formatter,
            "  Use `jj branch list` to see details. Use `jj git fetch` to resolve."
        )?;
    }

    if let Some(wc_commit) = &maybe_checkout {
        let parent_tree = wc_commit.parents()[0].tree();
        let tree = wc_commit.tree();
        if tree.id() == parent_tree.id() {
            formatter.write_str("The working copy is clean\n")?;
        } else {
            formatter.write_str("Working copy changes:\n")?;
            show_diff_summary(
                formatter,
                &workspace_command,
                parent_tree.diff(&tree, &EverythingMatcher),
            )?;
        }

        let conflicts = tree.conflicts();
        if !conflicts.is_empty() {
            formatter.with_label("conflict", |formatter| {
                writeln!(formatter, "There are unresolved conflicts at these paths:")
            })?;
            for (path, _) in conflicts {
                writeln!(formatter, "{}", &workspace_command.format_file_path(&path))?;
            }
        }
    }

    Ok(())
}

fn log_template(settings: &UserSettings) -> String {
    // TODO: define a method on boolean values, so we can get auto-coloring
    //       with e.g. `conflict.then("conflict")`
    let default_template = r#"
            change_id.short()
            " " author.email()
            " " label("timestamp", author.timestamp())
            if(branches, " " branches)
            if(tags, " " tags)
            if(working_copies, " " working_copies)
            if(is_git_head, label("git_head", " HEAD@git"))
            if(divergent, label("divergent", " divergent"))
            " " commit_id.short()
            if(conflict, label("conflict", " conflict"))
            "\n"
            description.first_line()
            "\n""#;
    settings
        .config()
        .get_string("template.log.graph")
        .unwrap_or_else(|_| default_template.to_string())
}

fn cmd_log(ui: &mut Ui, command: &CommandHelper, args: &LogArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;

    let default_revset = ui.settings().default_revset();
    let revset_expression =
        workspace_command.parse_revset(args.revisions.as_ref().unwrap_or(&default_revset))?;
    let repo = workspace_command.repo();
    let workspace_id = workspace_command.workspace_id();
    let checkout_id = repo.view().get_wc_commit_id(&workspace_id);
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let revset = workspace_command.evaluate_revset(&revset_expression)?;
    let revset = if !args.paths.is_empty() {
        revset::filter_by_diff(repo.as_repo_ref(), matcher.as_ref(), revset)
    } else {
        revset
    };

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

    {
        let mut formatter = ui.stdout_formatter();
        let mut formatter = formatter.as_mut();
        formatter.add_label("log")?;

        if !args.no_graph {
            let mut graph = AsciiGraphDrawer::new(&mut formatter);
            let iter: Box<dyn Iterator<Item = (IndexEntry, Vec<RevsetGraphEdge>)>> =
                if args.reversed {
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
                    let mut formatter = ui.new_formatter(&mut buffer);
                    if is_checkout {
                        formatter.with_label("working_copy", |formatter| {
                            template.format(&commit, formatter)
                        })?;
                    } else {
                        template.format(&commit, formatter.as_mut())?;
                    }
                }
                if !buffer.ends_with(b"\n") {
                    buffer.push(b'\n');
                }
                if let Some(diff_format) = diff_format {
                    let mut formatter = ui.new_formatter(&mut buffer);
                    show_patch(
                        formatter.as_mut(),
                        &workspace_command,
                        &commit,
                        matcher.as_ref(),
                        diff_format,
                    )?;
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
                    show_patch(
                        formatter,
                        &workspace_command,
                        &commit,
                        matcher.as_ref(),
                        diff_format,
                    )?;
                }
            }
        }
    }

    // Check to see if the user might have specified a path when they intended
    // to specify a revset.
    if let (None, [only_path]) = (&args.revisions, args.paths.as_slice()) {
        if only_path == "." && workspace_command.parse_file_path(only_path)?.is_root() {
            // For users of e.g. Mercurial, where `.` indicates the current commit.
            ui.write_warn(&format!(
                "warning: The argument {only_path:?} is being interpreted as a path, but this is \
                 often not useful because all non-empty commits touch '.'.  If you meant to show \
                 the working copy commit, pass -r '@' instead.\n"
            ))?;
        } else if revset.is_empty() && revset::parse(only_path, None).is_ok() {
            ui.write_warn(&format!(
                "warning: The argument {only_path:?} is being interpreted as a path. To specify a \
                 revset, pass -r {only_path:?} instead.\n"
            ))?;
        }
    }

    Ok(())
}

fn show_patch(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    commit: &Commit,
    matcher: &dyn Matcher,
    format: DiffFormat,
) -> Result<(), CommandError> {
    let parents = commit.parents();
    let from_tree = merge_commit_trees(workspace_command.repo().as_repo_ref(), &parents);
    let to_tree = commit.tree();
    let diff_iterator = from_tree.diff(&to_tree, matcher);
    show_diff(formatter, workspace_command, diff_iterator, format)
}

fn cmd_obslog(ui: &mut Ui, command: &CommandHelper, args: &ObslogArgs) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;

    let start_commit = workspace_command.resolve_single_rev(&args.revision)?;
    let workspace_id = workspace_command.workspace_id();
    let wc_commit_id = workspace_command
        .repo()
        .view()
        .get_wc_commit_id(&workspace_id);

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
    formatter.add_label("log")?;

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
                let mut formatter = ui.new_formatter(&mut buffer);
                template.format(&commit, formatter.as_mut())?;
            }
            if !buffer.ends_with(b"\n") {
                buffer.push(b'\n');
            }
            if let Some(diff_format) = diff_format {
                let mut formatter = ui.new_formatter(&mut buffer);
                show_predecessor_patch(
                    formatter.as_mut(),
                    &workspace_command,
                    &commit,
                    diff_format,
                )?;
            }
            let node_symbol = if Some(commit.id()) == wc_commit_id {
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
    let predecessors = commit.predecessors();
    let predecessor = match predecessors.first() {
        Some(predecessor) => predecessor,
        None => return Ok(()),
    };
    let predecessor_tree = rebase_to_dest_parent(workspace_command, predecessor, commit)?;
    let diff_iterator = predecessor_tree.diff(&commit.tree(), &EverythingMatcher);
    show_diff(formatter, workspace_command, diff_iterator, diff_format)
}

fn cmd_interdiff(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &InterdiffArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let from = workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"))?;
    let to = workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"))?;

    let from_tree = rebase_to_dest_parent(&workspace_command, &from, &to)?;
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let diff_iterator = from_tree.diff(&to.tree(), matcher.as_ref());
    show_diff(
        ui.stdout_formatter().as_mut(),
        &workspace_command,
        diff_iterator,
        diff_format_for(ui, &args.format),
    )
}

fn rebase_to_dest_parent(
    workspace_command: &WorkspaceCommandHelper,
    source: &Commit,
    destination: &Commit,
) -> Result<Tree, CommandError> {
    if source.parent_ids() == destination.parent_ids() {
        Ok(source.tree())
    } else {
        let destination_parent_tree = merge_commit_trees(
            workspace_command.repo().as_repo_ref(),
            &destination.parents(),
        );
        let source_parent_tree =
            merge_commit_trees(workspace_command.repo().as_repo_ref(), &source.parents());
        let rebased_tree_id = merge_trees(
            &destination_parent_tree,
            &source_parent_tree,
            &source.tree(),
        )?;
        let tree = workspace_command
            .repo()
            .store()
            .get_tree(&RepoPath::root(), &rebased_tree_id)?;
        Ok(tree)
    }
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
        .map_err(|_| user_error(format!("Failed to run editor '{editor}'")))?;
    if !exit_status.success() {
        return Err(user_error(format!(
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

fn cmd_commit(ui: &mut Ui, command: &CommandHelper, args: &CommitArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let commit_id = workspace_command
        .repo()
        .view()
        .get_wc_commit_id(&workspace_command.workspace_id())
        .ok_or_else(|| user_error("This command requires a working copy"))?;
    let commit = workspace_command.repo().store().get_commit(commit_id)?;

    let mut commit_builder = CommitBuilder::for_rewrite_from(ui.settings(), &commit);
    let description = if let Some(message) = &args.message {
        message.to_string()
    } else {
        edit_description(ui, workspace_command.repo(), commit.description())?
    };
    commit_builder = commit_builder.set_description(description);
    let mut tx = workspace_command.start_transaction(&format!("commit {}", commit.id().hex()));
    let new_commit = commit_builder.write_to_repo(tx.mut_repo());
    let workspace_ids = tx
        .mut_repo()
        .view()
        .workspaces_for_wc_commit_id(commit.id());
    if !workspace_ids.is_empty() {
        let new_checkout = CommitBuilder::for_new_commit(
            ui.settings(),
            vec![new_commit.id().clone()],
            new_commit.tree_id().clone(),
        )
        .write_to_repo(tx.mut_repo());
        for workspace_id in workspace_ids {
            tx.mut_repo().edit(workspace_id, &new_checkout).unwrap();
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
    write_commit_summary(
        ui.stdout_formatter().as_mut(),
        mut_repo.as_repo_ref(),
        &workspace_command.workspace_id(),
        &new_commit,
        ui.settings(),
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
    if workspace_command
        .repo()
        .view()
        .get_wc_commit_id(&workspace_id)
        == Some(new_commit.id())
    {
        ui.write("Already editing that commit\n")?;
    } else {
        let mut tx =
            workspace_command.start_transaction(&format!("edit commit {}", new_commit.id().hex()));
        tx.mut_repo().edit(workspace_id, &new_commit)?;
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_new(ui: &mut Ui, command: &CommandHelper, args: &NewArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    assert!(
        !args.revisions.is_empty(),
        "expected a non-empty list from clap"
    );
    let commits = resolve_base_revs(&workspace_command, &args.revisions)?;
    let parent_ids = commits.iter().map(|c| c.id().clone()).collect();
    let mut tx = workspace_command.start_transaction("new empty commit");
    let merged_tree = merge_commit_trees(workspace_command.repo().as_repo_ref(), &commits);
    let new_commit =
        CommitBuilder::for_new_commit(ui.settings(), parent_ids, merged_tree.id().clone())
            .set_description(args.message.clone())
            .write_to_repo(tx.mut_repo());
    let workspace_id = workspace_command.workspace_id();
    tx.mut_repo().edit(workspace_id, &new_commit).unwrap();
    workspace_command.finish_transaction(ui, tx)?;
    Ok(())
}

fn combine_messages(
    ui: &Ui,
    repo: &ReadonlyRepo,
    source: &Commit,
    destination: &Commit,
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
            edit_description(ui, repo, &combined)?
        }
    } else {
        destination.description().to_string()
    };
    Ok(description)
}

fn cmd_move(ui: &mut Ui, command: &CommandHelper, args: &MoveArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let source = workspace_command.resolve_single_rev(args.from.as_deref().unwrap_or("@"))?;
    let mut destination =
        workspace_command.resolve_single_rev(args.to.as_deref().unwrap_or("@"))?;
    if source.id() == destination.id() {
        return Err(user_error("Source and destination cannot be the same."));
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
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let new_parent_tree_id = workspace_command.select_diff(
        ui,
        &parent_tree,
        &source_tree,
        &instructions,
        args.interactive,
        matcher.as_ref(),
    )?;
    if &new_parent_tree_id == parent_tree.id() {
        return Err(user_error("No changes to move"));
    }
    let new_parent_tree = repo
        .store()
        .get_tree(&RepoPath::root(), &new_parent_tree_id)?;
    // Apply the reverse of the selected changes onto the source
    let new_source_tree_id = merge_trees(&source_tree, &new_parent_tree, &parent_tree)?;
    let abandon_source = new_source_tree_id == *parent_tree.id();
    if abandon_source {
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
    let description = combine_messages(
        ui,
        workspace_command.repo(),
        &source,
        &destination,
        abandon_source,
    )?;
    CommitBuilder::for_rewrite_from(ui.settings(), &destination)
        .set_tree(new_destination_tree_id)
        .set_description(description)
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
        return Err(user_error("Cannot squash merge commits"));
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
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
    let new_parent_tree_id = workspace_command.select_diff(
        ui,
        &parent.tree(),
        &commit.tree(),
        &instructions,
        args.interactive,
        matcher.as_ref(),
    )?;
    if &new_parent_tree_id == parent.tree_id() {
        return Err(user_error("No changes selected"));
    }
    // Abandon the child if the parent now has all the content from the child
    // (always the case in the non-interactive case).
    let abandon_child = &new_parent_tree_id == commit.tree_id();
    let mut_repo = tx.mut_repo();
    let description =
        combine_messages(ui, workspace_command.repo(), &commit, parent, abandon_child)?;
    let new_parent = CommitBuilder::for_rewrite_from(ui.settings(), parent)
        .set_tree(new_parent_tree_id)
        .set_predecessors(vec![parent.id().clone(), commit.id().clone()])
        .set_description(description)
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
        return Err(user_error("Cannot unsquash merge commits"));
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
            return Err(user_error("No changes selected"));
        }
    } else {
        new_parent_tree_id = parent_base_tree.id().clone();
    }
    // Abandon the parent if it is now empty (always the case in the non-interactive
    // case).
    if &new_parent_tree_id == parent_base_tree.id() {
        tx.mut_repo().record_abandoned_commit(parent.id().clone());
        let description = combine_messages(ui, workspace_command.repo(), parent, &commit, true)?;
        // Commit the new child on top of the parent's parents.
        CommitBuilder::for_rewrite_from(ui.settings(), &commit)
            .set_parents(parent.parent_ids().to_vec())
            .set_description(description)
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
        let matcher = workspace_command.matcher_from_values(&args.paths)?;
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
        write_commit_summary(
            ui.stdout_formatter().as_mut(),
            mut_repo.as_repo_ref(),
            &workspace_command.workspace_id(),
            &new_commit,
            ui.settings(),
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
        write_commit_summary(
            ui.stdout_formatter().as_mut(),
            mut_repo.as_repo_ref(),
            &workspace_command.workspace_id(),
            &new_commit,
            ui.settings(),
        )?;
        ui.write("\n")?;
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn description_template_for_cmd_split(
    workspace_command: &WorkspaceCommandHelper,
    intro: &str,
    overall_commit_description: &str,
    diff_iter: TreeDiffIterator,
) -> Result<String, CommandError> {
    let diff_summary_bytes = diff_as_bytes(workspace_command, diff_iter, DiffFormat::Summary)?;
    let diff_summary = std::str::from_utf8(&diff_summary_bytes).expect(
        "Summary diffs and repo paths must always be valid UTF8.",
        // Double-check this assumption for diffs that include file content.
    );
    Ok(format!("JJ: {intro}\n{overall_commit_description}\n")
        + "JJ: This part contains the following changes:\n"
        + &textwrap::indent(diff_summary, "JJ:     "))
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
(parent) commit. The remainder will be in the second commit. If you
don't make any changes, then the operation will be aborted.
",
        short_commit_description(&commit)
    );
    let matcher = workspace_command.matcher_from_values(&args.paths)?;
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
        let middle_tree = workspace_command
            .repo()
            .store()
            .get_tree(&RepoPath::root(), &tree_id)?;

        let first_template = description_template_for_cmd_split(
            &workspace_command,
            "Enter commit description for the first part (parent).",
            commit.description(),
            base_tree.diff(&middle_tree, &EverythingMatcher),
        )?;
        let first_description = edit_description(ui, tx.base_repo(), &first_template)?;
        let first_commit = CommitBuilder::for_rewrite_from(ui.settings(), &commit)
            .set_tree(tree_id)
            .set_description(first_description)
            .write_to_repo(tx.mut_repo());
        let second_template = description_template_for_cmd_split(
            &workspace_command,
            "Enter commit description for the second part (child).",
            commit.description(),
            middle_tree.diff(&commit.tree(), &EverythingMatcher),
        )?;
        let second_description = edit_description(ui, tx.base_repo(), &second_template)?;
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
        write_commit_summary(
            ui.stdout_formatter().as_mut(),
            tx.repo().as_repo_ref(),
            &workspace_command.workspace_id(),
            &first_commit,
            ui.settings(),
        )?;
        ui.write("\nSecond part: ")?;
        write_commit_summary(
            ui.stdout_formatter().as_mut(),
            tx.repo().as_repo_ref(),
            &workspace_command.workspace_id(),
            &second_commit,
            ui.settings(),
        )?;
        ui.write("\n")?;
        workspace_command.finish_transaction(ui, tx)?;
    }
    Ok(())
}

fn cmd_merge(ui: &mut Ui, command: &CommandHelper, args: &NewArgs) -> Result<(), CommandError> {
    if args.revisions.len() < 2 {
        return Err(CommandError::CliError(String::from(
            "Merge requires at least two revisions",
        )));
    }
    cmd_new(ui, command, args)
}

fn cmd_rebase(ui: &mut Ui, command: &CommandHelper, args: &RebaseArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let new_parents = resolve_base_revs(&workspace_command, &args.destination)?;
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
    let store = workspace_command.repo().store();
    for root_result in workspace_command
        .evaluate_revset(&roots_expression)
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
    let store = workspace_command.repo().store();

    for child_commit in workspace_command
        .evaluate_revset(&children_expression)
        .unwrap()
        .iter()
        .commits(store)
    {
        let child_commit = child_commit?;
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
        let new_child_parents_expression = RevsetExpression::Difference(
            RevsetExpression::commits(new_child_parent_ids.clone()),
            RevsetExpression::commits(new_child_parent_ids.clone())
                .parents()
                .ancestors(),
        );
        let new_child_parents: Result<Vec<Commit>, BackendError> = workspace_command
            .evaluate_revset(&new_child_parents_expression)
            .unwrap()
            .iter()
            .commits(store)
            .collect();

        rebase_commit(
            ui.settings(),
            tx.mut_repo(),
            &child_commit,
            &new_child_parents?,
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
            .repo()
            .index()
            .is_ancestor(commit.id(), parent.id())
        {
            return Err(user_error(format!(
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
                return Err(user_error(format!("No such branch: {}", branch_name)));
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
                    Some(_) => Err(user_error_with_hint(
                        format!("Branch already exists: {}", branch_name),
                        "Use `jj branch set` to update it.",
                    )),
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
                return Err(user_error_with_hint(
                    "Refusing to move branch backwards or sideways.",
                    "Use --allow-backwards to allow it.",
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
    let print_branch_target = |formatter: &mut dyn Formatter,
                               target: Option<&RefTarget>|
     -> Result<(), CommandError> {
        match target {
            Some(RefTarget::Normal(id)) => {
                write!(formatter, ": ")?;
                let commit = repo.store().get_commit(id)?;
                write_commit_summary(
                    formatter,
                    repo.as_repo_ref(),
                    &workspace_id,
                    &commit,
                    ui.settings(),
                )?;
                writeln!(formatter)?;
            }
            Some(RefTarget::Conflict { adds, removes }) => {
                write!(formatter, " ")?;
                formatter.with_label("conflict", |formatter| write!(formatter, "(conflicted)"))?;
                writeln!(formatter, ":")?;
                for id in removes {
                    let commit = repo.store().get_commit(id)?;
                    write!(formatter, "  - ")?;
                    write_commit_summary(
                        formatter,
                        repo.as_repo_ref(),
                        &workspace_id,
                        &commit,
                        ui.settings(),
                    )?;
                    writeln!(formatter)?;
                }
                for id in adds {
                    let commit = repo.store().get_commit(id)?;
                    write!(formatter, "  + ")?;
                    write_commit_summary(
                        formatter,
                        repo.as_repo_ref(),
                        &workspace_id,
                        &commit,
                        ui.settings(),
                    )?;
                    writeln!(formatter)?;
                }
            }
            None => {
                writeln!(formatter, " (deleted)")?;
            }
        }
        Ok(())
    };

    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();
    let index = repo.index();
    for (name, branch_target) in repo.view().branches() {
        formatter.with_label("branch", |formatter| write!(formatter, "{}", name))?;
        print_branch_target(formatter, branch_target.local_target.as_ref())?;

        for (remote, remote_target) in branch_target
            .remote_targets
            .iter()
            .sorted_by_key(|(name, _target)| name.to_owned())
        {
            if Some(remote_target) == branch_target.local_target.as_ref() {
                continue;
            }
            write!(formatter, "  ")?;
            formatter.with_label("branch", |formatter| write!(formatter, "@{}", remote))?;
            if let Some(local_target) = branch_target.local_target.as_ref() {
                let remote_ahead_count = index
                    .walk_revs(&remote_target.adds(), &local_target.adds())
                    .count();
                let local_ahead_count = index
                    .walk_revs(&local_target.adds(), &remote_target.adds())
                    .count();
                if remote_ahead_count != 0 && local_ahead_count == 0 {
                    write!(formatter, " (ahead by {} commits)", remote_ahead_count)?;
                } else if remote_ahead_count == 0 && local_ahead_count != 0 {
                    write!(formatter, " (behind by {} commits)", local_ahead_count)?;
                } else if remote_ahead_count != 0 && local_ahead_count != 0 {
                    write!(
                        formatter,
                        " (ahead by {} commits, behind by {} commits)",
                        remote_ahead_count, local_ahead_count
                    )?;
                }
            }
            print_branch_target(formatter, Some(remote_target))?;
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
        DebugCommands::Mangen(_mangen_matches) => {
            let mut buf = vec![];
            let man = clap_mangen::Man::new(command.app().clone());
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
            let workspace_command = command.workspace_helper(ui)?;
            let repo = workspace_command.repo();
            let repo = repo.reload_at(repo.operation());
            writeln!(
                ui,
                "Finished indexing {:?} commits.",
                repo.index().num_commits()
            )?;
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

// TODO: Move this somewhere where it can be reused by
// `template_parser::SignatureTimestamp`.
fn format_timestamp(timestamp: &Timestamp) -> String {
    let utc = match Utc.timestamp_opt(
        timestamp.timestamp.0.div_euclid(1000),
        (timestamp.timestamp.0.rem_euclid(1000)) as u32 * 1000000,
    ) {
        LocalResult::None => {
            return "<out-of-range date>".to_string();
        }
        LocalResult::Single(x) => x,
        LocalResult::Ambiguous(y, _z) => y,
    };
    let datetime = utc.with_timezone(
        &FixedOffset::east_opt(timestamp.tz_offset * 60)
            .unwrap_or_else(|| FixedOffset::east_opt(0).unwrap()),
    );
    datetime.format("%Y-%m-%d %H:%M:%S.%3f %:z").to_string()
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
            formatter.with_label("id", |formatter| formatter.write_str(&op.id().hex()[0..12]))?;
            formatter.write_str(" ")?;
            let metadata = &op.store_operation().metadata;
            formatter.with_label("user", |formatter| {
                formatter.write_str(&format!("{}@{}", metadata.username, metadata.hostname))
            })?;
            formatter.write_str(" ")?;
            formatter.with_label("time", |formatter| {
                formatter.write_str(&format!(
                    "{} - {}",
                    format_timestamp(&metadata.start_time),
                    format_timestamp(&metadata.end_time)
                ))
            })?;
            formatter.write_str("\n")?;
            formatter.with_label("description", |formatter| {
                formatter.write_str(&metadata.description)
            })?;
            for (key, value) in &metadata.tags {
                formatter.with_label("tags", |formatter| {
                    formatter.write_str(&format!("\n{}: {}", key, value))
                })?;
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
            let mut formatter = ui.new_formatter(&mut buffer);
            formatter.with_label("op-log", |formatter| {
                if is_head_op {
                    formatter.with_label("head", |formatter| template.format(&op, formatter))
                } else {
                    template.format(&op, formatter)
                }
            })?;
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
        return Err(user_error("Cannot undo a merge operation"));
    }
    if parent_ops.is_empty() {
        return Err(user_error("Cannot undo repo initialization"));
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
            "Workspace named '{}' already exists",
            name
        )));
    }
    let (new_workspace, repo) = Workspace::init_workspace_with_existing_repo(
        ui.settings(),
        &destination_path,
        repo,
        workspace_id,
    )?;
    writeln!(
        ui,
        "Created workspace in \"{}\"",
        file_util::relative_path(old_workspace_command.workspace_root(), &destination_path)
            .display()
    )?;

    let mut new_workspace_command = WorkspaceCommandHelper::new(
        ui,
        new_workspace,
        command.string_args().clone(),
        command.global_args(),
        repo,
    )?;
    let mut tx = new_workspace_command.start_transaction(&format!(
        "Create initial working-copy commit in workspace {}",
        &name
    ));
    // Check out a parent of the current workspace's working-copy commit, or the
    // root if there is no working-copy commit in the current workspace.
    let new_wc_commit = if let Some(old_checkout_id) = new_workspace_command
        .repo()
        .view()
        .get_wc_commit_id(&old_workspace_command.workspace_id())
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
        &new_wc_commit,
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
        .get_wc_commit_id(&workspace_id)
        .is_none()
    {
        return Err(user_error("No such workspace"));
    }

    let mut tx =
        workspace_command.start_transaction(&format!("forget workspace {}", workspace_id.as_str()));
    tx.mut_repo().remove_wc_commit(&workspace_id);
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
    for (workspace_id, checkout_id) in repo.view().wc_commit_ids().iter().sorted() {
        write!(ui, "{}: ", workspace_id.as_str())?;
        let commit = repo.store().get_commit(checkout_id)?;
        write_commit_summary(
            ui.stdout_formatter().as_mut(),
            repo.as_repo_ref(),
            workspace_id,
            &commit,
            ui.settings(),
        )?;
        writeln!(ui)?;
    }
    Ok(())
}

fn cmd_sparse(ui: &mut Ui, command: &CommandHelper, args: &SparseArgs) -> Result<(), CommandError> {
    if args.list {
        let workspace_command = command.workspace_helper(ui)?;
        for path in workspace_command.working_copy().sparse_patterns() {
            let ui_path = workspace_command.format_file_path(path);
            writeln!(ui, "{}", ui_path)?;
        }
    } else {
        let mut workspace_command = command.workspace_helper(ui)?;
        let paths_to_add = args
            .add
            .iter()
            .map(|v| workspace_command.parse_file_path(v))
            .collect::<Result<Vec<_>, _>>()?;
        let paths_to_remove = args
            .remove
            .iter()
            .map(|v| workspace_command.parse_file_path(v))
            .collect::<Result<Vec<_>, _>>()?;
        let (mut locked_wc, _wc_commit) = workspace_command.start_working_copy_mutation()?;
        let mut new_patterns = HashSet::new();
        if args.reset {
            new_patterns.insert(RepoPath::root());
        } else {
            if !args.clear {
                new_patterns.extend(locked_wc.sparse_patterns().iter().cloned());
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
        None => Err(user_error("The repo is not backed by a git repo")),
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
        return Err(user_error("Remote already exists"));
    }
    git_repo
        .remote(&args.remote, &args.url)
        .map_err(|err| user_error(err.to_string()))?;
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
        return Err(user_error("Remote doesn't exist"));
    }
    git_repo
        .remote_delete(&args.remote)
        .map_err(|err| user_error(err.to_string()))?;
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

fn cmd_git_remote_rename(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitRemoteRenameArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    if git_repo.find_remote(&args.old).is_err() {
        return Err(user_error("Remote doesn't exist"));
    }
    git_repo
        .remote_rename(&args.old, &args.new)
        .map_err(|err| user_error(err.to_string()))?;
    let mut tx = workspace_command
        .start_transaction(&format!("rename git remote {} to {}", &args.old, &args.new));
    tx.mut_repo().rename_remote(&args.old, &args.new);
    if tx.mut_repo().has_changes() {
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

#[tracing::instrument(skip(ui, command))]
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
    with_remote_callbacks(ui, |cb| {
        git::fetch(tx.mut_repo(), &git_repo, &args.remote, cb)
    })
    .map_err(|err| user_error(err.to_string()))?;
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
        return Err(user_error("'--repository' cannot be used with 'git clone'"));
    }
    let source = &args.source;
    let wc_path_str = args
        .destination
        .as_deref()
        .or_else(|| clone_destination_for_source(source))
        .ok_or_else(|| user_error("No destination specified and wasn't able to guess it"))?;
    let wc_path = ui.cwd().join(wc_path_str);
    let wc_path_existed = wc_path.exists();
    if wc_path_existed {
        if !is_empty_dir(&wc_path) {
            return Err(user_error(
                "Destination path exists and is not an empty directory",
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
    let (workspace, repo) = Workspace::init_internal_git(ui.settings(), wc_path)?;
    let git_repo = get_git_repo(repo.store())?;
    writeln!(ui, r#"Fetching into new repo in "{}""#, wc_path.display())?;
    let mut workspace_command = command.for_loaded_repo(ui, workspace, repo)?;
    let remote_name = "origin";
    git_repo.remote(remote_name, source).unwrap();
    let mut fetch_tx = workspace_command.start_transaction("fetch from git remote into empty repo");

    let maybe_default_branch = with_remote_callbacks(ui, |cb| {
        git::fetch(fetch_tx.mut_repo(), &git_repo, remote_name, cb)
    })
    .map_err(|err| match err {
        GitFetchError::NoSuchRemote(_) => {
            panic!("shouldn't happen as we just created the git remote")
        }
        GitFetchError::InternalGitError(err) => user_error(format!("Fetch failed: {err}")),
    })?;
    workspace_command.finish_transaction(ui, fetch_tx)?;
    Ok((workspace_command, maybe_default_branch))
}

#[allow(clippy::explicit_auto_deref)] // https://github.com/rust-lang/rust-clippy/issues/9763
fn with_remote_callbacks<T>(ui: &mut Ui, f: impl FnOnce(git::RemoteCallbacks<'_>) -> T) -> T {
    let mut ui = Mutex::new(ui);
    let mut callback = None;
    if ui.get_mut().unwrap().use_progress_indicator() {
        let mut progress = Progress::new(Instant::now());
        let ui = &ui;
        callback = Some(move |x: &git::Progress| {
            _ = progress.update(Instant::now(), x, *ui.lock().unwrap());
        });
    }
    let mut callbacks = git::RemoteCallbacks::default();
    callbacks.progress = callback
        .as_mut()
        .map(|x| x as &mut dyn FnMut(&git::Progress));
    let mut get_ssh_key = get_ssh_key; // Coerce to unit fn type
    callbacks.get_ssh_key = Some(&mut get_ssh_key);
    let mut get_pw = |url: &str, _username: &str| {
        pinentry_get_pw(url).or_else(|| terminal_get_pw(*ui.lock().unwrap(), url))
    };
    callbacks.get_password = Some(&mut get_pw);
    let mut get_user_pw = |url: &str| {
        let ui = &mut *ui.lock().unwrap();
        Some((terminal_get_username(ui, url)?, terminal_get_pw(ui, url)?))
    };
    callbacks.get_username_password = Some(&mut get_user_pw);
    f(callbacks)
}

fn terminal_get_username(ui: &mut Ui, url: &str) -> Option<String> {
    ui.prompt(&format!("Username for {}", url)).ok()
}

fn terminal_get_pw(ui: &mut Ui, url: &str) -> Option<String> {
    ui.prompt_password(&format!("Passphrase for {}: ", url))
        .ok()
}

fn pinentry_get_pw(url: &str) -> Option<String> {
    let mut pinentry = Command::new("pinentry")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    #[rustfmt::skip]
    pinentry
        .stdin
        .take()
        .unwrap()
        .write_all(
            format!(
                "SETTITLE jj passphrase\n\
                 SETDESC Enter passphrase for {url}\n\
                 SETPROMPT Passphrase:\n\
                 GETPIN\n"
            )
            .as_bytes(),
        )
        .ok()?;
    let mut out = String::new();
    pinentry
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .ok()?;
    _ = pinentry.wait();
    for line in out.split('\n') {
        if !line.starts_with("D ") {
            continue;
        }
        let (_, encoded) = line.split_at(2);
        return decode_assuan_data(encoded);
    }
    None
}

// https://www.gnupg.org/documentation/manuals/assuan/Server-responses.html#Server-responses
fn decode_assuan_data(encoded: &str) -> Option<String> {
    let encoded = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut i = 0;
    while i < encoded.len() {
        if encoded[i] != b'%' {
            decoded.push(encoded[i]);
            i += 1;
            continue;
        }
        i += 1;
        let byte =
            u8::from_str_radix(std::str::from_utf8(encoded.get(i..i + 2)?).ok()?, 16).ok()?;
        decoded.push(byte);
        i += 2;
    }
    String::from_utf8(decoded).ok()
}

#[tracing::instrument]
fn get_ssh_key(_username: &str) -> Option<PathBuf> {
    let home_dir = std::env::var("HOME").ok()?;
    let key_path = std::path::Path::new(&home_dir).join(".ssh").join("id_rsa");
    if key_path.is_file() {
        tracing::debug!(path = ?key_path, "found ssh key");
        Some(key_path)
    } else {
        tracing::debug!(path = ?key_path, "no ssh key found");
        None
    }
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
            .get_wc_commit_id(&workspace_command.workspace_id())
        {
            None => {
                return Err(user_error("Nothing checked out in this workspace"));
            }
            Some(checkout) => {
                fn find_branches_targeting<'a>(
                    view: &'a View,
                    target: &RefTarget,
                ) -> Vec<(&'a String, &'a BranchTarget)> {
                    view.branches()
                        .iter()
                        .filter(|(_, branch_target)| {
                            branch_target.local_target.as_ref() == Some(target)
                        })
                        .collect()
                }

                // Search for branches targeting @
                let mut branches = find_branches_targeting(
                    workspace_command.repo().view(),
                    &RefTarget::Normal(checkout.clone()),
                );
                if branches.is_empty() {
                    // Try @- instead if it has exactly one parent, such as after `jj squash`
                    let commit = workspace_command.repo().store().get_commit(checkout)?;
                    if let [parent] = commit.parent_ids() {
                        branches = find_branches_targeting(
                            workspace_command.repo().view(),
                            &RefTarget::Normal(parent.clone()),
                        );
                    }
                }
                for (branch_name, branch_target) in branches {
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
        if branch_updates.is_empty() {
            return Err(user_error("No current branch."));
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
    let mut force_pushed_branches = hashset! {};
    for (branch_name, update) in &branch_updates {
        let qualified_name = format!("refs/heads/{}", branch_name);
        if let Some(new_target) = &update.new_target {
            new_heads.push(new_target.clone());
            let force = match &update.old_target {
                None => false,
                Some(old_target) => !repo.index().is_ancestor(old_target, new_target),
            };
            if force {
                force_pushed_branches.insert(branch_name.to_string());
            }
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
            return Err(user_error(format!(
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
                if force_pushed_branches.contains(branch_name) {
                    writeln!(
                        ui,
                        "  Force branch {branch_name} from {} to {}",
                        short_commit_hash(old_target),
                        short_commit_hash(new_target)
                    )?;
                } else {
                    writeln!(
                        ui,
                        "  Move branch {branch_name} from {} to {}",
                        short_commit_hash(old_target),
                        short_commit_hash(new_target)
                    )?;
                }
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
    with_remote_callbacks(ui, |cb| {
        git::push_updates(&git_repo, &args.remote, &ref_updates, cb)
    })
    .map_err(|err| user_error(err.to_string()))?;
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
        .ok_or_else(|| user_error(format!("Branch {} doesn't exist", branch_name)))?;
    let push_action = classify_branch_push_action(branch_target, remote_name);

    match push_action {
        BranchPushAction::AlreadyMatches => Ok(None),
        BranchPushAction::LocalConflicted => {
            Err(user_error(format!("Branch {} is conflicted", branch_name)))
        }
        BranchPushAction::RemoteConflicted => Err(user_error(format!(
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
    let mut workspace_command = command.workspace_helper(ui)?;
    let repo = workspace_command.repo();
    let git_repo = get_git_repo(repo.store())?;
    let mut tx = workspace_command.start_transaction("export git refs");
    git::export_refs(tx.mut_repo(), &git_repo)?;
    workspace_command.finish_transaction(ui, tx)?;
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
        GitCommands::Remote(GitRemoteCommands::Rename(command_matches)) => {
            cmd_git_remote_rename(ui, command, command_matches)
        }
        GitCommands::Remote(GitRemoteCommands::List(command_matches)) => {
            cmd_git_remote_list(ui, command, command_matches)
        }
        GitCommands::Push(command_matches) => cmd_git_push(ui, command, command_matches),
        GitCommands::Import(command_matches) => cmd_git_import(ui, command, command_matches),
        GitCommands::Export(command_matches) => cmd_git_export(ui, command, command_matches),
    }
}

pub fn default_app() -> clap::Command {
    let app: clap::Command = Commands::augment_subcommands(Args::command());
    app.arg_required_else_help(true).subcommand_required(true)
}

pub fn run_command(
    ui: &mut Ui,
    command_helper: &CommandHelper,
    matches: &ArgMatches,
) -> Result<(), CommandError> {
    let derived_subcommands: Commands = Commands::from_arg_matches(matches).unwrap();
    match &derived_subcommands {
        Commands::Version(sub_args) => cmd_version(ui, command_helper, sub_args),
        Commands::Init(sub_args) => cmd_init(ui, command_helper, sub_args),
        Commands::Checkout(sub_args) => cmd_checkout(ui, command_helper, sub_args),
        Commands::Untrack(sub_args) => cmd_untrack(ui, command_helper, sub_args),
        Commands::Files(sub_args) => cmd_files(ui, command_helper, sub_args),
        Commands::Print(sub_args) => cmd_print(ui, command_helper, sub_args),
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
        Commands::New(sub_args) => cmd_new(ui, command_helper, sub_args),
        Commands::Move(sub_args) => cmd_move(ui, command_helper, sub_args),
        Commands::Squash(sub_args) => cmd_squash(ui, command_helper, sub_args),
        Commands::Unsquash(sub_args) => cmd_unsquash(ui, command_helper, sub_args),
        Commands::Restore(sub_args) => cmd_restore(ui, command_helper, sub_args),
        Commands::Touchup(sub_args) => cmd_touchup(ui, command_helper, sub_args),
        Commands::Split(sub_args) => cmd_split(ui, command_helper, sub_args),
        Commands::Merge(sub_args) => cmd_merge(ui, command_helper, sub_args),
        Commands::Rebase(sub_args) => cmd_rebase(ui, command_helper, sub_args),
        Commands::Backout(sub_args) => cmd_backout(ui, command_helper, sub_args),
        Commands::Branch(sub_args) => cmd_branch(ui, command_helper, sub_args),
        Commands::Undo(sub_args) => cmd_op_undo(ui, command_helper, sub_args),
        Commands::Operation(sub_args) => cmd_operation(ui, command_helper, sub_args),
        Commands::Workspace(sub_args) => cmd_workspace(ui, command_helper, sub_args),
        Commands::Sparse(sub_args) => cmd_sparse(ui, command_helper, sub_args),
        Commands::Git(sub_args) => cmd_git(ui, command_helper, sub_args),
        Commands::Debug(sub_args) => cmd_debug(ui, command_helper, sub_args),
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
