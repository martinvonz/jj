// Copyright 2020-2024 The Jujutsu Authors
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

//! Domain-specific language helpers.

use std::collections::HashMap;
use std::fmt;

use itertools::Itertools as _;
use pest::iterators::Pairs;
use pest::RuleType;

/// AST node without type or name checking.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpressionNode<'i, T> {
    /// Expression item such as identifier, literal, function call, etc.
    pub kind: T,
    /// Span of the node.
    pub span: pest::Span<'i>,
}

impl<'i, T> ExpressionNode<'i, T> {
    /// Wraps the given expression and span.
    pub fn new(kind: T, span: pest::Span<'i>) -> Self {
        ExpressionNode { kind, span }
    }
}

/// Function call in AST.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionCallNode<'i, T> {
    /// Function name.
    pub name: &'i str,
    /// Span of the function name.
    pub name_span: pest::Span<'i>,
    /// List of positional arguments.
    pub args: Vec<ExpressionNode<'i, T>>,
    // TODO: revset supports keyword args
    /// Span of the arguments list.
    pub args_span: pest::Span<'i>,
}

impl<'i, T> FunctionCallNode<'i, T> {
    /// Ensures that no arguments passed.
    pub fn expect_no_arguments(&self) -> Result<(), InvalidArguments<'i>> {
        let ([], []) = self.expect_arguments()?;
        Ok(())
    }

    /// Extracts exactly N required arguments.
    pub fn expect_exact_arguments<const N: usize>(
        &self,
    ) -> Result<&[ExpressionNode<'i, T>; N], InvalidArguments<'i>> {
        let (args, []) = self.expect_arguments()?;
        Ok(args)
    }

    /// Extracts N required arguments and remainders.
    #[allow(clippy::type_complexity)]
    pub fn expect_some_arguments<const N: usize>(
        &self,
    ) -> Result<(&[ExpressionNode<'i, T>; N], &[ExpressionNode<'i, T>]), InvalidArguments<'i>> {
        if self.args.len() >= N {
            let (required, rest) = self.args.split_at(N);
            Ok((required.try_into().unwrap(), rest))
        } else {
            Err(self.invalid_arguments_count(N, None))
        }
    }

    /// Extracts N required arguments and M optional arguments.
    #[allow(clippy::type_complexity)]
    pub fn expect_arguments<const N: usize, const M: usize>(
        &self,
    ) -> Result<
        (
            &[ExpressionNode<'i, T>; N],
            [Option<&ExpressionNode<'i, T>>; M],
        ),
        InvalidArguments<'i>,
    > {
        let count_range = N..=(N + M);
        if count_range.contains(&self.args.len()) {
            let (required, rest) = self.args.split_at(N);
            let mut optional = rest.iter().map(Some).collect_vec();
            optional.resize(M, None);
            Ok((
                required.try_into().unwrap(),
                optional.try_into().ok().unwrap(),
            ))
        } else {
            let (min, max) = count_range.into_inner();
            Err(self.invalid_arguments_count(min, Some(max)))
        }
    }

    fn invalid_arguments(&self, message: String) -> InvalidArguments<'i> {
        InvalidArguments {
            name: self.name,
            message,
            span: self.args_span,
        }
    }

    fn invalid_arguments_count(&self, min: usize, max: Option<usize>) -> InvalidArguments<'i> {
        let message = match (min, max) {
            (min, Some(max)) if min == max => format!("Expected {min} arguments"),
            (min, Some(max)) => format!("Expected {min} to {max} arguments"),
            (min, None) => format!("Expected at least {min} arguments"),
        };
        self.invalid_arguments(message)
    }
}

/// Unexpected number of arguments, or invalid combination of arguments.
///
/// This error is supposed to be converted to language-specific parse error
/// type, where lifetime `'i` will be eliminated.
#[derive(Clone, Debug)]
pub struct InvalidArguments<'i> {
    /// Function name.
    pub name: &'i str,
    /// Error message.
    pub message: String,
    /// Span of the bad arguments.
    pub span: pest::Span<'i>,
}

