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

use std::collections::HashMap;
use std::num::ParseIntError;
use std::ops::{RangeFrom, RangeInclusive};
use std::{error, fmt};

use itertools::Itertools as _;
use jujutsu_lib::backend::{Signature, Timestamp};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::op_store::WorkspaceId;
use jujutsu_lib::repo::Repo;
use jujutsu_lib::rewrite;
use pest::iterators::{Pair, Pairs};
use pest::Parser;
use pest_derive::Parser;
use thiserror::Error;

use crate::templater::{
    BranchProperty, CommitOrChangeId, ConditionalTemplate, FormattablePropertyTemplate,
    GitHeadProperty, GitRefsProperty, LabelTemplate, ListTemplate, Literal,
    PlainTextFormattedProperty, SeparateTemplate, ShortestIdPrefix, TagProperty, Template,
    TemplateFunction, TemplateProperty, TemplatePropertyFn, WorkingCopiesProperty,
};
use crate::{cli_util, time_util};

#[derive(Parser)]
#[grammar = "template.pest"]
struct TemplateParser;

type TemplateParseResult<T> = Result<T, TemplateParseError>;

#[derive(Clone, Debug)]
pub struct TemplateParseError {
    kind: TemplateParseErrorKind,
    pest_error: Box<pest::error::Error<Rule>>,
    origin: Option<Box<TemplateParseError>>,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum TemplateParseErrorKind {
    #[error("Syntax error")]
    SyntaxError,
    #[error("Invalid integer literal: {0}")]
    ParseIntError(#[source] ParseIntError),
    #[error(r#"Keyword "{0}" doesn't exist"#)]
    NoSuchKeyword(String),
    #[error(r#"Function "{0}" doesn't exist"#)]
    NoSuchFunction(String),
    #[error(r#"Method "{name}" doesn't exist for type "{type_name}""#)]
    NoSuchMethod { type_name: String, name: String },
    // TODO: clean up argument error variants
    #[error("Expected {0} arguments")]
    InvalidArgumentCountExact(usize),
    #[error("Expected {} to {} arguments", .0.start(), .0.end())]
    InvalidArgumentCountRange(RangeInclusive<usize>),
    #[error("Expected at least {} arguments", .0.start)]
    InvalidArgumentCountRangeFrom(RangeFrom<usize>),
    #[error(r#"Expected argument of type "{0}""#)]
    InvalidArgumentType(String),
    #[error("Redefinition of function parameter")]
    RedefinedFunctionParameter,
    #[error(r#"Alias "{0}" cannot be expanded"#)]
    BadAliasExpansion(String),
    #[error(r#"Alias "{0}" expanded recursively"#)]
    RecursiveAlias(String),
}

impl TemplateParseError {
    fn with_span(kind: TemplateParseErrorKind, span: pest::Span<'_>) -> Self {
        let pest_error = Box::new(pest::error::Error::new_from_span(
            pest::error::ErrorVariant::CustomError {
                message: kind.to_string(),
            },
            span,
        ));
        TemplateParseError {
            kind,
            pest_error,
            origin: None,
        }
    }

    fn with_span_and_origin(
        kind: TemplateParseErrorKind,
        span: pest::Span<'_>,
        origin: Self,
    ) -> Self {
        let pest_error = Box::new(pest::error::Error::new_from_span(
            pest::error::ErrorVariant::CustomError {
                message: kind.to_string(),
            },
            span,
        ));
        TemplateParseError {
            kind,
            pest_error,
            origin: Some(Box::new(origin)),
        }
    }

    fn no_such_keyword(name: impl Into<String>, span: pest::Span<'_>) -> Self {
        TemplateParseError::with_span(TemplateParseErrorKind::NoSuchKeyword(name.into()), span)
    }

    fn no_such_function(function: &FunctionCallNode) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::NoSuchFunction(function.name.to_owned()),
            function.name_span,
        )
    }

    fn no_such_method(type_name: impl Into<String>, function: &FunctionCallNode) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::NoSuchMethod {
                type_name: type_name.into(),
                name: function.name.to_owned(),
            },
            function.name_span,
        )
    }

    fn invalid_argument_count_exact(count: usize, span: pest::Span<'_>) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::InvalidArgumentCountExact(count),
            span,
        )
    }

    fn invalid_argument_count_range(count: RangeInclusive<usize>, span: pest::Span<'_>) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::InvalidArgumentCountRange(count),
            span,
        )
    }

    fn invalid_argument_count_range_from(count: RangeFrom<usize>, span: pest::Span<'_>) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::InvalidArgumentCountRangeFrom(count),
            span,
        )
    }

    fn invalid_argument_type(expected_type_name: impl Into<String>, span: pest::Span<'_>) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::InvalidArgumentType(expected_type_name.into()),
            span,
        )
    }

    fn within_alias_expansion(self, id: TemplateAliasId<'_>, span: pest::Span<'_>) -> Self {
        TemplateParseError::with_span_and_origin(
            TemplateParseErrorKind::BadAliasExpansion(id.to_string()),
            span,
            self,
        )
    }

    /// Original parsing error which typically occurred in an alias expression.
    pub fn origin(&self) -> Option<&Self> {
        self.origin.as_deref()
    }
}

impl From<pest::error::Error<Rule>> for TemplateParseError {
    fn from(err: pest::error::Error<Rule>) -> Self {
        TemplateParseError {
            kind: TemplateParseErrorKind::SyntaxError,
            pest_error: Box::new(err),
            origin: None,
        }
    }
}

impl fmt::Display for TemplateParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.pest_error.fmt(f)
    }
}

impl error::Error for TemplateParseError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        if let Some(e) = self.origin() {
            return Some(e as &dyn error::Error);
        }
        match &self.kind {
            // SyntaxError is a wrapper for pest::error::Error.
            TemplateParseErrorKind::SyntaxError => Some(&self.pest_error as &dyn error::Error),
            // Otherwise the kind represents this error.
            e => e.source(),
        }
    }
}

/// AST node without type or name checking.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpressionNode<'i> {
    kind: ExpressionKind<'i>,
    span: pest::Span<'i>,
}

