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

use std::fmt::Debug;
use std::io::Write;
use std::str::FromStr;

use hex::ToHex;
use indexmap::IndexMap;
use itertools::Itertools as _;
use jj_lib::commit::Commit;
use jj_lib::content_hash::blake2b_hash;
use jj_lib::footer::{get_footer_lines, FooterEntry};
use jj_lib::git::{self, GitRefUpdate};
use jj_lib::hex_util::to_reverse_hex;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::Repo;
use jj_lib::revset::RevsetIteratorExt as _;

use crate::cli_util::{short_commit_hash, CommandHelper, RemoteBranchNamePattern, RevisionArg};
use crate::command_error::{user_error, CommandError};
use crate::git_util::{get_git_repo, with_remote_git_callbacks, GitSidebandProgressMessageWriter};
use crate::ui::Ui;

#[derive(clap::Args, Clone, Debug)]
pub struct SendArgs {
    /// The revset, selecting which commits are sent in to Gerrit. This can be
    /// any arbitrary set of commits; they will be modified to include a
    /// `Change-Id` footer if one does not already exist, and then sent off to
    /// Gerrit for review.
    #[arg(long, short = 'r')]
    revision: Vec<RevisionArg>,

    /// The location where your changes are intended to land. This MUST be a
    /// reference of the form `branch@remote`, where `branch` is the name of
    /// the branch that the change will be submitted to, and `remote` is the
    /// remote to push the changes to.
    ///
    /// The specified `remote` MUST be a valid Git push URL for your Gerrit
    /// instance &mdash; typically a `ssh://` URL.
    #[arg(long = "for", short = 'f', default_value = "main@origin")]
    for_: String,

    /// If true, do not actually add `Change-Id`s to commits, and do not push
    /// the changes to Gerrit.
    #[arg(long = "dry-run", short = 'n')]
    dry_run: bool,
}

