// Copyright 2023 The Jujutsu Authors
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

mod common_ancestors;
mod is_ancestor;
mod resolve_prefix;
mod revset;

use std::fmt::Debug;
use std::io;
use std::time::Instant;

use clap::Subcommand;
use criterion::Criterion;

use self::common_ancestors::cmd_bench_common_ancestors;
use self::common_ancestors::BenchCommonAncestorsArgs;
use self::is_ancestor::cmd_bench_is_ancestor;
use self::is_ancestor::BenchIsAncestorArgs;
use self::resolve_prefix::cmd_bench_resolve_prefix;
use self::resolve_prefix::BenchResolvePrefixArgs;
use self::revset::cmd_bench_revset;
use self::revset::BenchRevsetArgs;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Commands for benchmarking internal operations
#[derive(Subcommand, Clone, Debug)]
#[command(hide = true)]
pub enum BenchCommand {
    CommonAncestors(BenchCommonAncestorsArgs),
    IsAncestor(BenchIsAncestorArgs),
    ResolvePrefix(BenchResolvePrefixArgs),
    Revset(BenchRevsetArgs),
}

pub(crate) fn cmd_bench(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &BenchCommand,
) -> Result<(), CommandError> {
    match subcommand {
        BenchCommand::CommonAncestors(args) => cmd_bench_common_ancestors(ui, command, args),
        BenchCommand::IsAncestor(args) => cmd_bench_is_ancestor(ui, command, args),
        BenchCommand::ResolvePrefix(args) => cmd_bench_resolve_prefix(ui, command, args),
        BenchCommand::Revset(args) => cmd_bench_revset(ui, command, args),
    }
}

#[derive(clap::Args, Clone, Debug)]
struct CriterionArgs {
    /// Name of baseline to save results
    #[arg(long, short = 's', group = "baseline_mode", default_value = "base")]
    save_baseline: String,
    /// Name of baseline to compare with
    #[arg(long, short = 'b', group = "baseline_mode")]
    baseline: Option<String>,
    /// Sample size for the benchmarks, which must be at least 10
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(10..))]
    sample_size: u32, // not usize because https://github.com/clap-rs/clap/issues/4253
}

fn new_criterion(ui: &Ui, args: &CriterionArgs) -> Criterion {
    let criterion = Criterion::default().with_output_color(ui.color());
    let criterion = if let Some(name) = &args.baseline {
        let strict = false; // Do not panic if previous baseline doesn't exist.
        criterion.retain_baseline(name.clone(), strict)
    } else {
        criterion.save_baseline(args.save_baseline.clone())
    };
    criterion.sample_size(args.sample_size as usize)
}

fn run_bench<R, O>(ui: &mut Ui, id: &str, args: &CriterionArgs, mut routine: R) -> io::Result<()>
where
    R: (FnMut() -> O) + Copy,
    O: Debug,
{
    let mut criterion = new_criterion(ui, args);
    let before = Instant::now();
    let result = routine();
    let after = Instant::now();
    writeln!(
        ui.status(),
        "First run took {:?} and produced: {:?}",
        after.duration_since(before),
        result
    )?;
    criterion.bench_function(id, |bencher: &mut criterion::Bencher| {
        bencher.iter(routine);
    });
    Ok(())
}