impl<'i> ExpressionNode<'i> {
    fn new(kind: ExpressionKind<'i>, span: pest::Span<'i>) -> Self {
        ExpressionNode { kind, span }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum ExpressionKind<'i> {
    Identifier(&'i str),
    Integer(i64),
    String(String),
    List(Vec<ExpressionNode<'i>>),
    FunctionCall(FunctionCallNode<'i>),
    MethodCall(MethodCallNode<'i>),
    /// Identity node to preserve the span in the source template text.
    AliasExpanded(TemplateAliasId<'i>, Box<ExpressionNode<'i>>),
}

#[derive(Clone, Debug, PartialEq)]
struct FunctionCallNode<'i> {
    name: &'i str,
    name_span: pest::Span<'i>,
    args: Vec<ExpressionNode<'i>>,
    args_span: pest::Span<'i>,
}

#[derive(Clone, Debug, PartialEq)]
struct MethodCallNode<'i> {
    object: Box<ExpressionNode<'i>>,
    function: FunctionCallNode<'i>,
}

fn parse_string_literal(pair: Pair<Rule>) -> String {
    assert_eq!(pair.as_rule(), Rule::literal);
    let mut result = String::new();
    for part in pair.into_inner() {
        match part.as_rule() {
            Rule::raw_literal => {
                result.push_str(part.as_str());
            }
            Rule::escape => match part.as_str().as_bytes()[1] as char {
                '"' => result.push('"'),
                '\\' => result.push('\\'),
                'n' => result.push('\n'),
                char => panic!("invalid escape: \\{char:?}"),
            },
            _ => panic!("unexpected part of string: {part:?}"),
        }
    }
    result
}

fn parse_function_call_node(pair: Pair<Rule>) -> TemplateParseResult<FunctionCallNode> {
    assert_eq!(pair.as_rule(), Rule::function);
    let mut inner = pair.into_inner();
    let name = inner.next().unwrap();
    let args_pair = inner.next().unwrap();
    let args_span = args_pair.as_span();
    assert_eq!(name.as_rule(), Rule::identifier);
    assert_eq!(args_pair.as_rule(), Rule::function_arguments);
    let args = args_pair
        .into_inner()
        .map(parse_template_node)
        .try_collect()?;
    Ok(FunctionCallNode {
        name: name.as_str(),
        name_span: name.as_span(),
        args,
        args_span,
    })
}

fn parse_term_node(pair: Pair<Rule>) -> TemplateParseResult<ExpressionNode> {
    assert_eq!(pair.as_rule(), Rule::term);
    let mut inner = pair.into_inner();
    let expr = inner.next().unwrap();
    let span = expr.as_span();
    let primary = match expr.as_rule() {
        Rule::literal => {
            let text = parse_string_literal(expr);
            ExpressionNode::new(ExpressionKind::String(text), span)
        }
        Rule::integer_literal => {
            let value = expr.as_str().parse().map_err(|err| {
                TemplateParseError::with_span(TemplateParseErrorKind::ParseIntError(err), span)
            })?;
            ExpressionNode::new(ExpressionKind::Integer(value), span)
        }
        Rule::identifier => ExpressionNode::new(ExpressionKind::Identifier(expr.as_str()), span),
        Rule::function => {
            let function = parse_function_call_node(expr)?;
            ExpressionNode::new(ExpressionKind::FunctionCall(function), span)
        }
        Rule::template => parse_template_node(expr)?,
        other => panic!("unexpected term: {other:?}"),
    };
    inner.try_fold(primary, |object, chain| {
        assert_eq!(chain.as_rule(), Rule::function);
        let span = chain.as_span();
        let method = MethodCallNode {
            object: Box::new(object),
            function: parse_function_call_node(chain)?,
        };
        Ok(ExpressionNode::new(
            ExpressionKind::MethodCall(method),
            span,
        ))
    })
}

fn parse_template_node(pair: Pair<Rule>) -> TemplateParseResult<ExpressionNode> {
    assert_eq!(pair.as_rule(), Rule::template);
    let span = pair.as_span();
    let inner = pair.into_inner();
    let mut nodes: Vec<_> = inner.map(parse_term_node).try_collect()?;
    if nodes.len() == 1 {
        Ok(nodes.pop().unwrap())
    } else {
        Ok(ExpressionNode::new(ExpressionKind::List(nodes), span))
    }
}

/// Parses text into AST nodes. No type/name checking is made at this stage.
pub fn parse_template(template_text: &str) -> TemplateParseResult<ExpressionNode> {
    let mut pairs: Pairs<Rule> = TemplateParser::parse(Rule::program, template_text)?;
    let first_pair = pairs.next().unwrap();
    if first_pair.as_rule() == Rule::EOI {
        let span = first_pair.as_span();
        Ok(ExpressionNode::new(ExpressionKind::List(Vec::new()), span))
    } else {
        parse_template_node(first_pair)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TemplateAliasesMap {
    symbol_aliases: HashMap<String, String>,
    function_aliases: HashMap<String, (Vec<String>, String)>,
}

impl TemplateAliasesMap {
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
    ) -> TemplateParseResult<()> {
        match TemplateAliasDeclaration::parse(decl.as_ref())? {
            TemplateAliasDeclaration::Symbol(name) => {
                self.symbol_aliases.insert(name, defn.into());
            }
            TemplateAliasDeclaration::Function(name, params) => {
                self.function_aliases.insert(name, (params, defn.into()));
            }
        }
        Ok(())
    }

    fn get_symbol(&self, name: &str) -> Option<(TemplateAliasId<'_>, &str)> {
        self.symbol_aliases
            .get_key_value(name)
            .map(|(name, defn)| (TemplateAliasId::Symbol(name), defn.as_ref()))
    }

    fn get_function(&self, name: &str) -> Option<(TemplateAliasId<'_>, &[String], &str)> {
        self.function_aliases
            .get_key_value(name)
            .map(|(name, (params, defn))| {
                (
                    TemplateAliasId::Function(name),
                    params.as_ref(),
                    defn.as_ref(),
                )
            })
    }
}

/// Parsed declaration part of alias rule.
#[derive(Clone, Debug)]
enum TemplateAliasDeclaration {
    Symbol(String),
    Function(String, Vec<String>),
}

impl TemplateAliasDeclaration {
    fn parse(source: &str) -> TemplateParseResult<Self> {
        let mut pairs = TemplateParser::parse(Rule::alias_declaration, source)?;
        let first = pairs.next().unwrap();
        match first.as_rule() {
            Rule::identifier => Ok(TemplateAliasDeclaration::Symbol(first.as_str().to_owned())),
            Rule::function_alias_declaration => {
                let mut inner = first.into_inner();
                let name_pair = inner.next().unwrap();
                let params_pair = inner.next().unwrap();
                let params_span = params_pair.as_span();
                assert_eq!(name_pair.as_rule(), Rule::identifier);
                assert_eq!(params_pair.as_rule(), Rule::formal_parameters);
                let name = name_pair.as_str().to_owned();
                let params = params_pair
                    .into_inner()
                    .map(|pair| match pair.as_rule() {
                        Rule::identifier => pair.as_str().to_owned(),
                        r => panic!("unexpected formal parameter rule {r:?}"),
                    })
                    .collect_vec();
                if params.iter().all_unique() {
                    Ok(TemplateAliasDeclaration::Function(name, params))
                } else {
                    Err(TemplateParseError::with_span(
                        TemplateParseErrorKind::RedefinedFunctionParameter,
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
enum TemplateAliasId<'a> {
    Symbol(&'a str),
    Function(&'a str),
}

impl fmt::Display for TemplateAliasId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TemplateAliasId::Symbol(name) => write!(f, "{name}"),
            TemplateAliasId::Function(name) => write!(f, "{name}()"),
        }
    }
}

/// Expand aliases recursively.
fn expand_aliases<'i>(
    node: ExpressionNode<'i>,
    aliases_map: &'i TemplateAliasesMap,
) -> TemplateParseResult<ExpressionNode<'i>> {
    #[derive(Clone, Copy, Debug)]
    struct State<'a, 'i> {
        aliases_map: &'i TemplateAliasesMap,
        aliases_expanding: &'a [TemplateAliasId<'a>],
        locals: &'a HashMap<&'a str, ExpressionNode<'i>>,
    }

    fn expand_defn<'i>(
        id: TemplateAliasId<'i>,
        defn: &'i str,
        locals: &HashMap<&str, ExpressionNode<'i>>,
        span: pest::Span<'i>,
        state: State<'_, 'i>,
    ) -> TemplateParseResult<ExpressionNode<'i>> {
        // The stack should be short, so let's simply do linear search and duplicate.
        if state.aliases_expanding.contains(&id) {
            return Err(TemplateParseError::with_span(
                TemplateParseErrorKind::RecursiveAlias(id.to_string()),
                span,
            ));
        }
        let mut aliases_expanding = state.aliases_expanding.to_vec();
        aliases_expanding.push(id);
        let expanding_state = State {
            aliases_map: state.aliases_map,
            aliases_expanding: &aliases_expanding,
            locals,
        };
        // Parsed defn could be cached if needed.
        parse_template(defn)
            .and_then(|node| expand_node(node, expanding_state))
            .map(|node| {
                ExpressionNode::new(ExpressionKind::AliasExpanded(id, Box::new(node)), span)
            })
            .map_err(|e| e.within_alias_expansion(id, span))
    }

    fn expand_list<'i>(
        nodes: Vec<ExpressionNode<'i>>,
        state: State<'_, 'i>,
    ) -> TemplateParseResult<Vec<ExpressionNode<'i>>> {
        nodes
            .into_iter()
            .map(|node| expand_node(node, state))
            .try_collect()
    }

    fn expand_function_call<'i>(
        function: FunctionCallNode<'i>,
        state: State<'_, 'i>,
    ) -> TemplateParseResult<FunctionCallNode<'i>> {
        Ok(FunctionCallNode {
            name: function.name,
            name_span: function.name_span,
            args: expand_list(function.args, state)?,
            args_span: function.args_span,
        })
    }

