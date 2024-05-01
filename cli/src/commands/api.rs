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

use clap::arg;
use jj_lib::api::server::{start_api, GrpcOptions, StartupOptions};
use jj_lib::api::servicer::Servicer;
use std::fmt::Debug;




use tracing::instrument;

use crate::command_error::{CommandError, CommandErrorKind};
use crate::commands::CommandHelper;
use crate::ui::Ui;

#[derive(clap::Subcommand, Clone, Debug)]
pub enum ApiCommand {
    Grpc(GrpcArgs),
}

#[derive(clap::Args, Clone, Debug)]
pub struct GrpcArgs {
    #[arg(long)]
    port: u16,

    #[arg(long)]
    web: bool,
}

#[instrument(skip_all)]
pub(crate) fn cmd_api(
    _ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &ApiCommand,
) -> Result<(), CommandError> {
    let startup_options = match subcommand {
        ApiCommand::Grpc(args) => StartupOptions::Grpc(GrpcOptions {
            port: args.port,
            web: args.web,
        }),
    };
    // Running jj api from a non-jj repository is still valid, as the user can provide the repository path in each individual request.
    let default_workspace_loader = command.workspace_loader().ok();
    start_api(
        startup_options,
        Servicer::new(default_workspace_loader.cloned()),
    )
    .map_err(|e| CommandError::new(CommandErrorKind::Internal, e))
}
