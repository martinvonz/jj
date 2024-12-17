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

//! This file contains the internal implementation of `run`.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use itertools::Itertools;
use jj_lib::backend::BackendError;
use jj_lib::backend::CommitId;
use jj_lib::backend::MergedTreeId;
use jj_lib::commit::Commit;
use jj_lib::commit::CommitIteratorExt;
use jj_lib::fsmonitor::FsmonitorSettings;
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::local_working_copy::TreeState;
use jj_lib::local_working_copy::TreeStateError;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::object_id::ObjectId;
use jj_lib::repo_path::RepoPath;
use jj_lib::tree::Tree;
use jj_lib::working_copy::SnapshotOptions;
use pollster::FutureExt;
use tokio::runtime::Builder;
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::cli_util::WorkspaceCommandTransaction;
use crate::command_error::CommandError;
use crate::ui::Ui;

#[derive(Debug, thiserror::Error)]
enum RunError {
    #[error("failed to checkout commit")]
    FailedCheckout,
    #[error("the command failed {} for {}", 0, 1)]
    CommandFailure(ExitStatus, CommitId),
    #[error(transparent)]
    IoError(#[from] io::Error),
    #[error("failed to load a commits tree")]
    TreeState(#[from] TreeStateError),
    #[error(transparent)]
    Backend(#[from] BackendError),
}

impl From<RunError> for CommandError {
    fn from(value: RunError) -> Self {
        CommandError::new(crate::command_error::CommandErrorKind::Cli, Box::new(value))
    }
}

/// Creates the required directories for a StoredWorkingCopy.
/// Returns a tuple of (`output_dir`, `working_copy` and `state`).
fn create_working_copy_paths(path: &Path) -> Result<(PathBuf, PathBuf, PathBuf), std::io::Error> {
    tracing::debug!(?path, "creating working copy paths for path");
    let output = path.join("output");
    let working_copy = path.join("working_copy");
    let state = path.join("state");
    std::fs::create_dir(&output)?;
    std::fs::create_dir(&working_copy)?;
    std::fs::create_dir(&state)?;
    Ok((output, working_copy, state))
}

/// Represent a `MergeTreeId` in a way that it may be used as a working-copy
/// name. This makes no stability guarantee, as the format may change at
/// any time.
fn to_wc_name(id: &MergedTreeId) -> String {
    match id {
        MergedTreeId::Legacy(tree_id) => tree_id.hex(),
        MergedTreeId::Merge(tree_ids) => {
            let mut obfuscated = tree_ids
                .map(|id| id.hex())
                .iter_mut()
                .enumerate()
                .map(|(i, s)| {
                    // Incredibly "smart" way to say, append "-" if the number is odd "+"
                    // otherwise.
                    if i & 1 != 0 {
                        s.push('-');
                    } else {
                        s.push('+');
                    }
                    s.to_owned()
                })
                .collect::<String>();
            // `PATH_MAX` could be a problem for different operating systems, so truncate
            // it.
            if obfuscated.len() >= 255 {
                obfuscated.truncate(200);
            }
            obfuscated
        }
    }
}

fn get_runtime(jobs: usize) -> tokio::runtime::Runtime {
    let mut builder = Builder::new_multi_thread();
    builder.max_blocking_threads(jobs);
    builder.build().unwrap()
}

/// A commit stored under `.jj/run/default/`
// TODO: Create a caching backend, which creates these on a dedicated thread or
// threadpool.
struct OnDiskCommit {
    /// Obfuscated name for an easier lookup. Not set for `StoredCommits` which
    /// have no longer have the original commits `TreeState` set.
    name: Option<String>,
    /// The respective commit unmodified.
    commit: Commit,
    /// The output directory of the commit, contains stdout and stderr for it
    output_dir: PathBuf,
    /// Self-explanatory
    working_copy_dir: PathBuf,
    /// Where the state is stored
    state_dir: PathBuf,
    /// The commits `TreeState`, which is loaded on creation and then replaced
    /// if necessary. Protected by a Mutex for crossthread compatibility.
    tree_state: Mutex<TreeState>,
}

impl OnDiskCommit {
    fn new(
        name: Option<String>,
        commit: &Commit,
        output_dir: PathBuf,
        working_copy_dir: PathBuf,
        state_dir: PathBuf,
        tree_state: Mutex<TreeState>,
    ) -> Self {
        Self {
            name,
            commit: commit.clone(),
            output_dir,
            working_copy_dir,
            tree_state,
            state_dir,
        }
    }
}

fn create_output_files(id: &CommitId, path: &Path) -> Result<(File, File), RunError> {
    // We use the hex id of the commit here to allow multiple `std{in,err}`s to be
    // placed beside each other in a single output directory.
    tracing::debug!(?id, "creating output files (stdout, stderr) for commit ");
    let stdout_path = path.join("output").join(format!("stdout.{}", id.hex()));
    let stderr_path = path.join("output").join(format!("stderr.{}", id.hex()));
    let mut file_options = OpenOptions::new();
    let stdout = file_options.write(true).create(true).open(stdout_path)?;
    let stderr = file_options.write(true).create(true).open(stderr_path)?;
    Ok((stdout, stderr))
}

fn create_working_copies(
    repo_path: &Path,
    commits: &[Commit],
) -> Result<Vec<Arc<OnDiskCommit>>, RunError> {
    let mut results = vec![];
    // TODO: should be stored in a backend and not hardcoded.
    // The parent() call is needed to not write under `.jj/repo/`.
    let base_path = repo_path.parent().unwrap().join("run").join("default");
    if !base_path.exists() {
        fs::create_dir_all(&base_path)?;
    }
    tracing::debug!(path = ?base_path, "creating working copies in path: ");
    for commit in commits {
        let name = to_wc_name(commit.tree_id());
        let commit_path = base_path.join(name.as_str());
        fs::create_dir(&commit_path)?;
        tracing::debug!(
            dir = ?commit_path,
            commit = commit.id().hex(),
            "creating directory for the commit"
        );

        let (output_dir, working_copy_dir, state_dir) = create_working_copy_paths(&commit_path)?;
        let tree_state = {
            tracing::debug!(
                commit = commit.id().hex(),
                "trying to create a treestate for commit"
            );
            let mut tree_state = TreeState::init(
                commit.store().clone(),
                working_copy_dir.clone(),
                state_dir.clone(),
            )?;
            tree_state
                .check_out(&commit.tree()?)
                .map_err(|_| RunError::FailedCheckout)?;
            Mutex::new(tree_state)
        };
        let stored_commit = OnDiskCommit::new(
            Some(name),
            commit,
            output_dir,
            working_copy_dir,
            state_dir,
            tree_state,
        );
        results.push(Arc::new(stored_commit));
    }
    Ok(results)
}

/// Get the shell to execute in.
// TODO: use something like `[run].shell`
fn get_shell_executable() -> String {
    if cfg!(target_os = "windows") {
        "cmd /c".into()
    } else {
        "/bin/sh -c".into()
    }
}

/// The result of a single command invocation.
struct RunJob {
    /// The old `CommitId` of the commit.
    old_id: CommitId,
    /// The new tree generated from the commit.
    new_tree: Tree,
}

// TODO: make this more revset/commit stream friendly.
async fn run_inner<'a>(
    tx: &WorkspaceCommandTransaction<'a>,
    sender: Sender<RunJob>,
    handle: &tokio::runtime::Handle,
    shell_command: Arc<String>,
    commits: Arc<Vec<Arc<OnDiskCommit>>>,
) -> Result<(), RunError> {
    let mut command_futures = JoinSet::new();
    for commit in commits.iter() {
        command_futures.spawn_on(
            rewrite_commit(
                tx.base_workspace_helper().base_ignores().unwrap().clone(),
                commit.clone(),
                shell_command.clone(),
            ),
            handle,
        );
    }

    while let Some(res) = command_futures.join_next().await {
        let done = res.unwrap().expect("should not fail joining a job");
        let should_quit = sender.send(done).is_err();
        if should_quit {
            tracing::debug!(
                ?should_quit,
                "receiver is no longer available, exiting loop"
            );
            break;
        }
    }
    Ok(())
}

/// Rewrite a single `OnDiskCommit`. The caller is responsible for creating the
/// final commit.
async fn rewrite_commit<'a>(
    base_ignores: Arc<GitIgnoreFile>,
    stored_commit: Arc<OnDiskCommit>,
    shell_command: Arc<String>,
) -> Result<RunJob, RunError> {
    let (stdout, stderr) =
        create_output_files(stored_commit.commit.id(), &stored_commit.output_dir)?;
    // TODO: Later this should take some trait which allows `run` to integrate with
    // something like Bazels RE protocol.
    // e.g
    // ```
    // let mut executor /* Arc<dyn CommandExecutor> */ = store.get_executor();
    // let command = executor.spawn(...)?; // RE or separate processes depending on impl.
    // ...
    // ```
    tracing::debug!(
        "trying to run {shell_command} on commit {id}",
        id = stored_commit.commit.id().hex(),
        shell_command = shell_command.as_str()
    );
    let mut command = tokio::process::Command::new(get_shell_executable())
        .arg(shell_command.as_str())
        // set cwd to the working copy directory.
        .current_dir(&stored_commit.working_copy_dir)
        // .arg()
        // TODO: relativize
        // .env("JJ_PATH", stored_commit.working_copy_dir)
        .env("JJ_CHANGE", stored_commit.commit.change_id().hex())
        .env("JJ_COMMIT_ID", stored_commit.commit.id().hex())
        .stdout(stdout)
        .stderr(stderr)
        .kill_on_drop(true) // No zombies allowed.
        .spawn()?;

    let commit = stored_commit.commit.clone();
    let old_id = commit.id().clone();

    let status = command.wait().await?;

    if !status.success() {
        return Err(RunError::CommandFailure(status, old_id.clone()));
    }

    let tree_state = &mut stored_commit.tree_state.lock().await;

    let options = SnapshotOptions {
        base_ignores,
        // TODO: read from current wc/settings
        start_tracking_matcher: &EverythingMatcher,
        fsmonitor_settings: FsmonitorSettings::None,
        progress: None,
        // TODO: read from current wc/settings
        max_new_file_size: 64_000_u64, // 64 MB for now,
    };
    tracing::debug!("trying to snapshot the new tree");
    let dirty = tree_state.snapshot(&options).unwrap();
    if !dirty {
        tracing::debug!(
            "commit {:?} was not modified as the passed command did nothing",
            commit.id()
        );
    }

    let rewritten_id = tree_state.current_tree_id().to_merge();
    let new_id = rewritten_id.as_resolved().unwrap();

    let new_tree = commit
        .store()
        .get_tree_async(RepoPath::root(), new_id)
        .await?;

    // TODO: Serialize the new tree into /output/{id-tree}

    Ok(RunJob { old_id, new_tree })
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
/// $ jj run 'pre-commit run .github/pre-commit.yaml' -r (trunk()..@) -j 4
///
/// This allows pre-commit integration and other funny stuff.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub struct RunArgs {
    /// The command to run across all selected revisions.
    shell_command: String,
    /// The revisions to change.
    #[arg(long, short, default_value = "@")]
    revisions: RevisionArg,
    /// A no-op option to match the interface of `git rebase -x`.
    #[arg(short = 'x', hide = true)]
    exec: bool,
    /// How many processes should run in parallel, uses by default all cores.
    #[arg(long, short)]
    jobs: Option<usize>,
}

pub fn cmd_run(ui: &mut Ui, command: &CommandHelper, args: &RunArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;
    // The commits are already returned in reverse topological order.
    let resolved_commits: Vec<_> = workspace_command
        .parse_revset(ui, &args.revisions)?
        .evaluate_to_commits()?
        .try_collect()?;
    // Jobs are resolved in this order:
    // 1. Commandline argument iff > 0.
    // 2. the amount of cores available.
    // 3. a single job, if all of the above fails.
    let jobs = match args.jobs {
        Some(0) | None => std::thread::available_parallelism().map(|t| t.into()).ok(),
        Some(jobs) => Some(jobs),
    }
    // Fallback to a single user-visible job.
    .unwrap_or(1usize);

    let rt = get_runtime(jobs);
    let mut done_commits = HashSet::new();
    let (sender_tx, receiver) = std::sync::mpsc::channel();

    let mut tx = workspace_command.start_transaction();
    let repo_path = tx.base_workspace_helper().repo_path();

    // TODO: consider on-demand creation for the inner loop.
    let stored_commits = Arc::new(create_working_copies(repo_path, &resolved_commits)?);
    let stored_len = stored_commits.len();

    // Start all the jobs.
    async {
        run_inner(
            &tx,
            sender_tx,
            rt.handle(),
            Arc::new(args.shell_command.clone()),
            stored_commits,
        )
        .await
    }
    .block_on()?;

    let mut rewritten_commits = HashMap::new();
    loop {
        if let Ok(res) = receiver.recv() {
            done_commits.insert(res.old_id.clone());
            rewritten_commits.insert(res.old_id.clone(), res.new_tree);
        }
        if rewritten_commits.len() == stored_len {
            break;
        }
    }
    drop(receiver);

    let mut count: u32 = 0;
    // TODO: handle the `--reparent` case here.
    tx.repo_mut().transform_descendants(
        command.settings(),
        resolved_commits.iter().ids().cloned().collect_vec(),
        |rewriter| {
            let old_id = rewriter.old_commit().id();
            let new_tree = rewritten_commits.get(old_id).unwrap();
            let new_tree_id = new_tree.id().clone();
            count += 1;
            let builder = rewriter.rebase(command.settings())?;
            builder
                .set_tree_id(MergedTreeId::resolved(new_tree_id))
                .write()?;
            Ok(())
        },
    )?;
    tx.finish(ui, "run: rewrite {count} commits with {shell_command}")?;

    Ok(())
}
