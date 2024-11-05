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

use std::rc::Rc;
use std::time::Instant;

use criterion::measurement::Measurement;
use criterion::BatchSize;
use criterion::BenchmarkGroup;
use criterion::BenchmarkId;
use jj_lib::revset::DefaultSymbolResolver;
use jj_lib::revset::SymbolResolverExtension;
use jj_lib::revset::UserRevsetExpression;

use super::new_criterion;
use super::CriterionArgs;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::CommandError;
use crate::ui::Ui;

/// Walk the revisions in the revset
#[derive(clap::Args, Clone, Debug)]
#[command(group(clap::ArgGroup::new("revset_source").required(true)))]
pub struct BenchRevsetArgs {
    #[arg(group = "revset_source")]
    revisions: Vec<RevisionArg>,
    /// Read revsets from file
    #[arg(long, short = 'f', group = "revset_source", value_hint = clap::ValueHint::FilePath)]
    file: Option<String>,
    #[command(flatten)]
    criterion: CriterionArgs,
}

pub fn cmd_bench_revset(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &BenchRevsetArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui)?;
    let revsets = if let Some(file_path) = &args.file {
        std::fs::read_to_string(command.cwd().join(file_path))?
            .lines()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(RevisionArg::from)
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
    Ok(())
}

fn bench_revset<M: Measurement>(
    ui: &mut Ui,
    command: &CommandHelper,
    workspace_command: &WorkspaceCommandHelper,
    group: &mut BenchmarkGroup<M>,
    revset: &RevisionArg,
) -> Result<(), CommandError> {
    writeln!(ui.status(), "----------Testing revset: {revset}----------")?;
    let expression = workspace_command
        .parse_revset(ui, revset)?
        .expression()
        .clone();
    // Time both evaluation and iteration.
    let routine = |workspace_command: &WorkspaceCommandHelper,
                   expression: Rc<UserRevsetExpression>| {
        // Evaluate the expression without parsing/evaluating short-prefixes.
        let repo = workspace_command.repo().as_ref();
        let symbol_resolver =
            DefaultSymbolResolver::new(repo, &([] as [Box<dyn SymbolResolverExtension>; 0]));
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
        ui.status(),
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
