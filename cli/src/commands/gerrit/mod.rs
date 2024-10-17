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

use clap::Subcommand;

use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::commands::gerrit;
use crate::ui::Ui;

/// Interact with Gerrit Code Review.
#[derive(Subcommand, Clone, Debug)]
pub enum GerritCommand {
    /// Send changes to Gerrit for code review, or update existing changes.
    ///
    /// Sending in a set of revisions to Gerrit creates a single "change" for
    /// each revision included in the revset. This change is then available for
    /// review on your Gerrit instance.
    ///
    /// This command modifies each commit in the revset to include a `Change-Id`
    /// footer in its commit message if one does not already exist. Note that
    /// this ID is NOT compatible with jj IDs, and is Gerrit-specific.
    ///
    /// If a change already exists for a given revision (i.e. it contains the
    /// same `Change-Id`), this command will update the contents of the existing
    /// change to match.
    ///
    /// Note: this command takes 1-or-more revsets arguments, each of which can
    /// resolve to multiple revisions; so you may post trees or ranges of
    /// commits to Gerrit for review all at once.
    Send(gerrit::send::SendArgs),
}

pub fn cmd_gerrit(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &GerritCommand,
) -> Result<(), CommandError> {
    match subcommand {
        GerritCommand::Send(review) => gerrit::send::cmd_send(ui, command, review),
    }
}

mod send;
