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

//! Parser for the fileset language.

use std::error;

use itertools::Itertools as _;
use once_cell::sync::Lazy;
use pest::iterators::Pair;
use pest::pratt_parser::{Assoc, Op, PrattParser};
use pest::Parser;
use pest_derive::Parser;
use thiserror::Error;

use crate::dsl_util::{self, InvalidArguments, StringLiteralParser};

#[derive(Parser)]
#[grammar = "fileset.pest"]
struct FilesetParser;

const STRING_LITERAL_PARSER: StringLiteralParser<Rule> = StringLiteralParser {
    content_rule: Rule::string_content,
    escape_rule: Rule::string_escape,
};

impl Rule {
    fn to_symbol(self) -> Option<&'static str> {
        match self {
            Rule::EOI => None,
            Rule::whitespace => None,
            Rule::identifier => None,
            Rule::strict_identifier_part => None,
            Rule::strict_identifier => None,
            Rule::bare_string => None,
            Rule::string_escape => None,
            Rule::string_content_char => None,
            Rule::string_content => None,
            Rule::string_literal => None,
            Rule::raw_string_content => None,
            Rule::raw_string_literal => None,
            Rule::pattern_kind_op => Some(":"),
            Rule::negate_op => Some("~"),
            Rule::union_op => Some("|"),
            Rule::intersection_op => Some("&"),
            Rule::difference_op => Some("~"),
            Rule::prefix_ops => None,
            Rule::infix_ops => None,
            Rule::function => None,
            Rule::function_name => None,
            Rule::function_arguments => None,
            Rule::string_pattern => None,
            Rule::bare_string_pattern => None,
            Rule::primary => None,
            Rule::expression => None,
            Rule::program => None,
            Rule::program_or_bare_string => None,
        }
    }
}

/// Result of fileset parsing and name resolution.
pub type FilesetParseResult<T> = Result<T, FilesetParseError>;

/// Error occurred during fileset parsing and name resolution.
#[derive(Debug, Error)]
#[error("{pest_error}")]
pub struct FilesetParseError {
    kind: FilesetParseErrorKind,
    pest_error: Box<pest::error::Error<Rule>>,
    source: Option<Box<dyn error::Error + Send + Sync>>,
}

/// Categories of fileset parsing and name resolution error.
#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum FilesetParseErrorKind {
    #[error("Syntax error")]
    SyntaxError,
    #[error(r#"Function "{name}" doesn't exist"#)]
    NoSuchFunction {
        name: String,
        candidates: Vec<String>,
    },
    #[error(r#"Function "{name}": {message}"#)]
    InvalidArguments { name: String, message: String },
    #[error("{0}")]
    Expression(String),
}

