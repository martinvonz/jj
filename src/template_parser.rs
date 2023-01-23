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
    ConditionalTemplate, ConflictProperty, ConstantTemplateProperty, DescriptionProperty,
    DivergentProperty, DynamicLabelTemplate, EmptyProperty, GitRefsProperty, IsGitHeadProperty,
    IsWorkingCopyProperty, LabelTemplate, ListTemplate, LiteralTemplate, SignatureTimestamp,
    StringPropertyTemplate, TagProperty, Template, TemplateFunction, TemplateProperty,
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

impl TemplateProperty<String, String> for StringFirstLine {
    fn extract(&self, context: &String) -> String {
        context.lines().next().unwrap().to_string()
    }
}

struct SignatureName;

impl TemplateProperty<Signature, String> for SignatureName {
    fn extract(&self, context: &Signature) -> String {
        context.name.clone()
    }
}

struct SignatureEmail;

impl TemplateProperty<Signature, String> for SignatureEmail {
    fn extract(&self, context: &Signature) -> String {
        context.email.clone()
    }
}

struct RelativeTimestampString;

impl TemplateProperty<Timestamp, String> for RelativeTimestampString {
    fn extract(&self, context: &Timestamp) -> String {
        time_util::format_timestamp_relative_to_now(context)
    }
}

enum Property<'a, I> {
    String(Box<dyn TemplateProperty<I, String> + 'a>),
    Boolean(Box<dyn TemplateProperty<I, bool> + 'a>),
    CommitOrChangeId(
        Box<dyn TemplateProperty<I, CommitOrChangeId> + 'a>,
        RepoRef<'a>,
    ),
    Signature(Box<dyn TemplateProperty<I, Signature> + 'a>),
    Timestamp(Box<dyn TemplateProperty<I, Timestamp> + 'a>),
}

impl<'a, I: 'a> Property<'a, I> {
    fn after<C: 'a>(self, first: Box<dyn TemplateProperty<C, I> + 'a>) -> Property<'a, C> {
        fn chain<'a, C: 'a, I: 'a, O: 'a>(
            first: Box<dyn TemplateProperty<C, I> + 'a>,
            second: Box<dyn TemplateProperty<I, O> + 'a>,
        ) -> Box<dyn TemplateProperty<C, O> + 'a> {
            Box::new(TemplateFunction::new(
                first,
                Box::new(move |value| second.extract(&value)),
            ))
        }
        match self {
            Property::String(property) => Property::String(chain(first, property)),
            Property::Boolean(property) => Property::Boolean(chain(first, property)),
            Property::CommitOrChangeId(property, repo) => {
                Property::CommitOrChangeId(chain(first, property), repo)
            }
            Property::Signature(property) => Property::Signature(chain(first, property)),
            Property::Timestamp(property) => Property::Timestamp(chain(first, property)),
        }
    }
}

fn parse_method_chain<'a, I: 'a>(
    pair: Pair<Rule>,
    input_property: Property<'a, I>,
) -> PropertyAndLabels<'a, I> {
    assert_eq!(pair.as_rule(), Rule::maybe_method);
    if pair.as_str().is_empty() {
        PropertyAndLabels(input_property, vec![])
    } else {
        fn chain<'a, I: 'a, O: 'a>(
            property: Box<dyn TemplateProperty<I, O> + 'a>,
            parse: impl FnOnce() -> PropertyAndLabels<'a, O>,
        ) -> (Property<'a, I>, Vec<String>) {
            let PropertyAndLabels(next_method, labels) = parse();
            (next_method.after(property), labels)
        }
        let method = pair.into_inner().next().unwrap();
        let label = method
            .clone()
            .into_inner()
            .next()
            .unwrap()
            .as_str()
            .to_string();
        let (property, mut labels) = match input_property {
            Property::String(property) => chain(property, || parse_string_method(method)),
            Property::Boolean(property) => chain(property, || parse_boolean_method(method)),
            Property::CommitOrChangeId(property, repo) => {
                chain(property, || parse_commit_or_change_id_method(method, repo))
            }
            Property::Signature(property) => chain(property, || parse_signature_method(method)),
            Property::Timestamp(property) => chain(property, || parse_timestamp_method(method)),
        };
        labels.insert(0, label);
        PropertyAndLabels(property, labels)
    }
}

