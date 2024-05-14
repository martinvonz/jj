// Copyright 2021-2024 The Jujutsu Authors
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

#![allow(missing_docs)]

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::{error, fmt};

use itertools::Itertools as _;
use once_cell::sync::Lazy;
use pest::iterators::{Pair, Pairs};
use pest::pratt_parser::{Assoc, Op, PrattParser};
use pest::Parser;
use pest_derive::Parser;
use thiserror::Error;

use crate::dsl_util::{collect_similar, StringLiteralParser};
use crate::op_store::WorkspaceId;
// TODO: remove reverse dependency on revset module
use crate::revset::{ParseState, RevsetExpression, RevsetModifier};

#[derive(Parser)]
#[grammar = "revset.pest"]
struct RevsetParser;

const STRING_LITERAL_PARSER: StringLiteralParser<Rule> = StringLiteralParser {
    content_rule: Rule::string_content,
    escape_rule: Rule::string_escape,
};

impl Rule {
    /// Whether this is a placeholder rule for compatibility with the other
    /// systems.
    fn is_compat(&self) -> bool {
        matches!(
            self,
            Rule::compat_parents_op
                | Rule::compat_dag_range_op
                | Rule::compat_dag_range_pre_op
                | Rule::compat_dag_range_post_op
                | Rule::compat_add_op
                | Rule::compat_sub_op
        )
    }

    fn to_symbol(self) -> Option<&'static str> {
        match self {
            Rule::EOI => None,
            Rule::whitespace => None,
            Rule::identifier_part => None,
            Rule::identifier => None,
            Rule::symbol => None,
            Rule::string_escape => None,
            Rule::string_content_char => None,
            Rule::string_content => None,
            Rule::string_literal => None,
            Rule::raw_string_content => None,
            Rule::raw_string_literal => None,
            Rule::at_op => Some("@"),
            Rule::pattern_kind_op => Some(":"),
            Rule::parents_op => Some("-"),
            Rule::children_op => Some("+"),
            Rule::compat_parents_op => Some("^"),
            Rule::dag_range_op
            | Rule::dag_range_pre_op
            | Rule::dag_range_post_op
            | Rule::dag_range_all_op => Some("::"),
            Rule::compat_dag_range_op
            | Rule::compat_dag_range_pre_op
            | Rule::compat_dag_range_post_op => Some(":"),
            Rule::range_op => Some(".."),
            Rule::range_pre_op | Rule::range_post_op | Rule::range_all_op => Some(".."),
            Rule::range_ops => None,
            Rule::range_pre_ops => None,
            Rule::range_post_ops => None,
            Rule::range_all_ops => None,
            Rule::negate_op => Some("~"),
            Rule::union_op => Some("|"),
            Rule::intersection_op => Some("&"),
            Rule::difference_op => Some("~"),
            Rule::compat_add_op => Some("+"),
            Rule::compat_sub_op => Some("-"),
            Rule::infix_op => None,
            Rule::function_name => None,
            Rule::keyword_argument => None,
            Rule::argument => None,
            Rule::function_arguments => None,
            Rule::formal_parameters => None,
            Rule::string_pattern => None,
            Rule::primary => None,
            Rule::neighbors_expression => None,
            Rule::range_expression => None,
            Rule::expression => None,
            Rule::program => None,
            Rule::program_modifier => None,
            Rule::program_with_modifier => None,
            Rule::alias_declaration_part => None,
            Rule::alias_declaration => None,
        }
    }
}

