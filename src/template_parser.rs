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

use itertools::Itertools as _;
use jujutsu_lib::backend::{Signature, Timestamp};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::op_store::WorkspaceId;
use jujutsu_lib::repo::RepoRef;
use pest::iterators::{Pair, Pairs};
use pest::Parser;
use pest_derive::Parser;

use crate::formatter::PlainTextFormatter;
use crate::templater::{
    AuthorProperty, BranchProperty, ChangeIdProperty, CommitIdProperty, CommitOrChangeId,
    CommitOrChangeIdShort, CommitOrChangeIdShortPrefixAndBrackets, CommitterProperty,
    ConditionalTemplate, ConflictProperty, DescriptionProperty, DivergentProperty,
    DynamicLabelTemplate, EmptyProperty, FormattablePropertyTemplate, GitRefsProperty,
    IsGitHeadProperty, IsWorkingCopyProperty, LabelTemplate, ListTemplate, Literal,
    SignatureTimestamp, TagProperty, Template, TemplateFunction, TemplateProperty,
    WorkingCopiesProperty,
};
use crate::time_util;

#[derive(Parser)]
#[grammar = "template.pest"]
pub struct TemplateParser;

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

struct StringFirstLine;

impl TemplateProperty<String> for StringFirstLine {
    type Output = String;

    fn extract(&self, context: &String) -> Self::Output {
        context.lines().next().unwrap().to_string()
    }
}

struct SignatureName;

impl TemplateProperty<Signature> for SignatureName {
    type Output = String;

    fn extract(&self, context: &Signature) -> Self::Output {
        context.name.clone()
    }
}

struct SignatureEmail;

impl TemplateProperty<Signature> for SignatureEmail {
    type Output = String;

    fn extract(&self, context: &Signature) -> Self::Output {
        context.email.clone()
    }
}

struct RelativeTimestampString;

impl TemplateProperty<Timestamp> for RelativeTimestampString {
    type Output = String;

    fn extract(&self, context: &Timestamp) -> Self::Output {
        time_util::format_timestamp_relative_to_now(context)
    }
}

enum Property<'a, I> {
    String(Box<dyn TemplateProperty<I, Output = String> + 'a>),
    Boolean(Box<dyn TemplateProperty<I, Output = bool> + 'a>),
    CommitOrChangeId(Box<dyn TemplateProperty<I, Output = CommitOrChangeId<'a>> + 'a>),
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
            Property::Signature(property) => Property::Signature(chain(first, property)),
            Property::Timestamp(property) => Property::Timestamp(chain(first, property)),
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
            Property::Signature(property) => wrap(property),
            Property::Timestamp(property) => wrap(property),
        }
    }
}

fn parse_method_chain<'a, I: 'a>(
    pair: Pair<Rule>,
    input_property: PropertyAndLabels<'a, I>,
) -> PropertyAndLabels<'a, I> {
    let PropertyAndLabels(mut property, mut labels) = input_property;
    assert_eq!(pair.as_rule(), Rule::maybe_method);
    for chain in pair.into_inner() {
        assert_eq!(chain.as_rule(), Rule::function);
        let (name, args) = {
            let mut inner = chain.into_inner();
            let name = inner.next().unwrap();
            let args_pair = inner.next().unwrap();
            assert_eq!(name.as_rule(), Rule::identifier);
            assert_eq!(args_pair.as_rule(), Rule::function_arguments);
            (name, args_pair.into_inner())
        };
        labels.push(name.as_str().to_owned());
        property = match property {
            Property::String(property) => parse_string_method(name, args).after(property),
            Property::Boolean(property) => parse_boolean_method(name, args).after(property),
            Property::CommitOrChangeId(property) => {
                parse_commit_or_change_id_method(name, args).after(property)
            }
            Property::Signature(property) => parse_signature_method(name, args).after(property),
            Property::Timestamp(property) => parse_timestamp_method(name, args).after(property),
        };
    }
    PropertyAndLabels(property, labels)
}

fn parse_string_method<'a>(name: Pair<Rule>, _args: Pairs<Rule>) -> Property<'a, String> {
    // TODO: validate arguments
    match name.as_str() {
        "first_line" => Property::String(Box::new(StringFirstLine)),
        name => panic!("no such string method: {name}"),
    }
}