/// Expression item that can be transformed recursively by using `folder: F`.
pub trait FoldableExpression<'i>: Sized {
    /// Transforms `self` by applying the `folder` to inner items.
    fn fold<F>(self, folder: &mut F, span: pest::Span<'i>) -> Result<Self, F::Error>
    where
        F: ExpressionFolder<'i, Self> + ?Sized;
}

/// Visitor-like interface to transform AST nodes recursively.
pub trait ExpressionFolder<'i, T: FoldableExpression<'i>> {
    /// Transform error.
    type Error;

    /// Transforms the expression `node`. By default, inner items are
    /// transformed recursively.
    fn fold_expression(
        &mut self,
        node: ExpressionNode<'i, T>,
    ) -> Result<ExpressionNode<'i, T>, Self::Error> {
        let ExpressionNode { kind, span } = node;
        let kind = kind.fold(self, span)?;
        Ok(ExpressionNode { kind, span })
    }

    /// Transforms identifier.
    fn fold_identifier(&mut self, name: &'i str, span: pest::Span<'i>) -> Result<T, Self::Error>;

    /// Transforms function call.
    fn fold_function_call(
        &mut self,
        function: Box<FunctionCallNode<'i, T>>,
        span: pest::Span<'i>,
    ) -> Result<T, Self::Error>;
}

/// Transforms list of `nodes` by using `folder`.
pub fn fold_expression_nodes<'i, F, T>(
    folder: &mut F,
    nodes: Vec<ExpressionNode<'i, T>>,
) -> Result<Vec<ExpressionNode<'i, T>>, F::Error>
where
    F: ExpressionFolder<'i, T> + ?Sized,
    T: FoldableExpression<'i>,
{
    nodes
        .into_iter()
        .map(|node| folder.fold_expression(node))
        .try_collect()
}

/// Transforms function call arguments by using `folder`.
pub fn fold_function_call_args<'i, F, T>(
    folder: &mut F,
    function: FunctionCallNode<'i, T>,
) -> Result<FunctionCallNode<'i, T>, F::Error>
where
    F: ExpressionFolder<'i, T> + ?Sized,
    T: FoldableExpression<'i>,
{
    Ok(FunctionCallNode {
        name: function.name,
        name_span: function.name_span,
        args: fold_expression_nodes(folder, function.args)?,
        args_span: function.args_span,
    })
}

/// Helper to parse string literal.
#[derive(Debug)]
pub struct StringLiteralParser<R> {
    /// String content part.
    pub content_rule: R,
    /// Escape sequence part including backslash character.
    pub escape_rule: R,
}

impl<R: RuleType> StringLiteralParser<R> {
    /// Parses the given string literal `pairs` into string.
    pub fn parse(&self, pairs: Pairs<R>) -> String {
        let mut result = String::new();
        for part in pairs {
            if part.as_rule() == self.content_rule {
                result.push_str(part.as_str());
            } else if part.as_rule() == self.escape_rule {
                match &part.as_str()[1..] {
                    "\"" => result.push('"'),
                    "\\" => result.push('\\'),
                    "t" => result.push('\t'),
                    "r" => result.push('\r'),
                    "n" => result.push('\n'),
                    "0" => result.push('\0'),
                    char => panic!("invalid escape: \\{char:?}"),
                }
            } else {
                panic!("unexpected part of string: {part:?}");
            }
        }
        result
    }
}

/// Map of symbol and function aliases.
#[derive(Clone, Debug, Default)]
pub struct AliasesMap<P> {
    symbol_aliases: HashMap<String, String>,
    function_aliases: HashMap<String, (Vec<String>, String)>,
    // Parser type P helps prevent misuse of AliasesMap of different language.
    parser: P,
}

impl<P> AliasesMap<P> {
    /// Creates an empty aliases map with default-constructed parser.
    pub fn new() -> Self
    where
        P: Default,
    {
        Self::default()
    }

    /// Adds new substitution rule `decl = defn`.
    ///
    /// Returns error if `decl` is invalid. The `defn` part isn't checked. A bad
    /// `defn` will be reported when the alias is substituted.
    pub fn insert(&mut self, decl: impl AsRef<str>, defn: impl Into<String>) -> Result<(), P::Error>
    where
        P: AliasDeclarationParser,
    {
        match self.parser.parse_declaration(decl.as_ref())? {
            AliasDeclaration::Symbol(name) => {
                self.symbol_aliases.insert(name, defn.into());
            }
            AliasDeclaration::Function(name, params) => {
                self.function_aliases.insert(name, (params, defn.into()));
            }
        }
        Ok(())
    }

