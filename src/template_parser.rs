// Copyright 2020 Google LLC
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

use chrono::{FixedOffset, LocalResult, TimeZone, Utc};
use jujutsu_lib::backend::{CommitId, Signature};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::op_store::WorkspaceId;
use jujutsu_lib::repo::RepoRef;
use pest::iterators::{Pair, Pairs};
use pest::Parser;
use pest_derive::Parser;

use crate::formatter::PlainTextFormatter;
use crate::templater::{
    AuthorProperty, BranchProperty, ChangeIdProperty, CommitIdKeyword, CommitterProperty,
    ConditionalTemplate, ConflictProperty, ConstantTemplateProperty, DescriptionProperty,
    DivergentProperty, DynamicLabelTemplate, GitRefsProperty, IsGitHeadProperty,
    IsWorkingCopyProperty, LabelTemplate, ListTemplate, LiteralTemplate, StringPropertyTemplate,
    TagProperty, Template, TemplateFunction, TemplateProperty, WorkingCopiesProperty,
};

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
                char => panic!("invalid escape: \\{:?}", char),
            },
            _ => panic!("unexpected part of string: {:?}", part),
        }
    }
    result
}

struct StringShort;

impl TemplateProperty<String, String> for StringShort {
    fn extract(&self, context: &String) -> String {
        context.chars().take(12).collect()
    }
}

struct StringFirstLine;

impl TemplateProperty<String, String> for StringFirstLine {
    fn extract(&self, context: &String) -> String {
        context.lines().next().unwrap().to_string()
    }
}

struct CommitIdShortest;

impl TemplateProperty<CommitId, String> for CommitIdShortest {
    fn extract(&self, context: &CommitId) -> String {
        CommitIdKeyword::shortest_format(context.clone())
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

struct SignatureTimestamp;

impl TemplateProperty<Signature, String> for SignatureTimestamp {
    fn extract(&self, context: &Signature) -> String {
        let utc = match Utc.timestamp_opt(
            context.timestamp.timestamp.0.div_euclid(1000),
            (context.timestamp.timestamp.0.rem_euclid(1000)) as u32 * 1000000,
        ) {
            LocalResult::None => {
                return "<out-of-range date>".to_string();
            }
            LocalResult::Single(x) => x,
            LocalResult::Ambiguous(y, _z) => y,
        };
        let datetime = utc.with_timezone(
            &FixedOffset::east_opt(context.timestamp.tz_offset * 60)
                .unwrap_or_else(|| FixedOffset::east_opt(0).unwrap()),
        );
        datetime.format("%Y-%m-%d %H:%M:%S.%3f %:z").to_string()
    }
}

fn parse_method_chain<'a, I: 'a>(
    pair: Pair<Rule>,
    input_property: Property<'a, I>,
) -> Property<'a, I> {
    assert_eq!(pair.as_rule(), Rule::maybe_method);
    if pair.as_str().is_empty() {
        input_property
    } else {
        let method = pair.into_inner().next().unwrap();
        match input_property {
            Property::String(property) => {
                let next_method = parse_string_method(method);
                next_method.after(property)
            }
            Property::Boolean(property) => {
                let next_method = parse_boolean_method(method);
                next_method.after(property)
            }
            Property::CommitId(property) => {
                let next_method = parse_commit_id_method(method);
                next_method.after(property)
            }
            Property::Signature(property) => {
                let next_method = parse_signature_method(method);
                next_method.after(property)
            }
        }
    }
}

fn parse_string_method<'a>(method: Pair<Rule>) -> Property<'a, String> {
    assert_eq!(method.as_rule(), Rule::method);
    let mut inner = method.into_inner();
    let name = inner.next().unwrap();
    // TODO: validate arguments

    let this_function = match name.as_str() {
        "short" => Property::String(Box::new(StringShort)),
        "first_line" => Property::String(Box::new(StringFirstLine)),
        name => panic!("no such string method: {}", name),
    };
    let chain_method = inner.last().unwrap();
    parse_method_chain(chain_method, this_function)
}

fn parse_boolean_method<'a>(method: Pair<Rule>) -> Property<'a, bool> {
    assert_eq!(method.as_rule(), Rule::maybe_method);
    let mut inner = method.into_inner();
    let name = inner.next().unwrap();
    // TODO: validate arguments

    panic!("no such boolean method: {}", name.as_str());
}

// TODO: pass a context to the returned function (we need the repo to find the
//       shortest unambiguous prefix)
fn parse_commit_id_method<'a>(method: Pair<Rule>) -> Property<'a, CommitId> {
    assert_eq!(method.as_rule(), Rule::method);
    let mut inner = method.into_inner();
    let name = inner.next().unwrap();
    // TODO: validate arguments

    let this_function = match name.as_str() {
        "short" => Property::String(Box::new(CommitIdShortest)),
        name => panic!("no such commit ID method: {}", name),
    };
    let chain_method = inner.last().unwrap();
    parse_method_chain(chain_method, this_function)
}

fn parse_signature_method<'a>(method: Pair<Rule>) -> Property<'a, Signature> {
    assert_eq!(method.as_rule(), Rule::method);
    let mut inner = method.into_inner();
    let name = inner.next().unwrap();
    // TODO: validate arguments

    let this_function: Property<'a, Signature> = match name.as_str() {
        // TODO: Automatically label these too (so author.name() gets
        //       labels "author" *and" "name". Perhaps drop parentheses
        //       from syntax for that? Or maybe this should be using
        //       syntax for nested records (e.g.
        //       `author % (name "<" email ">")`)?
        "name" => Property::String(Box::new(SignatureName)),
        "email" => Property::String(Box::new(SignatureEmail)),
        "timestamp" => Property::String(Box::new(SignatureTimestamp)),
        name => panic!("no such commit ID method: {}", name),
    };
    let chain_method = inner.last().unwrap();
    parse_method_chain(chain_method, this_function)
}

