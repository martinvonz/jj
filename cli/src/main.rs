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

use jj_cbits::{crash_handler_info, libfault};
use jj_cli::cli_util::CliRunner;

static APP_INFO: libfault::AppInfo = crash_handler_info! {
    app_name: env!("CARGO_PKG_NAME"),
    app_version: env!("JJ_VERSION"),
    bugreport_url: "https://github.com/martinvonz/jj/issues/new/choose",
    log_name: "/tmp/jj-cli-crash.",
};

fn main() -> std::process::ExitCode {
    libfault::install(&APP_INFO);
    CliRunner::init().version(env!("JJ_VERSION")).run()
}
