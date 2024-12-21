// Copyright 2024 The Jujutsu Authors
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

//! Git utilities shared by various commands.

use std::error;
use std::io::Read;
use std::io::Write;
use std::iter;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Instant;

use itertools::Itertools;
use jj_lib::git;
use jj_lib::git::FailedRefExport;
use jj_lib::git::FailedRefExportReason;
use jj_lib::git::GitFetchError;
use jj_lib::git::GitImportStats;
use jj_lib::git::RefName;
use jj_lib::git_backend::GitBackend;
use jj_lib::op_store::RefTarget;
use jj_lib::op_store::RemoteRef;
use jj_lib::repo::ReadonlyRepo;
use jj_lib::repo::Repo;
use jj_lib::store::Store;
use jj_lib::str_util::StringPattern;
use jj_lib::workspace::Workspace;
use unicode_width::UnicodeWidthStr;

use crate::cli_util::WorkspaceCommandTransaction;
use crate::command_error::user_error;
use crate::command_error::user_error_with_hint;
use crate::command_error::CommandError;
use crate::formatter::Formatter;
use crate::progress::Progress;
use crate::ui::Ui;

pub fn map_git_error(err: git2::Error) -> CommandError {
    if err.class() == git2::ErrorClass::Ssh {
        let hint =
            if err.code() == git2::ErrorCode::Certificate && std::env::var_os("HOME").is_none() {
                "The HOME environment variable is not set, and might be required for Git to \
                 successfully load certificates. Try setting it to the path of a directory that \
                 contains a `.ssh` directory."
            } else {
                "There was an error creating an SSH connection. Does `ssh -F /dev/null` to the \
                 host work?"
            };

        user_error_with_hint(err, hint)
    } else {
        user_error(err)
    }
}

pub fn get_git_repo(store: &Store) -> Result<git2::Repository, CommandError> {
    match store.backend_impl().downcast_ref::<GitBackend>() {
        None => Err(user_error("The repo is not backed by a git repo")),
        Some(git_backend) => Ok(git_backend.open_git_repo()?),
    }
}

pub fn is_colocated_git_workspace(workspace: &Workspace, repo: &ReadonlyRepo) -> bool {
    let Some(git_backend) = repo.store().backend_impl().downcast_ref::<GitBackend>() else {
        return false;
    };
    let Some(git_workdir) = git_backend.git_workdir() else {
        return false; // Bare repository
    };
    if git_workdir == workspace.workspace_root() {
        return true;
    }
    // Colocated workspace should have ".git" directory, file, or symlink. Compare
    // its parent as the git_workdir might be resolved from the real ".git" path.
    let Ok(dot_git_path) = workspace.workspace_root().join(".git").canonicalize() else {
        return false;
    };
    git_workdir.canonicalize().ok().as_deref() == dot_git_path.parent()
}

fn terminal_get_username(ui: &Ui, url: &str) -> Option<String> {
    ui.prompt(&format!("Username for {url}")).ok()
}

fn terminal_get_pw(ui: &Ui, url: &str) -> Option<String> {
    ui.prompt_password(&format!("Passphrase for {url}: ")).ok()
}

fn pinentry_get_pw(url: &str) -> Option<String> {
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

    let mut pinentry = std::process::Command::new("pinentry")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let mut interact = || -> std::io::Result<_> {
        #[rustfmt::skip]
        let req = format!(
            "SETTITLE jj passphrase\n\
             SETDESC Enter passphrase for {url}\n\
             SETPROMPT Passphrase:\n\
             GETPIN\n"
        );
        pinentry.stdin.take().unwrap().write_all(req.as_bytes())?;
        let mut out = String::new();
        pinentry.stdout.take().unwrap().read_to_string(&mut out)?;
        Ok(out)
    };
    let maybe_out = interact();
    _ = pinentry.wait();
    for line in maybe_out.ok()?.split('\n') {
        if !line.starts_with("D ") {
            continue;
        }
        let (_, encoded) = line.split_at(2);
        return decode_assuan_data(encoded);
    }
    None
}

