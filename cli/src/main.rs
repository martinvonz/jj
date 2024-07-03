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

use std::sync::OnceLock;

#[cfg(feature = "mimalloc")]
use jj_cbits::mimalloc::MiMalloc;
use jj_cli::cli_util::CliRunner;
use jj_cli::command_error::CommandError;

/// Lazy global static. Used only to defer printing mimalloc stats until the
/// program exits, if set to `true`.
static PRINT_HEAP_STATS: OnceLock<bool> = OnceLock::new();

#[derive(clap::Args, Clone, Debug)]
pub struct ShowAllocStats {
    /// Show memory allocation statistics from the internal heap allocator
    /// on `stderr`, when the program exits.
    #[arg(long, global = true)]
    show_heap_stats: bool,
}

/// Enable heap statistics for the user interface; should be used with
/// [`CliRunner::add_global_args`]. Does nothing if the memory allocator is
/// unused, i.e. `#[global_allocator]` is not set to mimalloc in your program.
pub fn heap_stats_enable(
    _ui: &mut jj_cli::ui::Ui,
    opts: ShowAllocStats,
) -> Result<(), CommandError> {
    if opts.show_heap_stats {
        PRINT_HEAP_STATS.set(true).unwrap();
    }
    Ok(())
}

#[cfg(feature = "mimalloc")]
#[global_allocator]
static ALLOC: MiMalloc = MiMalloc;

fn main() -> std::process::ExitCode {
    let result = CliRunner::init()
        // NOTE (aseipp): always attach heap_stats_enable here, even if compiled
        // without mimalloc; we don't want the test suite or other users to have
        // to worry about if the command exists or not based on the build
        // configuration
        .add_global_args(heap_stats_enable)
        .version(env!("JJ_VERSION"))
        .run();

    if PRINT_HEAP_STATS.get() == Some(&true) {
        #[cfg(feature = "mimalloc")]
        {
            // NOTE (aseipp): can we do our own custom printing here? it's kind of ugly
            eprintln!("========================================");
            eprintln!("mimalloc memory allocation statistics:\n");
            jj_cbits::mimalloc::stats_print(&|l| {
                eprint!("{}", l.to_string_lossy());
            });
        }

        #[cfg(not(feature = "mimalloc"))]
        {
            eprintln!(
                "Note: heap statistics requested, but custom memory allocator (mimalloc) is not \
                 enabled."
            );
        }
    }
    result
}
