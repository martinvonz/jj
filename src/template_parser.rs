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

use std::ops::{RangeFrom, RangeInclusive};
use std::{error, fmt};

use itertools::Itertools as _;
use jujutsu_lib::backend::{Signature, Timestamp};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::op_store::WorkspaceId;
use jujutsu_lib::repo::RepoRef;
use jujutsu_lib::rewrite;
use pest::iterators::{Pair, Pairs};
use pest::Parser;
use pest_derive::Parser;
use thiserror::Error;

use crate::templater::{
    BranchProperty, CommitOrChangeId, ConditionalTemplate, FormattablePropertyTemplate,
    GitHeadProperty, GitRefsProperty, IdWithHighlightedPrefix, LabelTemplate, ListTemplate,
    Literal, PlainTextFormattedProperty, SeparateTemplate, TagProperty, Template, TemplateFunction,
    TemplateProperty, TemplatePropertyFn, WorkingCopiesProperty,
};
use crate::{cli_util, time_util};

#[derive(Parser)]
#[grammar = "template.pest"]
pub struct TemplateParser;

type TemplateParseResult<T> = Result<T, TemplateParseError>;

#[derive(Clone, Debug)]
pub struct TemplateParseError {
    kind: TemplateParseErrorKind,
    pest_error: Box<pest::error::Error<Rule>>,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum TemplateParseErrorKind {
    #[error("Syntax error")]
    SyntaxError,
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
}

impl TemplateParseError {
    fn with_span(kind: TemplateParseErrorKind, span: pest::Span<'_>) -> Self {
        let pest_error = Box::new(pest::error::Error::new_from_span(
            pest::error::ErrorVariant::CustomError {
                message: kind.to_string(),
            },
            span,
        ));
        TemplateParseError { kind, pest_error }
    }

    fn no_such_keyword(pair: &Pair<'_, Rule>) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::NoSuchKeyword(pair.as_str().to_owned()),
            pair.as_span(),
        )
    }

    fn no_such_function(pair: &Pair<'_, Rule>) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::NoSuchFunction(pair.as_str().to_owned()),
            pair.as_span(),
        )
    }

    fn no_such_method(type_name: impl Into<String>, pair: &Pair<'_, Rule>) -> Self {
        TemplateParseError::with_span(
            TemplateParseErrorKind::NoSuchMethod {
                type_name: type_name.into(),
                name: pair.as_str().to_owned(),
            },
            pair.as_span(),
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
}

impl From<pest::error::Error<Rule>> for TemplateParseError {
    fn from(err: pest::error::Error<Rule>) -> Self {
        TemplateParseError {
            kind: TemplateParseErrorKind::SyntaxError,
            pest_error: Box::new(err),
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
        match &self.kind {
            // SyntaxError is a wrapper for pest::error::Error.
            TemplateParseErrorKind::SyntaxError => Some(&self.pest_error as &dyn error::Error),
            // Otherwise the kind represents this error.
            e => e.source(),
        }
    }
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

enum Property<'a, I> {
    String(Box<dyn TemplateProperty<I, Output = String> + 'a>),
    Boolean(Box<dyn TemplateProperty<I, Output = bool> + 'a>),
    CommitOrChangeId(Box<dyn TemplateProperty<I, Output = CommitOrChangeId<'a>> + 'a>),
    IdWithHighlightedPrefix(Box<dyn TemplateProperty<I, Output = IdWithHighlightedPrefix> + 'a>),
    Signature(Box<dyn TemplateProperty<I, Output = Signature> + 'a>),
    Timestamp(Box<dyn TemplateProperty<I, Output = Timestamp> + 'a>),
}

impl<'a, I: 'a> Property<'a, I> {
    fn after<C: 'a>(self, first: Box<dyn TemplateProperty<C, Output = I> + 'a>) -> Property<'a, C> {
        fn chain<'a, C: 'a, I: 'a, O: 'a>(
            first: Box<dyn TemplateProperty<C, Output = I> + 'a>,
            second: Box<dyn TemplateProperty<I, Output = O> + 'a>,
        ) -> Box<dyn TemplateProperty<C, Output = O> + 'a> {
            Box::new(TemplateFunction::new(first, move |value| {
                second.extract(&value)
            }))
        }
        match self {
            Property::String(property) => Property::String(chain(first, property)),
            Property::Boolean(property) => Property::Boolean(chain(first, property)),
            Property::CommitOrChangeId(property) => {
                Property::CommitOrChangeId(chain(first, property))
            }
            Property::IdWithHighlightedPrefix(property) => {
                Property::IdWithHighlightedPrefix(chain(first, property))
            }
            Property::Signature(property) => Property::Signature(chain(first, property)),
            Property::Timestamp(property) => Property::Timestamp(chain(first, property)),
        }
    }

    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<I, Output = bool> + 'a>> {
        match self {
            Property::String(property) => {
                Some(Box::new(TemplateFunction::new(property, |s| !s.is_empty())))
            }
            Property::Boolean(property) => Some(property),
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
            Property::CommitOrChangeId(property) => wrap(property),
            Property::IdWithHighlightedPrefix(property) => wrap(property),
            Property::Signature(property) => wrap(property),
            Property::Timestamp(property) => wrap(property),
        }
    }
}

struct PropertyAndLabels<'a, C>(Property<'a, C>, Vec<String>);

impl<'a, C: 'a> PropertyAndLabels<'a, C> {
    fn into_template(self) -> Box<dyn Template<C> + 'a> {
        let PropertyAndLabels(property, labels) = self;
        if labels.is_empty() {
            property.into_template()
        } else {
            Box::new(LabelTemplate::new(
                property.into_template(),
                Literal(labels),
            ))
        }
    }
}

