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

//! Primary entrypoint for the stock `jj` command-line tool. mimalloc enabled.

use jj_cbits::mimalloc::MiMalloc;
use jj_cli::cli_util::{heap_stats_enable, heap_stats_with_closure, CliRunner};

#[global_allocator]
static ALLOC: MiMalloc = MiMalloc;

fn main() -> std::process::ExitCode {
    heap_stats_with_closure(|| {
        CliRunner::init()
            .add_global_args(heap_stats_enable)
            .version(env!("JJ_VERSION"))
            .run()
    })
}