#[derive(Debug, Error)]
#[error("{pest_error}")]
pub struct RevsetParseError {
    // TODO: move parsing tests to this module and drop pub(super)
    pub(super) kind: RevsetParseErrorKind,
    pest_error: Box<pest::error::Error<Rule>>,
    source: Option<Box<dyn error::Error + Send + Sync>>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RevsetParseErrorKind {
    #[error("Syntax error")]
    SyntaxError,
    #[error("'{op}' is not a prefix operator")]
    NotPrefixOperator {
        op: String,
        similar_op: String,
        description: String,
    },
    #[error("'{op}' is not a postfix operator")]
    NotPostfixOperator {
        op: String,
        similar_op: String,
        description: String,
    },
    #[error("'{op}' is not an infix operator")]
    NotInfixOperator {
        op: String,
        similar_op: String,
        description: String,
    },
    #[error(r#"Modifier "{0}" doesn't exist"#)]
    NoSuchModifier(String),
    #[error(r#"Function "{name}" doesn't exist"#)]
    NoSuchFunction {
        name: String,
        candidates: Vec<String>,
    },
    #[error(r#"Function "{name}": {message}"#)]
    InvalidFunctionArguments { name: String, message: String },
    #[error("Cannot resolve file pattern without workspace")]
    FsPathWithoutWorkspace,
    #[error(r#"Cannot resolve "@" without workspace"#)]
    WorkingCopyWithoutWorkspace,
    #[error("Redefinition of function parameter")]
    RedefinedFunctionParameter,
    #[error(r#"Alias "{0}" cannot be expanded"#)]
    BadAliasExpansion(String),
    #[error(r#"Alias "{0}" expanded recursively"#)]
    RecursiveAlias(String),
}

impl RevsetParseError {
    pub(super) fn with_span(kind: RevsetParseErrorKind, span: pest::Span<'_>) -> Self {
        let message = kind.to_string();
        let pest_error = Box::new(pest::error::Error::new_from_span(
            pest::error::ErrorVariant::CustomError { message },
            span,
        ));
        RevsetParseError {
            kind,
            pest_error,
            source: None,
        }
    }

    pub(super) fn with_source(
        mut self,
        source: impl Into<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        self.source = Some(source.into());
        self
    }

    pub(super) fn invalid_arguments(
        name: impl Into<String>,
        message: impl Into<String>,
        span: pest::Span<'_>,
    ) -> Self {
        Self::with_span(
            RevsetParseErrorKind::InvalidFunctionArguments {
                name: name.into(),
                message: message.into(),
            },
            span,
        )
    }

    pub fn kind(&self) -> &RevsetParseErrorKind {
        &self.kind
    }

    /// Original parsing error which typically occurred in an alias expression.
    pub fn origin(&self) -> Option<&Self> {
        self.source.as_ref().and_then(|e| e.downcast_ref())
    }
}

impl From<pest::error::Error<Rule>> for RevsetParseError {
    fn from(err: pest::error::Error<Rule>) -> Self {
        RevsetParseError {
            kind: RevsetParseErrorKind::SyntaxError,
            pest_error: Box::new(rename_rules_in_pest_error(err)),
            source: None,
        }
    }
}

fn rename_rules_in_pest_error(mut err: pest::error::Error<Rule>) -> pest::error::Error<Rule> {
    let pest::error::ErrorVariant::ParsingError {
        positives,
        negatives,
    } = &mut err.variant
    else {
        return err;
    };

    // Remove duplicated symbols. Compat symbols are also removed from the
    // (positive) suggestion.
    let mut known_syms = HashSet::new();
    positives.retain(|rule| {
        !rule.is_compat() && rule.to_symbol().map_or(true, |sym| known_syms.insert(sym))
    });
    let mut known_syms = HashSet::new();
    negatives.retain(|rule| rule.to_symbol().map_or(true, |sym| known_syms.insert(sym)));
    err.renamed_rules(|rule| {
        rule.to_symbol()
            .map(|sym| format!("`{sym}`"))
            .unwrap_or_else(|| format!("<{rule:?}>"))
    })
}

pub(super) fn parse_program(
    revset_str: &str,
    state: ParseState,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let mut pairs = RevsetParser::parse(Rule::program, revset_str)?;
    let first = pairs.next().unwrap();
    parse_expression_rule(first.into_inner(), state)
}

pub(super) fn parse_program_with_modifier(
    revset_str: &str,
    state: ParseState,
) -> Result<(Rc<RevsetExpression>, Option<RevsetModifier>), RevsetParseError> {
    let mut pairs = RevsetParser::parse(Rule::program_with_modifier, revset_str)?;
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::expression => {
            let expression = parse_expression_rule(first.into_inner(), state)?;
            Ok((expression, None))
        }
        Rule::program_modifier => {
            let (lhs, op) = first.into_inner().collect_tuple().unwrap();
            let rhs = pairs.next().unwrap();
            assert_eq!(lhs.as_rule(), Rule::identifier);
            assert_eq!(op.as_rule(), Rule::pattern_kind_op);
            assert_eq!(rhs.as_rule(), Rule::expression);
            let modififer = match lhs.as_str() {
                "all" => RevsetModifier::All,
                "prompt" => RevsetModifier::Prompt,
                name => {
                    return Err(RevsetParseError::with_span(
                        RevsetParseErrorKind::NoSuchModifier(name.to_owned()),
                        lhs.as_span(),
                    ));
                }
            };
            let expression = parse_expression_rule(rhs.into_inner(), state)?;
            Ok((expression, Some(modififer)))
        }
        r => panic!("unexpected revset parse rule: {r:?}"),
    }
}