pub fn cmd_send(ui: &mut Ui, command: &CommandHelper, send: &SendArgs) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui)?;

    let to_send: Vec<Commit> = {
        let repo = workspace_command.repo();
        let expression = workspace_command.parse_union_revsets(&send.revision)?;
        let revset = workspace_command.evaluate_revset(expression)?;
        revset.iter().commits(repo.store()).try_collect()?
    };

    if to_send
        .iter()
        .any(|commit| commit.id() == workspace_command.repo().store().root_commit_id())
    {
        return Err(user_error("Cannot send the virtual 'root()' commit"));
    }

    workspace_command.check_rewritable(to_send.iter())?;

    let mut tx = workspace_command.start_transaction();
    let base_repo = tx.base_repo().clone();
    let mut_repo = tx.mut_repo();
    let store = base_repo.store();
    let git_repo = get_git_repo(store)?; // will fail if not a git repo

    let remote_pattern =
        RemoteBranchNamePattern::from_str(&send.for_).map_err(|err| user_error(err.to_string()))?;

    if !remote_pattern.is_exact() {
        return Err(user_error(format!(
            "The --for target '{}' is not exactly of the form 'branch@remote'",
            remote_pattern
        )));
    }

    let (for_branch, for_remote) = (
        remote_pattern.branch.as_exact().unwrap(),
        remote_pattern.remote.as_exact().unwrap(),
    );

    if !git_repo
        .remotes()?
        .iter()
        .any(|remote| remote == Some(for_remote))
    {
        return Err(user_error(format!(
            "The (Gerrit) remote '{}' does not exist",
            for_remote
        )));
    }

    // immediately error and reject any discardable commits, i.e. the
    // the empty wcc
    for commit in to_send.iter() {
        if commit.is_discardable() {
            return Err(user_error(format!(
                "Refusing to send in commit {} because it is an empty commit with no \
                 description\n(use 'jj amend' to add a description, or 'jj abandon' to discard it)",
                short_commit_hash(commit.id())
            )));
        }
    }

    // the mapping is from old -> [new, is_dry_run]; the dry_run flag is used to
    // disambiguate a later case when printing errors, so we know that if a
    // commit was mapped to itself, it was because --dry-run was set, and not
    // because e.g. it had an existing change id already
    let mut old_to_new: IndexMap<Commit, (Commit, bool)> = IndexMap::new();
    for commit_id in to_send.iter().map(|c| c.id()).rev() {
        let original_commit = store.get_commit(commit_id).unwrap();
        let description = original_commit.description().to_owned();
        let footer = get_footer_lines(&description);

        if !footer.is_empty() {
            // first, figure out if there are multiple Change-Id fields; if so, then we
            // error and continue
            if footer.iter().filter(|entry| entry.0 == "Change-Id").count() > 1 {
                writeln!(
                    ui.warning(),
                    "warning: multiple Change-Id footers in commit {}",
                    short_commit_hash(original_commit.id()),
                )?;
                continue;
            }

            // now, look up the existing change id footer
            let change_id = footer.iter().find(|entry| entry.0 == "Change-Id");
            if let Some(FooterEntry(_, cid)) = change_id {
                // map the old commit to itself
                old_to_new.insert(original_commit.clone(), (original_commit.clone(), false));

                // check the change-id format is correct in any case
                if cid.len() != 41 || !cid.starts_with('I') {
                    writeln!(
                        ui.warning(),
                        "warning: invalid Change-Id footer in commit {}",
                        short_commit_hash(original_commit.id()),
                    )?;
                    continue;
                }

                // XXX (aseipp): should we rewrite these invalid Change-Ids? i
                // don't think so, but I don't know what gerrit will do with
                // them, and I realized my old signoff.sh script created invalid
                // ones, so this is a helpful barrier.

                continue; // fallthrough
            }
        }

        if send.dry_run {
            // mark the old commit as rewritten to itself, but only because it
            // was a --dry-run, so we can give better error messages later
            old_to_new.insert(original_commit.clone(), (original_commit.clone(), true));
            continue;
        }

        // NOTE: Gerrit's change ID is not compatible with the alphabet used by
        // jj, and the needed length of the change-id is different as well.
        //
        // for us, we convert to gerrit's format is the character 'I', followed
        // by 40 characters of the truncated blake2 hash of the change id
        let change_id = to_reverse_hex(&original_commit.change_id().hex()).unwrap();
        let hashed_id: String = blake2b_hash(&change_id).encode_hex();
        let gerrit_change_id = format!("I{}", hashed_id.chars().take(40).collect::<String>());

        // XXX (aseipp): move this description junk for rewriting the description to
        // footer.rs; improves reusability and makes things a little cleaner
        let spacing = if footer.is_empty() { "\n\n" } else { "\n" };

        let new_description = format!(
            "{}{}Change-Id: {}\n",
            description.trim(),
            spacing,
            gerrit_change_id
        );

        // rewrite the set of parents to point to the commits that were
        // previously rewritten in toposort order
        //
        // TODO FIXME (aseipp): this whole dance with toposorting, calculating
        // new_parents, and then doing rewrite_commit is roughly equivalent to
        // what we do in duplicate.rs as well. we should probably refactor this?
        let new_parents = original_commit
            .parents()
            .iter()
            .map(|parent| {
                if let Some((rewritten_parent, _)) = old_to_new.get(parent) {
                    rewritten_parent
                } else {
                    parent
                }
                .id()
                .clone()
            })
            .collect();

        let new_commit = mut_repo
            .rewrite_commit(command.settings(), &original_commit)
            .set_description(new_description)
            .set_parents(new_parents)
            .write()?;
        old_to_new.insert(original_commit.clone(), (new_commit.clone(), false));
    }

    tx.finish(
        ui,
        format!(
            "describing {} commit(s) for sending to gerrit",
            old_to_new.len()
        ),
    )?;

    // XXX (aseipp): is this transaction safe to leave open? should it record a
    // push instead in the op log, even if it can't be meaningfully undone?
    let tx = workspace_command.start_transaction();
    let base_repo = tx.base_repo().clone();

    // NOTE(aseipp): write the status report *after* finishing the first
    // transaction. until we call 'tx.finish', the outstanding tx write set
    // contains a commit with a duplicated jj change-id, i.e. while the
    // transaction is open, it is ambiguous whether the change-id refers to the
    // newly written commit or the old one that already existed.
    //
    // this causes an awkward UX interaction, where write_commit_summary will
    // output a line with a red change-id indicating it's duplicated/conflicted,
    // AKA "??" status. but then the user will immediately run 'jj log' and not
    // see any conflicting change-ids, because the transaction was committed by
    // then and the new commits replaced the old ones! just printing this after
    // the transaction finishes avoids this weird case.
    //
    // XXX (aseipp): ask martin for feedback
    for (old, (new, is_dry)) in old_to_new.iter() {
        if old != new {
            write!(ui.stderr(), "Added Change-Id footer to ")?;
        } else if *is_dry {
            write!(ui.stderr(), "Dry-run: would have added Change-Id to ")?;
        } else {
            write!(ui.stderr(), "Skipped Change-Id (it already exists) for ")?;
        }
        tx.write_commit_summary(ui.stderr_formatter().as_mut(), new)?;
        writeln!(ui.stderr())?;
    }
    writeln!(ui.stderr())?;

    let new_commits = old_to_new.values().map(|x| &x.0).collect::<Vec<&Commit>>();
    let new_heads = base_repo
        .index()
        .heads(&mut new_commits.iter().map(|c| c.id()));
    let remote_ref = format!("refs/for/{}", for_branch);

    writeln!(
        ui.stderr(),
        "Found {} heads to push to Gerrit (remote '{}'), target branch '{}'",
        new_heads.len(),
        for_remote,
        for_branch,
    )?;

    // split these two loops to keep the output a little nicer; display first,
    // then push
    for head in &new_heads {
        let head_commit = store.get_commit(head).unwrap();

        write!(ui.stderr(), "    ")?;
        tx.write_commit_summary(ui.stderr_formatter().as_mut(), &head_commit)?;
        writeln!(ui.stderr())?;
    }
    writeln!(ui.stderr())?;

    if send.dry_run {
        writeln!(
            ui.stderr(),
            "Dry-run: Not performing push, as `--dry-run` was requested"
        )?;
        return Ok(());
    }

    // NOTE (aseipp): because we are pushing everything to the same remote ref,
    // we have to loop and push each commit one at a time, even though
    // push_updates in theory supports multiple GitRefUpdates at once, because
    // we obviously can't push multiple heads to the same ref.
    for head in &new_heads {
        let head_commit = store.get_commit(head).unwrap();
        let head_id = head_commit.id().clone();

        write!(ui.stderr(), "Pushing ")?;
        tx.write_commit_summary(ui.stderr_formatter().as_mut(), &head_commit)?;
        writeln!(ui.stderr())?;

        // how do we get better errors from the remote? 'git push' tells us
        // about rejected refs AND ALSO '(nothing changed)' when there are no
        // changes to push, but we don't get that here. RefUpdateRejected might
        // need more context, idk. is this a libgit2 problem?
        let mut writer = GitSidebandProgressMessageWriter::new(ui);
        let mut sideband_progress_callback = |msg: &[u8]| {
            _ = writer.write(ui, msg);
        };
        with_remote_git_callbacks(ui, Some(&mut sideband_progress_callback), |cb| {
            git::push_updates(
                &git_repo,
                for_remote,
                &[GitRefUpdate {
                    qualified_name: remote_ref.clone(),
                    force: false,
                    new_target: Some(head_id),
                }],
                cb,
            )
        })
        .map_or_else(
            |err| match err {
                git::GitPushError::RefUpdateRejected(_) => {
                    // gerrit rejects ref updates when there are no changes, i.e.
                    // you submit a change that is already up to date. just give
                    // the user a light warning and carry on
                    writeln!(
                        ui.warning(),
                        "warning: ref update rejected by gerrit; no changes to push (did you \
                         forget to update, amend, or add new changes?)"
                    )?;

                    Ok(())
                }
                // XXX (aseipp): more cases to handle here?
                _ => Err(user_error(err.to_string())),
            },
            Ok,
        )?;
    }

    Ok(())
}