enum Expression<'a, C> {
    Property(PropertyAndLabels<'a, C>),
    Template(Box<dyn Template<C> + 'a>),
}

impl<'a, C: 'a> Expression<'a, C> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<C, Output = bool> + 'a>> {
        match self {
            Expression::Property(PropertyAndLabels(property, _)) => property.try_into_boolean(),
            Expression::Template(_) => None,
        }
    }

    fn into_plain_text(self) -> Box<dyn TemplateProperty<C, Output = String> + 'a> {
        match self {
            Expression::Property(PropertyAndLabels(property, _)) => property.into_plain_text(),
            Expression::Template(template) => Box::new(PlainTextFormattedProperty::new(template)),
        }
    }

    fn into_template(self) -> Box<dyn Template<C> + 'a> {
        match self {
            Expression::Property(property_labels) => property_labels.into_template(),
            Expression::Template(template) => template,
        }
    }
}

fn parse_method_chain<'a, I: 'a>(
    pair: Pair<Rule>,
    input_property: PropertyAndLabels<'a, I>,
) -> TemplateParseResult<PropertyAndLabels<'a, I>> {
    let PropertyAndLabels(mut property, mut labels) = input_property;
    assert_eq!(pair.as_rule(), Rule::maybe_method);
    for chain in pair.into_inner() {
        assert_eq!(chain.as_rule(), Rule::function);
        let (name, args_pair) = {
            let mut inner = chain.into_inner();
            let name = inner.next().unwrap();
            let args_pair = inner.next().unwrap();
            assert_eq!(name.as_rule(), Rule::identifier);
            assert_eq!(args_pair.as_rule(), Rule::function_arguments);
            (name, args_pair)
        };
        labels.push(name.as_str().to_owned());
        property = match property {
            Property::String(property) => parse_string_method(name, args_pair)?.after(property),
            Property::Boolean(property) => parse_boolean_method(name, args_pair)?.after(property),
            Property::CommitOrChangeId(property) => {
                parse_commit_or_change_id_method(name, args_pair)?.after(property)
            }
            Property::IdWithHighlightedPrefix(_property) => {
                return Err(TemplateParseError::no_such_method(
                    "IdWithHighlightedPrefix",
                    &name,
                ));
            }
            Property::Signature(property) => {
                parse_signature_method(name, args_pair)?.after(property)
            }
            Property::Timestamp(property) => {
                parse_timestamp_method(name, args_pair)?.after(property)
            }
        };
    }
    Ok(PropertyAndLabels(property, labels))
}

fn parse_string_method<'a>(
    name: Pair<Rule>,
    _args_pair: Pair<Rule>,
) -> TemplateParseResult<Property<'a, String>> {
    fn wrap_fn<'a, O>(
        f: impl Fn(&String) -> O + 'a,
    ) -> Box<dyn TemplateProperty<String, Output = O> + 'a> {
        Box::new(TemplatePropertyFn(f))
    }
    // TODO: validate arguments
    let property = match name.as_str() {
        "first_line" => Property::String(wrap_fn(|s| {
            s.lines().next().unwrap_or_default().to_string()
        })),
        _ => return Err(TemplateParseError::no_such_method("String", &name)),
    };
    Ok(property)
}

fn parse_boolean_method<'a>(
    name: Pair<Rule>,
    _args_pair: Pair<Rule>,
) -> TemplateParseResult<Property<'a, bool>> {
    Err(TemplateParseError::no_such_method("Boolean", &name))
}