pub fn parse_expression_rule(
    pairs: Pairs<Rule>,
    state: ParseState,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    fn not_prefix_op(
        op: &Pair<Rule>,
        similar_op: impl Into<String>,
        description: impl Into<String>,
    ) -> RevsetParseError {
        RevsetParseError::with_span(
            RevsetParseErrorKind::NotPrefixOperator {
                op: op.as_str().to_owned(),
                similar_op: similar_op.into(),
                description: description.into(),
            },
            op.as_span(),
        )
    }

    fn not_postfix_op(
        op: &Pair<Rule>,
        similar_op: impl Into<String>,
        description: impl Into<String>,
    ) -> RevsetParseError {
        RevsetParseError::with_span(
            RevsetParseErrorKind::NotPostfixOperator {
                op: op.as_str().to_owned(),
                similar_op: similar_op.into(),
                description: description.into(),
            },
            op.as_span(),
        )
    }

    fn not_infix_op(
        op: &Pair<Rule>,
        similar_op: impl Into<String>,
        description: impl Into<String>,
    ) -> RevsetParseError {
        RevsetParseError::with_span(
            RevsetParseErrorKind::NotInfixOperator {
                op: op.as_str().to_owned(),
                similar_op: similar_op.into(),
                description: description.into(),
            },
            op.as_span(),
        )
    }

    static PRATT: Lazy<PrattParser<Rule>> = Lazy::new(|| {
        PrattParser::new()
            .op(Op::infix(Rule::union_op, Assoc::Left)
                | Op::infix(Rule::compat_add_op, Assoc::Left))
            .op(Op::infix(Rule::intersection_op, Assoc::Left)
                | Op::infix(Rule::difference_op, Assoc::Left)
                | Op::infix(Rule::compat_sub_op, Assoc::Left))
            .op(Op::prefix(Rule::negate_op))
            // Ranges can't be nested without parentheses. Associativity doesn't matter.
            .op(Op::infix(Rule::dag_range_op, Assoc::Left)
                | Op::infix(Rule::compat_dag_range_op, Assoc::Left)
                | Op::infix(Rule::range_op, Assoc::Left))
            .op(Op::prefix(Rule::dag_range_pre_op)
                | Op::prefix(Rule::compat_dag_range_pre_op)
                | Op::prefix(Rule::range_pre_op))
            .op(Op::postfix(Rule::dag_range_post_op)
                | Op::postfix(Rule::compat_dag_range_post_op)
                | Op::postfix(Rule::range_post_op))
            // Neighbors
            .op(Op::postfix(Rule::parents_op)
                | Op::postfix(Rule::children_op)
                | Op::postfix(Rule::compat_parents_op))
    });
    PRATT
        .map_primary(|primary| match primary.as_rule() {
            Rule::primary => parse_primary_rule(primary, state),
            Rule::dag_range_all_op => Ok(RevsetExpression::all()),
            Rule::range_all_op => {
                Ok(RevsetExpression::root().range(&RevsetExpression::visible_heads()))
            }
            r => panic!("unexpected primary rule {r:?}"),
        })
        .map_prefix(|op, rhs| match op.as_rule() {
            Rule::negate_op => Ok(rhs?.negated()),
            Rule::dag_range_pre_op => Ok(rhs?.ancestors()),
            Rule::compat_dag_range_pre_op => Err(not_prefix_op(&op, "::", "ancestors")),
            Rule::range_pre_op => Ok(RevsetExpression::root().range(&rhs?)),
            r => panic!("unexpected prefix operator rule {r:?}"),
        })
        .map_postfix(|lhs, op| match op.as_rule() {
            Rule::dag_range_post_op => Ok(lhs?.descendants()),
            Rule::compat_dag_range_post_op => Err(not_postfix_op(&op, "::", "descendants")),
            Rule::range_post_op => Ok(lhs?.range(&RevsetExpression::visible_heads())),
            Rule::parents_op => Ok(lhs?.parents()),
            Rule::children_op => Ok(lhs?.children()),
            Rule::compat_parents_op => Err(not_postfix_op(&op, "-", "parents")),
            r => panic!("unexpected postfix operator rule {r:?}"),
        })
        .map_infix(|lhs, op, rhs| match op.as_rule() {
            Rule::union_op => Ok(lhs?.union(&rhs?)),
            Rule::compat_add_op => Err(not_infix_op(&op, "|", "union")),
            Rule::intersection_op => Ok(lhs?.intersection(&rhs?)),
            Rule::difference_op => Ok(lhs?.minus(&rhs?)),
            Rule::compat_sub_op => Err(not_infix_op(&op, "~", "difference")),
            Rule::dag_range_op => Ok(lhs?.dag_range_to(&rhs?)),
            Rule::compat_dag_range_op => Err(not_infix_op(&op, "::", "DAG range")),
            Rule::range_op => Ok(lhs?.range(&rhs?)),
            r => panic!("unexpected infix operator rule {r:?}"),
        })
        .parse(pairs)
}

