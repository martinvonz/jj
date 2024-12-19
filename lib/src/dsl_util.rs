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

use std::array;
use std::collections::HashMap;
use std::fmt;
use std::slice;

use itertools::Itertools as _;
use pest::iterators::Pair;
use pest::iterators::Pairs;
use pest::RuleType;

/// Manages diagnostic messages emitted during parsing.
///
/// `T` is usually a parse error type of the language, which contains a message
/// and source span of 'static lifetime.
#[derive(Debug)]
pub struct Diagnostics<T> {
    // This might be extended to [{ kind: Warning|Error, message: T }, ..].
    diagnostics: Vec<T>,
}

impl<T> Diagnostics<T> {
    /// Creates new empty diagnostics collector.
    pub fn new() -> Self {
        Diagnostics {
            diagnostics: Vec::new(),
        }
    }

    /// Returns `true` if there are no diagnostic messages.
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// Returns the number of diagnostic messages.
    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    /// Returns iterator over diagnostic messages.
    pub fn iter(&self) -> slice::Iter<'_, T> {
        self.diagnostics.iter()
    }

    /// Adds a diagnostic message of warning level.
    pub fn add_warning(&mut self, diag: T) {
        self.diagnostics.push(diag);
    }

    /// Moves diagnostic messages of different type (such as fileset warnings
    /// emitted within `file()` revset.)
    pub fn extend_with<U>(&mut self, diagnostics: Diagnostics<U>, mut f: impl FnMut(U) -> T) {
        self.diagnostics
            .extend(diagnostics.diagnostics.into_iter().map(&mut f));
    }
}

impl<T> Default for Diagnostics<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, T> IntoIterator for &'a Diagnostics<T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

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
    /// List of keyword arguments.
    pub keyword_args: Vec<KeywordArgument<'i, T>>,
    /// Span of the arguments list.
    pub args_span: pest::Span<'i>,
}

/// Keyword argument pair in AST.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeywordArgument<'i, T> {
    /// Parameter name.
    pub name: &'i str,
    /// Span of the parameter name.
    pub name_span: pest::Span<'i>,
    /// Value expression.
    pub value: ExpressionNode<'i, T>,
}

impl<'i, T> FunctionCallNode<'i, T> {
    /// Number of arguments assuming named arguments are all unique.
    pub fn arity(&self) -> usize {
        self.args.len() + self.keyword_args.len()
    }

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
        self.ensure_no_keyword_arguments()?;
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
        self.ensure_no_keyword_arguments()?;
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

