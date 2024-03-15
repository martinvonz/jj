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

use std::fmt::Debug;
use std::io;
use std::io::Write as _;
use std::rc::Rc;
use std::time::Instant;

use clap::Subcommand;
use criterion::measurement::Measurement;
use criterion::{BatchSize, BenchmarkGroup, BenchmarkId, Criterion};
use jj_lib::object_id::HexPrefix;
use jj_lib::repo::Repo;
use jj_lib::revset::{self, DefaultSymbolResolver, RevsetExpression};

use crate::cli_util::{CommandHelper, WorkspaceCommandHelper};
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Commands for benchmarking internal operations
#[derive(Subcommand, Clone, Debug)]
#[command(hide = true)]
pub enum BenchCommand {
    #[command(name = "commonancestors")]
    CommonAncestors(BenchCommonAncestorsArgs),
    #[command(name = "isancestor")]
    IsAncestor(BenchIsAncestorArgs),
    #[command(name = "resolveprefix")]
    ResolvePrefix(BenchResolvePrefixArgs),
    #[command(name = "revset")]
    Revset(BenchRevsetArgs),
}

/// Find the common ancestor(s) of a set of commits
#[derive(clap::Args, Clone, Debug)]
pub struct BenchCommonAncestorsArgs {
    revision1: String,
    revision2: String,
    #[command(flatten)]
    criterion: CriterionArgs,
}

/// Checks if the first commit is an ancestor of the second commit
#[derive(clap::Args, Clone, Debug)]
pub struct BenchIsAncestorArgs {
    ancestor: String,
    descendant: String,
    #[command(flatten)]
    criterion: CriterionArgs,
}

/// Walk the revisions in the revset
#[derive(clap::Args, Clone, Debug)]
#[command(group(clap::ArgGroup::new("revset_source").required(true)))]
pub struct BenchRevsetArgs {
    #[arg(group = "revset_source")]
    revisions: Vec<String>,
    /// Read revsets from file
    #[arg(long, short = 'f', group = "revset_source", value_hint = clap::ValueHint::FilePath)]
    file: Option<String>,
    #[command(flatten)]
    criterion: CriterionArgs,
}

/// Resolve a commit ID prefix
#[derive(clap::Args, Clone, Debug)]
pub struct BenchResolvePrefixArgs {
    prefix: String,
    #[command(flatten)]
    criterion: CriterionArgs,
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
        ui.stderr(),
        "First run took {:?} and produced: {:?}",
        after.duration_since(before),
        result
    )?;
    criterion.bench_function(id, |bencher: &mut criterion::Bencher| {
        bencher.iter(routine);
    });
    Ok(())
}

pub(crate) fn cmd_bench(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &BenchCommand,
) -> Result<(), CommandError> {
    match subcommand {
        BenchCommand::CommonAncestors(args) => {
            let workspace_command = command.workspace_helper(ui)?;
            let commit1 = workspace_command.resolve_single_rev(&args.revision1)?;
            let commit2 = workspace_command.resolve_single_rev(&args.revision2)?;
            let index = workspace_command.repo().index();
            let routine =
                || index.common_ancestors(&[commit1.id().clone()], &[commit2.id().clone()]);
            run_bench(
                ui,
                &format!("commonancestors-{}-{}", &args.revision1, &args.revision2),
                &args.criterion,
                routine,
            )?;
        }
        BenchCommand::IsAncestor(args) => {
            let workspace_command = command.workspace_helper(ui)?;
            let ancestor_commit = workspace_command.resolve_single_rev(&args.ancestor)?;
            let descendant_commit = workspace_command.resolve_single_rev(&args.descendant)?;
            let index = workspace_command.repo().index();
            let routine = || index.is_ancestor(ancestor_commit.id(), descendant_commit.id());
            run_bench(
                ui,
                &format!("isancestor-{}-{}", &args.ancestor, &args.descendant),
                &args.criterion,
                routine,
            )?;
        }
        BenchCommand::ResolvePrefix(args) => {
            let workspace_command = command.workspace_helper(ui)?;
            let prefix = HexPrefix::new(&args.prefix).unwrap();
            let index = workspace_command.repo().index();
            let routine = || index.resolve_commit_id_prefix(&prefix);
            run_bench(
                ui,
                &format!("resolveprefix-{}", prefix.hex()),
                &args.criterion,
                routine,
            )?;
        }
        BenchCommand::Revset(args) => {
            let workspace_command = command.workspace_helper(ui)?;
            let revsets = if let Some(file_path) = &args.file {
                std::fs::read_to_string(command.cwd().join(file_path))?
                    .lines()
                    .map(|line| line.trim().to_owned())
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    .collect()
            } else {
                args.revisions.clone()
            };
            let mut criterion = new_criterion(ui, &args.criterion);
            let mut group = criterion.benchmark_group("revsets");
            for revset in &revsets {
                bench_revset(ui, command, &workspace_command, &mut group, revset)?;
            }
            // Neither of these seem to report anything...
            group.finish();
            criterion.final_summary();
        }
    }
    Ok(())
}

fn bench_revset<M: Measurement>(
    ui: &mut Ui,
    command: &CommandHelper,
    workspace_command: &WorkspaceCommandHelper,
    group: &mut BenchmarkGroup<M>,
    revset: &str,
) -> Result<(), CommandError> {
    writeln!(ui.stderr(), "----------Testing revset: {revset}----------")?;
    let expression = revset::optimize(workspace_command.parse_revset(revset)?);
    // Time both evaluation and iteration.
    let routine = |workspace_command: &WorkspaceCommandHelper, expression: Rc<RevsetExpression>| {
        // Evaluate the expression without parsing/evaluating short-prefixes.
        let repo = workspace_command.repo().as_ref();
        let symbol_resolver = DefaultSymbolResolver::new(repo);
        let resolved = expression
            .resolve_user_expression(repo, &symbol_resolver)
            .unwrap();
        let revset = resolved.evaluate(repo).unwrap();
        revset.iter().count()
    };
    let before = Instant::now();
    let result = routine(workspace_command, expression.clone());
    let after = Instant::now();
    writeln!(
        ui.stderr(),
        "First run took {:?} and produced {result} commits",
        after.duration_since(before),
    )?;

    group.bench_with_input(
        BenchmarkId::from_parameter(revset),
        &expression,
        |bencher, expression| {
            bencher.iter_batched(
                // Reload repo and backend store to clear caches (such as commit objects
                // in `Store`), but preload index since it's more likely to be loaded
                // by preceding operation. `repo.reload_at()` isn't enough to clear
                // store cache.
                || {
                    let workspace_command = command.workspace_helper_no_snapshot(ui).unwrap();
                    workspace_command.repo().readonly_index();
                    workspace_command
                },
                |workspace_command| routine(&workspace_command, expression.clone()),
                // Index-preloaded repo may consume a fair amount of memory
                BatchSize::LargeInput,
            );
        },
    );
    Ok(())
}
