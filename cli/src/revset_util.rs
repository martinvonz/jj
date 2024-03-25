// Copyright 2022-2024 The Jujutsu Authors
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

//! Utility for parsing and evaluating user-provided revset expressions.

use std::rc::Rc;

use itertools::Itertools as _;
use jj_lib::backend::CommitId;
use jj_lib::id_prefix::IdPrefixContext;
use jj_lib::repo::Repo;
use jj_lib::revset::{
    self, DefaultSymbolResolver, Revset, RevsetAliasesMap, RevsetEvaluationError, RevsetExpression,
    RevsetParseContext, RevsetParseError, RevsetResolutionError,
};
use jj_lib::settings::ConfigResultExt as _;
use thiserror::Error;

use crate::command_error::{user_error, CommandError};
use crate::config::LayeredConfigs;
use crate::ui::Ui;

const BUILTIN_IMMUTABLE_HEADS: &str = "immutable_heads";

#[derive(Debug, Error)]
pub enum UserRevsetEvaluationError {
    #[error(transparent)]
    Resolution(RevsetResolutionError),
    #[error(transparent)]
    Evaluation(RevsetEvaluationError),
}

pub fn load_revset_aliases(
    ui: &Ui,
    layered_configs: &LayeredConfigs,
) -> Result<RevsetAliasesMap, CommandError> {
    const TABLE_KEY: &str = "revset-aliases";
    let mut aliases_map = RevsetAliasesMap::new();
    // Load from all config layers in order. 'f(x)' in default layer should be
    // overridden by 'f(a)' in user.
    for (_, config) in layered_configs.sources() {
        let table = if let Some(table) = config.get_table(TABLE_KEY).optional()? {
            table
        } else {
            continue;
        };
        for (decl, value) in table.into_iter().sorted_by(|a, b| a.0.cmp(&b.0)) {
            let r = value
                .into_string()
                .map_err(|e| e.to_string())
                .and_then(|v| aliases_map.insert(&decl, v).map_err(|e| e.to_string()));
            if let Err(s) = r {
                writeln!(
                    ui.warning_default(),
                    r#"Failed to load "{TABLE_KEY}.{decl}": {s}"#
                )?;
            }
        }
    }

    // TODO: If we add support for function overloading (#2966), this check can
    // be removed.
    let (params, _) = aliases_map.get_function(BUILTIN_IMMUTABLE_HEADS).unwrap();
    if !params.is_empty() {
        return Err(user_error(format!(
            "The `revset-aliases.{name}()` function must be declared without arguments",
            name = BUILTIN_IMMUTABLE_HEADS
        )));
    }

    Ok(aliases_map)
}

pub fn evaluate<'a>(
    repo: &'a dyn Repo,
    symbol_resolver: &DefaultSymbolResolver,
    expression: Rc<RevsetExpression>,
) -> Result<Box<dyn Revset + 'a>, UserRevsetEvaluationError> {
    let resolved = revset::optimize(expression)
        .resolve_user_expression(repo, symbol_resolver)
        .map_err(UserRevsetEvaluationError::Resolution)?;
    resolved
        .evaluate(repo)
        .map_err(UserRevsetEvaluationError::Evaluation)
}

/// Wraps the given `IdPrefixContext` in `SymbolResolver` to be passed in to
/// `evaluate()`.
pub fn default_symbol_resolver<'a>(
    repo: &'a dyn Repo,
    id_prefix_context: &'a IdPrefixContext,
) -> DefaultSymbolResolver<'a> {
    let commit_id_resolver: revset::PrefixResolver<CommitId> =
        Box::new(|repo, prefix| id_prefix_context.resolve_commit_prefix(repo, prefix));
    let change_id_resolver: revset::PrefixResolver<Vec<CommitId>> =
        Box::new(|repo, prefix| id_prefix_context.resolve_change_prefix(repo, prefix));
    DefaultSymbolResolver::new(repo)
        .with_commit_id_resolver(commit_id_resolver)
        .with_change_id_resolver(change_id_resolver)
}

/// Parses user-configured expression defining the immutable set.
pub fn parse_immutable_expression(
    context: &RevsetParseContext,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let (params, immutable_heads_str) = context
        .aliases_map
        .get_function(BUILTIN_IMMUTABLE_HEADS)
        .unwrap();
    assert!(
        params.is_empty(),
        "invalid declaration should have been rejected by load_revset_aliases()"
    );
    // Negated ancestors expression `~::(<heads> | root())` is slightly easier
    // to optimize than negated union `~(::<heads> | root())`.
    let heads = revset::parse(immutable_heads_str, context)?;
    Ok(heads.union(&RevsetExpression::root()).ancestors())
}