impl FilesetParseError {
    pub(super) fn new(kind: FilesetParseErrorKind, span: pest::Span<'_>) -> Self {
        let message = kind.to_string();
        let pest_error = Box::new(pest::error::Error::new_from_span(
            pest::error::ErrorVariant::CustomError { message },
            span,
        ));
        FilesetParseError {
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
    pub(super) fn expression(message: impl Into<String>, span: pest::Span<'_>) -> Self {
        FilesetParseError::new(FilesetParseErrorKind::Expression(message.into()), span)
    }

    /// Category of the underlying error.
    pub fn kind(&self) -> &FilesetParseErrorKind {
        &self.kind
    }
}

impl From<pest::error::Error<Rule>> for FilesetParseError {
    fn from(err: pest::error::Error<Rule>) -> Self {
        FilesetParseError {
            kind: FilesetParseErrorKind::SyntaxError,
            pest_error: Box::new(rename_rules_in_pest_error(err)),
            source: None,
        }
    }
}

impl From<InvalidArguments<'_>> for FilesetParseError {
    fn from(err: InvalidArguments<'_>) -> Self {
        let kind = FilesetParseErrorKind::InvalidArguments {
            name: err.name.to_owned(),
            message: err.message,
        };
        Self::new(kind, err.span)
    }
}

fn rename_rules_in_pest_error(err: pest::error::Error<Rule>) -> pest::error::Error<Rule> {
    err.renamed_rules(|rule| {
        rule.to_symbol()
            .map(|sym| format!("`{sym}`"))
            .unwrap_or_else(|| format!("<{rule:?}>"))
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpressionKind<'i> {
    Identifier(&'i str),
    String(String),
    StringPattern {
        kind: &'i str,
        value: String,
    },
    Unary(UnaryOp, Box<ExpressionNode<'i>>),
    Binary(BinaryOp, Box<ExpressionNode<'i>>, Box<ExpressionNode<'i>>),
    /// `x | y | ..`
    UnionAll(Vec<ExpressionNode<'i>>),
    FunctionCall(Box<FunctionCallNode<'i>>),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UnaryOp {
    /// `~`
    Negate,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BinaryOp {
    /// `&`
    Intersection,
    /// `~`
    Difference,
}

pub type ExpressionNode<'i> = dsl_util::ExpressionNode<'i, ExpressionKind<'i>>;
pub type FunctionCallNode<'i> = dsl_util::FunctionCallNode<'i, ExpressionKind<'i>>;

fn union_nodes<'i>(lhs: ExpressionNode<'i>, rhs: ExpressionNode<'i>) -> ExpressionNode<'i> {
    let span = lhs.span.start_pos().span(&rhs.span.end_pos());
    let expr = match lhs.kind {
        // Flatten "x | y | z" to save recursion stack. Machine-generated query
        // might have long chain of unions.
        ExpressionKind::UnionAll(mut nodes) => {
            nodes.push(rhs);
            ExpressionKind::UnionAll(nodes)
        }
        _ => ExpressionKind::UnionAll(vec![lhs, rhs]),
    };
    ExpressionNode::new(expr, span)
}

fn parse_function_call_node(pair: Pair<Rule>) -> FilesetParseResult<FunctionCallNode> {
    assert_eq!(pair.as_rule(), Rule::function);
    let (name_pair, args_pair) = pair.into_inner().collect_tuple().unwrap();
    assert_eq!(name_pair.as_rule(), Rule::function_name);
    assert_eq!(args_pair.as_rule(), Rule::function_arguments);
    let name_span = name_pair.as_span();
    let args_span = args_pair.as_span();
    let name = name_pair.as_str();
    let args = args_pair
        .into_inner()
        .map(parse_expression_node)
        .try_collect()?;
    Ok(FunctionCallNode {
        name,
        name_span,
        args,
        keyword_args: vec![], // unsupported
        args_span,
    })
}

fn parse_as_string_literal(pair: Pair<Rule>) -> String {
    match pair.as_rule() {
        Rule::identifier => pair.as_str().to_owned(),
        Rule::string_literal => STRING_LITERAL_PARSER.parse(pair.into_inner()),
        Rule::raw_string_literal => {
            let (content,) = pair.into_inner().collect_tuple().unwrap();
            assert_eq!(content.as_rule(), Rule::raw_string_content);
            content.as_str().to_owned()
        }
        r => panic!("unexpected string literal rule: {r:?}"),
    }
}

fn parse_primary_node(pair: Pair<Rule>) -> FilesetParseResult<ExpressionNode> {
    assert_eq!(pair.as_rule(), Rule::primary);
    let first = pair.into_inner().next().unwrap();
    let span = first.as_span();
    let expr = match first.as_rule() {
        Rule::expression => return parse_expression_node(first),
        Rule::function => {
            let function = Box::new(parse_function_call_node(first)?);
            ExpressionKind::FunctionCall(function)
        }
        Rule::string_pattern => {
            let (lhs, op, rhs) = first.into_inner().collect_tuple().unwrap();
            assert_eq!(lhs.as_rule(), Rule::strict_identifier);
            assert_eq!(op.as_rule(), Rule::pattern_kind_op);
            let kind = lhs.as_str();
            let value = parse_as_string_literal(rhs);
            ExpressionKind::StringPattern { kind, value }
        }
        Rule::identifier => ExpressionKind::Identifier(first.as_str()),
        Rule::string_literal | Rule::raw_string_literal => {
            ExpressionKind::String(parse_as_string_literal(first))
        }
        r => panic!("unexpected primary rule: {r:?}"),
    };
    Ok(ExpressionNode::new(expr, span))
}

fn parse_expression_node(pair: Pair<Rule>) -> FilesetParseResult<ExpressionNode> {
    assert_eq!(pair.as_rule(), Rule::expression);
    static PRATT: Lazy<PrattParser<Rule>> = Lazy::new(|| {
        PrattParser::new()
            .op(Op::infix(Rule::union_op, Assoc::Left))
            .op(Op::infix(Rule::intersection_op, Assoc::Left)
                | Op::infix(Rule::difference_op, Assoc::Left))
            .op(Op::prefix(Rule::negate_op))
    });
    PRATT
        .map_primary(parse_primary_node)
        .map_prefix(|op, rhs| {
            let op_kind = match op.as_rule() {
                Rule::negate_op => UnaryOp::Negate,
                r => panic!("unexpected prefix operator rule {r:?}"),
            };
            let rhs = Box::new(rhs?);
            let span = op.as_span().start_pos().span(&rhs.span.end_pos());
            let expr = ExpressionKind::Unary(op_kind, rhs);
            Ok(ExpressionNode::new(expr, span))
        })
        .map_infix(|lhs, op, rhs| {
            let op_kind = match op.as_rule() {
                Rule::union_op => return Ok(union_nodes(lhs?, rhs?)),
                Rule::intersection_op => BinaryOp::Intersection,
                Rule::difference_op => BinaryOp::Difference,
                r => panic!("unexpected infix operator rule {r:?}"),
            };
            let lhs = Box::new(lhs?);
            let rhs = Box::new(rhs?);
            let span = lhs.span.start_pos().span(&rhs.span.end_pos());
            let expr = ExpressionKind::Binary(op_kind, lhs, rhs);
            Ok(ExpressionNode::new(expr, span))
        })
        .parse(pair.into_inner())
}

/// Parses text into expression tree. No name resolution is made at this stage.
pub fn parse_program(text: &str) -> FilesetParseResult<ExpressionNode> {
    let mut pairs = FilesetParser::parse(Rule::program, text)?;
    let first = pairs.next().unwrap();
    parse_expression_node(first)
}

/// Parses text into expression tree with bare string fallback. No name
/// resolution is made at this stage.
///
/// If the text can't be parsed as a fileset expression, and if it doesn't
/// contain any operator-like characters, it will be parsed as a file path.
pub fn parse_program_or_bare_string(text: &str) -> FilesetParseResult<ExpressionNode> {
    let mut pairs = FilesetParser::parse(Rule::program_or_bare_string, text)?;
    let first = pairs.next().unwrap();
    let span = first.as_span();
    let expr = match first.as_rule() {
        Rule::expression => return parse_expression_node(first),
        Rule::bare_string_pattern => {
            let (lhs, op, rhs) = first.into_inner().collect_tuple().unwrap();
            assert_eq!(lhs.as_rule(), Rule::strict_identifier);
            assert_eq!(op.as_rule(), Rule::pattern_kind_op);
            assert_eq!(rhs.as_rule(), Rule::bare_string);
            let kind = lhs.as_str();
            let value = rhs.as_str().to_owned();
            ExpressionKind::StringPattern { kind, value }
        }
        Rule::bare_string => ExpressionKind::String(first.as_str().to_owned()),
        r => panic!("unexpected program or bare string rule: {r:?}"),
    };
    Ok(ExpressionNode::new(expr, span))
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;
    use crate::dsl_util::KeywordArgument;

    fn parse_into_kind(text: &str) -> Result<ExpressionKind, FilesetParseErrorKind> {
        parse_program(text)
            .map(|node| node.kind)
            .map_err(|err| err.kind)
    }

    fn parse_maybe_bare_into_kind(text: &str) -> Result<ExpressionKind, FilesetParseErrorKind> {
        parse_program_or_bare_string(text)
            .map(|node| node.kind)
            .map_err(|err| err.kind)
    }

    fn parse_normalized(text: &str) -> ExpressionNode {
        normalize_tree(parse_program(text).unwrap())
    }

    fn parse_maybe_bare_normalized(text: &str) -> ExpressionNode {
        normalize_tree(parse_program_or_bare_string(text).unwrap())
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
            | ExpressionKind::StringPattern { .. } => node.kind,
            ExpressionKind::Unary(op, arg) => {
                let arg = Box::new(normalize_tree(*arg));
                ExpressionKind::Unary(op, arg)
            }
            ExpressionKind::Binary(op, lhs, rhs) => {
                let lhs = Box::new(normalize_tree(*lhs));
                let rhs = Box::new(normalize_tree(*rhs));
                ExpressionKind::Binary(op, lhs, rhs)
            }
            ExpressionKind::UnionAll(nodes) => {
                let nodes = normalize_list(nodes);
                ExpressionKind::UnionAll(nodes)
            }
            ExpressionKind::FunctionCall(function) => {
                let function = Box::new(normalize_function_call(*function));
                ExpressionKind::FunctionCall(function)
            }
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
    fn test_parse_whitespace() {
        let ascii_whitespaces: String = ('\x00'..='\x7f')
            .filter(char::is_ascii_whitespace)
            .collect();
        assert_eq!(
            parse_normalized(&format!("{ascii_whitespaces}f()")),
            parse_normalized("f()")
        );
    }

    #[test]
    fn test_parse_identifier() {
        assert_eq!(
            parse_into_kind("dir/foo-bar_0.baz"),
            Ok(ExpressionKind::Identifier("dir/foo-bar_0.baz"))
        );
        assert_eq!(
            parse_into_kind("cli-reference@.md.snap"),
            Ok(ExpressionKind::Identifier("cli-reference@.md.snap"))
        );
        assert_eq!(
            parse_into_kind("柔術.jj"),
            Ok(ExpressionKind::Identifier("柔術.jj"))
        );
        assert_eq!(
            parse_into_kind(r#"Windows\Path"#),
            Ok(ExpressionKind::Identifier(r#"Windows\Path"#))
        );
        assert_eq!(
            parse_into_kind("glob*[chars]?"),
            Ok(ExpressionKind::Identifier("glob*[chars]?"))
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
            Err(FilesetParseErrorKind::SyntaxError)
        );

        // Single-quoted raw string
        assert_eq!(
            parse_into_kind(r#" '' "#),
            Ok(ExpressionKind::String("".to_owned())),
        );
        assert_eq!(
            parse_into_kind(r#" 'a\n' "#),
            Ok(ExpressionKind::String(r"a\n".to_owned())),
        );
        assert_eq!(
            parse_into_kind(r#" '\' "#),
            Ok(ExpressionKind::String(r"\".to_owned())),
        );
        assert_eq!(
            parse_into_kind(r#" '"' "#),
            Ok(ExpressionKind::String(r#"""#.to_owned())),
        );
    }

    #[test]
    fn test_parse_string_pattern() {
        assert_eq!(
            parse_into_kind(r#" foo:bar "#),
            Ok(ExpressionKind::StringPattern {
                kind: "foo",
                value: "bar".to_owned()
            })
        );
        assert_eq!(
            parse_into_kind(" foo:glob*[chars]? "),
            Ok(ExpressionKind::StringPattern {
                kind: "foo",
                value: "glob*[chars]?".to_owned()
            })
        );
        assert_eq!(
            parse_into_kind(r#" foo:"bar" "#),
            Ok(ExpressionKind::StringPattern {
                kind: "foo",
                value: "bar".to_owned()
            })
        );
        assert_eq!(
            parse_into_kind(r#" foo:"" "#),
            Ok(ExpressionKind::StringPattern {
                kind: "foo",
                value: "".to_owned()
            })
        );
        assert_eq!(
            parse_into_kind(r#" foo:'\' "#),
            Ok(ExpressionKind::StringPattern {
                kind: "foo",
                value: r"\".to_owned()
            })
        );
        assert_eq!(
            parse_into_kind(r#" foo: "#),
            Err(FilesetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind(r#" foo: "" "#),
            Err(FilesetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_into_kind(r#" foo :"" "#),
            Err(FilesetParseErrorKind::SyntaxError)
        );
    }

    #[test]
    fn test_parse_operator() {
        assert_matches!(
            parse_into_kind("~x"),
            Ok(ExpressionKind::Unary(UnaryOp::Negate, _))
        );
        assert_matches!(
            parse_into_kind("x|y"),
            Ok(ExpressionKind::UnionAll(nodes)) if nodes.len() == 2
        );
        assert_matches!(
            parse_into_kind("x|y|z"),
            Ok(ExpressionKind::UnionAll(nodes)) if nodes.len() == 3
        );
        assert_matches!(
            parse_into_kind("x&y"),
            Ok(ExpressionKind::Binary(BinaryOp::Intersection, _, _))
        );
        assert_matches!(
            parse_into_kind("x~y"),
            Ok(ExpressionKind::Binary(BinaryOp::Difference, _, _))
        );

        // Set operator associativity/precedence
        assert_eq!(parse_normalized("~x|y"), parse_normalized("(~x)|y"));
        assert_eq!(parse_normalized("x&~y"), parse_normalized("x&(~y)"));
        assert_eq!(parse_normalized("x~~y"), parse_normalized("x~(~y)"));
        assert_eq!(parse_normalized("x~~~y"), parse_normalized("x~(~(~y))"));
        assert_eq!(parse_normalized("x|y|z"), parse_normalized("(x|y)|z"));
        assert_eq!(parse_normalized("x&y|z"), parse_normalized("(x&y)|z"));
        assert_eq!(parse_normalized("x|y&z"), parse_normalized("x|(y&z)"));
        assert_eq!(parse_normalized("x|y~z"), parse_normalized("x|(y~z)"));
        assert_eq!(parse_normalized("~x:y"), parse_normalized("~(x:y)"));
        assert_eq!(parse_normalized("x|y:z"), parse_normalized("x|(y:z)"));

        // Expression span
        assert_eq!(parse_program(" ~ x ").unwrap().span.as_str(), "~ x");
        assert_eq!(parse_program(" x |y ").unwrap().span.as_str(), "x |y");
    }

    #[test]
    fn test_parse_function_call() {
        assert_matches!(
            parse_into_kind("foo()"),
            Ok(ExpressionKind::FunctionCall(_))
        );

        // Trailing comma isn't allowed for empty argument
        assert!(parse_into_kind("foo(,)").is_err());

        // Trailing comma is allowed for the last argument
        assert_eq!(parse_normalized("foo(a,)"), parse_normalized("foo(a)"));
        assert_eq!(parse_normalized("foo(a ,  )"), parse_normalized("foo(a)"));
        assert!(parse_into_kind("foo(,a)").is_err());
        assert!(parse_into_kind("foo(a,,)").is_err());
        assert!(parse_into_kind("foo(a  , , )").is_err());
        assert_eq!(parse_normalized("foo(a,b,)"), parse_normalized("foo(a,b)"));
        assert!(parse_into_kind("foo(a,,b)").is_err());
    }

    #[test]
    fn test_parse_bare_string() {
        // Valid expression should be parsed as such
        assert_eq!(
            parse_maybe_bare_into_kind(" valid "),
            Ok(ExpressionKind::Identifier("valid"))
        );
        assert_eq!(
            parse_maybe_bare_normalized("f(x)&y"),
            parse_normalized("f(x)&y")
        );

        // Bare string
        assert_eq!(
            parse_maybe_bare_into_kind("Foo Bar.txt"),
            Ok(ExpressionKind::String("Foo Bar.txt".to_owned()))
        );
        assert_eq!(
            parse_maybe_bare_into_kind(r#"Windows\Path with space"#),
            Ok(ExpressionKind::String(
                r#"Windows\Path with space"#.to_owned()
            ))
        );
        assert_eq!(
            parse_maybe_bare_into_kind("柔 術 . j j"),
            Ok(ExpressionKind::String("柔 術 . j j".to_owned()))
        );
        assert_eq!(
            parse_maybe_bare_into_kind("Unicode emoji 💩"),
            Ok(ExpressionKind::String("Unicode emoji 💩".to_owned()))
        );
        assert_eq!(
            parse_maybe_bare_into_kind("looks like & expression"),
            Err(FilesetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_maybe_bare_into_kind("unbalanced_parens("),
            Err(FilesetParseErrorKind::SyntaxError)
        );

        // Bare string pattern
        assert_eq!(
            parse_maybe_bare_into_kind("foo: bar baz"),
            Ok(ExpressionKind::StringPattern {
                kind: "foo",
                value: " bar baz".to_owned()
            })
        );
        assert_eq!(
            parse_maybe_bare_into_kind("foo:glob * [chars]?"),
            Ok(ExpressionKind::StringPattern {
                kind: "foo",
                value: "glob * [chars]?".to_owned()
            })
        );
        assert_eq!(
            parse_maybe_bare_into_kind("foo:bar:baz"),
            Err(FilesetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_maybe_bare_into_kind("foo:"),
            Err(FilesetParseErrorKind::SyntaxError)
        );
        assert_eq!(
            parse_maybe_bare_into_kind(r#"foo:"unclosed quote"#),
            Err(FilesetParseErrorKind::SyntaxError)
        );

        // Surrounding spaces are simply preserved. They could be trimmed, but
        // space is valid bare_string character.
        assert_eq!(
            parse_maybe_bare_into_kind(" No trim "),
            Ok(ExpressionKind::String(" No trim ".to_owned()))
        );
    }

    #[test]
    fn test_parse_error() {
        insta::assert_snapshot!(parse_program("foo|").unwrap_err().to_string(), @r###"
         --> 1:5
          |
        1 | foo|
          |     ^---
          |
          = expected `~` or <primary>
        "###);
    }
}
