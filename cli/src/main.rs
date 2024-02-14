// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use jj_cli::cli_util::CliRunner;

fn main() -> std::process::ExitCode {
    CliRunner::init().version(env!("JJ_VERSION")).run()
}