    /// Extracts N required arguments and M optional arguments. Some of them can
    /// be specified as keyword arguments.
    ///
    /// `names` is a list of parameter names. Unnamed positional arguments
    /// should be padded with `""`.
    #[allow(clippy::type_complexity)]
    pub fn expect_named_arguments<const N: usize, const M: usize>(
        &self,
        names: &[&str],
    ) -> Result<
        (
            [&ExpressionNode<'i, T>; N],
            [Option<&ExpressionNode<'i, T>>; M],
        ),
        InvalidArguments<'i>,
    > {
        if self.keyword_args.is_empty() {
            let (required, optional) = self.expect_arguments::<N, M>()?;
            // TODO: use .each_ref() if MSRV is bumped to 1.77.0
            Ok((array::from_fn(|i| &required[i]), optional))
        } else {
            let (required, optional) = self.expect_named_arguments_vec(names, N, N + M)?;
            Ok((
                required.try_into().ok().unwrap(),
                optional.try_into().ok().unwrap(),
            ))
        }
    }

    #[allow(clippy::type_complexity)]
    fn expect_named_arguments_vec(
        &self,
        names: &[&str],
        min: usize,
        max: usize,
    ) -> Result<
        (
            Vec<&ExpressionNode<'i, T>>,
            Vec<Option<&ExpressionNode<'i, T>>>,
        ),
        InvalidArguments<'i>,
    > {
        assert!(names.len() <= max);

        if self.args.len() > max {
            return Err(self.invalid_arguments_count(min, Some(max)));
        }
        let mut extracted = Vec::with_capacity(max);
        extracted.extend(self.args.iter().map(Some));
        extracted.resize(max, None);

        for arg in &self.keyword_args {
            let name = arg.name;
            let span = arg.name_span.start_pos().span(&arg.value.span.end_pos());
            let pos = names.iter().position(|&n| n == name).ok_or_else(|| {
                self.invalid_arguments(format!(r#"Unexpected keyword argument "{name}""#), span)
            })?;
            if extracted[pos].is_some() {
                return Err(self.invalid_arguments(
                    format!(r#"Got multiple values for keyword "{name}""#),
                    span,
                ));
            }
            extracted[pos] = Some(&arg.value);
        }

        let optional = extracted.split_off(min);
        let required = extracted.into_iter().flatten().collect_vec();
        if required.len() != min {
            return Err(self.invalid_arguments_count(min, Some(max)));
        }
        Ok((required, optional))
    }

    fn ensure_no_keyword_arguments(&self) -> Result<(), InvalidArguments<'i>> {
        if let (Some(first), Some(last)) = (self.keyword_args.first(), self.keyword_args.last()) {
            let span = first.name_span.start_pos().span(&last.value.span.end_pos());
            Err(self.invalid_arguments("Unexpected keyword arguments".to_owned(), span))
        } else {
            Ok(())
        }
    }

    fn invalid_arguments(&self, message: String, span: pest::Span<'i>) -> InvalidArguments<'i> {
        InvalidArguments {
            name: self.name,
            message,
            span,
        }
    }

    fn invalid_arguments_count(&self, min: usize, max: Option<usize>) -> InvalidArguments<'i> {
        let message = match (min, max) {
            (min, Some(max)) if min == max => format!("Expected {min} arguments"),
            (min, Some(max)) => format!("Expected {min} to {max} arguments"),
            (min, None) => format!("Expected at least {min} arguments"),
        };
        self.invalid_arguments(message, self.args_span)
    }

    fn invalid_arguments_count_with_arities(
        &self,
        arities: impl IntoIterator<Item = usize>,
    ) -> InvalidArguments<'i> {
        let message = format!("Expected {} arguments", arities.into_iter().join(", "));
        self.invalid_arguments(message, self.args_span)
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
        keyword_args: function
            .keyword_args
            .into_iter()
            .map(|arg| {
                Ok(KeywordArgument {
                    name: arg.name,
                    name_span: arg.name_span,
                    value: folder.fold_expression(arg.value)?,
                })
            })
            .try_collect()?,
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
                    "e" => result.push('\x1b'),
                    hex if hex.starts_with('x') => {
                        result.push(char::from(
                            u8::from_str_radix(&hex[1..], 16).expect("hex characters"),
                        ));
                    }
                    char => panic!("invalid escape: \\{char:?}"),
                }
            } else {
                panic!("unexpected part of string: {part:?}");
            }
        }
        result
    }
}

/// Helper to parse function call.
#[derive(Debug)]
pub struct FunctionCallParser<R> {
    /// Function name.
    pub function_name_rule: R,
    /// List of positional and keyword arguments.
    pub function_arguments_rule: R,
    /// Pair of parameter name and value.
    pub keyword_argument_rule: R,
    /// Parameter name.
    pub argument_name_rule: R,
    /// Value expression.
    pub argument_value_rule: R,
}