fn parse_primary_rule(
    pair: Pair<Rule>,
    state: ParseState,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let span = pair.as_span();
    let mut pairs = pair.into_inner();
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::expression => parse_expression_rule(first.into_inner(), state),
        Rule::function_name => {
            let arguments_pair = pairs.next().unwrap();
            parse_function_expression(first, arguments_pair, state, span)
        }
        Rule::string_pattern => parse_string_pattern_rule(first, state),
        // Symbol without "@" may be substituted by aliases. Primary expression including "@"
        // is considered an indecomposable unit, and no alias substitution would be made.
        Rule::symbol if pairs.peek().is_none() => parse_symbol_rule(first.into_inner(), state),
        Rule::symbol => {
            let name = parse_symbol_rule_as_literal(first.into_inner());
            assert_eq!(pairs.next().unwrap().as_rule(), Rule::at_op);
            if let Some(second) = pairs.next() {
                // infix "<name>@<remote>"
                assert_eq!(second.as_rule(), Rule::symbol);
                let remote = parse_symbol_rule_as_literal(second.into_inner());
                Ok(RevsetExpression::remote_symbol(name, remote))
            } else {
                // postfix "<workspace_id>@"
                Ok(RevsetExpression::working_copy(WorkspaceId::new(name)))
            }
        }
        Rule::at_op => {
            // nullary "@"
            let ctx = state.workspace_ctx.as_ref().ok_or_else(|| {
                RevsetParseError::with_span(RevsetParseErrorKind::WorkingCopyWithoutWorkspace, span)
            })?;
            Ok(RevsetExpression::working_copy(ctx.workspace_id.clone()))
        }
        _ => {
            panic!("unexpected revset parse rule: {:?}", first.as_str());
        }
    }
}