fn parse_boolean_method<'a>(name: Pair<Rule>, _args: Pairs<Rule>) -> Property<'a, bool> {
    // TODO: validate arguments
    panic!("no such boolean method: {}", name.as_str());
}

fn parse_commit_or_change_id_method<'a>(
    name: Pair<Rule>,
    _args: Pairs<Rule>,
) -> Property<'a, CommitOrChangeId<'a>> {
    // TODO: validate arguments
    match name.as_str() {
        "short" => Property::String(Box::new(CommitOrChangeIdShort)),
        "short_prefix_and_brackets" => {
            Property::String(Box::new(CommitOrChangeIdShortPrefixAndBrackets))
        }
        name => panic!("no such commit ID method: {name}"),
    }
}

fn parse_signature_method<'a>(name: Pair<Rule>, _args: Pairs<Rule>) -> Property<'a, Signature> {
    // TODO: validate arguments
    match name.as_str() {
        "name" => Property::String(Box::new(SignatureName)),
        "email" => Property::String(Box::new(SignatureEmail)),
        "timestamp" => Property::Timestamp(Box::new(SignatureTimestamp)),
        name => panic!("no such commit ID method: {name}"),
    }
}

fn parse_timestamp_method<'a>(name: Pair<Rule>, _args: Pairs<Rule>) -> Property<'a, Timestamp> {
    // TODO: validate arguments
    match name.as_str() {
        "ago" => Property::String(Box::new(RelativeTimestampString)),
        name => panic!("no such timestamp method: {name}"),
    }
}

struct PropertyAndLabels<'a, C>(Property<'a, C>, Vec<String>);

fn parse_commit_keyword<'a>(
    repo: RepoRef<'a>,
    workspace_id: &WorkspaceId,
    pair: Pair<Rule>,
) -> PropertyAndLabels<'a, Commit> {
    assert_eq!(pair.as_rule(), Rule::identifier);
    let property = match pair.as_str() {
        "description" => Property::String(Box::new(DescriptionProperty)),
        "change_id" => Property::CommitOrChangeId(Box::new(ChangeIdProperty { repo })),
        "commit_id" => Property::CommitOrChangeId(Box::new(CommitIdProperty { repo })),
        "author" => Property::Signature(Box::new(AuthorProperty)),
        "committer" => Property::Signature(Box::new(CommitterProperty)),
        "working_copies" => Property::String(Box::new(WorkingCopiesProperty { repo })),
        "current_working_copy" => Property::Boolean(Box::new(IsWorkingCopyProperty {
            repo,
            workspace_id: workspace_id.clone(),
        })),
        "branches" => Property::String(Box::new(BranchProperty { repo })),
        "tags" => Property::String(Box::new(TagProperty { repo })),
        "git_refs" => Property::String(Box::new(GitRefsProperty { repo })),
        "is_git_head" => Property::Boolean(Box::new(IsGitHeadProperty::new(repo))),
        "divergent" => Property::Boolean(Box::new(DivergentProperty::new(repo))),
        "conflict" => Property::Boolean(Box::new(ConflictProperty)),
        "empty" => Property::Boolean(Box::new(EmptyProperty { repo })),
        name => panic!("unexpected identifier: {name}"),
    };
    PropertyAndLabels(property, vec![pair.as_str().to_string()])
}

fn parse_boolean_commit_property<'a>(
    repo: RepoRef<'a>,
    workspace_id: &WorkspaceId,
    pair: Pair<Rule>,
) -> Box<dyn TemplateProperty<Commit, Output = bool> + 'a> {
    let mut inner = pair.into_inner();
    let pair = inner.next().unwrap();
    let _method = inner.next().unwrap();
    assert!(inner.next().is_none());
    match pair.as_rule() {
        Rule::identifier => match parse_commit_keyword(repo, workspace_id, pair.clone()).0 {
            Property::Boolean(property) => property,
            Property::String(property) => {
                Box::new(TemplateFunction::new(property, |string| !string.is_empty()))
            }
            _ => panic!("cannot yet use this as boolean: {pair:?}"),
        },
        _ => panic!("cannot yet use this as boolean: {pair:?}"),
    }
}