    fn expand_node<'i>(
        mut node: ExpressionNode<'i>,
        state: State<'_, 'i>,
    ) -> TemplateParseResult<ExpressionNode<'i>> {
        match node.kind {
            ExpressionKind::Identifier(name) => {
                if let Some(node) = state.locals.get(name) {
                    Ok(node.clone())
                } else if let Some((id, defn)) = state.aliases_map.get_symbol(name) {
                    let locals = HashMap::new(); // Don't spill out the current scope
                    expand_defn(id, defn, &locals, node.span, state)
                } else {
                    Ok(node)
                }
            }
            ExpressionKind::Integer(_) => Ok(node),
            ExpressionKind::String(_) => Ok(node),
            ExpressionKind::List(nodes) => {
                node.kind = ExpressionKind::List(expand_list(nodes, state)?);
                Ok(node)
            }
            ExpressionKind::FunctionCall(function) => {
                if let Some((id, params, defn)) = state.aliases_map.get_function(function.name) {
                    if function.args.len() != params.len() {
                        return Err(TemplateParseError::invalid_argument_count_exact(
                            params.len(),
                            function.args_span,
                        ));
                    }
                    // Resolve arguments in the current scope, and pass them in to the alias
                    // expansion scope.
                    let args = expand_list(function.args, state)?;
                    let locals = params.iter().map(|s| s.as_str()).zip(args).collect();
                    expand_defn(id, defn, &locals, node.span, state)
                } else {
                    node.kind =
                        ExpressionKind::FunctionCall(expand_function_call(function, state)?);
                    Ok(node)
                }
            }
            ExpressionKind::MethodCall(method) => {
                node.kind = ExpressionKind::MethodCall(MethodCallNode {
                    object: Box::new(expand_node(*method.object, state)?),
                    function: expand_function_call(method.function, state)?,
                });
                Ok(node)
            }
            ExpressionKind::AliasExpanded(id, subst) => {
                // Just in case the original tree contained AliasExpanded node.
                let subst = Box::new(expand_node(*subst, state)?);
                node.kind = ExpressionKind::AliasExpanded(id, subst);
                Ok(node)
            }
        }
    }

    let state = State {
        aliases_map,
        aliases_expanding: &[],
        locals: &HashMap::new(),
    };
    expand_node(node, state)
}