fn parse_string_pattern_rule(
    pair: Pair<Rule>,
    state: ParseState,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    assert_eq!(pair.as_rule(), Rule::string_pattern);
    let (lhs, op, rhs) = pair.into_inner().collect_tuple().unwrap();
    assert_eq!(lhs.as_rule(), Rule::identifier);
    assert_eq!(op.as_rule(), Rule::pattern_kind_op);
    assert_eq!(rhs.as_rule(), Rule::symbol);
    if state.allow_string_pattern {
        let kind = lhs.as_str().to_owned();
        let value = parse_symbol_rule_as_literal(rhs.into_inner());
        Ok(Rc::new(RevsetExpression::StringPattern { kind, value }))
    } else {
        Err(RevsetParseError::with_span(
            RevsetParseErrorKind::NotInfixOperator {
                op: op.as_str().to_owned(),
                similar_op: "::".to_owned(),
                description: "DAG range".to_owned(),
            },
            op.as_span(),
        ))
    }
}

/// Parses symbol to expression, expands aliases as needed.
fn parse_symbol_rule(
    pairs: Pairs<Rule>,
    state: ParseState,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let first = pairs.peek().unwrap();
    match first.as_rule() {
        Rule::identifier => {
            let name = first.as_str();
            if let Some(expr) = state.locals.get(name) {
                Ok(expr.clone())
            } else if let Some(defn) = state.aliases_map.get_symbol(name) {
                let id = RevsetAliasId::Symbol(name);
                let locals = HashMap::new(); // Don't spill out the current scope
                state.with_alias_expanding(id, &locals, first.as_span(), |state| {
                    parse_program(defn, state)
                })
            } else {
                Ok(RevsetExpression::symbol(name.to_owned()))
            }
        }
        _ => {
            let text = parse_symbol_rule_as_literal(pairs);
            Ok(RevsetExpression::symbol(text))
        }
    }
}

/// Parses part of compound symbol to string without alias substitution.
fn parse_symbol_rule_as_literal(mut pairs: Pairs<Rule>) -> String {
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::identifier => first.as_str().to_owned(),
        Rule::string_literal => STRING_LITERAL_PARSER.parse(first.into_inner()),
        Rule::raw_string_literal => {
            let (content,) = first.into_inner().collect_tuple().unwrap();
            assert_eq!(content.as_rule(), Rule::raw_string_content);
            content.as_str().to_owned()
        }
        _ => {
            panic!("unexpected symbol parse rule: {:?}", first.as_str());
        }
    }
}

fn parse_function_expression(
    name_pair: Pair<Rule>,
    arguments_pair: Pair<Rule>,
    state: ParseState,
    primary_span: pest::Span<'_>,
) -> Result<Rc<RevsetExpression>, RevsetParseError> {
    let name = name_pair.as_str();
    if let Some((params, defn)) = state.aliases_map.get_function(name) {
        // Resolve arguments in the current scope, and pass them in to the alias
        // expansion scope.
        let (required, optional) =
            expect_named_arguments_vec(name, &[], arguments_pair, params.len(), params.len())?;
        assert!(optional.is_empty());
        let args: Vec<_> = required
            .into_iter()
            .map(|arg| parse_expression_rule(arg.into_inner(), state))
            .try_collect()?;
        let id = RevsetAliasId::Function(name);
        let locals = params.iter().map(|s| s.as_str()).zip(args).collect();
        state.with_alias_expanding(id, &locals, primary_span, |state| {
            parse_program(defn, state)
        })
    } else if let Some(func) = state.function_map.get(name) {
        func(name, arguments_pair, state)
    } else {
        Err(RevsetParseError::with_span(
            RevsetParseErrorKind::NoSuchFunction {
                name: name.to_owned(),
                candidates: collect_similar(
                    name,
                    itertools::chain(
                        state.function_map.keys().copied(),
                        state
                            .aliases_map
                            .function_aliases
                            .keys()
                            .map(|n| n.as_ref()),
                    ),
                ),
            },
            name_pair.as_span(),
        ))
    }
}