fn parse_string_method<'a>(method: Pair<Rule>) -> PropertyAndLabels<'a, String> {
    assert_eq!(method.as_rule(), Rule::method);
    let mut inner = method.into_inner();
    let name = inner.next().unwrap();
    // TODO: validate arguments

    let this_function = match name.as_str() {
        "first_line" => Property::String(Box::new(StringFirstLine)),
        name => panic!("no such string method: {name}"),
    };
    let chain_method = inner.last().unwrap();
    parse_method_chain(chain_method, this_function)
}

fn parse_boolean_method<'a>(method: Pair<Rule>) -> PropertyAndLabels<'a, bool> {
    assert_eq!(method.as_rule(), Rule::maybe_method);
    let mut inner = method.into_inner();
    let name = inner.next().unwrap();
    // TODO: validate arguments

    panic!("no such boolean method: {}", name.as_str());
}

fn parse_commit_or_change_id_method<'a>(
    method: Pair<Rule>,
    repo: RepoRef<'a>,
) -> PropertyAndLabels<'a, CommitOrChangeId> {
    assert_eq!(method.as_rule(), Rule::method);
    let mut inner = method.into_inner();
    let name = inner.next().unwrap();
    // TODO: validate arguments

    let this_function = match name.as_str() {
        "short" => Property::String(Box::new(CommitOrChangeIdShort { repo })),
        "short_prefix_and_brackets" => {
            Property::String(Box::new(CommitOrChangeIdShortPrefixAndBrackets { repo }))
        }
        name => panic!("no such commit ID method: {name}"),
    };
    let chain_method = inner.last().unwrap();
    parse_method_chain(chain_method, this_function)
}

fn parse_signature_method<'a>(method: Pair<Rule>) -> PropertyAndLabels<'a, Signature> {
    assert_eq!(method.as_rule(), Rule::method);
    let mut inner = method.into_inner();
    let name = inner.next().unwrap();
    // TODO: validate arguments

    let this_function: Property<'a, Signature> = match name.as_str() {
        "name" => Property::String(Box::new(SignatureName)),
        "email" => Property::String(Box::new(SignatureEmail)),
        "timestamp" => Property::Timestamp(Box::new(SignatureTimestamp)),
        name => panic!("no such commit ID method: {name}"),
    };
    let chain_method = inner.last().unwrap();
    parse_method_chain(chain_method, this_function)
}

fn parse_timestamp_method<'a>(method: Pair<Rule>) -> PropertyAndLabels<'a, Timestamp> {
    assert_eq!(method.as_rule(), Rule::method);
    let mut inner = method.into_inner();
    let name = inner.next().unwrap();
    // TODO: validate arguments

    let this_function = match name.as_str() {
        "ago" => Property::String(Box::new(RelativeTimestampString)),
        name => panic!("no such timestamp method: {name}"),
    };
    let chain_method = inner.last().unwrap();
    parse_method_chain(chain_method, this_function)
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
        "change_id" => Property::CommitOrChangeId(Box::new(ChangeIdProperty), repo),
        "commit_id" => Property::CommitOrChangeId(Box::new(CommitIdProperty), repo),
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

fn coerce_to_string<'a, I: 'a>(
    property: Property<'a, I>,
) -> Box<dyn TemplateProperty<I, String> + 'a> {
    match property {
        Property::String(property) => property,
        Property::Boolean(property) => Box::new(TemplateFunction::new(
            property,
            Box::new(|value| String::from(if value { "true" } else { "false" })),
        )),
        Property::CommitOrChangeId(property, _) => {
            Box::new(TemplateFunction::new(property, Box::new(|id| id.hex())))
        }
        Property::Signature(property) => Box::new(TemplateFunction::new(
            property,
            Box::new(|signature| signature.name),
        )),
        Property::Timestamp(property) => Box::new(TemplateFunction::new(
            property,
            Box::new(|timestamp| time_util::format_absolute_timestamp(&timestamp)),
        )),
    }
}

