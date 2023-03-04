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
use pest::iterators::{Pair, Pairs};
use pest::Parser;
use pest_derive::Parser;
use thiserror::Error;

use crate::templater::{
    ConcatTemplate, ConditionalTemplate, FormattablePropertyListTemplate, IntoTemplate,
    LabelTemplate, Literal, PlainTextFormattedProperty, ReformatTemplate, SeparateTemplate,
    Template, TemplateFunction, TemplateProperty, TimestampRange,
};
use crate::{text_util, time_util};

#[derive(Parser)]
#[grammar = "template.pest"]
struct TemplateParser;

pub type TemplateParseResult<T> = Result<T, TemplateParseError>;

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
    pub fn with_span(kind: TemplateParseErrorKind, span: pest::Span<'_>) -> Self {
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

    pub fn with_span_and_origin(
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

    pub fn no_such_keyword(name: impl Into<String>, span: pest::Span<'_>) -> Self {
        TemplateParseError::with_span(TemplateParseErrorKind::NoSuchKeyword(name.into()), span)
    }

    pub fn no_such_function(function: &FunctionCallNode) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::NoSuchFunction(function.name.to_owned()),
            function.name_span,
        )
    }

    pub fn no_such_method(type_name: impl Into<String>, function: &FunctionCallNode) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::NoSuchMethod {
                type_name: type_name.into(),
                name: function.name.to_owned(),
            },
            function.name_span,
        )
    }

    pub fn invalid_argument_count_exact(count: usize, span: pest::Span<'_>) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::InvalidArgumentCountExact(count),
            span,
        )
    }

    pub fn invalid_argument_count_range(
        count: RangeInclusive<usize>,
        span: pest::Span<'_>,
    ) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::InvalidArgumentCountRange(count),
            span,
        )
    }

    pub fn invalid_argument_count_range_from(
        count: RangeFrom<usize>,
        span: pest::Span<'_>,
    ) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::InvalidArgumentCountRangeFrom(count),
            span,
        )
    }

    pub fn invalid_argument_type(
        expected_type_name: impl Into<String>,
        span: pest::Span<'_>,
    ) -> Self {
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
    pub span: pest::Span<'i>,
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
    Concat(Vec<ExpressionNode<'i>>),
    FunctionCall(FunctionCallNode<'i>),
    MethodCall(MethodCallNode<'i>),
    /// Identity node to preserve the span in the source template text.
    AliasExpanded(TemplateAliasId<'i>, Box<ExpressionNode<'i>>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionCallNode<'i> {
    pub name: &'i str,
    pub name_span: pest::Span<'i>,
    pub args: Vec<ExpressionNode<'i>>,
    pub args_span: pest::Span<'i>,
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
                't' => result.push('\t'),
                'r' => result.push('\r'),
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
        Ok(ExpressionNode::new(ExpressionKind::Concat(nodes), span))
    }
}

/// Parses text into AST nodes. No type/name checking is made at this stage.
pub fn parse_template(template_text: &str) -> TemplateParseResult<ExpressionNode> {
    let mut pairs: Pairs<Rule> = TemplateParser::parse(Rule::program, template_text)?;
    let first_pair = pairs.next().unwrap();
    if first_pair.as_rule() == Rule::EOI {
        let span = first_pair.as_span();
        Ok(ExpressionNode::new(ExpressionKind::Concat(vec![]), span))
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
pub fn expand_aliases<'i>(
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
            ExpressionKind::Concat(nodes) => {
                node.kind = ExpressionKind::Concat(expand_list(nodes, state)?);
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

/// Callbacks to build language-specific evaluation objects from AST nodes.
pub trait TemplateLanguage<'a> {
    type Context: 'a;
    type Property: IntoTemplateProperty<'a, Self::Context>;

    fn wrap_string(
        &self,
        property: impl TemplateProperty<Self::Context, Output = String> + 'a,
    ) -> Self::Property;
    fn wrap_string_list(
        &self,
        property: impl TemplateProperty<Self::Context, Output = Vec<String>> + 'a,
    ) -> Self::Property;
    fn wrap_boolean(
        &self,
        property: impl TemplateProperty<Self::Context, Output = bool> + 'a,
    ) -> Self::Property;
    fn wrap_integer(
        &self,
        property: impl TemplateProperty<Self::Context, Output = i64> + 'a,
    ) -> Self::Property;
    fn wrap_signature(
        &self,
        property: impl TemplateProperty<Self::Context, Output = Signature> + 'a,
    ) -> Self::Property;
    fn wrap_timestamp(
        &self,
        property: impl TemplateProperty<Self::Context, Output = Timestamp> + 'a,
    ) -> Self::Property;
    fn wrap_timestamp_range(
        &self,
        property: impl TemplateProperty<Self::Context, Output = TimestampRange> + 'a,
    ) -> Self::Property;
    fn wrap_template(&self, template: impl Template<Self::Context> + 'a) -> Self::Property;

    fn build_keyword(&self, name: &str, span: pest::Span) -> TemplateParseResult<Self::Property>;
    fn build_method(
        &self,
        property: Self::Property,
        function: &FunctionCallNode,
    ) -> TemplateParseResult<Self::Property>;
}

/// Implements `TemplateLanguage::wrap_<type>()` functions.
///
/// - `impl_core_wrap_property_fns('a)` for `CoreTemplatePropertyKind`,
/// - `impl_core_wrap_property_fns('a, MyKind::Core)` for `MyKind::Core(..)`.
macro_rules! impl_core_wrap_property_fns {
    ($a:lifetime) => {
        $crate::template_parser::impl_core_wrap_property_fns!($a, std::convert::identity);
    };
    ($a:lifetime, $outer:path) => {
        $crate::template_parser::impl_wrap_property_fns!(
            $a, $crate::template_parser::CoreTemplatePropertyKind, $outer, {
                wrap_string(String) => String,
                wrap_string_list(Vec<String>) => StringList,
                wrap_boolean(bool) => Boolean,
                wrap_integer(i64) => Integer,
                wrap_signature(jujutsu_lib::backend::Signature) => Signature,
                wrap_timestamp(jujutsu_lib::backend::Timestamp) => Timestamp,
                wrap_timestamp_range($crate::templater::TimestampRange) => TimestampRange,
            }
        );
        fn wrap_template(
            &self,
            template: impl $crate::templater::Template<Self::Context> + $a,
        ) -> Self::Property {
            use $crate::template_parser::CoreTemplatePropertyKind as Kind;
            $outer(Kind::Template(Box::new(template)))
        }
    };
}

macro_rules! impl_wrap_property_fns {
    ($a:lifetime, $kind:path, $outer:path, { $( $func:ident($ty:ty) => $var:ident, )+ }) => {
        $(
            fn $func(
                &self,
                property: impl $crate::templater::TemplateProperty<
                    Self::Context, Output = $ty> + $a,
            ) -> Self::Property {
                use $kind as Kind; // https://github.com/rust-lang/rust/issues/48067
                $outer(Kind::$var(Box::new(property)))
            }
        )+
    };
}

pub(crate) use {impl_core_wrap_property_fns, impl_wrap_property_fns};

/// Provides access to basic template property types.
pub trait IntoTemplateProperty<'a, C>: IntoTemplate<'a, C> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<C, Output = bool> + 'a>>;
    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<C, Output = i64> + 'a>>;

    fn into_plain_text(self) -> Box<dyn TemplateProperty<C, Output = String> + 'a>;
}

pub enum CoreTemplatePropertyKind<'a, I> {
    String(Box<dyn TemplateProperty<I, Output = String> + 'a>),
    StringList(Box<dyn TemplateProperty<I, Output = Vec<String>> + 'a>),
    Boolean(Box<dyn TemplateProperty<I, Output = bool> + 'a>),
    Integer(Box<dyn TemplateProperty<I, Output = i64> + 'a>),
    Signature(Box<dyn TemplateProperty<I, Output = Signature> + 'a>),
    Timestamp(Box<dyn TemplateProperty<I, Output = Timestamp> + 'a>),
    TimestampRange(Box<dyn TemplateProperty<I, Output = TimestampRange> + 'a>),

    // Similar to `TemplateProperty<I, Output = Box<dyn Template<()> + 'a>`, but doesn't
    // capture `I` to produce `Template<()>`. The context `I` would have to be cloned
    // to convert `Template<I>` to `Template<()>`.
    Template(Box<dyn Template<I> + 'a>),
}

impl<'a, I: 'a> IntoTemplateProperty<'a, I> for CoreTemplatePropertyKind<'a, I> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<I, Output = bool> + 'a>> {
        match self {
            CoreTemplatePropertyKind::String(property) => {
                Some(Box::new(TemplateFunction::new(property, |s| !s.is_empty())))
            }
            // TODO: should we allow implicit cast of List type?
            CoreTemplatePropertyKind::Boolean(property) => Some(property),
            _ => None,
        }
    }

    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<I, Output = i64> + 'a>> {
        match self {
            CoreTemplatePropertyKind::Integer(property) => Some(property),
            _ => None,
        }
    }

    fn into_plain_text(self) -> Box<dyn TemplateProperty<I, Output = String> + 'a> {
        match self {
            CoreTemplatePropertyKind::String(property) => property,
            _ => Box::new(PlainTextFormattedProperty::new(self.into_template())),
        }
    }
}

impl<'a, I: 'a> IntoTemplate<'a, I> for CoreTemplatePropertyKind<'a, I> {
    fn into_template(self) -> Box<dyn Template<I> + 'a> {
        match self {
            CoreTemplatePropertyKind::String(property) => property.into_template(),
            CoreTemplatePropertyKind::StringList(property) => property.into_template(),
            CoreTemplatePropertyKind::Boolean(property) => property.into_template(),
            CoreTemplatePropertyKind::Integer(property) => property.into_template(),
            CoreTemplatePropertyKind::Signature(property) => property.into_template(),
            CoreTemplatePropertyKind::Timestamp(property) => property.into_template(),
            CoreTemplatePropertyKind::TimestampRange(property) => property.into_template(),
            CoreTemplatePropertyKind::Template(template) => template,
        }
    }
}

/// Opaque struct that represents a template value.
pub struct Expression<P> {
    property: P,
    labels: Vec<String>,
}

impl<P> Expression<P> {
    fn unlabeled(property: P) -> Self {
        let labels = vec![];
        Expression { property, labels }
    }

    fn with_label(property: P, label: impl Into<String>) -> Self {
        let labels = vec![label.into()];
        Expression { property, labels }
    }

    pub fn try_into_boolean<'a, C: 'a>(
        self,
    ) -> Option<Box<dyn TemplateProperty<C, Output = bool> + 'a>>
    where
        P: IntoTemplateProperty<'a, C>,
    {
        self.property.try_into_boolean()
    }

    pub fn try_into_integer<'a, C: 'a>(
        self,
    ) -> Option<Box<dyn TemplateProperty<C, Output = i64> + 'a>>
    where
        P: IntoTemplateProperty<'a, C>,
    {
        self.property.try_into_integer()
    }

    pub fn into_plain_text<'a, C: 'a>(self) -> Box<dyn TemplateProperty<C, Output = String> + 'a>
    where
        P: IntoTemplateProperty<'a, C>,
    {
        self.property.into_plain_text()
    }
}

impl<'a, C: 'a, P: IntoTemplate<'a, C>> IntoTemplate<'a, C> for Expression<P> {
    fn into_template(self) -> Box<dyn Template<C> + 'a> {
        let template = self.property.into_template();
        if self.labels.is_empty() {
            template
        } else {
            Box::new(LabelTemplate::new(template, Literal(self.labels)))
        }
    }
}

pub fn expect_no_arguments(function: &FunctionCallNode) -> TemplateParseResult<()> {
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
pub fn expect_exact_arguments<'a, 'i, const N: usize>(
    function: &'a FunctionCallNode<'i>,
) -> TemplateParseResult<&'a [ExpressionNode<'i>; N]> {
    function
        .args
        .as_slice()
        .try_into()
        .map_err(|_| TemplateParseError::invalid_argument_count_exact(N, function.args_span))
}

/// Extracts N required arguments and remainders.
pub fn expect_some_arguments<'a, 'i, const N: usize>(
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
pub fn expect_arguments<'a, 'i, const N: usize, const M: usize>(
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

fn build_method_call<'a, L: TemplateLanguage<'a>>(
    language: &L,
    method: &MethodCallNode,
) -> TemplateParseResult<Expression<L::Property>> {
    let mut expression = build_expression(language, &method.object)?;
    expression.property = language.build_method(expression.property, &method.function)?;
    expression.labels.push(method.function.name.to_owned());
    Ok(expression)
}

pub fn build_core_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    property: CoreTemplatePropertyKind<'a, L::Context>,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    match property {
        CoreTemplatePropertyKind::String(property) => {
            build_string_method(language, property, function)
        }
        CoreTemplatePropertyKind::StringList(property) => {
            build_list_method(language, property, function)
        }
        CoreTemplatePropertyKind::Boolean(property) => {
            build_boolean_method(language, property, function)
        }
        CoreTemplatePropertyKind::Integer(property) => {
            build_integer_method(language, property, function)
        }
        CoreTemplatePropertyKind::Signature(property) => {
            build_signature_method(language, property, function)
        }
        CoreTemplatePropertyKind::Timestamp(property) => {
            build_timestamp_method(language, property, function)
        }
        CoreTemplatePropertyKind::TimestampRange(property) => {
            build_timestamp_range_method(language, property, function)
        }
        CoreTemplatePropertyKind::Template(_) => {
            Err(TemplateParseError::no_such_method("Template", function))
        }
    }
}

fn build_string_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    self_property: impl TemplateProperty<L::Context, Output = String> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "contains" => {
            let [needle_node] = expect_exact_arguments(function)?;
            // TODO: or .try_into_string() to disable implicit type cast?
            let needle_property = build_expression(language, needle_node)?.into_plain_text();
            language.wrap_boolean(TemplateFunction::new(
                (self_property, needle_property),
                |(haystack, needle)| haystack.contains(&needle),
            ))
        }
        "first_line" => {
            expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |s| {
                s.lines().next().unwrap_or_default().to_string()
            }))
        }
        "lines" => {
            expect_no_arguments(function)?;
            language.wrap_string_list(TemplateFunction::new(self_property, |s| {
                s.lines().map(|l| l.to_owned()).collect()
            }))
        }
        "upper" => {
            expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |s| s.to_uppercase()))
        }
        "lower" => {
            expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |s| s.to_lowercase()))
        }
        _ => return Err(TemplateParseError::no_such_method("String", function)),
    };
    Ok(property)
}

