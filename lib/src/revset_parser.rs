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

use std::collections::HashSet;
use std::{error, mem};

use itertools::Itertools as _;
use once_cell::sync::Lazy;
use pest::iterators::{Pair, Pairs};
use pest::pratt_parser::{Assoc, Op, PrattParser};
use pest::Parser;
use pest_derive::Parser;
use thiserror::Error;

use crate::dsl_util::{
    self, collect_similar, AliasDeclaration, AliasDeclarationParser, AliasDefinitionParser,
    AliasExpandError, AliasExpandableExpression, AliasId, AliasesMap, ExpressionFolder,
    FoldableExpression, InvalidArguments, KeywordArgument, StringLiteralParser,
};
// TODO: remove reverse dependency on revset module
use crate::revset::RevsetModifier;

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
            Rule::function => None,
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
            Rule::function_alias_declaration => None,
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
    #[error(r#"Function parameter "{0}" cannot be expanded"#)]
    BadParameterExpansion(String),
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

    /// If this is a `NoSuchFunction` error, expands the candidates list with
    /// the given `other_functions`.
    pub(super) fn extend_function_candidates<I>(mut self, other_functions: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        if let RevsetParseErrorKind::NoSuchFunction { name, candidates } = &mut self.kind {
            let other_candidates = collect_similar(name, other_functions);
            *candidates = itertools::merge(mem::take(candidates), other_candidates)
                .dedup()
                .collect();
        }
        self
    }

    pub fn kind(&self) -> &RevsetParseErrorKind {
        &self.kind
    }

    /// Original parsing error which typically occurred in an alias expression.
    pub fn origin(&self) -> Option<&Self> {
        self.source.as_ref().and_then(|e| e.downcast_ref())
    }
}

impl AliasExpandError for RevsetParseError {
    fn invalid_arguments(err: InvalidArguments<'_>) -> Self {
        err.into()
    }

    fn recursive_expansion(id: AliasId<'_>, span: pest::Span<'_>) -> Self {
        Self::with_span(RevsetParseErrorKind::RecursiveAlias(id.to_string()), span)
    }