#[tracing::instrument]
fn get_ssh_keys(_username: &str) -> Vec<PathBuf> {
    let mut paths = vec![];
    if let Some(home_dir) = dirs::home_dir() {
        let ssh_dir = Path::new(&home_dir).join(".ssh");
        for filename in ["id_ed25519_sk", "id_ed25519", "id_rsa"] {
            let key_path = ssh_dir.join(filename);
            if key_path.is_file() {
                tracing::info!(path = ?key_path, "found ssh key");
                paths.push(key_path);
            }
        }
    }
    if paths.is_empty() {
        tracing::info!("no ssh key found");
    }
    paths
}

// Based on Git's implementation: https://github.com/git/git/blob/43072b4ca132437f21975ac6acc6b72dc22fd398/sideband.c#L178
pub struct GitSidebandProgressMessageWriter {
    display_prefix: &'static [u8],
    suffix: &'static [u8],
    scratch: Vec<u8>,
}

impl GitSidebandProgressMessageWriter {
    pub fn new(ui: &Ui) -> Self {
        let is_terminal = ui.use_progress_indicator();

        GitSidebandProgressMessageWriter {
            display_prefix: "remote: ".as_bytes(),
            suffix: if is_terminal { "\x1B[K" } else { "        " }.as_bytes(),
            scratch: Vec::new(),
        }
    }

    pub fn write(&mut self, ui: &Ui, progress_message: &[u8]) -> std::io::Result<()> {
        let mut index = 0;
        // Append a suffix to each nonempty line to clear the end of the screen line.
        loop {
            let Some(i) = progress_message[index..]
                .iter()
                .position(|&c| c == b'\r' || c == b'\n')
                .map(|i| index + i)
            else {
                break;
            };
            let line_length = i - index;

            // For messages sent across the packet boundary, there would be a nonempty
            // "scratch" buffer from last call of this function, and there may be a leading
            // CR/LF in this message. For this case we should add a clear-to-eol suffix to
            // clean leftover letters we previously have written on the same line.
            if !self.scratch.is_empty() && line_length == 0 {
                self.scratch.extend_from_slice(self.suffix);
            }

            if self.scratch.is_empty() {
                self.scratch.extend_from_slice(self.display_prefix);
            }

            // Do not add the clear-to-eol suffix to empty lines:
            // For progress reporting we may receive a bunch of percentage updates
            // followed by '\r' to remain on the same line, and at the end receive a single
            // '\n' to move to the next line. We should preserve the final
            // status report line by not appending clear-to-eol suffix to this single line
            // break.
            if line_length > 0 {
                self.scratch.extend_from_slice(&progress_message[index..i]);
                self.scratch.extend_from_slice(self.suffix);
            }
            self.scratch.extend_from_slice(&progress_message[i..i + 1]);

            ui.status().write_all(&self.scratch)?;
            self.scratch.clear();

            index = i + 1;
        }

        // Add leftover message to "scratch" buffer to be printed in next call.
        if index < progress_message.len() && progress_message[index] != 0 {
            if self.scratch.is_empty() {
                self.scratch.extend_from_slice(self.display_prefix);
            }
            self.scratch.extend_from_slice(&progress_message[index..]);
        }

        Ok(())
    }

    pub fn flush(&mut self, ui: &Ui) -> std::io::Result<()> {
        if !self.scratch.is_empty() {
            self.scratch.push(b'\n');
            ui.status().write_all(&self.scratch)?;
            self.scratch.clear();
        }

        Ok(())
    }
}

type SidebandProgressCallback<'a> = &'a mut dyn FnMut(&[u8]);