#[derive(Clone, Debug, Default)]
pub struct RevsetAliasesMap {
    symbol_aliases: HashMap<String, String>,
    function_aliases: HashMap<String, (Vec<String>, String)>,
}

impl RevsetAliasesMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds new substitution rule `decl = defn`.
    ///
    /// Returns error if `decl` is invalid. The `defn` part isn't checked. A bad
    /// `defn` will be reported when the alias is substituted.
    pub fn insert(
        &mut self,
        decl: impl AsRef<str>,
        defn: impl Into<String>,
    ) -> Result<(), RevsetParseError> {
        match RevsetAliasDeclaration::parse(decl.as_ref())? {
            RevsetAliasDeclaration::Symbol(name) => {
                self.symbol_aliases.insert(name, defn.into());
            }
            RevsetAliasDeclaration::Function(name, params) => {
                self.function_aliases.insert(name, (params, defn.into()));
            }
        }
        Ok(())
    }

    pub fn get_symbol(&self, name: &str) -> Option<&str> {
        self.symbol_aliases.get(name).map(|defn| defn.as_ref())
    }

    pub fn get_function(&self, name: &str) -> Option<(&[String], &str)> {
        self.function_aliases
            .get(name)
            .map(|(params, defn)| (params.as_ref(), defn.as_ref()))
    }
}

/// Parsed declaration part of alias rule.
#[derive(Clone, Debug)]
enum RevsetAliasDeclaration {
    Symbol(String),
    Function(String, Vec<String>),
}

impl RevsetAliasDeclaration {
    fn parse(source: &str) -> Result<Self, RevsetParseError> {
        let mut pairs = RevsetParser::parse(Rule::alias_declaration, source)?;
        let first = pairs.next().unwrap();
        match first.as_rule() {
            Rule::identifier => Ok(RevsetAliasDeclaration::Symbol(first.as_str().to_owned())),
            Rule::function_name => {
                let name = first.as_str().to_owned();
                let params_pair = pairs.next().unwrap();
                let params_span = params_pair.as_span();
                let params = params_pair
                    .into_inner()
                    .map(|pair| match pair.as_rule() {
                        Rule::identifier => pair.as_str().to_owned(),
                        r => panic!("unexpected formal parameter rule {r:?}"),
                    })
                    .collect_vec();
                if params.iter().all_unique() {
                    Ok(RevsetAliasDeclaration::Function(name, params))
                } else {
                    Err(RevsetParseError::with_span(
                        RevsetParseErrorKind::RedefinedFunctionParameter,
                        params_span,
                    ))
                }
            }
            r => panic!("unexpected alias declaration rule {r:?}"),
        }
    }
}

/// Borrowed reference to identify alias expression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RevsetAliasId<'a> {
    Symbol(&'a str),
    Function(&'a str),
}

impl fmt::Display for RevsetAliasId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RevsetAliasId::Symbol(name) => write!(f, "{name}"),
            RevsetAliasId::Function(name) => write!(f, "{name}()"),
        }
    }
}

type OptionalArg<'i> = Option<Pair<'i, Rule>>;

pub fn expect_no_arguments(
    function_name: &str,
    arguments_pair: Pair<Rule>,
) -> Result<(), RevsetParseError> {
    let ([], []) = expect_arguments(function_name, arguments_pair)?;
    Ok(())
}