    fn within_alias_expansion(self, id: AliasId<'_>, span: pest::Span<'_>) -> Self {
        let kind = match id {
            AliasId::Symbol(_) | AliasId::Function(_) => {
                RevsetParseErrorKind::BadAliasExpansion(id.to_string())
            }
            AliasId::Parameter(_) => RevsetParseErrorKind::BadParameterExpansion(id.to_string()),
        };
        Self::with_span(kind, span).with_source(self)
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

impl From<InvalidArguments<'_>> for RevsetParseError {
    fn from(err: InvalidArguments<'_>) -> Self {
        // TODO: Perhaps, we can add generic Expression error for invalid
        // pattern, etc., and Self::invalid_arguments() can be inlined.
        Self::invalid_arguments(err.name, err.message, err.span)
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpressionKind<'i> {
    /// Unquoted symbol.
    Identifier(&'i str),
    /// Quoted symbol or string.
    String(String),
    /// `<kind>:<value>`
    StringPattern {
        kind: &'i str,
        value: String,
    },
    /// `<name>@<remote>`
    RemoteSymbol {
        name: String,
        remote: String,
    },
    /// `<workspace_id>@`
    AtWorkspace(String),
    /// `@`
    AtCurrentWorkspace,
    /// `::`
    DagRangeAll,
    /// `..`
    RangeAll,
    Unary(UnaryOp, Box<ExpressionNode<'i>>),
    Binary(BinaryOp, Box<ExpressionNode<'i>>, Box<ExpressionNode<'i>>),
    FunctionCall(Box<FunctionCallNode<'i>>),
    /// Identity node to preserve the span in the source text.
    AliasExpanded(AliasId<'i>, Box<ExpressionNode<'i>>),
}

impl<'i> FoldableExpression<'i> for ExpressionKind<'i> {
    fn fold<F>(self, folder: &mut F, span: pest::Span<'i>) -> Result<Self, F::Error>
    where
        F: ExpressionFolder<'i, Self> + ?Sized,
    {
        match self {
            ExpressionKind::Identifier(name) => folder.fold_identifier(name, span),
            ExpressionKind::String(_)
            | ExpressionKind::StringPattern { .. }
            | ExpressionKind::RemoteSymbol { .. }
            | ExpressionKind::AtWorkspace(_)
            | ExpressionKind::AtCurrentWorkspace
            | ExpressionKind::DagRangeAll
            | ExpressionKind::RangeAll => Ok(self),
            ExpressionKind::Unary(op, arg) => {
                let arg = Box::new(folder.fold_expression(*arg)?);
                Ok(ExpressionKind::Unary(op, arg))
            }
            ExpressionKind::Binary(op, lhs, rhs) => {
                let lhs = Box::new(folder.fold_expression(*lhs)?);
                let rhs = Box::new(folder.fold_expression(*rhs)?);
                Ok(ExpressionKind::Binary(op, lhs, rhs))
            }
            ExpressionKind::FunctionCall(function) => folder.fold_function_call(function, span),
            ExpressionKind::AliasExpanded(id, subst) => {
                let subst = Box::new(folder.fold_expression(*subst)?);
                Ok(ExpressionKind::AliasExpanded(id, subst))
            }
        }
    }
}

impl<'i> AliasExpandableExpression<'i> for ExpressionKind<'i> {
    fn identifier(name: &'i str) -> Self {
        ExpressionKind::Identifier(name)
    }

    fn function_call(function: Box<FunctionCallNode<'i>>) -> Self {
        ExpressionKind::FunctionCall(function)
    }

    fn alias_expanded(id: AliasId<'i>, subst: Box<ExpressionNode<'i>>) -> Self {
        ExpressionKind::AliasExpanded(id, subst)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UnaryOp {
    /// `~x`
    Negate,
    /// `::x`
    DagRangePre,
    /// `x::`
    DagRangePost,
    /// `..x`
    RangePre,
    /// `x..`
    RangePost,
    /// `x-`
    Parents,
    /// `x+`
    Children,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BinaryOp {
    /// `|`
    Union,
    /// `&`
    Intersection,
    /// `~`
    Difference,
    /// `::`
    DagRange,
    /// `..`
    Range,
}

pub type ExpressionNode<'i> = dsl_util::ExpressionNode<'i, ExpressionKind<'i>>;
pub type FunctionCallNode<'i> = dsl_util::FunctionCallNode<'i, ExpressionKind<'i>>;

pub(super) fn parse_program(revset_str: &str) -> Result<ExpressionNode, RevsetParseError> {
    let mut pairs = RevsetParser::parse(Rule::program, revset_str)?;
    let first = pairs.next().unwrap();
    parse_expression_node(first.into_inner())
}

pub(super) fn parse_program_with_modifier(
    revset_str: &str,
) -> Result<(ExpressionNode, Option<RevsetModifier>), RevsetParseError> {
    let mut pairs = RevsetParser::parse(Rule::program_with_modifier, revset_str)?;
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::expression => {
            let node = parse_expression_node(first.into_inner())?;
            Ok((node, None))
        }
        Rule::program_modifier => {
            let (lhs, op) = first.into_inner().collect_tuple().unwrap();
            let rhs = pairs.next().unwrap();
            assert_eq!(lhs.as_rule(), Rule::identifier);
            assert_eq!(op.as_rule(), Rule::pattern_kind_op);
            assert_eq!(rhs.as_rule(), Rule::expression);
            let modififer = match lhs.as_str() {
                "all" => RevsetModifier::All,
                name => {
                    return Err(RevsetParseError::with_span(
                        RevsetParseErrorKind::NoSuchModifier(name.to_owned()),
                        lhs.as_span(),
                    ));
                }
            };
            let node = parse_expression_node(rhs.into_inner())?;
            Ok((node, Some(modififer)))
        }
        r => panic!("unexpected revset parse rule: {r:?}"),
    }
}

fn parse_expression_node(pairs: Pairs<Rule>) -> Result<ExpressionNode, RevsetParseError> {
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
        .map_primary(|primary| {
            let expr = match primary.as_rule() {
                Rule::primary => return parse_primary_node(primary),
                Rule::dag_range_all_op => ExpressionKind::DagRangeAll,
                Rule::range_all_op => ExpressionKind::RangeAll,
                r => panic!("unexpected primary rule {r:?}"),
            };
            Ok(ExpressionNode::new(expr, primary.as_span()))
        })
        .map_prefix(|op, rhs| {
            let op_kind = match op.as_rule() {
                Rule::negate_op => UnaryOp::Negate,
                Rule::dag_range_pre_op => UnaryOp::DagRangePre,
                Rule::compat_dag_range_pre_op => Err(not_prefix_op(&op, "::", "ancestors"))?,
                Rule::range_pre_op => UnaryOp::RangePre,
                r => panic!("unexpected prefix operator rule {r:?}"),
            };
            let rhs = Box::new(rhs?);
            let span = op.as_span().start_pos().span(&rhs.span.end_pos());
            let expr = ExpressionKind::Unary(op_kind, rhs);
            Ok(ExpressionNode::new(expr, span))
        })
        .map_postfix(|lhs, op| {
            let op_kind = match op.as_rule() {
                Rule::dag_range_post_op => UnaryOp::DagRangePost,
                Rule::compat_dag_range_post_op => Err(not_postfix_op(&op, "::", "descendants"))?,
                Rule::range_post_op => UnaryOp::RangePost,
                Rule::parents_op => UnaryOp::Parents,
                Rule::children_op => UnaryOp::Children,
                Rule::compat_parents_op => Err(not_postfix_op(&op, "-", "parents"))?,
                r => panic!("unexpected postfix operator rule {r:?}"),
            };
            let lhs = Box::new(lhs?);
            let span = lhs.span.start_pos().span(&op.as_span().end_pos());
            let expr = ExpressionKind::Unary(op_kind, lhs);
            Ok(ExpressionNode::new(expr, span))
        })
        .map_infix(|lhs, op, rhs| {
            let op_kind = match op.as_rule() {
                Rule::union_op => BinaryOp::Union,
                Rule::compat_add_op => Err(not_infix_op(&op, "|", "union"))?,
                Rule::intersection_op => BinaryOp::Intersection,
                Rule::difference_op => BinaryOp::Difference,
                Rule::compat_sub_op => Err(not_infix_op(&op, "~", "difference"))?,
                Rule::dag_range_op => BinaryOp::DagRange,
                Rule::compat_dag_range_op => Err(not_infix_op(&op, "::", "DAG range"))?,
                Rule::range_op => BinaryOp::Range,
                r => panic!("unexpected infix operator rule {r:?}"),
            };
            let lhs = Box::new(lhs?);
            let rhs = Box::new(rhs?);
            let span = lhs.span.start_pos().span(&rhs.span.end_pos());
            let expr = ExpressionKind::Binary(op_kind, lhs, rhs);
            Ok(ExpressionNode::new(expr, span))
        })
        .parse(pairs)
}

fn parse_primary_node(pair: Pair<Rule>) -> Result<ExpressionNode, RevsetParseError> {
    let span = pair.as_span();
    let mut pairs = pair.into_inner();
    let first = pairs.next().unwrap();
    let expr = match first.as_rule() {
        Rule::expression => return parse_expression_node(first.into_inner()),
        Rule::function => {
            let function = Box::new(parse_function_call_node(first)?);
            ExpressionKind::FunctionCall(function)
        }
        Rule::string_pattern => {
            let (lhs, op, rhs) = first.into_inner().collect_tuple().unwrap();
            assert_eq!(lhs.as_rule(), Rule::identifier);
            assert_eq!(op.as_rule(), Rule::pattern_kind_op);
            let kind = lhs.as_str();
            let value = parse_as_string_literal(rhs);
            ExpressionKind::StringPattern { kind, value }
        }
        // Identifier without "@" may be substituted by aliases. Primary expression including "@"
        // is considered an indecomposable unit, and no alias substitution would be made.
        Rule::identifier if pairs.peek().is_none() => ExpressionKind::Identifier(first.as_str()),
        Rule::identifier | Rule::string_literal | Rule::raw_string_literal => {
            let name = parse_as_string_literal(first);
            match pairs.next() {
                None => ExpressionKind::String(name),
                Some(op) => {
                    assert_eq!(op.as_rule(), Rule::at_op);
                    match pairs.next() {
                        // postfix "<workspace_id>@"
                        None => ExpressionKind::AtWorkspace(name),
                        // infix "<name>@<remote>"
                        Some(second) => {
                            let remote = parse_as_string_literal(second);
                            ExpressionKind::RemoteSymbol { name, remote }
                        }
                    }
                }
            }
        }
        // nullary "@"
        Rule::at_op => ExpressionKind::AtCurrentWorkspace,
        r => panic!("unexpected revset parse rule: {r:?}"),
    };
    Ok(ExpressionNode::new(expr, span))
}

/// Parses part of compound symbol to string.
fn parse_as_string_literal(pair: Pair<Rule>) -> String {
    match pair.as_rule() {
        Rule::identifier => pair.as_str().to_owned(),
        Rule::string_literal => STRING_LITERAL_PARSER.parse(pair.into_inner()),
        Rule::raw_string_literal => {
            let (content,) = pair.into_inner().collect_tuple().unwrap();
            assert_eq!(content.as_rule(), Rule::raw_string_content);
            content.as_str().to_owned()
        }
        _ => {
            panic!("unexpected string literal rule: {:?}", pair.as_str());
        }
    }
}

fn parse_function_call_node(pair: Pair<Rule>) -> Result<FunctionCallNode, RevsetParseError> {
    assert_eq!(pair.as_rule(), Rule::function);
    let (name_pair, args_pair) = pair.into_inner().collect_tuple().unwrap();
    assert_eq!(name_pair.as_rule(), Rule::function_name);
    assert_eq!(args_pair.as_rule(), Rule::function_arguments);
    let name_span = name_pair.as_span();
    let args_span = args_pair.as_span();
    let function_name = name_pair.as_str();
    let mut args = Vec::new();
    let mut keyword_args = Vec::new();
    for pair in args_pair.into_inner() {
        let span = pair.as_span();
        match pair.as_rule() {
            Rule::expression => {
                if !keyword_args.is_empty() {
                    return Err(RevsetParseError::invalid_arguments(
                        function_name,
                        "Positional argument follows keyword argument",
                        span,
                    ));
                }
                args.push(parse_expression_node(pair.into_inner())?);
            }
            Rule::keyword_argument => {
                let mut pairs = pair.into_inner();
                let name = pairs.next().unwrap();
                let expr = pairs.next().unwrap();
                assert_eq!(name.as_rule(), Rule::identifier);
                assert_eq!(expr.as_rule(), Rule::expression);
                let arg = KeywordArgument {
                    name: name.as_str(),
                    name_span: name.as_span(),
                    value: parse_expression_node(expr.into_inner())?,
                };
                keyword_args.push(arg);
            }
            r => panic!("unexpected argument rule {r:?}"),
        }
    }
    Ok(FunctionCallNode {
        name: function_name,
        name_span,
        args,
        keyword_args,
        args_span,
    })
}

pub type RevsetAliasesMap = AliasesMap<RevsetAliasParser>;

#[derive(Clone, Debug, Default)]
pub struct RevsetAliasParser;

impl AliasDeclarationParser for RevsetAliasParser {
    type Error = RevsetParseError;

    fn parse_declaration(&self, source: &str) -> Result<AliasDeclaration, Self::Error> {
        let mut pairs = RevsetParser::parse(Rule::alias_declaration, source)?;
        let first = pairs.next().unwrap();
        match first.as_rule() {
            Rule::identifier => Ok(AliasDeclaration::Symbol(first.as_str().to_owned())),
            Rule::function_alias_declaration => {
                let (name_pair, params_pair) = first.into_inner().collect_tuple().unwrap();
                assert_eq!(name_pair.as_rule(), Rule::function_name);
                assert_eq!(params_pair.as_rule(), Rule::formal_parameters);
                let name = name_pair.as_str().to_owned();
                let params_span = params_pair.as_span();
                let params = params_pair
                    .into_inner()
                    .map(|pair| match pair.as_rule() {
                        Rule::identifier => pair.as_str().to_owned(),
                        r => panic!("unexpected formal parameter rule {r:?}"),
                    })
                    .collect_vec();
                if params.iter().all_unique() {
                    Ok(AliasDeclaration::Function(name, params))
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

impl AliasDefinitionParser for RevsetAliasParser {
    type Output<'i> = ExpressionKind<'i>;
    type Error = RevsetParseError;

    fn parse_definition<'i>(&self, source: &'i str) -> Result<ExpressionNode<'i>, Self::Error> {
        parse_program(source)
    }
}

/// Applies the give function to the innermost `node` by unwrapping alias
/// expansion nodes.
pub(super) fn expect_literal_with<T>(
    node: &ExpressionNode,
    f: impl FnOnce(&ExpressionNode) -> Result<T, RevsetParseError>,
) -> Result<T, RevsetParseError> {
    if let ExpressionKind::AliasExpanded(id, subst) = &node.kind {
        expect_literal_with(subst, f).map_err(|e| e.within_alias_expansion(*id, node.span))
    } else {
        f(node)
    }
}