impl<R: RuleType> FunctionCallParser<R> {
    /// Parses the given `pair` as function call.
    pub fn parse<'i, T, E: From<InvalidArguments<'i>>>(
        &self,
        pair: Pair<'i, R>,
        // parse_name can be defined for any Pair<'_, R>, but parse_value should
        // be allowed to construct T by capturing Pair<'i, R>.
        parse_name: impl Fn(Pair<'i, R>) -> Result<&'i str, E>,
        parse_value: impl Fn(Pair<'i, R>) -> Result<ExpressionNode<'i, T>, E>,
    ) -> Result<FunctionCallNode<'i, T>, E> {
        let (name_pair, args_pair) = pair.into_inner().collect_tuple().unwrap();
        assert_eq!(name_pair.as_rule(), self.function_name_rule);
        assert_eq!(args_pair.as_rule(), self.function_arguments_rule);
        let name_span = name_pair.as_span();
        let args_span = args_pair.as_span();
        let function_name = parse_name(name_pair)?;
        let mut args = Vec::new();
        let mut keyword_args = Vec::new();
        for pair in args_pair.into_inner() {
            let span = pair.as_span();
            if pair.as_rule() == self.argument_value_rule {
                if !keyword_args.is_empty() {
                    return Err(InvalidArguments {
                        name: function_name,
                        message: "Positional argument follows keyword argument".to_owned(),
                        span,
                    }
                    .into());
                }
                args.push(parse_value(pair)?);
            } else if pair.as_rule() == self.keyword_argument_rule {
                let (name_pair, value_pair) = pair.into_inner().collect_tuple().unwrap();
                assert_eq!(name_pair.as_rule(), self.argument_name_rule);
                assert_eq!(value_pair.as_rule(), self.argument_value_rule);
                let name_span = name_pair.as_span();
                let arg = KeywordArgument {
                    name: parse_name(name_pair)?,
                    name_span,
                    value: parse_value(value_pair)?,
                };
                keyword_args.push(arg);
            } else {
                panic!("unexpected argument rule {pair:?}");
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
}

/// Map of symbol and function aliases.
#[derive(Clone, Debug, Default)]
pub struct AliasesMap<P, V> {
    symbol_aliases: HashMap<String, V>,
    // name: [(params, defn)] (sorted by arity)
    function_aliases: HashMap<String, Vec<(Vec<String>, V)>>,
    // Parser type P helps prevent misuse of AliasesMap of different language.
    parser: P,
}

impl<P, V> AliasesMap<P, V> {
    /// Creates an empty aliases map with default-constructed parser.
    pub fn new() -> Self
    where
        P: Default,
    {
        Self {
            symbol_aliases: Default::default(),
            function_aliases: Default::default(),
            parser: Default::default(),
        }
    }

    /// Adds new substitution rule `decl = defn`.
    ///
    /// Returns error if `decl` is invalid. The `defn` part isn't checked. A bad
    /// `defn` will be reported when the alias is substituted.
    pub fn insert(&mut self, decl: impl AsRef<str>, defn: impl Into<V>) -> Result<(), P::Error>
    where
        P: AliasDeclarationParser,
    {
        match self.parser.parse_declaration(decl.as_ref())? {
            AliasDeclaration::Symbol(name) => {
                self.symbol_aliases.insert(name, defn.into());
            }
            AliasDeclaration::Function(name, params) => {
                self.insert_function(name, params, defn.into());
            }
        }
        Ok(())
    }

    fn insert_function(&mut self, name: String, params: Vec<String>, defn: V) {
        let overloads = self.function_aliases.entry(name).or_default();
        match overloads.binary_search_by_key(&params.len(), |(params, _)| params.len()) {
            Ok(i) => overloads[i] = (params, defn),
            Err(i) => overloads.insert(i, (params, defn)),
        }
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
    pub fn get_symbol(&self, name: &str) -> Option<(AliasId<'_>, &V)> {
        self.symbol_aliases
            .get_key_value(name)
            .map(|(name, defn)| (AliasId::Symbol(name), defn))
    }

    /// Looks up function alias by name and arity. Returns identifier, list of
    /// parameter names, and definition text.
    pub fn get_function(&self, name: &str, arity: usize) -> Option<(AliasId<'_>, &[String], &V)> {
        let overloads = self.get_function_overloads(name)?;
        overloads.find_by_arity(arity)
    }

    /// Looks up a function alias by name and arity, assuming that the function
    /// can take extra parameters (eg. *args in python).
    /// Returns identifier, list of parameter names, and definition text.
    pub fn get_function_with_leftovers(
        &self,
        name: &str,
        arity: usize,
    ) -> Option<(AliasId<'_>, &[String], &V)> {
        let overloads = self.get_function_overloads(name)?;
        overloads.find_by_arity_with_leftovers(arity)
    }

    /// Looks up function aliases by name.
    fn get_function_overloads(&self, name: &str) -> Option<AliasFunctionOverloads<'_, V>> {
        let (name, overloads) = self.function_aliases.get_key_value(name)?;
        Some(AliasFunctionOverloads { name, overloads })
    }
}

#[derive(Clone, Debug)]
struct AliasFunctionOverloads<'a, V> {
    name: &'a String,
    overloads: &'a Vec<(Vec<String>, V)>,
}

impl<'a, V> AliasFunctionOverloads<'a, V> {
    fn arities(&self) -> impl DoubleEndedIterator<Item = usize> + ExactSizeIterator + 'a {
        self.overloads.iter().map(|(params, _)| params.len())
    }

    fn min_arity(&self) -> usize {
        self.arities().next().unwrap()
    }

    fn max_arity(&self) -> usize {
        self.arities().next_back().unwrap()
    }

    fn get_overload(&self, index: usize) -> (AliasId<'a>, &'a [String], &'a V) {
        let (params, defn) = &self.overloads[index];
        // Exact parameter names aren't needed to identify a function, but they
        // provide a better error indication. (e.g. "foo(x, y)" is easier to
        // follow than "foo/2".)
        (AliasId::Function(self.name, params), params, defn)
    }

    fn find_by_arity(&self, arity: usize) -> Option<(AliasId<'a>, &'a [String], &'a V)> {
        Some(
            self.get_overload(
                self.overloads
                    .binary_search_by_key(&arity, |(params, _)| params.len())
                    .ok()?,
            ),
        )
    }

    // `find_arity(3)` is equivalent to finding the correct overload for `fn(a, b, c, *args)`.
    // We need to find the longest match that is still less than arity.
    fn find_by_arity_with_leftovers(
        &self,
        arity: usize,
    ) -> Option<(AliasId<'a>, &'a [String], &'a V)> {
        if arity < self.min_arity() {
            // This is like calling `fn(a, b)` when the definition is `fn(a, b, c ,*args)`
            None
        } else {
            let first_invalid = self
                .overloads
                .partition_point(|(args, _)| arity >= args.len());
            Some(self.get_overload(first_invalid - 1))
        }
    }
}

/// Borrowed reference to identify alias expression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AliasId<'a> {
    /// Symbol name.
    Symbol(&'a str),
    /// Function name and parameter names.
    Function(&'a str, &'a [String]),
    /// Function parameter name.
    Parameter(&'a str),
}

impl fmt::Display for AliasId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AliasId::Symbol(name) => write!(f, "{name}"),
            AliasId::Function(name, params) => {
                write!(f, "{name}({params})", params = params.join(", "))
            }
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
    aliases_map: &'i AliasesMap<P, String>,
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
        // For better error indication, builtin functions are shadowed by name,
        // not by (name, arity).
        if let Some(overloads) = self.aliases_map.get_function_overloads(function.name) {
            // TODO: add support for keyword arguments
            function
                .ensure_no_keyword_arguments()
                .map_err(E::invalid_arguments)?;
            let Some((id, params, defn)) = overloads.find_by_arity(function.arity()) else {
                let min = overloads.min_arity();
                let max = overloads.max_arity();
                let err = if max - min + 1 == overloads.arities().len() {
                    function.invalid_arguments_count(min, Some(max))
                } else {
                    function.invalid_arguments_count_with_arities(overloads.arities())
                };
                return Err(E::invalid_arguments(err));
            };
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
    aliases_map: &'i AliasesMap<P, String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expect_arguments() {
        fn empty_span() -> pest::Span<'static> {
            pest::Span::new("", 0, 0).unwrap()
        }

        fn function(
            name: &'static str,
            args: impl Into<Vec<ExpressionNode<'static, u32>>>,
            keyword_args: impl Into<Vec<KeywordArgument<'static, u32>>>,
        ) -> FunctionCallNode<'static, u32> {
            FunctionCallNode {
                name,
                name_span: empty_span(),
                args: args.into(),
                keyword_args: keyword_args.into(),
                args_span: empty_span(),
            }
        }

        fn value(v: u32) -> ExpressionNode<'static, u32> {
            ExpressionNode::new(v, empty_span())
        }

