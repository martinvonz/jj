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
use std::str::FromStr;
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
            Rule::program_modifier => None,
            Rule::program => None,
            Rule::function_alias_declaration => None,
            Rule::alias_declaration => None,
        }
    }
}

#[derive(Debug, Error)]
#[error("{pest_error}")]
pub struct RevsetParseError {
    kind: RevsetParseErrorKind,
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
    #[error("{0}")]
    Expression(String),
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

    /// Some other expression error.
    pub fn expression(message: impl Into<String>, span: pest::Span<'_>) -> Self {
        Self::with_span(RevsetParseErrorKind::Expression(message.into()), span)
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
            AliasId::Symbol(_) | AliasId::Function(..) => {
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
        let kind = RevsetParseErrorKind::InvalidFunctionArguments {
            name: err.name.to_owned(),
            message: err.message,
        };
        Self::with_span(kind, err.span)
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
    /// `name: body`
    Modifier(Box<ModifierNode<'i>>),
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
            ExpressionKind::Modifier(modifier) => {
                let modifier = Box::new(ModifierNode {
                    name: modifier.name,
                    name_span: modifier.name_span,
                    body: folder.fold_expression(modifier.body)?,
                });
                Ok(ExpressionKind::Modifier(modifier))
            }
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

/// Expression with modifier `name: body`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModifierNode<'i> {
    /// Modifier name.
    pub name: &'i str,
    /// Span of the modifier name.
    pub name_span: pest::Span<'i>,
    /// Expression body.
    pub body: ExpressionNode<'i>,
}

pub(super) fn parse_program(revset_str: &str) -> Result<ExpressionNode, RevsetParseError> {
    let mut pairs = RevsetParser::parse(Rule::program, revset_str)?;
    let first = pairs.next().unwrap();
    match first.as_rule() {
        Rule::expression => parse_expression_node(first.into_inner()),
        Rule::program_modifier => {
            let (lhs, op) = first.into_inner().collect_tuple().unwrap();
            let rhs = pairs.next().unwrap();
            assert_eq!(lhs.as_rule(), Rule::identifier);
            assert_eq!(op.as_rule(), Rule::pattern_kind_op);
            assert_eq!(rhs.as_rule(), Rule::expression);
            let span = lhs.as_span().start_pos().span(&rhs.as_span().end_pos());
            let modifier = Box::new(ModifierNode {
                name: lhs.as_str(),
                name_span: lhs.as_span(),
                body: parse_expression_node(rhs.into_inner())?,
            });
            let expr = ExpressionKind::Modifier(modifier);
            Ok(ExpressionNode::new(expr, span))
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
                    return Err(InvalidArguments {
                        name: function_name,
                        message: "Positional argument follows keyword argument".to_owned(),
                        span,
                    }
                    .into());
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

/// Applies the given functions to the top-level expression body node with an
/// optional modifier. Alias expansion nodes are unwrapped accordingly.
pub(super) fn expect_program_with<B, M>(
    node: &ExpressionNode,
    parse_body: impl FnOnce(&ExpressionNode) -> Result<B, RevsetParseError>,
    parse_modifier: impl FnOnce(&str, pest::Span<'_>) -> Result<M, RevsetParseError>,
) -> Result<(B, Option<M>), RevsetParseError> {
    expect_literal_with(node, |node| match &node.kind {
        ExpressionKind::Modifier(modifier) => {
            let parsed_modifier = parse_modifier(modifier.name, modifier.name_span)?;
            Ok((parse_body(&modifier.body)?, Some(parsed_modifier)))
        }
        _ => Ok((parse_body(node)?, None)),
    })
}

pub(super) fn expect_pattern_with<T, E: Into<Box<dyn error::Error + Send + Sync>>>(
    type_name: &str,
    node: &ExpressionNode,
    parse_pattern: impl FnOnce(&str, Option<&str>) -> Result<T, E>,
) -> Result<T, RevsetParseError> {
    let wrap_error = |err: E| {
        RevsetParseError::expression(format!("Invalid {type_name}"), node.span).with_source(err)
    };
    expect_literal_with(node, |node| match &node.kind {
        ExpressionKind::Identifier(name) => parse_pattern(name, None).map_err(wrap_error),
        ExpressionKind::String(name) => parse_pattern(name, None).map_err(wrap_error),
        ExpressionKind::StringPattern { kind, value } => {
            parse_pattern(value, Some(kind)).map_err(wrap_error)
        }
        _ => Err(RevsetParseError::expression(
            format!("Expected expression of {type_name}"),
            node.span,
        )),
    })
}

pub fn expect_literal<T: FromStr>(
    type_name: &str,
    node: &ExpressionNode,
) -> Result<T, RevsetParseError> {
    let make_error = || {
        RevsetParseError::expression(
            format!("Expected expression of type {type_name}"),
            node.span,
        )
    };
    expect_literal_with(node, |node| match &node.kind {
        ExpressionKind::Identifier(name) => name.parse().map_err(|_| make_error()),
        ExpressionKind::String(name) => name.parse().map_err(|_| make_error()),
        _ => Err(make_error()),
    })
}

/// Applies the give function to the innermost `node` by unwrapping alias
/// expansion nodes.
fn expect_literal_with<T>(
    node: &ExpressionNode,
    f: impl FnOnce(&ExpressionNode) -> Result<T, RevsetParseError>,
) -> Result<T, RevsetParseError> {
    if let ExpressionKind::AliasExpanded(id, subst) = &node.kind {
        expect_literal_with(subst, f).map_err(|e| e.within_alias_expansion(*id, node.span))
    } else {
        f(node)
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;

    #[derive(Debug)]
    struct WithRevsetAliasesMap(RevsetAliasesMap);

    impl WithRevsetAliasesMap {
        fn parse<'i>(&'i self, text: &'i str) -> Result<ExpressionNode<'i>, RevsetParseError> {
            let node = parse_program(text)?;
            dsl_util::expand_aliases(node, &self.0)
        }

        fn parse_normalized<'i>(&'i self, text: &'i str) -> ExpressionNode<'i> {
            normalize_tree(self.parse(text).unwrap())
        }
    }

    fn with_aliases(
        aliases: impl IntoIterator<Item = (impl AsRef<str>, impl Into<String>)>,
    ) -> WithRevsetAliasesMap {
        let mut aliases_map = RevsetAliasesMap::new();
        for (decl, defn) in aliases {
            aliases_map.insert(decl, defn).unwrap();
        }
        WithRevsetAliasesMap(aliases_map)
    }

    fn parse_into_kind(text: &str) -> Result<ExpressionKind, RevsetParseErrorKind> {
        parse_program(text)
            .map(|node| node.kind)
            .map_err(|err| err.kind)
    }

    fn parse_normalized(text: &str) -> ExpressionNode {
        normalize_tree(parse_program(text).unwrap())
    }

    /// Drops auxiliary data from parsed tree so it can be compared with other.
    fn normalize_tree(node: ExpressionNode) -> ExpressionNode {
        fn empty_span() -> pest::Span<'static> {
            pest::Span::new("", 0, 0).unwrap()
        }

        fn normalize_list(nodes: Vec<ExpressionNode>) -> Vec<ExpressionNode> {
            nodes.into_iter().map(normalize_tree).collect()
        }

        fn normalize_function_call(function: FunctionCallNode) -> FunctionCallNode {
            FunctionCallNode {
                name: function.name,
                name_span: empty_span(),
                args: normalize_list(function.args),
                keyword_args: function
                    .keyword_args
                    .into_iter()
                    .map(|arg| KeywordArgument {
                        name: arg.name,
                        name_span: empty_span(),
                        value: normalize_tree(arg.value),
                    })
                    .collect(),
                args_span: empty_span(),
            }
        }

        let normalized_kind = match node.kind {
            ExpressionKind::Identifier(_)
            | ExpressionKind::String(_)
            | ExpressionKind::StringPattern { .. }
            | ExpressionKind::RemoteSymbol { .. }
            | ExpressionKind::AtWorkspace(_)
            | ExpressionKind::AtCurrentWorkspace
            | ExpressionKind::DagRangeAll
            | ExpressionKind::RangeAll => node.kind,
            ExpressionKind::Unary(op, arg) => {
                let arg = Box::new(normalize_tree(*arg));
                ExpressionKind::Unary(op, arg)
            }
            ExpressionKind::Binary(op, lhs, rhs) => {
                let lhs = Box::new(normalize_tree(*lhs));
                let rhs = Box::new(normalize_tree(*rhs));
                ExpressionKind::Binary(op, lhs, rhs)
            }
            ExpressionKind::FunctionCall(function) => {
                let function = Box::new(normalize_function_call(*function));
                ExpressionKind::FunctionCall(function)
            }
            ExpressionKind::Modifier(modifier) => {
                let modifier = Box::new(ModifierNode {
                    name: modifier.name,
                    name_span: empty_span(),
                    body: normalize_tree(modifier.body),
                });
                ExpressionKind::Modifier(modifier)
            }
            ExpressionKind::AliasExpanded(_, subst) => normalize_tree(*subst).kind,
        };
        ExpressionNode {
            kind: normalized_kind,
            span: empty_span(),
        }
    }

    #[test]
    fn test_parse_tree_eq() {
        assert_eq!(
            parse_normalized(r#" foo( x ) | ~bar:"baz" "#),
            parse_normalized(r#"(foo(x))|(~(bar:"baz"))"#)
        );
        assert_ne!(parse_normalized(r#" foo "#), parse_normalized(r#" "foo" "#));
    }

    #[test]
    fn test_parse_revset() {
        // Parse "@" (the current working copy)
        assert_eq!(parse_into_kind("@"), Ok(ExpressionKind::AtCurrentWorkspace));
        assert_eq!(
            parse_into_kind("main@"),
            Ok(ExpressionKind::AtWorkspace("main".to_owned()))
        );
        assert_eq!(
            parse_into_kind("main@origin"),
            Ok(ExpressionKind::RemoteSymbol {
                name: "main".to_owned(),
                remote: "origin".to_owned()
            })
        );
        // Quoted component in @ expression
        assert_eq!(
            parse_into_kind(r#""foo bar"@"#),
            Ok(ExpressionKind::AtWorkspace("foo bar".to_owned()))
        );
        assert_eq!(
            parse_into_kind(r#""foo bar"@origin"#),
            Ok(ExpressionKind::RemoteSymbol {
                name: "foo bar".to_owned(),
                remote: "origin".to_owned()
            })
        );
        assert_eq!(
            parse_into_kind(r#"main@"foo bar""#),
            Ok(ExpressionKind::RemoteSymbol {
                name: "main".to_owned(),
                remote: "foo bar".to_owned()
            })
        );
        assert_eq!(
            parse_into_kind(r#"'foo bar'@'bar baz'"#),
            Ok(ExpressionKind::RemoteSymbol {
                name: "foo bar".to_owned(),
                remote: "bar baz".to_owned()
            })
        );
        // Quoted "@" is not interpreted as a working copy or remote symbol
        assert_eq!(
            parse_into_kind(r#""@""#),
            Ok(ExpressionKind::String("@".to_owned()))
        );
        assert_eq!(
            parse_into_kind(r#""main@""#),
            Ok(ExpressionKind::String("main@".to_owned()))
        );
        assert_eq!(
            parse_into_kind(r#""main@origin""#),
            Ok(ExpressionKind::String("main@origin".to_owned()))
        );
        // Internal '.', '-', and '+' are allowed
        assert_eq!(
            parse_into_kind("foo.bar-v1+7"),
            Ok(ExpressionKind::Identifier("foo.bar-v1+7"))
        );
        assert_eq!(
            parse_normalized("foo.bar-v1+7-"),
            parse_normalized("(foo.bar-v1+7)-")
        );
        // '.' is not allowed at the beginning or end
        assert_eq!(
            parse_into_kind(".foo"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("foo."),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        // Multiple '.', '-', '+' are not allowed
        assert_eq!(
            parse_into_kind("foo.+bar"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("foo--bar"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("foo+-bar"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        // Parse a parenthesized symbol
        assert_eq!(parse_normalized("(foo)"), parse_normalized("foo"));
        // Parse a quoted symbol
        assert_eq!(
            parse_into_kind("\"foo\""),
            Ok(ExpressionKind::String("foo".to_owned()))
        );
        assert_eq!(
            parse_into_kind("'foo'"),
            Ok(ExpressionKind::String("foo".to_owned()))
        );
        // Parse the "parents" operator
        assert_matches!(
            parse_into_kind("foo-"),
            Ok(ExpressionKind::Unary(UnaryOp::Parents, _))
        );
        // Parse the "children" operator
        assert_matches!(
            parse_into_kind("foo+"),
            Ok(ExpressionKind::Unary(UnaryOp::Children, _))
        );
        // Parse the "ancestors" operator
        assert_matches!(
            parse_into_kind("::foo"),
            Ok(ExpressionKind::Unary(UnaryOp::DagRangePre, _))
        );
        // Parse the "descendants" operator
        assert_matches!(
            parse_into_kind("foo::"),
            Ok(ExpressionKind::Unary(UnaryOp::DagRangePost, _))
        );
        // Parse the "dag range" operator
        assert_matches!(
            parse_into_kind("foo::bar"),
            Ok(ExpressionKind::Binary(BinaryOp::DagRange, _, _))
        );
        // Parse the nullary "dag range" operator
        assert_matches!(parse_into_kind("::"), Ok(ExpressionKind::DagRangeAll));
        // Parse the "range" prefix operator
        assert_matches!(
            parse_into_kind("..foo"),
            Ok(ExpressionKind::Unary(UnaryOp::RangePre, _))
        );
        assert_matches!(
            parse_into_kind("foo.."),
            Ok(ExpressionKind::Unary(UnaryOp::RangePost, _))
        );
        assert_matches!(
            parse_into_kind("foo..bar"),
            Ok(ExpressionKind::Binary(BinaryOp::Range, _, _))
        );
        // Parse the nullary "range" operator
        assert_matches!(parse_into_kind(".."), Ok(ExpressionKind::RangeAll));
        // Parse the "negate" operator
        assert_matches!(
            parse_into_kind("~ foo"),
            Ok(ExpressionKind::Unary(UnaryOp::Negate, _))
        );
        assert_eq!(
            parse_normalized("~ ~~ foo"),
            parse_normalized("~(~(~(foo)))"),
        );
        // Parse the "intersection" operator
        assert_matches!(
            parse_into_kind("foo & bar"),
            Ok(ExpressionKind::Binary(BinaryOp::Intersection, _, _))
        );
        // Parse the "union" operator
        assert_matches!(
            parse_into_kind("foo | bar"),
            Ok(ExpressionKind::Binary(BinaryOp::Union, _, _))
        );
        // Parse the "difference" operator
        assert_matches!(
            parse_into_kind("foo ~ bar"),
            Ok(ExpressionKind::Binary(BinaryOp::Difference, _, _))
        );
        // Parentheses are allowed before suffix operators
        assert_eq!(parse_normalized("(foo)-"), parse_normalized("foo-"));
        // Space is allowed around expressions
        assert_eq!(parse_normalized(" ::foo "), parse_normalized("::foo"));
        assert_eq!(parse_normalized("( ::foo )"), parse_normalized("::foo"));
        // Space is not allowed around prefix operators
        assert_eq!(
            parse_into_kind(" :: foo "),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        // Incomplete parse
        assert_eq!(
            parse_into_kind("foo | -"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        // Space is allowed around infix operators and function arguments
        assert_eq!(
            parse_normalized(
                "   description(  arg1 ) ~    file(  arg1 ,   arg2 )  ~ visible_heads(  )  ",
            ),
            parse_normalized("(description(arg1) ~ file(arg1, arg2)) ~ visible_heads()"),
        );
        // Space is allowed around keyword arguments
        assert_eq!(
            parse_normalized("remote_branches( remote  =   foo  )"),
            parse_normalized("remote_branches(remote=foo)"),
        );

        // Trailing comma isn't allowed for empty argument
        assert!(parse_into_kind("branches(,)").is_err());
        // Trailing comma is allowed for the last argument
        assert_eq!(
            parse_normalized("branches(a,)"),
            parse_normalized("branches(a)")
        );
        assert_eq!(
            parse_normalized("branches(a ,  )"),
            parse_normalized("branches(a)")
        );
        assert!(parse_into_kind("branches(,a)").is_err());
        assert!(parse_into_kind("branches(a,,)").is_err());
        assert!(parse_into_kind("branches(a  , , )").is_err());
        assert_eq!(
            parse_normalized("file(a,b,)"),
            parse_normalized("file(a, b)")
        );
        assert!(parse_into_kind("file(a,,b)").is_err());
        assert_eq!(
            parse_normalized("remote_branches(a,remote=b  , )"),
            parse_normalized("remote_branches(a, remote=b)"),
        );
        assert!(parse_into_kind("remote_branches(a,,remote=b)").is_err());
    }

    #[test]
    fn test_parse_revset_with_modifier() {
        // all: is a program modifier, but all:: isn't
        assert_eq!(
            parse_into_kind("all:"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_matches!(
            parse_into_kind("all:foo"),
            Ok(ExpressionKind::Modifier(modifier)) if modifier.name == "all"
        );
        assert_matches!(
            parse_into_kind("all::"),
            Ok(ExpressionKind::Unary(UnaryOp::DagRangePost, _))
        );
        assert_matches!(
            parse_into_kind("all::foo"),
            Ok(ExpressionKind::Binary(BinaryOp::DagRange, _, _))
        );

        // all::: could be parsed as all:(::), but rejected for simplicity
        assert_eq!(
            parse_into_kind("all:::"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("all:::foo"),
            Err(RevsetParseErrorKind::SyntaxError)
        );

        assert_eq!(parse_normalized("all:(foo)"), parse_normalized("all:foo"));
        assert_eq!(
            parse_normalized("all:all::foo"),
            parse_normalized("all:(all::foo)"),
        );
        assert_eq!(
            parse_normalized("all:all | foo"),
            parse_normalized("all:(all | foo)"),
        );

        assert_eq!(
            parse_normalized("all: ::foo"),
            parse_normalized("all:(::foo)"),
        );
        assert_eq!(parse_normalized(" all: foo"), parse_normalized("all:foo"));
        assert_eq!(
            parse_into_kind("(all:foo)"),
            Ok(ExpressionKind::StringPattern {
                kind: "all",
                value: "foo".to_owned()
            })
        );
        assert_matches!(
            parse_into_kind("all :foo"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_normalized("all:all:all"),
            parse_normalized("all:(all:all)"),
        );
    }

    #[test]
    fn test_parse_whitespace() {
        let ascii_whitespaces: String = ('\x00'..='\x7f')
            .filter(char::is_ascii_whitespace)
            .collect();
        assert_eq!(
            parse_normalized(&format!("{ascii_whitespaces}all()")),
            parse_normalized("all()"),
        );
    }

    #[test]
    fn test_parse_string_literal() {
        // "\<char>" escapes
        assert_eq!(
            parse_into_kind(r#" "\t\r\n\"\\\0" "#),
            Ok(ExpressionKind::String("\t\r\n\"\\\0".to_owned()))
        );

        // Invalid "\<char>" escape
        assert_eq!(
            parse_into_kind(r#" "\y" "#),
            Err(RevsetParseErrorKind::SyntaxError)
        );

        // Single-quoted raw string
        assert_eq!(
            parse_into_kind(r#" '' "#),
            Ok(ExpressionKind::String("".to_owned()))
        );
        assert_eq!(
            parse_into_kind(r#" 'a\n' "#),
            Ok(ExpressionKind::String(r"a\n".to_owned()))
        );
        assert_eq!(
            parse_into_kind(r#" '\' "#),
            Ok(ExpressionKind::String(r"\".to_owned()))
        );
        assert_eq!(
            parse_into_kind(r#" '"' "#),
            Ok(ExpressionKind::String(r#"""#.to_owned()))
        );
    }

    #[test]
    fn test_parse_string_pattern() {
        assert_eq!(
            parse_into_kind(r#"(substring:"foo")"#),
            Ok(ExpressionKind::StringPattern {
                kind: "substring",
                value: "foo".to_owned()
            })
        );
        assert_eq!(
            parse_into_kind(r#"("exact:foo")"#),
            Ok(ExpressionKind::String("exact:foo".to_owned()))
        );
        assert_eq!(
            parse_normalized(r#"(exact:"foo" )"#),
            parse_normalized(r#"(exact:"foo")"#),
        );
        assert_eq!(
            parse_into_kind(r#"(exact:'\')"#),
            Ok(ExpressionKind::StringPattern {
                kind: "exact",
                value: r"\".to_owned()
            })
        );
        assert_matches!(
            parse_into_kind(r#"(exact:("foo" ))"#),
            Err(RevsetParseErrorKind::NotInfixOperator { .. })
        );
    }

    #[test]
    fn test_parse_revset_alias_symbol_decl() {
        let mut aliases_map = RevsetAliasesMap::new();
        // Working copy or remote symbol cannot be used as an alias name.
        assert!(aliases_map.insert("@", "none()").is_err());
        assert!(aliases_map.insert("a@", "none()").is_err());
        assert!(aliases_map.insert("a@b", "none()").is_err());
    }

    #[test]
    fn test_parse_revset_alias_func_decl() {
        let mut aliases_map = RevsetAliasesMap::new();
        aliases_map.insert("func()", r#""is function 0""#).unwrap();
        aliases_map
            .insert("func(a, b)", r#""is function 2""#)
            .unwrap();
        aliases_map.insert("func(a)", r#""is function a""#).unwrap();
        aliases_map.insert("func(b)", r#""is function b""#).unwrap();

        let (id, params, defn) = aliases_map.get_function("func", 0).unwrap();
        assert_eq!(id, AliasId::Function("func", &[]));
        assert!(params.is_empty());
        assert_eq!(defn, r#""is function 0""#);

        let (id, params, defn) = aliases_map.get_function("func", 1).unwrap();
        assert_eq!(id, AliasId::Function("func", &["b".to_owned()]));
        assert_eq!(params, ["b"]);
        assert_eq!(defn, r#""is function b""#);

        let (id, params, defn) = aliases_map.get_function("func", 2).unwrap();
        assert_eq!(
            id,
            AliasId::Function("func", &["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(params, ["a", "b"]);
        assert_eq!(defn, r#""is function 2""#);

        assert!(aliases_map.get_function("func", 3).is_none());
    }

    #[test]
    fn test_parse_revset_alias_formal_parameter() {
        let mut aliases_map = RevsetAliasesMap::new();
        // Working copy or remote symbol cannot be used as an parameter name.
        assert!(aliases_map.insert("f(@)", "none()").is_err());
        assert!(aliases_map.insert("f(a@)", "none()").is_err());
        assert!(aliases_map.insert("f(a@b)", "none()").is_err());
        // Trailing comma isn't allowed for empty parameter
        assert!(aliases_map.insert("f(,)", "none()").is_err());
        // Trailing comma is allowed for the last parameter
        assert!(aliases_map.insert("g(a,)", "none()").is_ok());
        assert!(aliases_map.insert("h(a ,  )", "none()").is_ok());
        assert!(aliases_map.insert("i(,a)", "none()").is_err());
        assert!(aliases_map.insert("j(a,,)", "none()").is_err());
        assert!(aliases_map.insert("k(a  , , )", "none()").is_err());
        assert!(aliases_map.insert("l(a,b,)", "none()").is_ok());
        assert!(aliases_map.insert("m(a,,b)", "none()").is_err());
    }

    #[test]
    fn test_parse_revset_compat_operator() {
        assert_eq!(
            parse_into_kind(":foo"),
            Err(RevsetParseErrorKind::NotPrefixOperator {
                op: ":".to_owned(),
                similar_op: "::".to_owned(),
                description: "ancestors".to_owned(),
            })
        );
        assert_eq!(
            parse_into_kind("foo^"),
            Err(RevsetParseErrorKind::NotPostfixOperator {
                op: "^".to_owned(),
                similar_op: "-".to_owned(),
                description: "parents".to_owned(),
            })
        );
        assert_eq!(
            parse_into_kind("foo + bar"),
            Err(RevsetParseErrorKind::NotInfixOperator {
                op: "+".to_owned(),
                similar_op: "|".to_owned(),
                description: "union".to_owned(),
            })
        );
        assert_eq!(
            parse_into_kind("foo - bar"),
            Err(RevsetParseErrorKind::NotInfixOperator {
                op: "-".to_owned(),
                similar_op: "~".to_owned(),
                description: "difference".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_revset_operator_combinations() {
        // Parse repeated "parents" operator
        assert_eq!(parse_normalized("foo---"), parse_normalized("((foo-)-)-"));
        // Parse repeated "children" operator
        assert_eq!(parse_normalized("foo+++"), parse_normalized("((foo+)+)+"));
        // Set operator associativity/precedence
        assert_eq!(parse_normalized("~x|y"), parse_normalized("(~x)|y"));
        assert_eq!(parse_normalized("x&~y"), parse_normalized("x&(~y)"));
        assert_eq!(parse_normalized("x~~y"), parse_normalized("x~(~y)"));
        assert_eq!(parse_normalized("x~~~y"), parse_normalized("x~(~(~y))"));
        assert_eq!(parse_normalized("~x::y"), parse_normalized("~(x::y)"));
        assert_eq!(parse_normalized("x|y|z"), parse_normalized("(x|y)|z"));
        assert_eq!(parse_normalized("x&y|z"), parse_normalized("(x&y)|z"));
        assert_eq!(parse_normalized("x|y&z"), parse_normalized("x|(y&z)"));
        assert_eq!(parse_normalized("x|y~z"), parse_normalized("x|(y~z)"));
        assert_eq!(parse_normalized("::&.."), parse_normalized("(::)&(..)"));
        // Parse repeated "ancestors"/"descendants"/"dag range"/"range" operators
        assert_eq!(
            parse_into_kind("::foo::"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind(":::foo"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("::::foo"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("foo:::"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("foo::::"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("foo:::bar"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("foo::::bar"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("::foo::bar"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("foo::bar::"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("::::"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("....foo"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("foo...."),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("foo.....bar"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("..foo..bar"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("foo..bar.."),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("...."),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("::.."),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        // Parse combinations of "parents"/"children" operators and the range operators.
        // The former bind more strongly.
        assert_eq!(parse_normalized("foo-+"), parse_normalized("(foo-)+"));
        assert_eq!(parse_normalized("foo-::"), parse_normalized("(foo-)::"));
        assert_eq!(parse_normalized("::foo+"), parse_normalized("::(foo+)"));
        assert_eq!(
            parse_into_kind("::-"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind("..+"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
    }

    #[test]
    fn test_parse_revset_function() {
        assert_matches!(
            parse_into_kind("parents(foo)"),
            Ok(ExpressionKind::FunctionCall(_))
        );
        assert_eq!(
            parse_normalized("parents((foo))"),
            parse_normalized("parents(foo)"),
        );
        assert_eq!(
            parse_into_kind("parents(foo"),
            Err(RevsetParseErrorKind::SyntaxError)
        );
    }

    #[test]
    fn test_expand_symbol_alias() {
        assert_eq!(
            with_aliases([("AB", "a|b")]).parse_normalized("AB|c"),
            parse_normalized("(a|b)|c")
        );
        assert_eq!(
            with_aliases([("AB", "a|b")]).parse_normalized("AB::heads(AB)"),
            parse_normalized("(a|b)::heads(a|b)")
        );

        // Not string substitution 'a&b|c', but tree substitution.
        assert_eq!(
            with_aliases([("BC", "b|c")]).parse_normalized("a&BC"),
            parse_normalized("a&(b|c)")
        );

        // String literal should not be substituted with alias.
        assert_eq!(
            with_aliases([("A", "a")]).parse_normalized(r#"A|"A"|'A'"#),
            parse_normalized("a|'A'|'A'")
        );

        // Part of string pattern cannot be substituted.
        assert_eq!(
            with_aliases([("A", "a")]).parse_normalized("author(exact:A)"),
            parse_normalized("author(exact:A)")
        );

        // Part of @ symbol cannot be substituted.
        assert_eq!(
            with_aliases([("A", "a")]).parse_normalized("A@"),
            parse_normalized("A@")
        );
        assert_eq!(
            with_aliases([("A", "a")]).parse_normalized("A@b"),
            parse_normalized("A@b")
        );
        assert_eq!(
            with_aliases([("B", "b")]).parse_normalized("a@B"),
            parse_normalized("a@B")
        );

        // Modifier cannot be substituted.
        assert_eq!(
            with_aliases([("all", "ALL")]).parse_normalized("all:all"),
            parse_normalized("all:ALL")
        );

        // Top-level alias can be substituted to modifier expression.
        assert_eq!(
            with_aliases([("A", "all:a")]).parse_normalized("A"),
            parse_normalized("all:a")
        );

        // Multi-level substitution.
        assert_eq!(
            with_aliases([("A", "BC"), ("BC", "b|C"), ("C", "c")]).parse_normalized("A"),
            parse_normalized("b|c")
        );

        // Infinite recursion, where the top-level error isn't of RecursiveAlias kind.
        assert_eq!(
            with_aliases([("A", "A")]).parse("A").unwrap_err().kind,
            RevsetParseErrorKind::BadAliasExpansion("A".to_owned())
        );
        assert_eq!(
            with_aliases([("A", "B"), ("B", "b|C"), ("C", "c|B")])
                .parse("A")
                .unwrap_err()
                .kind,
            RevsetParseErrorKind::BadAliasExpansion("A".to_owned())
        );

        // Error in alias definition.
        assert_eq!(
            with_aliases([("A", "a(")]).parse("A").unwrap_err().kind,
            RevsetParseErrorKind::BadAliasExpansion("A".to_owned())
        );
    }

    #[test]
    fn test_expand_function_alias() {
        assert_eq!(
            with_aliases([("F(  )", "a")]).parse_normalized("F()"),
            parse_normalized("a")
        );
        assert_eq!(
            with_aliases([("F( x  )", "x")]).parse_normalized("F(a)"),
            parse_normalized("a")
        );
        assert_eq!(
            with_aliases([("F( x,  y )", "x|y")]).parse_normalized("F(a, b)"),
            parse_normalized("a|b")
        );

        // Not recursion because functions are overloaded by arity.
        assert_eq!(
            with_aliases([("F(x)", "F(x,b)"), ("F(x,y)", "x|y")]).parse_normalized("F(a)"),
            parse_normalized("a|b")
        );

        // Arguments should be resolved in the current scope.
        assert_eq!(
            with_aliases([("F(x,y)", "x|y")]).parse_normalized("F(a::y,b::x)"),
            parse_normalized("(a::y)|(b::x)")
        );
        // F(a) -> G(a)&y -> (x|a)&y
        assert_eq!(
            with_aliases([("F(x)", "G(x)&y"), ("G(y)", "x|y")]).parse_normalized("F(a)"),
            parse_normalized("(x|a)&y")
        );
        // F(G(a)) -> F(x|a) -> G(x|a)&y -> (x|(x|a))&y
        assert_eq!(
            with_aliases([("F(x)", "G(x)&y"), ("G(y)", "x|y")]).parse_normalized("F(G(a))"),
            parse_normalized("(x|(x|a))&y")
        );

        // Function parameter should precede the symbol alias.
        assert_eq!(
            with_aliases([("F(X)", "X"), ("X", "x")]).parse_normalized("F(a)|X"),
            parse_normalized("a|x")
        );

        // Function parameter shouldn't be expanded in symbol alias.
        assert_eq!(
            with_aliases([("F(x)", "x|A"), ("A", "x")]).parse_normalized("F(a)"),
            parse_normalized("a|x")
        );

        // String literal should not be substituted with function parameter.
        assert_eq!(
            with_aliases([("F(x)", r#"x|"x""#)]).parse_normalized("F(a)"),
            parse_normalized("a|'x'")
        );

        // Modifier expression body as parameter.
        assert_eq!(
            with_aliases([("F(x)", "all:x")]).parse_normalized("F(a|b)"),
            parse_normalized("all:(a|b)")
        );

        // Function and symbol aliases reside in separate namespaces.
        assert_eq!(
            with_aliases([("A()", "A"), ("A", "a")]).parse_normalized("A()"),
            parse_normalized("a")
        );

        // Invalid number of arguments.
        assert_eq!(
            with_aliases([("F()", "x")]).parse("F(a)").unwrap_err().kind,
            RevsetParseErrorKind::InvalidFunctionArguments {
                name: "F".to_owned(),
                message: "Expected 0 arguments".to_owned()
            }
        );
        assert_eq!(
            with_aliases([("F(x)", "x")]).parse("F()").unwrap_err().kind,
            RevsetParseErrorKind::InvalidFunctionArguments {
                name: "F".to_owned(),
                message: "Expected 1 arguments".to_owned()
            }
        );
        assert_eq!(
            with_aliases([("F(x,y)", "x|y")])
                .parse("F(a,b,c)")
                .unwrap_err()
                .kind,
            RevsetParseErrorKind::InvalidFunctionArguments {
                name: "F".to_owned(),
                message: "Expected 2 arguments".to_owned()
            }
        );
        assert_eq!(
            with_aliases([("F(x)", "x"), ("F(x,y)", "x|y")])
                .parse("F()")
                .unwrap_err()
                .kind,
            RevsetParseErrorKind::InvalidFunctionArguments {
                name: "F".to_owned(),
                message: "Expected 1 to 2 arguments".to_owned()
            }
        );
        assert_eq!(
            with_aliases([("F()", "x"), ("F(x,y)", "x|y")])
                .parse("F(a)")
                .unwrap_err()
                .kind,
            RevsetParseErrorKind::InvalidFunctionArguments {
                name: "F".to_owned(),
                message: "Expected 0, 2 arguments".to_owned()
            }
        );

        // Keyword argument isn't supported for now.
        assert_eq!(
            with_aliases([("F(x)", "x")])
                .parse("F(x=y)")
                .unwrap_err()
                .kind,
            RevsetParseErrorKind::InvalidFunctionArguments {
                name: "F".to_owned(),
                message: "Unexpected keyword arguments".to_owned()
            }
        );

        // Infinite recursion, where the top-level error isn't of RecursiveAlias kind.
        assert_eq!(
            with_aliases([("F(x)", "G(x)"), ("G(x)", "H(x)"), ("H(x)", "F(x)")])
                .parse("F(a)")
                .unwrap_err()
                .kind,
            RevsetParseErrorKind::BadAliasExpansion("F(x)".to_owned())
        );
        assert_eq!(
            with_aliases([("F(x)", "F(x,b)"), ("F(x,y)", "F(x|y)")])
                .parse("F(a)")
                .unwrap_err()
                .kind,
            RevsetParseErrorKind::BadAliasExpansion("F(x)".to_owned())
        );
    }
}