pub fn with_remote_git_callbacks<T>(
    ui: &Ui,
    sideband_progress_callback: Option<SidebandProgressCallback<'_>>,
    f: impl FnOnce(git::RemoteCallbacks<'_>) -> T,
) -> T {
    let mut callbacks = git::RemoteCallbacks::default();
    let mut progress_callback = None;
    if let Some(mut output) = ui.progress_output() {
        let mut progress = Progress::new(Instant::now());
        progress_callback = Some(move |x: &git::Progress| {
            _ = progress.update(Instant::now(), x, &mut output);
        });
    }
    callbacks.progress = progress_callback
        .as_mut()
        .map(|x| x as &mut dyn FnMut(&git::Progress));
    callbacks.sideband_progress = sideband_progress_callback.map(|x| x as &mut dyn FnMut(&[u8]));
    let mut get_ssh_keys = get_ssh_keys; // Coerce to unit fn type
    callbacks.get_ssh_keys = Some(&mut get_ssh_keys);
    let mut get_pw =
        |url: &str, _username: &str| pinentry_get_pw(url).or_else(|| terminal_get_pw(ui, url));
    callbacks.get_password = Some(&mut get_pw);
    let mut get_user_pw =
        |url: &str| Some((terminal_get_username(ui, url)?, terminal_get_pw(ui, url)?));
    callbacks.get_username_password = Some(&mut get_user_pw);
    f(callbacks)
}

pub fn print_git_import_stats(
    ui: &Ui,
    repo: &dyn Repo,
    stats: &GitImportStats,
    show_ref_stats: bool,
) -> Result<(), CommandError> {
    let Some(mut formatter) = ui.status_formatter() else {
        return Ok(());
    };
    if show_ref_stats {
        let refs_stats = stats
            .changed_remote_refs
            .iter()
            .map(|(ref_name, (remote_ref, ref_target))| {
                RefStatus::new(ref_name, remote_ref, ref_target, repo)
            })
            .collect_vec();

        let has_both_ref_kinds = refs_stats
            .iter()
            .any(|x| matches!(x.ref_kind, RefKind::Branch))
            && refs_stats
                .iter()
                .any(|x| matches!(x.ref_kind, RefKind::Tag));

        let max_width = refs_stats.iter().map(|x| x.ref_name.width()).max();
        if let Some(max_width) = max_width {
            for status in refs_stats {
                status.output(max_width, has_both_ref_kinds, &mut *formatter)?;
            }
        }
    }

    if !stats.abandoned_commits.is_empty() {
        writeln!(
            formatter,
            "Abandoned {} commits that are no longer reachable.",
            stats.abandoned_commits.len()
        )?;
    }

    Ok(())
}

struct RefStatus {
    ref_kind: RefKind,
    ref_name: String,
    tracking_status: TrackingStatus,
    import_status: ImportStatus,
}

impl RefStatus {
    fn new(
        ref_name: &RefName,
        remote_ref: &RemoteRef,
        ref_target: &RefTarget,
        repo: &dyn Repo,
    ) -> Self {
        let (ref_name, ref_kind, tracking_status) = match ref_name {
            RefName::RemoteBranch { branch, remote } => (
                format!("{branch}@{remote}"),
                RefKind::Branch,
                if repo
                    .view()
                    .get_remote_bookmark(branch, remote)
                    .is_tracking()
                {
                    TrackingStatus::Tracked
                } else {
                    TrackingStatus::Untracked
                },
            ),
            RefName::Tag(tag) => (tag.clone(), RefKind::Tag, TrackingStatus::NotApplicable),
            RefName::LocalBranch(branch) => {
                (branch.clone(), RefKind::Branch, TrackingStatus::Tracked)
            }
        };

        let import_status = match (remote_ref.target.is_absent(), ref_target.is_absent()) {
            (true, false) => ImportStatus::New,
            (false, true) => ImportStatus::Deleted,
            _ => ImportStatus::Updated,
        };

        Self {
            ref_name,
            tracking_status,
            import_status,
            ref_kind,
        }
    }

    fn output(
        &self,
        max_ref_name_width: usize,
        has_both_ref_kinds: bool,
        out: &mut dyn Formatter,
    ) -> std::io::Result<()> {
        let tracking_status = match self.tracking_status {
            TrackingStatus::Tracked => "tracked",
            TrackingStatus::Untracked => "untracked",
            TrackingStatus::NotApplicable => "",
        };

        let import_status = match self.import_status {
            ImportStatus::New => "new",
            ImportStatus::Deleted => "deleted",
            ImportStatus::Updated => "updated",
        };

        let ref_name_display_width = self.ref_name.width();
        let pad_width = max_ref_name_width.saturating_sub(ref_name_display_width);
        let padded_ref_name = format!("{}{:>pad_width$}", self.ref_name, "", pad_width = pad_width);

        let ref_kind = match self.ref_kind {
            RefKind::Branch => "bookmark: ",
            RefKind::Tag if !has_both_ref_kinds => "tag: ",
            RefKind::Tag => "tag:    ",
        };

        write!(out, "{ref_kind}")?;
        write!(out.labeled("bookmark"), "{padded_ref_name}")?;
        writeln!(out, " [{import_status}] {tracking_status}")
    }
}

enum RefKind {
    Branch,
    Tag,
}

enum TrackingStatus {
    Tracked,
    Untracked,
    NotApplicable, // for tags
}

enum ImportStatus {
    New,
    Deleted,
    Updated,
}

pub fn print_failed_git_export(
    ui: &Ui,
    failed_refs: &[FailedRefExport],
) -> Result<(), std::io::Error> {
    if !failed_refs.is_empty() {
        writeln!(ui.warning_default(), "Failed to export some bookmarks:")?;
        let mut formatter = ui.stderr_formatter();
        for FailedRefExport { name, reason } in failed_refs {
            write!(formatter, "  ")?;
            write!(formatter.labeled("bookmark"), "{name}")?;
            for err in iter::successors(Some(reason as &dyn error::Error), |err| err.source()) {
                write!(formatter, ": {err}")?;
            }
            writeln!(formatter)?;
        }
        drop(formatter);
        if failed_refs
            .iter()
            .any(|failed| matches!(failed.reason, FailedRefExportReason::FailedToSet(_)))
        {
            writeln!(
                ui.hint_default(),
                r#"Git doesn't allow a branch name that looks like a parent directory of
another (e.g. `foo` and `foo/bar`). Try to rename the bookmarks that failed to
export or their "parent" bookmarks."#,
            )?;
        }
    }
    Ok(())
}

pub fn git_fetch(
    ui: &mut Ui,
    tx: &mut WorkspaceCommandTransaction,
    git_repo: &git2::Repository,
    remotes: &[String],
    branch: &[StringPattern],
) -> Result<(), CommandError> {
    let git_settings = tx.settings().git_settings()?;

    for remote in remotes {
        let stats = with_remote_git_callbacks(ui, None, |cb| {
            git::fetch(
                tx.repo_mut(),
                git_repo,
                remote,
                branch,
                cb,
                &git_settings,
                None,
            )
        })
        .map_err(|err| match err {
            GitFetchError::InvalidBranchPattern => {
                if branch
                    .iter()
                    .any(|pattern| pattern.as_exact().is_some_and(|s| s.contains('*')))
                {
                    user_error_with_hint(
                        "Branch names may not include `*`.",
                        "Prefix the pattern with `glob:` to expand `*` as a glob",
                    )
                } else {
                    user_error(err)
                }
            }
            GitFetchError::GitImportError(err) => err.into(),
            GitFetchError::InternalGitError(err) => map_git_error(err),
            _ => user_error(err),
        })?;
        print_git_import_stats(ui, tx.repo(), &stats.import_stats, true)?;
    }
    warn_if_branches_not_found(
        ui,
        tx,
        branch,
        &remotes.iter().map(StringPattern::exact).collect_vec(),
    )
}

fn warn_if_branches_not_found(
    ui: &mut Ui,
    tx: &WorkspaceCommandTransaction,
    branches: &[StringPattern],
    remotes: &[StringPattern],
) -> Result<(), CommandError> {
    for branch in branches {
        let matches = remotes.iter().any(|remote| {
            tx.repo()
                .view()
                .remote_bookmarks_matching(branch, remote)
                .next()
                .is_some()
                || tx
                    .base_repo()
                    .view()
                    .remote_bookmarks_matching(branch, remote)
                    .next()
                    .is_some()
        });
        if !matches {
            writeln!(
                ui.warning_default(),
                "No branch matching `{branch}` found on any specified/configured remote",
            )?;
        }
    }

    Ok(())
}