enum Property<'a, I> {
    String(Box<dyn TemplateProperty<I, Output = String> + 'a>),
    Boolean(Box<dyn TemplateProperty<I, Output = bool> + 'a>),
    Integer(Box<dyn TemplateProperty<I, Output = i64> + 'a>),
    CommitOrChangeId(Box<dyn TemplateProperty<I, Output = CommitOrChangeId<'a>> + 'a>),
    ShortestIdPrefix(Box<dyn TemplateProperty<I, Output = ShortestIdPrefix> + 'a>),
    Signature(Box<dyn TemplateProperty<I, Output = Signature> + 'a>),
    Timestamp(Box<dyn TemplateProperty<I, Output = Timestamp> + 'a>),
}

impl<'a, I: 'a> Property<'a, I> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<I, Output = bool> + 'a>> {
        match self {
            Property::String(property) => {
                Some(Box::new(TemplateFunction::new(property, |s| !s.is_empty())))
            }
            Property::Boolean(property) => Some(property),
            _ => None,
        }
    }

    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<I, Output = i64> + 'a>> {
        match self {
            Property::Integer(property) => Some(property),
            _ => None,
        }
    }

    fn into_plain_text(self) -> Box<dyn TemplateProperty<I, Output = String> + 'a> {
        match self {
            Property::String(property) => property,
            _ => Box::new(PlainTextFormattedProperty::new(self.into_template())),
        }
    }

    fn into_template(self) -> Box<dyn Template<I> + 'a> {
        fn wrap<'a, I: 'a, O: Template<()> + 'a>(
            property: Box<dyn TemplateProperty<I, Output = O> + 'a>,
        ) -> Box<dyn Template<I> + 'a> {
            Box::new(FormattablePropertyTemplate::new(property))
        }
        match self {
            Property::String(property) => wrap(property),
            Property::Boolean(property) => wrap(property),
            Property::Integer(property) => wrap(property),
            Property::CommitOrChangeId(property) => wrap(property),
            Property::ShortestIdPrefix(property) => wrap(property),
            Property::Signature(property) => wrap(property),
            Property::Timestamp(property) => wrap(property),
        }
    }
}

enum Expression<'a, C> {
    Property(Property<'a, C>, Vec<String>),
    Template(Box<dyn Template<C> + 'a>),
}

impl<'a, C: 'a> Expression<'a, C> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<C, Output = bool> + 'a>> {
        match self {
            Expression::Property(property, _) => property.try_into_boolean(),
            Expression::Template(_) => None,
        }
    }

    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<C, Output = i64> + 'a>> {
        match self {
            Expression::Property(property, _) => property.try_into_integer(),
            Expression::Template(_) => None,
        }
    }

    fn into_plain_text(self) -> Box<dyn TemplateProperty<C, Output = String> + 'a> {
        match self {
            Expression::Property(property, _) => property.into_plain_text(),
            Expression::Template(template) => Box::new(PlainTextFormattedProperty::new(template)),
        }
    }

    fn into_template(self) -> Box<dyn Template<C> + 'a> {
        match self {
            Expression::Property(property, labels) => {
                let template = property.into_template();
                if labels.is_empty() {
                    template
                } else {
                    Box::new(LabelTemplate::new(template, Literal(labels)))
                }
            }
            Expression::Template(template) => template,
        }
    }
}

fn expect_no_arguments(function: &FunctionCallNode) -> TemplateParseResult<()> {
    if function.args.is_empty() {
        Ok(())
    } else {
        Err(TemplateParseError::invalid_argument_count_exact(
            0,
            function.args_span,
        ))
    }
}

/// Extracts exactly N required arguments.
fn expect_exact_arguments<'a, 'i, const N: usize>(
    function: &'a FunctionCallNode<'i>,
) -> TemplateParseResult<&'a [ExpressionNode<'i>; N]> {
    function
        .args
        .as_slice()
        .try_into()
        .map_err(|_| TemplateParseError::invalid_argument_count_exact(N, function.args_span))
}

/// Extracts N required arguments and remainders.
fn expect_some_arguments<'a, 'i, const N: usize>(
    function: &'a FunctionCallNode<'i>,
) -> TemplateParseResult<(&'a [ExpressionNode<'i>; N], &'a [ExpressionNode<'i>])> {
    if function.args.len() >= N {
        let (required, rest) = function.args.split_at(N);
        Ok((required.try_into().unwrap(), rest))
    } else {
        Err(TemplateParseError::invalid_argument_count_range_from(
            N..,
            function.args_span,
        ))
    }
}

/// Extracts N required arguments and M optional arguments.
fn expect_arguments<'a, 'i, const N: usize, const M: usize>(
    function: &'a FunctionCallNode<'i>,
) -> TemplateParseResult<(
    &'a [ExpressionNode<'i>; N],
    [Option<&'a ExpressionNode<'i>>; M],
)> {
    let count_range = N..=(N + M);
    if count_range.contains(&function.args.len()) {
        let (required, rest) = function.args.split_at(N);
        let mut optional = rest.iter().map(Some).collect_vec();
        optional.resize(M, None);
        Ok((required.try_into().unwrap(), optional.try_into().unwrap()))
    } else {
        Err(TemplateParseError::invalid_argument_count_range(
            count_range,
            function.args_span,
        ))
    }
}

fn split_email(email: &str) -> (&str, Option<&str>) {
    if let Some((username, rest)) = email.split_once('@') {
        (username, Some(rest))
    } else {
        (email, None)
    }
}

fn build_method_call<'a, I: 'a>(
    method: &MethodCallNode,
    build_keyword: &impl Fn(&str, pest::Span) -> TemplateParseResult<Property<'a, I>>,
) -> TemplateParseResult<Expression<'a, I>> {
    match build_expression(&method.object, build_keyword)? {
        Expression::Property(property, mut labels) => {
            let property = match property {
                Property::String(property) => {
                    build_string_method(property, &method.function, build_keyword)?
                }
                Property::Boolean(property) => {
                    build_boolean_method(property, &method.function, build_keyword)?
                }
                Property::Integer(property) => {
                    build_integer_method(property, &method.function, build_keyword)?
                }
                Property::CommitOrChangeId(property) => {
                    build_commit_or_change_id_method(property, &method.function, build_keyword)?
                }
                Property::ShortestIdPrefix(property) => {
                    build_shortest_id_prefix_method(property, &method.function, build_keyword)?
                }
                Property::Signature(property) => {
                    build_signature_method(property, &method.function, build_keyword)?
                }
                Property::Timestamp(property) => {
                    build_timestamp_method(property, &method.function, build_keyword)?
                }
            };
            labels.push(method.function.name.to_owned());
            Ok(Expression::Property(property, labels))
        }
        Expression::Template(_) => Err(TemplateParseError::no_such_method(
            "Template",
            &method.function,
        )),
    }
}