pub fn expect_one_argument<'i>(
    function_name: &str,
    arguments_pair: Pair<'i, Rule>,
) -> Result<Pair<'i, Rule>, RevsetParseError> {
    let ([arg], []) = expect_arguments(function_name, arguments_pair)?;
    Ok(arg)
}

pub fn expect_arguments<'i, const N: usize, const M: usize>(
    function_name: &str,
    arguments_pair: Pair<'i, Rule>,
) -> Result<([Pair<'i, Rule>; N], [OptionalArg<'i>; M]), RevsetParseError> {
    expect_named_arguments(function_name, &[], arguments_pair)
}

/// Extracts N required arguments and M optional arguments.
///
/// `argument_names` is a list of argument names. Unnamed positional arguments
/// should be padded with `""`.
pub fn expect_named_arguments<'i, const N: usize, const M: usize>(
    function_name: &str,
    argument_names: &[&str],
    arguments_pair: Pair<'i, Rule>,
) -> Result<([Pair<'i, Rule>; N], [OptionalArg<'i>; M]), RevsetParseError> {
    let (required, optional) =
        expect_named_arguments_vec(function_name, argument_names, arguments_pair, N, N + M)?;
    Ok((required.try_into().unwrap(), optional.try_into().unwrap()))
}

pub fn expect_named_arguments_vec<'i>(
    function_name: &str,
    argument_names: &[&str],
    arguments_pair: Pair<'i, Rule>,
    min_arg_count: usize,
    max_arg_count: usize,
) -> Result<(Vec<Pair<'i, Rule>>, Vec<OptionalArg<'i>>), RevsetParseError> {
    assert!(argument_names.len() <= max_arg_count);
    let arguments_span = arguments_pair.as_span();
    let make_count_error = || {
        let message = if min_arg_count == max_arg_count {
            format!("Expected {min_arg_count} arguments")
        } else {
            format!("Expected {min_arg_count} to {max_arg_count} arguments")
        };
        RevsetParseError::invalid_arguments(function_name, message, arguments_span)
    };

    let mut pos_iter = Some(0..max_arg_count);
    let mut extracted_pairs = vec![None; max_arg_count];
    for pair in arguments_pair.into_inner() {
        let span = pair.as_span();
        match pair.as_rule() {
            Rule::expression => {
                let pos = pos_iter
                    .as_mut()
                    .ok_or_else(|| {
                        RevsetParseError::invalid_arguments(
                            function_name,
                            "Positional argument follows keyword argument",
                            span,
                        )
                    })?
                    .next()
                    .ok_or_else(make_count_error)?;
                assert!(extracted_pairs[pos].is_none());
                extracted_pairs[pos] = Some(pair);
            }
            Rule::keyword_argument => {
                pos_iter = None; // No more positional arguments
                let mut pairs = pair.into_inner();
                let name = pairs.next().unwrap();
                let expr = pairs.next().unwrap();
                assert_eq!(name.as_rule(), Rule::identifier);
                assert_eq!(expr.as_rule(), Rule::expression);
                let pos = argument_names
                    .iter()
                    .position(|&n| n == name.as_str())
                    .ok_or_else(|| {
                        RevsetParseError::invalid_arguments(
                            function_name,
                            format!(r#"Unexpected keyword argument "{}""#, name.as_str()),
                            span,
                        )
                    })?;
                if extracted_pairs[pos].is_some() {
                    return Err(RevsetParseError::invalid_arguments(
                        function_name,
                        format!(r#"Got multiple values for keyword "{}""#, name.as_str()),
                        span,
                    ));
                }
                extracted_pairs[pos] = Some(expr);
            }
            r => panic!("unexpected argument rule {r:?}"),
        }
    }

    assert_eq!(extracted_pairs.len(), max_arg_count);
    let optional = extracted_pairs.split_off(min_arg_count);
    let required = extracted_pairs.into_iter().flatten().collect_vec();
    if required.len() != min_arg_count {
        return Err(make_count_error());
    }
    Ok((required, optional))
}