fn build_boolean_method<'a, L: TemplateLanguage<'a>>(
    _language: &L,
    _self_property: impl TemplateProperty<L::Context, Output = bool> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    Err(TemplateParseError::no_such_method("Boolean", function))
}

fn build_integer_method<'a, L: TemplateLanguage<'a>>(
    _language: &L,
    _self_property: impl TemplateProperty<L::Context, Output = i64> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    Err(TemplateParseError::no_such_method("Integer", function))
}

fn build_signature_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    self_property: impl TemplateProperty<L::Context, Output = Signature> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "name" => {
            expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |signature| {
                signature.name
            }))
        }
        "email" => {
            expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |signature| {
                signature.email
            }))
        }
        "username" => {
            expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |signature| {
                let (username, _) = split_email(&signature.email);
                username.to_owned()
            }))
        }
        "timestamp" => {
            expect_no_arguments(function)?;
            language.wrap_timestamp(TemplateFunction::new(self_property, |signature| {
                signature.timestamp
            }))
        }
        _ => return Err(TemplateParseError::no_such_method("Signature", function)),
    };
    Ok(property)
}

fn build_timestamp_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    self_property: impl TemplateProperty<L::Context, Output = Timestamp> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "ago" => {
            expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |timestamp| {
                time_util::format_timestamp_relative_to_now(&timestamp)
            }))
        }
        _ => return Err(TemplateParseError::no_such_method("Timestamp", function)),
    };
    Ok(property)
}