    /// Iterates symbol names in arbitrary order.
    pub fn symbol_names(&self) -> impl Iterator<Item = &str> {
        self.symbol_aliases.keys().map(|n| n.as_ref())
    }

    /// Iterates function names in arbitrary order.
    pub fn function_names(&self) -> impl Iterator<Item = &str> {
        self.function_aliases.keys().map(|n| n.as_ref())
    }

    /// Looks up symbol alias by name. Returns identifier and definition text.
    pub fn get_symbol(&self, name: &str) -> Option<(AliasId<'_>, &str)> {
        self.symbol_aliases
            .get_key_value(name)
            .map(|(name, defn)| (AliasId::Symbol(name), defn.as_ref()))
    }

    /// Looks up function alias by name. Returns identifier, list of parameter
    /// names, and definition text.
    pub fn get_function(&self, name: &str) -> Option<(AliasId<'_>, &[String], &str)> {
        self.function_aliases
            .get_key_value(name)
            .map(|(name, (params, defn))| (AliasId::Function(name), params.as_ref(), defn.as_ref()))
    }
}

/// Borrowed reference to identify alias expression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AliasId<'a> {
    /// Symbol name.
    Symbol(&'a str),
    /// Function name.
    Function(&'a str),
    /// Function parameter name.
    Parameter(&'a str),
}

impl fmt::Display for AliasId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AliasId::Symbol(name) => write!(f, "{name}"),
            AliasId::Function(name) => write!(f, "{name}()"),
            AliasId::Parameter(name) => write!(f, "{name}"),
        }
    }
}

/// Parsed declaration part of alias rule.
#[derive(Clone, Debug)]
pub enum AliasDeclaration {
    /// Symbol name.
    Symbol(String),
    /// Function name and parameters.
    Function(String, Vec<String>),
}

// AliasDeclarationParser and AliasDefinitionParser can be merged into a single
// trait, but it's unclear whether doing that would simplify the abstraction.
// For now, they have to be separate traits because revset isn't migrated to
// ExpressionNode tree yet.

/// Parser for symbol and function alias declaration.
pub trait AliasDeclarationParser {
    /// Parse error type.
    type Error;

    /// Parses symbol or function name and parameters.
    fn parse_declaration(&self, source: &str) -> Result<AliasDeclaration, Self::Error>;
}

/// Parser for symbol and function alias definition.
pub trait AliasDefinitionParser {
    /// Expression item type.
    type Output<'i>;
    /// Parse error type.
    type Error;

    /// Parses alias body.
    fn parse_definition<'i>(
        &self,
        source: &'i str,
    ) -> Result<ExpressionNode<'i, Self::Output<'i>>, Self::Error>;
}

