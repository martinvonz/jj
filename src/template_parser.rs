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
use std::{error, fmt};

use itertools::Itertools as _;
use pest::iterators::{Pair, Pairs};
use pest::Parser;
use pest_derive::Parser;
use thiserror::Error;

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
    #[error(r#"Field "{name}" doesn't exist for type "{type_name}""#)]
    NoSuchField { type_name: String, name: String },
    #[error(r#"Method "{name}" doesn't exist for type "{type_name}""#)]
    NoSuchMethod { type_name: String, name: String },
    #[error(r#"Function "{name}": {message}"#)]
    InvalidArguments { name: String, message: String },
    #[error("Redefinition of function parameter")]
    RedefinedFunctionParameter,
    #[error("{0}")]
    UnexpectedExpression(String),
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

    pub fn no_such_field(type_name: impl Into<String>, field: &FieldNode) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::NoSuchField {
                type_name: type_name.into(),
                name: field.name.to_owned(),
            },
            field.name_span,
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

    pub fn invalid_arguments(function: &FunctionCallNode, message: impl Into<String>) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::InvalidArguments {
                name: function.name.to_owned(),
                message: message.into(),
            },
            function.args_span,
        )
    }

    pub fn expected_type(type_name: &str, span: pest::Span<'_>) -> Self {
        let message = format!(r#"Expected expression of type "{type_name}""#);
        TemplateParseError::unexpected_expression(message, span)
    }

    pub fn unexpected_expression(message: impl Into<String>, span: pest::Span<'_>) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::UnexpectedExpression(message.into()),
            span,
        )
    }

    pub fn within_alias_expansion(self, id: TemplateAliasId<'_>, span: pest::Span<'_>) -> Self {
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
    pub kind: ExpressionKind<'i>,
    pub span: pest::Span<'i>,
}

