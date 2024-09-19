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

use std::any::Any;
use std::rc::Rc;

use itertools::Itertools;
use jj_cli::cli_util::CliRunner;
use jj_cli::commit_templater::CommitTemplateBuildFnTable;
use jj_cli::commit_templater::CommitTemplateLanguage;
use jj_cli::commit_templater::CommitTemplateLanguageExtension;
use jj_cli::template_builder::TemplateLanguage;
use jj_cli::template_parser;
use jj_cli::template_parser::TemplateParseError;
use jj_cli::templater::TemplatePropertyExt as _;
use jj_lib::backend::CommitId;
use jj_lib::commit::Commit;
use jj_lib::extensions_map::ExtensionsMap;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::Repo;
use jj_lib::revset::FunctionCallNode;
use jj_lib::revset::PartialSymbolResolver;
use jj_lib::revset::RevsetDiagnostics;
use jj_lib::revset::RevsetExpression;
use jj_lib::revset::RevsetFilterExtension;
use jj_lib::revset::RevsetFilterPredicate;
use jj_lib::revset::RevsetParseContext;
use jj_lib::revset::RevsetParseError;
use jj_lib::revset::RevsetResolutionError;
use jj_lib::revset::SymbolResolverExtension;
use once_cell::sync::OnceCell;

struct HexCounter;

fn num_digits_in_id(id: &CommitId) -> i64 {
    let mut count = 0;
    for ch in id.hex().chars() {
        if ch.is_ascii_digit() {
            count += 1;
        }
    }
    count
}

fn num_char_in_id(commit: Commit, ch_match: char) -> i64 {
    let mut count = 0;
    for ch in commit.id().hex().chars() {
        if ch == ch_match {
            count += 1;
        }
    }
    count
}

#[derive(Default)]
struct MostDigitsInId {
    count: OnceCell<i64>,
}

impl MostDigitsInId {
    fn count(&self, repo: &dyn Repo) -> i64 {
        *self.count.get_or_init(|| {
            RevsetExpression::all()
                .evaluate_programmatic(repo)
                .unwrap()
                .iter()
                .map(|id| num_digits_in_id(&id))
                .max()
                .unwrap_or(0)
        })
    }
}

#[derive(Default)]
struct TheDigitestResolver {
    cache: MostDigitsInId,
}

impl PartialSymbolResolver for TheDigitestResolver {
    fn resolve_symbol(
        &self,
        repo: &dyn Repo,
        symbol: &str,
    ) -> Result<Option<Vec<CommitId>>, RevsetResolutionError> {
        if symbol != "thedigitest" {
            return Ok(None);
        }

        Ok(Some(
            RevsetExpression::all()
                .evaluate_programmatic(repo)
                .map_err(|err| RevsetResolutionError::Other(err.into()))?
                .iter()
                .filter(|id| num_digits_in_id(id) == self.cache.count(repo))
                .collect_vec(),
        ))
    }
}

struct TheDigitest;

impl SymbolResolverExtension for TheDigitest {
    fn new_resolvers<'a>(&self, _repo: &'a dyn Repo) -> Vec<Box<dyn PartialSymbolResolver + 'a>> {
        vec![Box::<TheDigitestResolver>::default()]
    }
}

impl CommitTemplateLanguageExtension for HexCounter {
    fn build_fn_table<'repo>(&self) -> CommitTemplateBuildFnTable<'repo> {
        type L<'repo> = CommitTemplateLanguage<'repo>;
        let mut table = CommitTemplateBuildFnTable::empty();
        table.commit_methods.insert(
            "has_most_digits",
            |language, _build_context, property, call| {
                call.expect_no_arguments()?;
                let most_digits = language
                    .cache_extension::<MostDigitsInId>()
                    .unwrap()
                    .count(language.repo());
                Ok(L::wrap_boolean(property.map(move |commit| {
                    num_digits_in_id(commit.id()) == most_digits
                })))
            },
        );
        table.commit_methods.insert(
            "num_digits_in_id",
            |_language, _build_context, property, call| {
                call.expect_no_arguments()?;
                Ok(L::wrap_integer(
                    property.map(|commit| num_digits_in_id(commit.id())),
                ))
            },
        );
        table.commit_methods.insert(
            "num_char_in_id",
            |_language, _build_context, property, call| {
                let [string_arg] = call.expect_exact_arguments()?;
                let char_arg =
                    template_parser::expect_string_literal_with(string_arg, |string, span| {
                        let chars: Vec<_> = string.chars().collect();
                        match chars[..] {
                            [ch] => Ok(ch),
                            _ => Err(TemplateParseError::expression(
                                "Expected singular character argument",
                                span,
                            )),
                        }
                    })?;

                Ok(L::wrap_integer(
                    property.map(move |commit| num_char_in_id(commit, char_arg)),
                ))
            },
        );

        table
    }

    fn build_cache_extensions(&self, extensions: &mut ExtensionsMap) {
        extensions.insert(MostDigitsInId::default());
    }
}

#[derive(Debug)]
struct EvenDigitsFilter;

impl RevsetFilterExtension for EvenDigitsFilter {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn matches_commit(&self, commit: &Commit) -> bool {
        num_digits_in_id(commit.id()) % 2 == 0
    }
}

fn even_digits(
    _diagnostics: &mut RevsetDiagnostics,
    function: &FunctionCallNode,
    _context: &RevsetParseContext,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    function.expect_no_arguments()?;
    Ok(RevsetExpression::filter(RevsetFilterPredicate::Extension(
        Rc::new(EvenDigitsFilter),
    )))
}

fn main() -> std::process::ExitCode {
    CliRunner::init()
        .add_symbol_resolver_extension(Box::new(TheDigitest))
        .add_revset_function_extension("even_digits", even_digits)
        .add_commit_template_extension(Box::new(HexCounter))
        .run()
}