/// Expression item that supports alias substitution.
pub trait AliasExpandableExpression<'i>: FoldableExpression<'i> {
    /// Wraps identifier.
    fn identifier(name: &'i str) -> Self;
    /// Wraps function call.
    fn function_call(function: Box<FunctionCallNode<'i, Self>>) -> Self;
    /// Wraps substituted expression.
    fn alias_expanded(id: AliasId<'i>, subst: Box<ExpressionNode<'i, Self>>) -> Self;
}

/// Error that may occur during alias substitution.
pub trait AliasExpandError: Sized {
    /// Unexpected number of arguments, or invalid combination of arguments.
    fn invalid_arguments(err: InvalidArguments<'_>) -> Self;
    /// Recursion detected during alias substitution.
    fn recursive_expansion(id: AliasId<'_>, span: pest::Span<'_>) -> Self;
    /// Attaches alias trace to the current error.
    fn within_alias_expansion(self, id: AliasId<'_>, span: pest::Span<'_>) -> Self;
}

/// Expands aliases recursively in tree of `T`.
#[derive(Debug)]
struct AliasExpander<'i, T, P> {
    /// Alias symbols and functions that are globally available.
    aliases_map: &'i AliasesMap<P>,
    /// Stack of aliases and local parameters currently expanding.
    states: Vec<AliasExpandingState<'i, T>>,
}

#[derive(Debug)]
struct AliasExpandingState<'i, T> {
    id: AliasId<'i>,
    locals: HashMap<&'i str, ExpressionNode<'i, T>>,
}

impl<'i, T, P, E> AliasExpander<'i, T, P>
where
    T: AliasExpandableExpression<'i> + Clone,
    P: AliasDefinitionParser<Output<'i> = T, Error = E>,
    E: AliasExpandError,
{
    fn expand_defn(
        &mut self,
        id: AliasId<'i>,
        defn: &'i str,
        locals: HashMap<&'i str, ExpressionNode<'i, T>>,
        span: pest::Span<'i>,
    ) -> Result<T, E> {
        // The stack should be short, so let's simply do linear search.
        if self.states.iter().any(|s| s.id == id) {
            return Err(E::recursive_expansion(id, span));
        }
        self.states.push(AliasExpandingState { id, locals });
        // Parsed defn could be cached if needed.
        let result = self
            .aliases_map
            .parser
            .parse_definition(defn)
            .and_then(|node| self.fold_expression(node))
            .map(|node| T::alias_expanded(id, Box::new(node)))
            .map_err(|e| e.within_alias_expansion(id, span));
        self.states.pop();
        result
    }
}

impl<'i, T, P, E> ExpressionFolder<'i, T> for AliasExpander<'i, T, P>
where
    T: AliasExpandableExpression<'i> + Clone,
    P: AliasDefinitionParser<Output<'i> = T, Error = E>,
    E: AliasExpandError,
{
    type Error = E;

    fn fold_identifier(&mut self, name: &'i str, span: pest::Span<'i>) -> Result<T, Self::Error> {
        if let Some(subst) = self.states.last().and_then(|s| s.locals.get(name)) {
            let id = AliasId::Parameter(name);
            Ok(T::alias_expanded(id, Box::new(subst.clone())))
        } else if let Some((id, defn)) = self.aliases_map.get_symbol(name) {
            let locals = HashMap::new(); // Don't spill out the current scope
            self.expand_defn(id, defn, locals, span)
        } else {
            Ok(T::identifier(name))
        }
    }

    fn fold_function_call(
        &mut self,
        function: Box<FunctionCallNode<'i, T>>,
        span: pest::Span<'i>,
    ) -> Result<T, Self::Error> {
        if let Some((id, params, defn)) = self.aliases_map.get_function(function.name) {
            let arity = params.len();
            if function.args.len() != arity {
                return Err(E::invalid_arguments(
                    function.invalid_arguments_count(arity, Some(arity)),
                ));
            }
            // Resolve arguments in the current scope, and pass them in to the alias
            // expansion scope.
            let args = fold_expression_nodes(self, function.args)?;
            let locals = params.iter().map(|s| s.as_str()).zip(args).collect();
            self.expand_defn(id, defn, locals, span)
        } else {
            let function = Box::new(fold_function_call_args(self, *function)?);
            Ok(T::function_call(function))
        }
    }
}

/// Expands aliases recursively.
pub fn expand_aliases<'i, T, P>(
    node: ExpressionNode<'i, T>,
    aliases_map: &'i AliasesMap<P>,
) -> Result<ExpressionNode<'i, T>, P::Error>
where
    T: AliasExpandableExpression<'i> + Clone,
    P: AliasDefinitionParser<Output<'i> = T>,
    P::Error: AliasExpandError,
{
    let mut expander = AliasExpander {
        aliases_map,
        states: Vec::new(),
    };
    expander.fold_expression(node)
}

/// Collects similar names from the `candidates` list.
pub fn collect_similar<I>(name: &str, candidates: I) -> Vec<String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    candidates
        .into_iter()
        .filter(|cand| {
            // The parameter is borrowed from clap f5540d26
            strsim::jaro(name, cand.as_ref()) > 0.7
        })
        .map(|s| s.as_ref().to_owned())
        .sorted_unstable()
        .dedup()
        .collect()
}