fn parse_boolean_commit_property<'a>(
    repo: RepoRef<'a>,
    workspace_id: &WorkspaceId,
    pair: Pair<Rule>,
) -> Box<dyn TemplateProperty<Commit, bool> + 'a> {
    let mut inner = pair.into_inner();
    let pair = inner.next().unwrap();
    let _method = inner.next().unwrap();
    assert!(inner.next().is_none());
    match pair.as_rule() {
        Rule::identifier => match parse_commit_keyword(repo, workspace_id, pair.clone()).0 {
            Property::Boolean(property) => property,
            Property::String(property) => Box::new(TemplateFunction::new(
                property,
                Box::new(|string| !string.is_empty()),
            )),
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
    if pair.as_str().is_empty() {
        Box::new(LiteralTemplate(String::new()))
    } else {
        let mut inner = pair.into_inner();
        let expr = inner.next().unwrap();
        let maybe_method = inner.next().unwrap();
        assert!(inner.next().is_none());
        match expr.as_rule() {
            Rule::literal => {
                let text = parse_string_literal(expr);
                if maybe_method.as_str().is_empty() {
                    Box::new(LiteralTemplate(text))
                } else {
                    let input_property =
                        Property::String(Box::new(ConstantTemplateProperty { output: text }));
                    let PropertyAndLabels(property, method_labels) =
                        parse_method_chain(maybe_method, input_property);
                    let string_property = coerce_to_string(property);
                    Box::new(LabelTemplate::new(
                        Box::new(StringPropertyTemplate {
                            property: string_property,
                        }),
                        method_labels,
                    ))
                }
            }
            Rule::identifier => {
                let PropertyAndLabels(term_property, keyword_labels) =
                    parse_commit_keyword(repo, workspace_id, expr);
                let PropertyAndLabels(property, method_labels) =
                    parse_method_chain(maybe_method, term_property);
                let string_property = coerce_to_string(property);
                let mut labels = keyword_labels;
                labels.extend(method_labels);
                Box::new(LabelTemplate::new(
                    Box::new(StringPropertyTemplate {
                        property: string_property,
                    }),
                    labels,
                ))
            }
            Rule::function => {
                let mut inner = expr.into_inner();
                let name = inner.next().unwrap().as_str();
                match name {
                    "label" => {
                        let label_pair = inner.next().unwrap();
                        let label_template = parse_commit_template_rule(
                            repo,
                            workspace_id,
                            label_pair.into_inner().next().unwrap(),
                        );
                        let arg_template = match inner.next() {
                            None => panic!("label() requires two arguments"),
                            Some(pair) => pair,
                        };
                        if inner.next().is_some() {
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
                        Box::new(DynamicLabelTemplate::new(content, Box::new(get_labels)))
                    }
                    "if" => {
                        let condition_pair = inner.next().unwrap();
                        let condition_template = condition_pair.into_inner().next().unwrap();
                        let condition =
                            parse_boolean_commit_property(repo, workspace_id, condition_template);

                        let true_template = match inner.next() {
                            None => panic!("if() requires at least two arguments"),
                            Some(pair) => parse_commit_template_rule(repo, workspace_id, pair),
                        };
                        let false_template = inner
                            .next()
                            .map(|pair| parse_commit_template_rule(repo, workspace_id, pair));
                        if inner.next().is_some() {
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
            other => panic!("unexpected term: {other:?}"),
        }
    }
}

fn parse_commit_template_rule<'a>(
    repo: RepoRef<'a>,
    workspace_id: &WorkspaceId,
    pair: Pair<Rule>,
) -> Box<dyn Template<Commit> + 'a> {
    match pair.as_rule() {
        Rule::template => {
            let mut inner = pair.into_inner();
            let formatter = parse_commit_template_rule(repo, workspace_id, inner.next().unwrap());
            assert!(inner.next().is_none());
            formatter
        }
        Rule::term => parse_commit_term(repo, workspace_id, pair),
        Rule::list => {
            let mut formatters: Vec<Box<dyn Template<Commit>>> = vec![];
            for inner_pair in pair.into_inner() {
                formatters.push(parse_commit_template_rule(repo, workspace_id, inner_pair));
            }
            Box::new(ListTemplate(formatters))
        }
        _ => Box::new(LiteralTemplate(String::new())),
    }
}

pub fn parse_commit_template<'a>(
    repo: RepoRef<'a>,
    workspace_id: &WorkspaceId,
    template_text: &str,
) -> Box<dyn Template<Commit> + 'a> {
    let mut pairs: Pairs<Rule> = TemplateParser::parse(Rule::template, template_text).unwrap();

    let first_pair = pairs.next().unwrap();
    assert!(pairs.next().is_none());

    assert_eq!(
        first_pair.as_span().end(),
        template_text.len(),
        "failed to parse template past position {}",
        first_pair.as_span().end()
    );

    parse_commit_template_rule(repo, workspace_id, first_pair)
}