fn build_timestamp_range_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    self_property: impl TemplateProperty<L::Context, Output = TimestampRange> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "start" => {
            expect_no_arguments(function)?;
            language.wrap_timestamp(TemplateFunction::new(self_property, |time_range| {
                time_range.start
            }))
        }
        "end" => {
            expect_no_arguments(function)?;
            language.wrap_timestamp(TemplateFunction::new(self_property, |time_range| {
                time_range.end
            }))
        }
        "duration" => {
            expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |time_range| {
                time_range.duration()
            }))
        }
        _ => {
            return Err(TemplateParseError::no_such_method(
                "TimestampRange",
                function,
            ))
        }
    };
    Ok(property)
}

pub fn build_list_method<'a, L: TemplateLanguage<'a>, P: Template<()> + 'a>(
    language: &L,
    self_property: impl TemplateProperty<L::Context, Output = Vec<P>> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "join" => {
            let [separator_node] = expect_exact_arguments(function)?;
            let separator = build_expression(language, separator_node)?.into_template();
            let template = FormattablePropertyListTemplate::new(self_property, separator);
            language.wrap_template(template)
        }
        // TODO: .map()
        _ => return Err(TemplateParseError::no_such_method("List", function)),
    };
    Ok(property)
}

fn build_global_function<'a, L: TemplateLanguage<'a>>(
    language: &L,
    function: &FunctionCallNode,
) -> TemplateParseResult<Expression<L::Property>> {
    let property = match function.name {
        "fill" => {
            let [width_node, content_node] = expect_exact_arguments(function)?;
            let width = expect_integer_expression(language, width_node)?;
            let content = build_expression(language, content_node)?.into_template();
            let template = ReformatTemplate::new(content, move |context, formatter, recorded| {
                let width = width.extract(context).try_into().unwrap_or(0);
                text_util::write_wrapped(formatter, recorded, width)
            });
            language.wrap_template(template)
        }
        "indent" => {
            let [prefix_node, content_node] = expect_exact_arguments(function)?;
            let prefix = build_expression(language, prefix_node)?.into_template();
            let content = build_expression(language, content_node)?.into_template();
            let template = ReformatTemplate::new(content, move |context, formatter, recorded| {
                text_util::write_indented(formatter, recorded, |formatter| {
                    // If Template::format() returned a custom error type, we would need to
                    // handle template evaluation error out of this closure:
                    //   prefix.format(context, &mut prefix_recorder)?;
                    //   write_indented(formatter, recorded, |formatter| {
                    //       prefix_recorder.replay(formatter)
                    //   })
                    prefix.format(context, formatter)
                })
            });
            language.wrap_template(template)
        }
        "label" => {
            let [label_node, content_node] = expect_exact_arguments(function)?;
            let label_property = build_expression(language, label_node)?.into_plain_text();
            let content = build_expression(language, content_node)?.into_template();
            let labels = TemplateFunction::new(label_property, |s| {
                s.split_whitespace().map(ToString::to_string).collect()
            });
            language.wrap_template(LabelTemplate::new(content, labels))
        }
        "if" => {
            let ([condition_node, true_node], [false_node]) = expect_arguments(function)?;
            let condition = expect_boolean_expression(language, condition_node)?;
            let true_template = build_expression(language, true_node)?.into_template();
            let false_template = false_node
                .map(|node| build_expression(language, node))
                .transpose()?
                .map(|x| x.into_template());
            let template = ConditionalTemplate::new(condition, true_template, false_template);
            language.wrap_template(template)
        }
        "concat" => {
            let contents = function
                .args
                .iter()
                .map(|node| build_expression(language, node).map(|x| x.into_template()))
                .try_collect()?;
            language.wrap_template(ConcatTemplate(contents))
        }
        "separate" => {
            let ([separator_node], content_nodes) = expect_some_arguments(function)?;
            let separator = build_expression(language, separator_node)?.into_template();
            let contents = content_nodes
                .iter()
                .map(|node| build_expression(language, node).map(|x| x.into_template()))
                .try_collect()?;
            language.wrap_template(SeparateTemplate::new(separator, contents))
        }
        _ => return Err(TemplateParseError::no_such_function(function)),
    };
    Ok(Expression::unlabeled(property))
}