fn parse_commit_term<'a>(
    repo: RepoRef<'a>,
    workspace_id: &WorkspaceId,
    pair: Pair<Rule>,
) -> Box<dyn Template<Commit> + 'a> {
    assert_eq!(pair.as_rule(), Rule::term);
    let mut inner = pair.into_inner();
    let expr = inner.next().unwrap();
    let maybe_method = inner.next().unwrap();
    assert!(inner.next().is_none());
    match expr.as_rule() {
        Rule::literal => {
            let text = parse_string_literal(expr);
            let term = PropertyAndLabels(Property::String(Box::new(Literal(text))), vec![]);
            let PropertyAndLabels(property, labels) = parse_method_chain(maybe_method, term);
            if labels.is_empty() {
                property.into_template()
            } else {
                Box::new(LabelTemplate::new(property.into_template(), labels))
            }
        }
        Rule::identifier => {
            let term = parse_commit_keyword(repo, workspace_id, expr);
            let PropertyAndLabels(property, labels) = parse_method_chain(maybe_method, term);
            Box::new(LabelTemplate::new(property.into_template(), labels))
        }
        Rule::function => {
            let (name, mut args) = {
                let mut inner = expr.into_inner();
                let name = inner.next().unwrap();
                let args_pair = inner.next().unwrap();
                assert_eq!(name.as_rule(), Rule::identifier);
                assert_eq!(args_pair.as_rule(), Rule::function_arguments);
                (name, args_pair.into_inner())
            };
            match name.as_str() {
                "label" => {
                    let label_pair = args.next().unwrap();
                    let label_template = parse_commit_template_rule(repo, workspace_id, label_pair);
                    let arg_template = match args.next() {
                        None => panic!("label() requires two arguments"),
                        Some(pair) => pair,
                    };
                    if args.next().is_some() {
                        panic!("label() accepts only two arguments")
                    }
                    let content: Box<dyn Template<Commit> + 'a> =
                        parse_commit_template_rule(repo, workspace_id, arg_template);
                    let get_labels = move |commit: &Commit| -> Vec<String> {
                        let mut buf = vec![];
                        let mut formatter = PlainTextFormatter::new(&mut buf);
                        label_template.format(commit, &mut formatter).unwrap();
                        String::from_utf8(buf)
                            .unwrap()
                            .split_whitespace()
                            .map(ToString::to_string)
                            .collect()
                    };
                    Box::new(DynamicLabelTemplate::new(content, get_labels))
                }
                "if" => {
                    let condition_pair = args.next().unwrap();
                    let condition_template = condition_pair.into_inner().next().unwrap();
                    let condition =
                        parse_boolean_commit_property(repo, workspace_id, condition_template);

                    let true_template = match args.next() {
                        None => panic!("if() requires at least two arguments"),
                        Some(pair) => parse_commit_template_rule(repo, workspace_id, pair),
                    };
                    let false_template = args
                        .next()
                        .map(|pair| parse_commit_template_rule(repo, workspace_id, pair));
                    if args.next().is_some() {
                        panic!("if() accepts at most three arguments")
                    }
                    Box::new(ConditionalTemplate::new(
                        condition,
                        true_template,
                        false_template,
                    ))
                }
                name => panic!("function {name} not implemented"),
            }
        }
        Rule::template => parse_commit_template_rule(repo, workspace_id, expr),
        other => panic!("unexpected term: {other:?}"),
    }
}

fn parse_commit_template_rule<'a>(
    repo: RepoRef<'a>,
    workspace_id: &WorkspaceId,
    pair: Pair<Rule>,
) -> Box<dyn Template<Commit> + 'a> {
    assert_eq!(pair.as_rule(), Rule::template);
    let inner = pair.into_inner();
    let mut templates = inner
        .map(|term| parse_commit_term(repo, workspace_id, term))
        .collect_vec();
    if templates.len() == 1 {
        templates.pop().unwrap()
    } else {
        Box::new(ListTemplate(templates))
    }
}

pub fn parse_commit_template<'a>(
    repo: RepoRef<'a>,
    workspace_id: &WorkspaceId,
    template_text: &str,
) -> Box<dyn Template<Commit> + 'a> {
    let mut pairs: Pairs<Rule> = TemplateParser::parse(Rule::program, template_text).unwrap();
    let first_pair = pairs.next().unwrap();
    if first_pair.as_rule() == Rule::EOI {
        Box::new(Literal(String::new()))
    } else {
        parse_commit_template_rule(repo, workspace_id, first_pair)
    }
}