enum Property<'a, I> {
    String(Box<dyn TemplateProperty<I, String> + 'a>),
    Boolean(Box<dyn TemplateProperty<I, bool> + 'a>),
    CommitId(Box<dyn TemplateProperty<I, CommitId> + 'a>),
    Signature(Box<dyn TemplateProperty<I, Signature> + 'a>),
}

impl<'a, I: 'a> Property<'a, I> {
    fn after<C: 'a>(self, first: Box<dyn TemplateProperty<C, I> + 'a>) -> Property<'a, C> {
        match self {
            Property::String(property) => Property::String(Box::new(TemplateFunction::new(
                first,
                Box::new(move |value| property.extract(&value)),
            ))),
            Property::Boolean(property) => Property::Boolean(Box::new(TemplateFunction::new(
                first,
                Box::new(move |value| property.extract(&value)),
            ))),
            Property::CommitId(property) => Property::CommitId(Box::new(TemplateFunction::new(
                first,
                Box::new(move |value| property.extract(&value)),
            ))),
            Property::Signature(property) => Property::Signature(Box::new(TemplateFunction::new(
                first,
                Box::new(move |value| property.extract(&value)),
            ))),
        }
    }
}

fn parse_commit_keyword<'a>(
    repo: RepoRef<'a>,
    workspace_id: &WorkspaceId,
    pair: Pair<Rule>,
) -> (Property<'a, Commit>, String) {
    assert_eq!(pair.as_rule(), Rule::identifier);
    let property = match pair.as_str() {
        "description" => Property::String(Box::new(DescriptionProperty)),
        "change_id" => Property::String(Box::new(ChangeIdProperty)),
        "commit_id" => Property::CommitId(Box::new(CommitIdKeyword)),
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
        name => panic!("unexpected identifier: {}", name),
    };
    (property, pair.as_str().to_string())
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
        Property::CommitId(property) => Box::new(TemplateFunction::new(
            property,
            Box::new(CommitIdKeyword::default_format),
        )),
        Property::Signature(property) => Box::new(TemplateFunction::new(
            property,
            Box::new(|signature| signature.name),
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
            _ => panic!("cannot yet use this as boolean: {:?}", pair),
        },
        _ => panic!("cannot yet use this as boolean: {:?}", pair),
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
                    let property = parse_method_chain(maybe_method, input_property);
                    let string_property = coerce_to_string(property);
                    Box::new(StringPropertyTemplate {
                        property: string_property,
                    })
                }
            }
            Rule::identifier => {
                let (term_property, labels) = parse_commit_keyword(repo, workspace_id, expr);
                let property = parse_method_chain(maybe_method, term_property);
                let string_property = coerce_to_string(property);
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
                        let get_labels = move |commit: &Commit| -> String {
                            let mut buf = vec![];
                            let mut formatter = PlainTextFormatter::new(&mut buf);
                            label_template.format(commit, &mut formatter).unwrap();
                            String::from_utf8(buf).unwrap()
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
                    name => panic!("function {} not implemented", name),
                }
            }
            other => panic!("unexpected term: {:?}", other),
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