/// Builds template evaluation tree from AST nodes.
pub fn build_expression<'a, L: TemplateLanguage<'a>>(
    language: &L,
    node: &ExpressionNode,
) -> TemplateParseResult<Expression<L::Property>> {
    match &node.kind {
        ExpressionKind::Identifier(name) => {
            let property = language.build_keyword(name, node.span)?;
            Ok(Expression::with_label(property, *name))
        }
        ExpressionKind::Integer(value) => {
            let property = language.wrap_integer(Literal(*value));
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::String(value) => {
            let property = language.wrap_string(Literal(value.clone()));
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::Concat(nodes) => {
            let templates = nodes
                .iter()
                .map(|node| build_expression(language, node).map(|x| x.into_template()))
                .try_collect()?;
            let property = language.wrap_template(ConcatTemplate(templates));
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::FunctionCall(function) => build_global_function(language, function),
        ExpressionKind::MethodCall(method) => build_method_call(language, method),
        ExpressionKind::AliasExpanded(id, subst) => {
            build_expression(language, subst).map_err(|e| e.within_alias_expansion(*id, node.span))
        }
    }
}

pub fn expect_boolean_expression<'a, L: TemplateLanguage<'a>>(
    language: &L,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn TemplateProperty<L::Context, Output = bool> + 'a>> {
    build_expression(language, node)?
        .try_into_boolean()
        .ok_or_else(|| TemplateParseError::invalid_argument_type("Boolean", node.span))
}

pub fn expect_integer_expression<'a, L: TemplateLanguage<'a>>(
    language: &L,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn TemplateProperty<L::Context, Output = i64> + 'a>> {
    build_expression(language, node)?
        .try_into_integer()
        .ok_or_else(|| TemplateParseError::invalid_argument_type("Integer", node.span))
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

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

    fn parse_into_kind(template_text: &str) -> Result<ExpressionKind, TemplateParseErrorKind> {
        parse_template(template_text)
            .map(|node| node.kind)
            .map_err(|err| err.kind)
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
            ExpressionKind::Concat(nodes) => ExpressionKind::Concat(normalize_list(nodes)),
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
            normalize_tree(parse_template(r#" commit_id.short(1 )  ++ description"#).unwrap()),
            normalize_tree(parse_template(r#"commit_id.short( 1 )++(description)"#).unwrap()),
        );
        assert_ne!(
            normalize_tree(parse_template(r#" "ab" "#).unwrap()),
            normalize_tree(parse_template(r#" "a" ++ "b" "#).unwrap()),
        );
        assert_ne!(
            normalize_tree(parse_template(r#" "foo" ++ "0" "#).unwrap()),
            normalize_tree(parse_template(r#" "foo" ++ 0 "#).unwrap()),
        );
    }

    #[test]
    fn test_parse_whitespace() {
        let ascii_whitespaces: String = ('\x00'..='\x7f')
            .filter(char::is_ascii_whitespace)
            .collect();
        assert_eq!(
            parse_normalized(&format!("{ascii_whitespaces}f()")).unwrap(),
            parse_normalized("f()").unwrap(),
        );
    }

    #[test]
    fn test_function_call_syntax() {
        // Trailing comma isn't allowed for empty argument
        assert!(parse_template(r#" "".first_line() "#).is_ok());
        assert!(parse_template(r#" "".first_line(,) "#).is_err());

        // Trailing comma is allowed for the last argument
        assert!(parse_template(r#" "".contains("") "#).is_ok());
        assert!(parse_template(r#" "".contains("",) "#).is_ok());
        assert!(parse_template(r#" "".contains("" ,  ) "#).is_ok());
        assert!(parse_template(r#" "".contains(,"") "#).is_err());
        assert!(parse_template(r#" "".contains("",,) "#).is_err());
        assert!(parse_template(r#" "".contains("" , , ) "#).is_err());
        assert!(parse_template(r#" label("","") "#).is_ok());
        assert!(parse_template(r#" label("","",) "#).is_ok());
        assert!(parse_template(r#" label("",,"") "#).is_err());
    }

    #[test]
    fn test_string_literal() {
        // "\<char>" escapes
        assert_eq!(
            parse_into_kind(r#" "\t\r\n\"\\" "#),
            Ok(ExpressionKind::String("\t\r\n\"\\".to_owned())),
        );

        // Invalid "\<char>" escape
        assert_eq!(
            parse_into_kind(r#" "\y" "#),
            Err(TemplateParseErrorKind::SyntaxError),
        );
    }

    #[test]
    fn test_integer_literal() {
        assert_eq!(parse_into_kind("0"), Ok(ExpressionKind::Integer(0)));
        assert_eq!(parse_into_kind("(42)"), Ok(ExpressionKind::Integer(42)));
        assert_eq!(
            parse_into_kind("00"),
            Err(TemplateParseErrorKind::SyntaxError),
        );

        assert_eq!(
            parse_into_kind(&format!("{}", i64::MAX)),
            Ok(ExpressionKind::Integer(i64::MAX)),
        );
        assert_matches!(
            parse_into_kind(&format!("{}", (i64::MAX as u64) + 1)),
            Err(TemplateParseErrorKind::ParseIntError(_))
        );
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
            with_aliases([("AB", "a ++ b")])
                .parse_normalized("AB ++ c")
                .unwrap(),
            parse_normalized("(a ++ b) ++ c").unwrap(),
        );
        assert_eq!(
            with_aliases([("AB", "a ++ b")])
                .parse_normalized("if(AB, label(c, AB))")
                .unwrap(),
            parse_normalized("if((a ++ b), label(c, (a ++ b)))").unwrap(),
        );

        // Multi-level substitution.
        assert_eq!(
            with_aliases([("A", "BC"), ("BC", "b ++ C"), ("C", "c")])
                .parse_normalized("A")
                .unwrap(),
            parse_normalized("b ++ c").unwrap(),
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
            with_aliases([("A", "B"), ("B", "b ++ C"), ("C", "c ++ B")])
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
            with_aliases([("F( x, y )", "x ++ y")])
                .parse_normalized("F(a, b)")
                .unwrap(),
            parse_normalized("a ++ b").unwrap(),
        );

        // Arguments should be resolved in the current scope.
        assert_eq!(
            with_aliases([("F(x,y)", "if(x, y)")])
                .parse_normalized("F(a ++ y, b ++ x)")
                .unwrap(),
            parse_normalized("if((a ++ y), (b ++ x))").unwrap(),
        );
        // F(a) -> if(G(a), y) -> if((x ++ a), y)
        assert_eq!(
            with_aliases([("F(x)", "if(G(x), y)"), ("G(y)", "x ++ y")])
                .parse_normalized("F(a)")
                .unwrap(),
            parse_normalized("if((x ++ a), y)").unwrap(),
        );
        // F(G(a)) -> F(x ++ a) -> if(G(x ++ a), y) -> if((x ++ (x ++ a)), y)
        assert_eq!(
            with_aliases([("F(x)", "if(G(x), y)"), ("G(y)", "x ++ y")])
                .parse_normalized("F(G(a))")
                .unwrap(),
            parse_normalized("if((x ++ (x ++ a)), y)").unwrap(),
        );

        // Function parameter should precede the symbol alias.
        assert_eq!(
            with_aliases([("F(X)", "X"), ("X", "x")])
                .parse_normalized("F(a) ++ X")
                .unwrap(),
            parse_normalized("a ++ x").unwrap(),
        );

        // Function parameter shouldn't be expanded in symbol alias.
        assert_eq!(
            with_aliases([("F(x)", "x ++ A"), ("A", "x")])
                .parse_normalized("F(a)")
                .unwrap(),
            parse_normalized("a ++ x").unwrap(),
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
            with_aliases([("F(x,y)", "x ++ y")])
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