impl<'i> ExpressionNode<'i> {
    fn new(kind: ExpressionKind<'i>, span: pest::Span<'i>) -> Self {
        ExpressionNode { kind, span }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExpressionKind<'i> {
    Identifier(&'i str),
    Integer(i64),
    String(String),
    Concat(Vec<ExpressionNode<'i>>),
    FunctionCall(FunctionCallNode<'i>),
    FieldAccess(FieldAccessNode<'i>),
    MethodCall(MethodCallNode<'i>),
    Lambda(LambdaNode<'i>),
    /// Identity node to preserve the span in the source template text.
    AliasExpanded(TemplateAliasId<'i>, Box<ExpressionNode<'i>>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldNode<'i> {
    pub name: &'i str,
    pub name_span: pest::Span<'i>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldAccessNode<'i> {
    pub object: Box<ExpressionNode<'i>>,
    pub field: FieldNode<'i>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionCallNode<'i> {
    pub name: &'i str,
    pub name_span: pest::Span<'i>,
    pub args: Vec<ExpressionNode<'i>>,
    pub args_span: pest::Span<'i>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodCallNode<'i> {
    pub object: Box<ExpressionNode<'i>>,
    pub function: FunctionCallNode<'i>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LambdaNode<'i> {
    pub params: Vec<&'i str>,
    pub params_span: pest::Span<'i>,
    pub body: Box<ExpressionNode<'i>>,
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

fn parse_formal_parameters(params_pair: Pair<Rule>) -> TemplateParseResult<Vec<&str>> {
    assert_eq!(params_pair.as_rule(), Rule::formal_parameters);
    let params_span = params_pair.as_span();
    let params = params_pair
        .into_inner()
        .map(|pair| match pair.as_rule() {
            Rule::identifier => pair.as_str(),
            r => panic!("unexpected formal parameter rule {r:?}"),
        })
        .collect_vec();
    if params.iter().all_unique() {
        Ok(params)
    } else {
        Err(TemplateParseError::with_span(
            TemplateParseErrorKind::RedefinedFunctionParameter,
            params_span,
        ))
    }
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

fn parse_lambda_node(pair: Pair<Rule>) -> TemplateParseResult<LambdaNode> {
    assert_eq!(pair.as_rule(), Rule::lambda);
    let mut inner = pair.into_inner();
    let params_pair = inner.next().unwrap();
    let params_span = params_pair.as_span();
    let body_pair = inner.next().unwrap();
    let params = parse_formal_parameters(params_pair)?;
    let body = parse_template_node(body_pair)?;
    Ok(LambdaNode {
        params,
        params_span,
        body: Box::new(body),
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
        Rule::lambda => {
            let lambda = parse_lambda_node(expr)?;
            ExpressionNode::new(ExpressionKind::Lambda(lambda), span)
        }
        Rule::template => parse_template_node(expr)?,
        other => panic!("unexpected term: {other:?}"),
    };
    inner.try_fold(primary, |object, chain| {
        let span = object.span.start_pos().span(&chain.as_span().end_pos());
        match chain.as_rule() {
            Rule::identifier => {
                let access = FieldAccessNode {
                    object: Box::new(object),
                    field: FieldNode {
                        name: chain.as_str(),
                        name_span: chain.as_span(),
                    },
                };
                Ok(ExpressionNode::new(
                    ExpressionKind::FieldAccess(access),
                    span,
                ))
            }
            Rule::function => {
                let method = MethodCallNode {
                    object: Box::new(object),
                    function: parse_function_call_node(chain)?,
                };
                Ok(ExpressionNode::new(
                    ExpressionKind::MethodCall(method),
                    span,
                ))
            }
            other => panic!("unexpected object expression: {other:?}"),
        }
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
                assert_eq!(name_pair.as_rule(), Rule::identifier);
                let name = name_pair.as_str().to_owned();
                let params = parse_formal_parameters(params_pair)?
                    .into_iter()
                    .map(|s| s.to_owned())
                    .collect();
                Ok(TemplateAliasDeclaration::Function(name, params))
            }
            r => panic!("unexpected alias declaration rule {r:?}"),
        }
    }
}

/// Borrowed reference to identify alias expression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemplateAliasId<'a> {
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
                        return Err(TemplateParseError::invalid_arguments(
                            &function,
                            format!("Expected {} arguments", params.len()),
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
            ExpressionKind::FieldAccess(access) => {
                node.kind = ExpressionKind::FieldAccess(FieldAccessNode {
                    object: Box::new(expand_node(*access.object, state)?),
                    field: access.field,
                });
                Ok(node)
            }
            ExpressionKind::MethodCall(method) => {
                node.kind = ExpressionKind::MethodCall(MethodCallNode {
                    object: Box::new(expand_node(*method.object, state)?),
                    function: expand_function_call(method.function, state)?,
                });
                Ok(node)
            }
            ExpressionKind::Lambda(lambda) => {
                node.kind = ExpressionKind::Lambda(LambdaNode {
                    params: lambda.params,
                    params_span: lambda.params_span,
                    body: Box::new(expand_node(*lambda.body, state)?),
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

/// Parses text into AST nodes, and expands aliases.
///
/// No type/name checking is made at this stage.
pub fn parse<'i>(
    template_text: &'i str,
    aliases_map: &'i TemplateAliasesMap,
) -> TemplateParseResult<ExpressionNode<'i>> {
    let node = parse_template(template_text)?;
    expand_aliases(node, aliases_map)
}

pub fn expect_no_arguments(function: &FunctionCallNode) -> TemplateParseResult<()> {
    if function.args.is_empty() {
        Ok(())
    } else {
        Err(TemplateParseError::invalid_arguments(
            function,
            "Expected 0 arguments",
        ))
    }
}

/// Extracts exactly N required arguments.
pub fn expect_exact_arguments<'a, 'i, const N: usize>(
    function: &'a FunctionCallNode<'i>,
) -> TemplateParseResult<&'a [ExpressionNode<'i>; N]> {
    function.args.as_slice().try_into().map_err(|_| {
        TemplateParseError::invalid_arguments(function, format!("Expected {N} arguments"))
    })
}

/// Extracts N required arguments and remainders.
pub fn expect_some_arguments<'a, 'i, const N: usize>(
    function: &'a FunctionCallNode<'i>,
) -> TemplateParseResult<(&'a [ExpressionNode<'i>; N], &'a [ExpressionNode<'i>])> {
    if function.args.len() >= N {
        let (required, rest) = function.args.split_at(N);
        Ok((required.try_into().unwrap(), rest))
    } else {
        Err(TemplateParseError::invalid_arguments(
            function,
            format!("Expected at least {N} arguments"),
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
        Err(TemplateParseError::invalid_arguments(
            function,
            format!("Expected {min} to {max} arguments", min = N, max = N + M),
        ))
    }
}

/// Applies the given function if the `node` is a string literal.
pub fn expect_string_literal_with<'a, 'i, T>(
    node: &'a ExpressionNode<'i>,
    f: impl FnOnce(&'a str, pest::Span<'i>) -> TemplateParseResult<T>,
) -> TemplateParseResult<T> {
    match &node.kind {
        ExpressionKind::String(s) => f(s, node.span),
        ExpressionKind::Identifier(_)
        | ExpressionKind::Integer(_)
        | ExpressionKind::Concat(_)
        | ExpressionKind::FunctionCall(_)
        | ExpressionKind::FieldAccess(_)
        | ExpressionKind::MethodCall(_)
        | ExpressionKind::Lambda(_) => Err(TemplateParseError::unexpected_expression(
            "Expected string literal",
            node.span,
        )),
        ExpressionKind::AliasExpanded(id, subst) => expect_string_literal_with(subst, f)
            .map_err(|e| e.within_alias_expansion(*id, node.span)),
    }
}

/// Applies the given function if the `node` is a lambda.
pub fn expect_lambda_with<'a, 'i, T>(
    node: &'a ExpressionNode<'i>,
    f: impl FnOnce(&'a LambdaNode<'i>, pest::Span<'i>) -> TemplateParseResult<T>,
) -> TemplateParseResult<T> {
    match &node.kind {
        ExpressionKind::Lambda(lambda) => f(lambda, node.span),
        ExpressionKind::String(_)
        | ExpressionKind::Identifier(_)
        | ExpressionKind::Integer(_)
        | ExpressionKind::Concat(_)
        | ExpressionKind::FunctionCall(_)
        | ExpressionKind::FieldAccess(_)
        | ExpressionKind::MethodCall(_) => Err(TemplateParseError::unexpected_expression(
            "Expected lambda expression",
            node.span,
        )),
        ExpressionKind::AliasExpanded(id, subst) => {
            expect_lambda_with(subst, f).map_err(|e| e.within_alias_expansion(*id, node.span))
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;

    #[derive(Debug)]
    struct WithTemplateAliasesMap(TemplateAliasesMap);

    impl WithTemplateAliasesMap {
        fn parse<'i>(&'i self, template_text: &'i str) -> TemplateParseResult<ExpressionNode<'i>> {
            parse(template_text, &self.0)
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
            ExpressionKind::FieldAccess(access) => {
                let object = Box::new(normalize_tree(*access.object));
                ExpressionKind::FieldAccess(FieldAccessNode {
                    object,
                    field: FieldNode {
                        name: access.field.name,
                        name_span: empty_span(),
                    },
                })
            }
            ExpressionKind::MethodCall(method) => {
                let object = Box::new(normalize_tree(*method.object));
                let function = normalize_function_call(method.function);
                ExpressionKind::MethodCall(MethodCallNode { object, function })
            }
            ExpressionKind::Lambda(lambda) => {
                let body = Box::new(normalize_tree(*lambda.body));
                ExpressionKind::Lambda(LambdaNode {
                    params: lambda.params,
                    params_span: empty_span(),
                    body,
                })
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
    fn test_field_access_syntax() {
        assert_eq!(
            parse_normalized("x.a.b").unwrap(),
            parse_normalized("(x.a).b").unwrap(),
        );

        // Expression span
        assert_eq!(parse_template(" x.a ").unwrap().span.as_str(), "x.a");
        assert_eq!(parse_template(" x.a.b ").unwrap().span.as_str(), "x.a.b");
    }

    #[test]
    fn test_method_call_syntax() {
        assert_eq!(
            parse_normalized("x.f().g()").unwrap(),
            parse_normalized("(x.f()).g()").unwrap(),
        );

        // Expression span
        assert_eq!(parse_template(" x.f() ").unwrap().span.as_str(), "x.f()");
        assert_eq!(
            parse_template(" x.f().g() ").unwrap().span.as_str(),
            "x.f().g()",
        );
    }

    #[test]
    fn test_lambda_syntax() {
        fn unwrap_lambda(node: ExpressionNode<'_>) -> LambdaNode<'_> {
            match node.kind {
                ExpressionKind::Lambda(lambda) => lambda,
                _ => panic!("unexpected expression: {node:?}"),
            }
        }

        let lambda = unwrap_lambda(parse_template("|| a").unwrap());
        assert_eq!(lambda.params.len(), 0);
        assert_eq!(lambda.body.kind, ExpressionKind::Identifier("a"));
        let lambda = unwrap_lambda(parse_template("|foo| a").unwrap());
        assert_eq!(lambda.params.len(), 1);
        let lambda = unwrap_lambda(parse_template("|foo, b| a").unwrap());
        assert_eq!(lambda.params.len(), 2);

        // No body
        assert!(parse_template("||").is_err());

        // Binding
        assert_eq!(
            parse_normalized("||  x ++ y").unwrap(),
            parse_normalized("|| (x ++ y)").unwrap(),
        );
        assert_eq!(
            parse_normalized("f( || x,   || y)").unwrap(),
            parse_normalized("f((|| x), (|| y))").unwrap(),
        );
        assert_eq!(
            parse_normalized("||  x ++  || y").unwrap(),
            parse_normalized("|| (x ++ (|| y))").unwrap(),
        );

        // Trailing comma
        assert!(parse_template("|,| a").is_err());
        assert!(parse_template("|x,| a").is_ok());
        assert!(parse_template("|x , | a").is_ok());
        assert!(parse_template("|,x| a").is_err());
        assert!(parse_template("| x,y,| a").is_ok());
        assert!(parse_template("|x,,y| a").is_err());

        // Formal parameter can't be redefined
        assert_eq!(
            parse_template("|x, x| a").unwrap_err().kind,
            TemplateParseErrorKind::RedefinedFunctionParameter
        );
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

        // Field access shouldn't be substituted by symbol alias.
        assert_eq!(
            with_aliases([("A", "a")]).parse_normalized("x.A").unwrap(),
            parse_normalized("x.A").unwrap(),
        );

        // Field/method receiver and arguments should be expanded.
        assert_eq!(
            with_aliases([("A", "a")]).parse_normalized("A.b").unwrap(),
            parse_normalized("a.b").unwrap(),
        );
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

        // Lambda expression body should be expanded.
        assert_eq!(
            with_aliases([("A", "a")]).parse_normalized("|| A").unwrap(),
            parse_normalized("|| a").unwrap(),
        );
        // No matter if 'A' is a formal parameter. Alias substitution isn't scoped.
        // If we don't like this behavior, maybe we can turn off alias substitution
        // for lambda parameters.
        assert_eq!(
            with_aliases([("A", "a ++ b")])
                .parse_normalized("|A| A")
                .unwrap(),
            parse_normalized("|A| (a ++ b)").unwrap(),
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

        // Formal parameter shouldn't be substituted by alias parameter, but
        // the expression should be substituted.
        assert_eq!(
            with_aliases([("F(x)", "|x| x")])
                .parse_normalized("F(a ++ b)")
                .unwrap(),
            parse_normalized("|x| (a ++ b)").unwrap(),
        );

        // Invalid number of arguments.
        assert_matches!(
            with_aliases([("F()", "x")]).parse("F(a)").unwrap_err().kind,
            TemplateParseErrorKind::InvalidArguments { .. }
        );
        assert_matches!(
            with_aliases([("F(x)", "x")]).parse("F()").unwrap_err().kind,
            TemplateParseErrorKind::InvalidArguments { .. }
        );
        assert_matches!(
            with_aliases([("F(x,y)", "x ++ y")])
                .parse("F(a,b,c)")
                .unwrap_err()
                .kind,
            TemplateParseErrorKind::InvalidArguments { .. }
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