fn parse_commit_or_change_id_method<'a>(
    name: Pair<Rule>,
    _args_pair: Pair<Rule>,
) -> TemplateParseResult<Property<'a, CommitOrChangeId<'a>>> {
    fn wrap_fn<'a, O>(
        f: impl Fn(&CommitOrChangeId<'a>) -> O + 'a,
    ) -> Box<dyn TemplateProperty<CommitOrChangeId<'a>, Output = O> + 'a> {
        Box::new(TemplatePropertyFn(f))
    }
    // TODO: validate arguments
    let property = match name.as_str() {
        "short" => Property::String(wrap_fn(|id| id.short())),
        "shortest_prefix_and_brackets" => {
            Property::String(wrap_fn(|id| id.shortest_prefix_and_brackets()))
        }
        "shortest_styled_prefix" => {
            Property::IdWithHighlightedPrefix(wrap_fn(|id| id.shortest_styled_prefix()))
        }
        _ => {
            return Err(TemplateParseError::no_such_method(
                "CommitOrChangeId",
                &name,
            ));
        }
    };
    Ok(property)
}

fn parse_signature_method<'a>(
    name: Pair<Rule>,
    _args_pair: Pair<Rule>,
) -> TemplateParseResult<Property<'a, Signature>> {
    fn wrap_fn<'a, O>(
        f: impl Fn(&Signature) -> O + 'a,
    ) -> Box<dyn TemplateProperty<Signature, Output = O> + 'a> {
        Box::new(TemplatePropertyFn(f))
    }
    // TODO: validate arguments
    let property = match name.as_str() {
        "name" => Property::String(wrap_fn(|signature| signature.name.clone())),
        "email" => Property::String(wrap_fn(|signature| signature.email.clone())),
        "timestamp" => Property::Timestamp(wrap_fn(|signature| signature.timestamp.clone())),
        _ => return Err(TemplateParseError::no_such_method("Signature", &name)),
    };
    Ok(property)
}

fn parse_timestamp_method<'a>(
    name: Pair<Rule>,
    _args_pair: Pair<Rule>,
) -> TemplateParseResult<Property<'a, Timestamp>> {
    fn wrap_fn<'a, O>(
        f: impl Fn(&Timestamp) -> O + 'a,
    ) -> Box<dyn TemplateProperty<Timestamp, Output = O> + 'a> {
        Box::new(TemplatePropertyFn(f))
    }
    // TODO: validate arguments
    let property = match name.as_str() {
        "ago" => Property::String(wrap_fn(time_util::format_timestamp_relative_to_now)),
        _ => return Err(TemplateParseError::no_such_method("Timestamp", &name)),
    };
    Ok(property)
}