        fn keyword(name: &'static str, v: u32) -> KeywordArgument<'static, u32> {
            KeywordArgument {
                name,
                name_span: empty_span(),
                value: value(v),
            }
        }

        let f = function("foo", [], []);
        assert!(f.expect_no_arguments().is_ok());
        assert!(f.expect_some_arguments::<0>().is_ok());
        assert!(f.expect_arguments::<0, 0>().is_ok());
        assert!(f.expect_named_arguments::<0, 0>(&[]).is_ok());

        let f = function("foo", [value(0)], []);
        assert!(f.expect_no_arguments().is_err());
        assert_eq!(
            f.expect_some_arguments::<0>().unwrap(),
            (&[], [value(0)].as_slice())
        );
        assert_eq!(
            f.expect_some_arguments::<1>().unwrap(),
            (&[value(0)], [].as_slice())
        );
        assert!(f.expect_arguments::<0, 0>().is_err());
        assert_eq!(
            f.expect_arguments::<0, 1>().unwrap(),
            (&[], [Some(&value(0))])
        );
        assert_eq!(f.expect_arguments::<1, 1>().unwrap(), (&[value(0)], [None]));
        assert!(f.expect_named_arguments::<0, 0>(&[]).is_err());
        assert_eq!(
            f.expect_named_arguments::<0, 1>(&["a"]).unwrap(),
            ([], [Some(&value(0))])
        );
        assert_eq!(
            f.expect_named_arguments::<1, 0>(&["a"]).unwrap(),
            ([&value(0)], [])
        );

        let f = function("foo", [], [keyword("a", 0)]);
        assert!(f.expect_no_arguments().is_err());
        assert!(f.expect_some_arguments::<1>().is_err());
        assert!(f.expect_arguments::<0, 1>().is_err());
        assert!(f.expect_arguments::<1, 0>().is_err());
        assert!(f.expect_named_arguments::<0, 0>(&[]).is_err());
        assert!(f.expect_named_arguments::<0, 1>(&[]).is_err());
        assert!(f.expect_named_arguments::<1, 0>(&[]).is_err());
        assert_eq!(
            f.expect_named_arguments::<1, 0>(&["a"]).unwrap(),
            ([&value(0)], [])
        );
        assert_eq!(
            f.expect_named_arguments::<1, 1>(&["a", "b"]).unwrap(),
            ([&value(0)], [None])
        );
        assert!(f.expect_named_arguments::<1, 1>(&["b", "a"]).is_err());

        let f = function("foo", [value(0)], [keyword("a", 1), keyword("b", 2)]);
        assert!(f.expect_named_arguments::<0, 0>(&[]).is_err());
        assert!(f.expect_named_arguments::<1, 1>(&["a", "b"]).is_err());
        assert_eq!(
            f.expect_named_arguments::<1, 2>(&["c", "a", "b"]).unwrap(),
            ([&value(0)], [Some(&value(1)), Some(&value(2))])
        );
        assert_eq!(
            f.expect_named_arguments::<2, 1>(&["c", "b", "a"]).unwrap(),
            ([&value(0), &value(2)], [Some(&value(1))])
        );
        assert_eq!(
            f.expect_named_arguments::<0, 3>(&["c", "b", "a"]).unwrap(),
            ([], [Some(&value(0)), Some(&value(2)), Some(&value(1))])
        );

        let f = function("foo", [], [keyword("a", 0), keyword("a", 1)]);
        assert!(f.expect_named_arguments::<1, 1>(&["", "a"]).is_err());
    }