fn chain_properties<'a, I: 'a, J: 'a, O: 'a>(
    first: impl TemplateProperty<I, Output = J> + 'a,
    second: impl TemplateProperty<J, Output = O> + 'a,
) -> Box<dyn TemplateProperty<I, Output = O> + 'a> {
    Box::new(TemplateFunction::new(first, move |value| {
        second.extract(&value)
    }))
}

fn build_string_method<'a, I: 'a>(
    self_property: impl TemplateProperty<I, Output = String> + 'a,
    function: &FunctionCallNode,
    build_keyword: &impl Fn(&str, pest::Span) -> TemplateParseResult<Property<'a, I>>,
) -> TemplateParseResult<Property<'a, I>> {
    let property = match function.name {
        "contains" => {
            let [needle_node] = expect_exact_arguments(function)?;
            // TODO: or .try_into_string() to disable implicit type cast?
            let needle_property = build_expression(needle_node, build_keyword)?.into_plain_text();
            Property::Boolean(chain_properties(
                (self_property, needle_property),
                TemplatePropertyFn(|(haystack, needle): &(String, String)| {
                    haystack.contains(needle)
                }),
            ))
        }
        "first_line" => {
            expect_no_arguments(function)?;
            Property::String(chain_properties(
                self_property,
                TemplatePropertyFn(|s: &String| s.lines().next().unwrap_or_default().to_string()),
            ))
        }
        _ => return Err(TemplateParseError::no_such_method("String", function)),
    };
    Ok(property)
}

fn build_boolean_method<'a, I: 'a>(
    _self_property: impl TemplateProperty<I, Output = bool> + 'a,
    function: &FunctionCallNode,
    _build_keyword: &impl Fn(&str, pest::Span) -> TemplateParseResult<Property<'a, I>>,
) -> TemplateParseResult<Property<'a, I>> {
    Err(TemplateParseError::no_such_method("Boolean", function))
}

fn build_integer_method<'a, I: 'a>(
    _self_property: impl TemplateProperty<I, Output = i64> + 'a,
    function: &FunctionCallNode,
    _build_keyword: &impl Fn(&str, pest::Span) -> TemplateParseResult<Property<'a, I>>,
) -> TemplateParseResult<Property<'a, I>> {
    Err(TemplateParseError::no_such_method("Integer", function))
}

fn build_commit_or_change_id_method<'a, I: 'a>(
    self_property: impl TemplateProperty<I, Output = CommitOrChangeId<'a>> + 'a,
    function: &FunctionCallNode,
    build_keyword: &impl Fn(&str, pest::Span) -> TemplateParseResult<Property<'a, I>>,
) -> TemplateParseResult<Property<'a, I>> {
    let parse_optional_integer = |function| -> Result<Option<_>, TemplateParseError> {
        let ([], [len_node]) = expect_arguments(function)?;
        len_node
            .map(|node| {
                build_expression(node, build_keyword).and_then(|p| {
                    p.try_into_integer().ok_or_else(|| {
                        TemplateParseError::invalid_argument_type("Integer", node.span)
                    })
                })
            })
            .transpose()
    };
    let property = match function.name {
        "short" => {
            let len_property = parse_optional_integer(function)?;
            Property::String(chain_properties(
                (self_property, len_property),
                TemplatePropertyFn(|(id, len): &(CommitOrChangeId, Option<i64>)| {
                    id.short(len.and_then(|l| l.try_into().ok()).unwrap_or(12))
                }),
            ))
        }
        "shortest" => {
            let len_property = parse_optional_integer(function)?;
            Property::ShortestIdPrefix(chain_properties(
                (self_property, len_property),
                TemplatePropertyFn(|(id, len): &(CommitOrChangeId, Option<i64>)| {
                    id.shortest(len.and_then(|l| l.try_into().ok()).unwrap_or(0))
                }),
            ))
        }
        _ => {
            return Err(TemplateParseError::no_such_method(
                "CommitOrChangeId",
                function,
            ))
        }
    };
    Ok(property)
}

fn build_shortest_id_prefix_method<'a, I: 'a>(
    self_property: impl TemplateProperty<I, Output = ShortestIdPrefix> + 'a,
    function: &FunctionCallNode,
    _build_keyword: &impl Fn(&str, pest::Span) -> TemplateParseResult<Property<'a, I>>,
) -> TemplateParseResult<Property<'a, I>> {
    let property = match function.name {
        "prefix" => {
            expect_no_arguments(function)?;
            Property::String(chain_properties(
                self_property,
                TemplatePropertyFn(|id: &ShortestIdPrefix| id.prefix.clone()),
            ))
        }
        "rest" => {
            expect_no_arguments(function)?;
            Property::String(chain_properties(
                self_property,
                TemplatePropertyFn(|id: &ShortestIdPrefix| id.rest.clone()),
            ))
        }
        _ => {
            return Err(TemplateParseError::no_such_method(
                "ShortestIdPrefix",
                function,
            ))
        }
    };
    Ok(property)
}

fn build_signature_method<'a, I: 'a>(
    self_property: impl TemplateProperty<I, Output = Signature> + 'a,
    function: &FunctionCallNode,
    _build_keyword: &impl Fn(&str, pest::Span) -> TemplateParseResult<Property<'a, I>>,
) -> TemplateParseResult<Property<'a, I>> {
    let property = match function.name {
        "name" => {
            expect_no_arguments(function)?;
            Property::String(chain_properties(
                self_property,
                TemplatePropertyFn(|signature: &Signature| signature.name.clone()),
            ))
        }
        "email" => {
            expect_no_arguments(function)?;
            Property::String(chain_properties(
                self_property,
                TemplatePropertyFn(|signature: &Signature| signature.email.clone()),
            ))
        }
        "username" => {
            expect_no_arguments(function)?;
            Property::String(chain_properties(
                self_property,
                TemplatePropertyFn(|signature: &Signature| {
                    let (username, _) = split_email(&signature.email);
                    username.to_owned()
                }),
            ))
        }
        "timestamp" => {
            expect_no_arguments(function)?;
            Property::Timestamp(chain_properties(
                self_property,
                TemplatePropertyFn(|signature: &Signature| signature.timestamp.clone()),
            ))
        }
        _ => return Err(TemplateParseError::no_such_method("Signature", function)),
    };
    Ok(property)
}