fn parse_commit_keyword<'a>(
    repo: RepoRef<'a>,
    workspace_id: &WorkspaceId,
    pair: Pair<Rule>,
) -> TemplateParseResult<PropertyAndLabels<'a, Commit>> {
    fn wrap_fn<'a, O>(
        f: impl Fn(&Commit) -> O + 'a,
    ) -> Box<dyn TemplateProperty<Commit, Output = O> + 'a> {
        Box::new(TemplatePropertyFn(f))
    }
    assert_eq!(pair.as_rule(), Rule::identifier);
    let property = match pair.as_str() {
        "description" => Property::String(wrap_fn(|commit| {
            cli_util::complete_newline(commit.description())
        })),
        "change_id" => Property::CommitOrChangeId(wrap_fn(move |commit| {
            CommitOrChangeId::new(repo, commit.change_id())
        })),
        "commit_id" => Property::CommitOrChangeId(wrap_fn(move |commit| {
            CommitOrChangeId::new(repo, commit.id())
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
        _ => return Err(TemplateParseError::no_such_keyword(&pair)),
    };
    Ok(PropertyAndLabels(property, vec![pair.as_str().to_string()]))
}

fn parse_term<'a, C: 'a>(
    pair: Pair<Rule>,
    parse_keyword: &impl Fn(Pair<Rule>) -> TemplateParseResult<PropertyAndLabels<'a, C>>,
) -> TemplateParseResult<Expression<'a, C>> {
    assert_eq!(pair.as_rule(), Rule::term);
    let mut inner = pair.into_inner();
    let expr = inner.next().unwrap();
    let maybe_method = inner.next().unwrap();
    assert!(inner.next().is_none());
    match expr.as_rule() {
        Rule::literal => {
            let text = parse_string_literal(expr);
            let term = PropertyAndLabels(Property::String(Box::new(Literal(text))), vec![]);
            let property = parse_method_chain(maybe_method, term)?;
            Ok(Expression::Property(property))
        }
        Rule::identifier => {
            let term = parse_keyword(expr)?;
            let property = parse_method_chain(maybe_method, term)?;
            Ok(Expression::Property(property))
        }
        Rule::function => {
            let (name, args_span, mut args) = {
                let mut inner = expr.into_inner();
                let name = inner.next().unwrap();
                let args_pair = inner.next().unwrap();
                assert_eq!(name.as_rule(), Rule::identifier);
                assert_eq!(args_pair.as_rule(), Rule::function_arguments);
                (name, args_pair.as_span(), args_pair.into_inner())
            };
            let expression = match name.as_str() {
                "label" => {
                    let arg_count_error =
                        || TemplateParseError::invalid_argument_count_exact(2, args_span);
                    let label_pair = args.next().ok_or_else(arg_count_error)?;
                    let label_property =
                        parse_template_rule(label_pair, parse_keyword)?.into_plain_text();
                    let arg_template = args.next().ok_or_else(arg_count_error)?;
                    if args.next().is_some() {
                        return Err(arg_count_error());
                    }
                    let content = parse_template_rule(arg_template, parse_keyword)?.into_template();
                    let labels = TemplateFunction::new(label_property, |s| {
                        s.split_whitespace().map(ToString::to_string).collect()
                    });
                    let template = Box::new(LabelTemplate::new(content, labels));
                    Expression::Template(template)
                }
                "if" => {
                    let arg_count_error =
                        || TemplateParseError::invalid_argument_count_range(2..=3, args_span);
                    let condition_pair = args.next().ok_or_else(arg_count_error)?;
                    let condition_span = condition_pair.as_span();
                    let condition = parse_template_rule(condition_pair, parse_keyword)?
                        .try_into_boolean()
                        .ok_or_else(|| {
                            TemplateParseError::invalid_argument_type("Boolean", condition_span)
                        })?;

                    let true_template = args
                        .next()
                        .ok_or_else(arg_count_error)
                        .and_then(|pair| parse_template_rule(pair, parse_keyword))?
                        .into_template();
                    let false_template = args
                        .next()
                        .map(|pair| parse_template_rule(pair, parse_keyword))
                        .transpose()?
                        .map(|x| x.into_template());
                    if args.next().is_some() {
                        return Err(arg_count_error());
                    }
                    let template = Box::new(ConditionalTemplate::new(
                        condition,
                        true_template,
                        false_template,
                    ));
                    Expression::Template(template)
                }
                "separate" => {
                    let arg_count_error =
                        || TemplateParseError::invalid_argument_count_range_from(1.., args_span);
                    let separator_pair = args.next().ok_or_else(arg_count_error)?;
                    let separator =
                        parse_template_rule(separator_pair, parse_keyword)?.into_template();
                    let contents = args
                        .map(|pair| {
                            parse_template_rule(pair, parse_keyword).map(|x| x.into_template())
                        })
                        .try_collect()?;
                    let template = Box::new(SeparateTemplate::new(separator, contents));
                    Expression::Template(template)
                }
                _ => return Err(TemplateParseError::no_such_function(&name)),
            };
            Ok(expression)
        }
        Rule::template => parse_template_rule(expr, parse_keyword),
        other => panic!("unexpected term: {other:?}"),
    }
}

fn parse_template_rule<'a, C: 'a>(
    pair: Pair<Rule>,
    parse_keyword: &impl Fn(Pair<Rule>) -> TemplateParseResult<PropertyAndLabels<'a, C>>,
) -> TemplateParseResult<Expression<'a, C>> {
    assert_eq!(pair.as_rule(), Rule::template);
    let inner = pair.into_inner();
    let mut expressions: Vec<_> = inner
        .map(|term| parse_term(term, parse_keyword))
        .try_collect()?;
    if expressions.len() == 1 {
        Ok(expressions.pop().unwrap())
    } else {
        let templates = expressions.into_iter().map(|x| x.into_template()).collect();
        Ok(Expression::Template(Box::new(ListTemplate(templates))))
    }
}

// TODO: We'll probably need a trait that abstracts the Property enum and
// keyword/method parsing functions per the top-level context.
fn parse_template_str<'a, C: 'a>(
    template_text: &str,
    parse_keyword: impl Fn(Pair<Rule>) -> TemplateParseResult<PropertyAndLabels<'a, C>>,
) -> TemplateParseResult<Box<dyn Template<C> + 'a>> {
    let mut pairs: Pairs<Rule> = TemplateParser::parse(Rule::program, template_text)?;
    let first_pair = pairs.next().unwrap();
    if first_pair.as_rule() == Rule::EOI {
        Ok(Box::new(Literal(String::new())))
    } else {
        parse_template_rule(first_pair, &parse_keyword).map(|x| x.into_template())
    }
}

pub fn parse_commit_template<'a>(
    repo: RepoRef<'a>,
    workspace_id: &WorkspaceId,
    template_text: &str,
) -> TemplateParseResult<Box<dyn Template<Commit> + 'a>> {
    parse_template_str(template_text, |pair| {
        parse_commit_keyword(repo, workspace_id, pair)
    })
}