    #[test]
    fn test_aliases_map_arity() {
        let mut aliases_map = AliasesMap::<(), i32>::default();
        aliases_map.insert_function("single".to_string(), vec![], 0);
        aliases_map.insert_function("overload".to_string(), vec!["first".to_string()], 1);
        aliases_map.insert_function(
            "overload".to_string(),
            vec![
                "first".to_string(),
                "second".to_string(),
                "third".to_string(),
            ],
            3,
        );

        let get_alias = |name, arity| {
            let (_, params, defn) = aliases_map.get_function_with_leftovers(name, arity)?;
            Some((params, *defn))
        };

        assert_eq!(get_alias("nonexistent", 1), None);
        assert_eq!(get_alias("single", 3), Some(([].as_slice(), 0)));
        assert_eq!(get_alias("overload", 0), None);

        assert_eq!(
            get_alias("overload", 1),
            Some((["first".to_string()].as_slice(), 1))
        );

        assert_eq!(
            get_alias("overload", 2),
            Some((["first".to_string()].as_slice(), 1))
        );

        assert_eq!(
            get_alias("overload", 3),
            Some((
                [
                    "first".to_string(),
                    "second".to_string(),
                    "third".to_string()
                ]
                .as_slice(),
                3
            ))
        );

        assert_eq!(
            get_alias("overload", 4),
            Some((
                [
                    "first".to_string(),
                    "second".to_string(),
                    "third".to_string()
                ]
                .as_slice(),
                3
            ))
        );
    }
}