fn build_timestamp_method<'a, I: 'a>(
    self_property: impl TemplateProperty<I, Output = Timestamp> + 'a,
    function: &FunctionCallNode,
    _build_keyword: &impl Fn(&str, pest::Span) -> TemplateParseResult<Property<'a, I>>,
) -> TemplateParseResult<Property<'a, I>> {
    let property = match function.name {
        "ago" => {
            expect_no_arguments(function)?;
            Property::String(chain_properties(
                self_property,
                TemplatePropertyFn(time_util::format_timestamp_relative_to_now),
            ))
        }
        _ => return Err(TemplateParseError::no_such_method("Timestamp", function)),
    };
    Ok(property)
}

fn build_global_function<'a, C: 'a>(
    function: &FunctionCallNode,
    build_keyword: &impl Fn(&str, pest::Span) -> TemplateParseResult<Property<'a, C>>,
) -> TemplateParseResult<Expression<'a, C>> {
    let expression = match function.name {
        "label" => {
            let [label_node, content_node] = expect_exact_arguments(function)?;
            let label_property = build_expression(label_node, build_keyword)?.into_plain_text();
            let content = build_expression(content_node, build_keyword)?.into_template();
            let labels = TemplateFunction::new(label_property, |s| {
                s.split_whitespace().map(ToString::to_string).collect()
            });
            let template = Box::new(LabelTemplate::new(content, labels));
            Expression::Template(template)
        }
        "if" => {
            let ([condition_node, true_node], [false_node]) = expect_arguments(function)?;
            let condition = build_expression(condition_node, build_keyword)?
                .try_into_boolean()
                .ok_or_else(|| {
                    TemplateParseError::invalid_argument_type("Boolean", condition_node.span)
                })?;
            let true_template = build_expression(true_node, build_keyword)?.into_template();
            let false_template = false_node
                .map(|node| build_expression(node, build_keyword))
                .transpose()?
                .map(|x| x.into_template());
            let template = Box::new(ConditionalTemplate::new(
                condition,
                true_template,
                false_template,
            ));
            Expression::Template(template)
        }
        "separate" => {
            let ([separator_node], content_nodes) = expect_some_arguments(function)?;
            let separator = build_expression(separator_node, build_keyword)?.into_template();
            let contents = content_nodes
                .iter()
                .map(|node| build_expression(node, build_keyword).map(|x| x.into_template()))
                .try_collect()?;
            let template = Box::new(SeparateTemplate::new(separator, contents));
            Expression::Template(template)
        }
        _ => return Err(TemplateParseError::no_such_function(function)),
    };
    Ok(expression)
}

fn build_commit_keyword<'a>(
    repo: &'a dyn Repo,
    workspace_id: &WorkspaceId,
    name: &str,
    span: pest::Span,
) -> TemplateParseResult<Property<'a, Commit>> {
    fn wrap_fn<'a, O>(
        f: impl Fn(&Commit) -> O + 'a,
    ) -> Box<dyn TemplateProperty<Commit, Output = O> + 'a> {
        Box::new(TemplatePropertyFn(f))
    }
    let property = match name {
        "description" => Property::String(wrap_fn(|commit| {
            cli_util::complete_newline(commit.description())
        })),
        "change_id" => Property::CommitOrChangeId(wrap_fn(move |commit| {
            CommitOrChangeId::change_id(repo, commit.change_id())
        })),
        "commit_id" => Property::CommitOrChangeId(wrap_fn(move |commit| {
            CommitOrChangeId::commit_id(repo, commit.id())
        })),
        "author" => Property::Signature(wrap_fn(|commit| commit.author().clone())),
        "committer" => Property::Signature(wrap_fn(|commit| commit.committer().clone())),
        "working_copies" => Property::String(Box::new(WorkingCopiesProperty { repo })),
        "current_working_copy" => {
            let workspace_id = workspace_id.clone();
            Property::Boolean(wrap_fn(move |commit| {
                Some(commit.id()) == repo.view().get_wc_commit_id(&workspace_id)
            }))
        }
        "branches" => Property::String(Box::new(BranchProperty { repo })),
        "tags" => Property::String(Box::new(TagProperty { repo })),
        "git_refs" => Property::String(Box::new(GitRefsProperty { repo })),
        "git_head" => Property::String(Box::new(GitHeadProperty::new(repo))),
        "divergent" => Property::Boolean(wrap_fn(move |commit| {
            // The given commit could be hidden in e.g. obslog.
            let maybe_entries = repo.resolve_change_id(commit.change_id());
            maybe_entries.map_or(0, |entries| entries.len()) > 1
        })),
        "conflict" => Property::Boolean(wrap_fn(|commit| commit.tree().has_conflict())),
        "empty" => Property::Boolean(wrap_fn(move |commit| {
            commit.tree().id() == rewrite::merge_commit_trees(repo, &commit.parents()).id()
        })),
        _ => return Err(TemplateParseError::no_such_keyword(name, span)),
    };
    Ok(property)
}

/// Builds template evaluation tree from AST nodes.
fn build_expression<'a, C: 'a>(
    node: &ExpressionNode,
    build_keyword: &impl Fn(&str, pest::Span) -> TemplateParseResult<Property<'a, C>>,
) -> TemplateParseResult<Expression<'a, C>> {
    match &node.kind {
        ExpressionKind::Identifier(name) => {
            let property = build_keyword(name, node.span)?;
            let labels = vec![(*name).to_owned()];
            Ok(Expression::Property(property, labels))
        }
        ExpressionKind::Integer(value) => {
            let property = Property::Integer(Box::new(Literal(*value)));
            Ok(Expression::Property(property, vec![]))
        }
        ExpressionKind::String(value) => {
            let property = Property::String(Box::new(Literal(value.clone())));
            Ok(Expression::Property(property, vec![]))
        }
        ExpressionKind::List(nodes) => {
            let templates = nodes
                .iter()
                .map(|node| build_expression(node, build_keyword).map(|x| x.into_template()))
                .try_collect()?;
            Ok(Expression::Template(Box::new(ListTemplate(templates))))
        }
        ExpressionKind::FunctionCall(function) => build_global_function(function, build_keyword),
        ExpressionKind::MethodCall(method) => build_method_call(method, build_keyword),
        ExpressionKind::AliasExpanded(id, subst) => build_expression(subst, build_keyword)
            .map_err(|e| e.within_alias_expansion(*id, node.span)),
    }
}

// TODO: We'll probably need a trait that abstracts the Property enum and
// keyword/method parsing functions per the top-level context.
pub fn parse_commit_template<'a>(
    repo: &'a dyn Repo,
    workspace_id: &WorkspaceId,
    template_text: &str,
    aliases_map: &TemplateAliasesMap,
) -> TemplateParseResult<Box<dyn Template<Commit> + 'a>> {
    let node = parse_template(template_text)?;
    let node = expand_aliases(node, aliases_map)?;
    let expression = build_expression(&node, &|name, span| {
        build_commit_keyword(repo, workspace_id, name, span)
    })?;
    Ok(expression.into_template())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct WithTemplateAliasesMap(TemplateAliasesMap);

    impl WithTemplateAliasesMap {
        fn parse<'i>(&'i self, template_text: &'i str) -> TemplateParseResult<ExpressionNode<'i>> {
            let node = parse_template(template_text)?;
            expand_aliases(node, &self.0)
        }

        fn parse_normalized<'i>(
            &'i self,
            template_text: &'i str,
        ) -> TemplateParseResult<ExpressionNode<'i>> {
            self.parse(template_text).map(normalize_tree)
        }
    }

    fn with_aliases(
        aliases: impl IntoIterator<Item = (impl AsRef<str>, impl Into<String>)>,
    ) -> WithTemplateAliasesMap {
        let mut aliases_map = TemplateAliasesMap::new();
        for (decl, defn) in aliases {
            aliases_map.insert(decl, defn).unwrap();
        }
        WithTemplateAliasesMap(aliases_map)
    }

    fn parse(template_text: &str) -> TemplateParseResult<Expression<()>> {
        let node = parse_template(template_text)?;
        build_expression(&node, &|name, span| {
            Err(TemplateParseError::no_such_keyword(name, span))
        })
    }

    fn parse_normalized(template_text: &str) -> TemplateParseResult<ExpressionNode> {
        parse_template(template_text).map(normalize_tree)
    }

    /// Drops auxiliary data of AST so it can be compared with other node.
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
                args_span: empty_span(),
            }
        }

        let normalized_kind = match node.kind {
            ExpressionKind::Identifier(_)
            | ExpressionKind::Integer(_)
            | ExpressionKind::String(_) => node.kind,
            ExpressionKind::List(nodes) => ExpressionKind::List(normalize_list(nodes)),
            ExpressionKind::FunctionCall(function) => {
                ExpressionKind::FunctionCall(normalize_function_call(function))
            }
            ExpressionKind::MethodCall(method) => {
                let object = Box::new(normalize_tree(*method.object));
                let function = normalize_function_call(method.function);
                ExpressionKind::MethodCall(MethodCallNode { object, function })
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
            normalize_tree(parse_template(r#" commit_id.short(1 )  description"#).unwrap()),
            normalize_tree(parse_template(r#"commit_id.short( 1 ) (description)"#).unwrap()),
        );
        assert_ne!(
            normalize_tree(parse_template(r#" "ab" "#).unwrap()),
            normalize_tree(parse_template(r#" "a" "b" "#).unwrap()),
        );
        assert_ne!(
            normalize_tree(parse_template(r#" "foo" "0" "#).unwrap()),
            normalize_tree(parse_template(r#" "foo" 0 "#).unwrap()),
        );
    }

    #[test]
    fn test_function_call_syntax() {
        // Trailing comma isn't allowed for empty argument
        assert!(parse(r#" "".first_line() "#).is_ok());
        assert!(parse(r#" "".first_line(,) "#).is_err());

        // Trailing comma is allowed for the last argument
        assert!(parse(r#" "".contains("") "#).is_ok());
        assert!(parse(r#" "".contains("",) "#).is_ok());
        assert!(parse(r#" "".contains("" ,  ) "#).is_ok());
        assert!(parse(r#" "".contains(,"") "#).is_err());
        assert!(parse(r#" "".contains("",,) "#).is_err());
        assert!(parse(r#" "".contains("" , , ) "#).is_err());
        assert!(parse(r#" label("","") "#).is_ok());
        assert!(parse(r#" label("","",) "#).is_ok());
        assert!(parse(r#" label("",,"") "#).is_err());
    }

    #[test]
    fn test_integer_literal() {
        let extract = |x: Expression<()>| x.try_into_integer().unwrap().extract(&());

        assert_eq!(extract(parse("0").unwrap()), 0);
        assert_eq!(extract(parse("(42)").unwrap()), 42);
        assert!(parse("00").is_err());

        assert_eq!(extract(parse(&format!("{}", i64::MAX)).unwrap()), i64::MAX);
        assert!(parse(&format!("{}", (i64::MAX as u64) + 1)).is_err());
    }

    #[test]
    fn test_parse_alias_decl() {
        let mut aliases_map = TemplateAliasesMap::new();
        aliases_map.insert("sym", r#""is symbol""#).unwrap();
        aliases_map.insert("func(a)", r#""is function""#).unwrap();

        let (id, defn) = aliases_map.get_symbol("sym").unwrap();
        assert_eq!(id, TemplateAliasId::Symbol("sym"));
        assert_eq!(defn, r#""is symbol""#);

        let (id, params, defn) = aliases_map.get_function("func").unwrap();
        assert_eq!(id, TemplateAliasId::Function("func"));
        assert_eq!(params, ["a"]);
        assert_eq!(defn, r#""is function""#);

        // Formal parameter 'a' can't be redefined
        assert_eq!(
            aliases_map.insert("f(a, a)", r#""""#).unwrap_err().kind,
            TemplateParseErrorKind::RedefinedFunctionParameter
        );

        // Trailing comma isn't allowed for empty parameter
        assert!(aliases_map.insert("f(,)", r#"""#).is_err());
        // Trailing comma is allowed for the last parameter
        assert!(aliases_map.insert("g(a,)", r#"""#).is_ok());
        assert!(aliases_map.insert("h(a ,  )", r#"""#).is_ok());
        assert!(aliases_map.insert("i(,a)", r#"""#).is_err());
        assert!(aliases_map.insert("j(a,,)", r#"""#).is_err());
        assert!(aliases_map.insert("k(a  , , )", r#"""#).is_err());
        assert!(aliases_map.insert("l(a,b,)", r#"""#).is_ok());
        assert!(aliases_map.insert("m(a,,b)", r#"""#).is_err());
    }

    #[test]
    fn test_expand_symbol_alias() {
        assert_eq!(
            with_aliases([("AB", "a b")])
                .parse_normalized("AB c")
                .unwrap(),
            parse_normalized("(a b) c").unwrap(),
        );
        assert_eq!(
            with_aliases([("AB", "a b")])
                .parse_normalized("if(AB, label(c, AB))")
                .unwrap(),
            parse_normalized("if((a b), label(c, (a b)))").unwrap(),
        );

        // Multi-level substitution.
        assert_eq!(
            with_aliases([("A", "BC"), ("BC", "b C"), ("C", "c")])
                .parse_normalized("A")
                .unwrap(),
            parse_normalized("b c").unwrap(),
        );

        // Method receiver and arguments should be expanded.
        assert_eq!(
            with_aliases([("A", "a")])
                .parse_normalized("A.f()")
                .unwrap(),
            parse_normalized("a.f()").unwrap(),
        );
        assert_eq!(
            with_aliases([("A", "a"), ("B", "b")])
                .parse_normalized("x.f(A, B)")
                .unwrap(),
            parse_normalized("x.f(a, b)").unwrap(),
        );

        // Infinite recursion, where the top-level error isn't of RecursiveAlias kind.
        assert_eq!(
            with_aliases([("A", "A")]).parse("A").unwrap_err().kind,
            TemplateParseErrorKind::BadAliasExpansion("A".to_owned()),
        );
        assert_eq!(
            with_aliases([("A", "B"), ("B", "b C"), ("C", "c B")])
                .parse("A")
                .unwrap_err()
                .kind,
            TemplateParseErrorKind::BadAliasExpansion("A".to_owned()),
        );

        // Error in alias definition.
        assert_eq!(
            with_aliases([("A", "a(")]).parse("A").unwrap_err().kind,
            TemplateParseErrorKind::BadAliasExpansion("A".to_owned()),
        );
    }

    #[test]
    fn test_expand_function_alias() {
        assert_eq!(
            with_aliases([("F(  )", "a")])
                .parse_normalized("F()")
                .unwrap(),
            parse_normalized("a").unwrap(),
        );
        assert_eq!(
            with_aliases([("F( x )", "x")])
                .parse_normalized("F(a)")
                .unwrap(),
            parse_normalized("a").unwrap(),
        );
        assert_eq!(
            with_aliases([("F( x, y )", "x y")])
                .parse_normalized("F(a, b)")
                .unwrap(),
            parse_normalized("a b").unwrap(),
        );

        // Arguments should be resolved in the current scope.
        assert_eq!(
            with_aliases([("F(x,y)", "if(x, y)")])
                .parse_normalized("F(a y, b x)")
                .unwrap(),
            parse_normalized("if((a y), (b x))").unwrap(),
        );
        // F(a) -> if(G(a), y) -> if((x a), y)
        assert_eq!(
            with_aliases([("F(x)", "if(G(x), y)"), ("G(y)", "x y")])
                .parse_normalized("F(a)")
                .unwrap(),
            parse_normalized("if((x a), y)").unwrap(),
        );
        // F(G(a)) -> F(x a) -> if(G(x a), y) -> if((x (x a)), y)
        assert_eq!(
            with_aliases([("F(x)", "if(G(x), y)"), ("G(y)", "x y")])
                .parse_normalized("F(G(a))")
                .unwrap(),
            parse_normalized("if((x (x a)), y)").unwrap(),
        );

        // Function parameter should precede the symbol alias.
        assert_eq!(
            with_aliases([("F(X)", "X"), ("X", "x")])
                .parse_normalized("F(a) X")
                .unwrap(),
            parse_normalized("a x").unwrap(),
        );

        // Function parameter shouldn't be expanded in symbol alias.
        assert_eq!(
            with_aliases([("F(x)", "x A"), ("A", "x")])
                .parse_normalized("F(a)")
                .unwrap(),
            parse_normalized("a x").unwrap(),
        );

        // Function and symbol aliases reside in separate namespaces.
        assert_eq!(
            with_aliases([("A()", "A"), ("A", "a")])
                .parse_normalized("A()")
                .unwrap(),
            parse_normalized("a").unwrap(),
        );

        // Method call shouldn't be substituted by function alias.
        assert_eq!(
            with_aliases([("F()", "f()")])
                .parse_normalized("x.F()")
                .unwrap(),
            parse_normalized("x.F()").unwrap(),
        );

        // Invalid number of arguments.
        assert_eq!(
            with_aliases([("F()", "x")]).parse("F(a)").unwrap_err().kind,
            TemplateParseErrorKind::InvalidArgumentCountExact(0),
        );
        assert_eq!(
            with_aliases([("F(x)", "x")]).parse("F()").unwrap_err().kind,
            TemplateParseErrorKind::InvalidArgumentCountExact(1),
        );
        assert_eq!(
            with_aliases([("F(x,y)", "x y")])
                .parse("F(a,b,c)")
                .unwrap_err()
                .kind,
            TemplateParseErrorKind::InvalidArgumentCountExact(2),
        );

        // Infinite recursion, where the top-level error isn't of RecursiveAlias kind.
        assert_eq!(
            with_aliases([("F(x)", "G(x)"), ("G(x)", "H(x)"), ("H(x)", "F(x)")])
                .parse("F(a)")
                .unwrap_err()
                .kind,
            TemplateParseErrorKind::BadAliasExpansion("F()".to_owned()),
        );
    }
}
