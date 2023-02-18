// Copyright 2020-2023 The Jujutsu Authors
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
use jujutsu_lib::repo::Repo;
use jujutsu_lib::rewrite;

use crate::cli_util;
use crate::template_parser::{
    self, CoreTemplatePropertyKind, FunctionCallNode, IntoTemplateProperty, TemplateAliasesMap,
    TemplateLanguage, TemplateParseError, TemplateParseResult,
};
use crate::templater::{
    BranchProperty, CommitOrChangeId, FormattablePropertyTemplate, GitHeadProperty,
    GitRefsProperty, PlainTextFormattedProperty, ShortestIdPrefix, TagProperty, Template,
    TemplateProperty, TemplatePropertyFn, WorkingCopiesProperty,
};

struct CommitTemplateLanguage<'a, 'b> {
    repo: &'a dyn Repo,
    workspace_id: &'b WorkspaceId,
}

impl<'a> TemplateLanguage<'a> for CommitTemplateLanguage<'a, '_> {
    type Context = Commit;
    type Property = CommitTemplatePropertyKind<'a>;

    // TODO: maybe generate wrap_<type>() by macro?
    fn wrap_string(
        &self,
        property: Box<dyn TemplateProperty<Self::Context, Output = String> + 'a>,
    ) -> Self::Property {
        CommitTemplatePropertyKind::Core(CoreTemplatePropertyKind::String(property))
    }
    fn wrap_boolean(
        &self,
        property: Box<dyn TemplateProperty<Self::Context, Output = bool> + 'a>,
    ) -> Self::Property {
        CommitTemplatePropertyKind::Core(CoreTemplatePropertyKind::Boolean(property))
    }
    fn wrap_integer(
        &self,
        property: Box<dyn TemplateProperty<Self::Context, Output = i64> + 'a>,
    ) -> Self::Property {
        CommitTemplatePropertyKind::Core(CoreTemplatePropertyKind::Integer(property))
    }
    fn wrap_signature(
        &self,
        property: Box<dyn TemplateProperty<Self::Context, Output = Signature> + 'a>,
    ) -> Self::Property {
        CommitTemplatePropertyKind::Core(CoreTemplatePropertyKind::Signature(property))
    }
    fn wrap_timestamp(
        &self,
        property: Box<dyn TemplateProperty<Self::Context, Output = Timestamp> + 'a>,
    ) -> Self::Property {
        CommitTemplatePropertyKind::Core(CoreTemplatePropertyKind::Timestamp(property))
    }

    fn build_keyword(&self, name: &str, span: pest::Span) -> TemplateParseResult<Self::Property> {
        build_commit_keyword(self, name, span)
    }

    fn build_method(
        &self,
        property: Self::Property,
        function: &FunctionCallNode,
    ) -> TemplateParseResult<Self::Property> {
        match property {
            CommitTemplatePropertyKind::Core(property) => {
                template_parser::build_core_method(self, property, function)
            }
            CommitTemplatePropertyKind::CommitOrChangeId(property) => {
                build_commit_or_change_id_method(self, property, function)
            }
            CommitTemplatePropertyKind::ShortestIdPrefix(property) => {
                build_shortest_id_prefix_method(self, property, function)
            }
        }
    }
}

// If we need to add multiple languages that support Commit types, this can be
// turned into a trait which extends TemplateLanguage.
impl<'a> CommitTemplateLanguage<'a, '_> {
    fn wrap_commit_or_change_id(
        &self,
        property: Box<dyn TemplateProperty<Commit, Output = CommitOrChangeId<'a>> + 'a>,
    ) -> CommitTemplatePropertyKind<'a> {
        CommitTemplatePropertyKind::CommitOrChangeId(property)
    }

    fn wrap_shortest_id_prefix(
        &self,
        property: Box<dyn TemplateProperty<Commit, Output = ShortestIdPrefix> + 'a>,
    ) -> CommitTemplatePropertyKind<'a> {
        CommitTemplatePropertyKind::ShortestIdPrefix(property)
    }
}

enum CommitTemplatePropertyKind<'a> {
    Core(CoreTemplatePropertyKind<'a, Commit>),
    CommitOrChangeId(Box<dyn TemplateProperty<Commit, Output = CommitOrChangeId<'a>> + 'a>),
    ShortestIdPrefix(Box<dyn TemplateProperty<Commit, Output = ShortestIdPrefix> + 'a>),
}

impl<'a> IntoTemplateProperty<'a, Commit> for CommitTemplatePropertyKind<'a> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<Commit, Output = bool> + 'a>> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.try_into_boolean(),
            _ => None,
        }
    }

    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<Commit, Output = i64> + 'a>> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.try_into_integer(),
            _ => None,
        }
    }

    fn into_plain_text(self) -> Box<dyn TemplateProperty<Commit, Output = String> + 'a> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.into_plain_text(),
            _ => Box::new(PlainTextFormattedProperty::new(self.into_template())),
        }
    }

    fn into_template(self) -> Box<dyn Template<Commit> + 'a> {
        fn wrap<'a, O: Template<()> + 'a>(
            property: Box<dyn TemplateProperty<Commit, Output = O> + 'a>,
        ) -> Box<dyn Template<Commit> + 'a> {
            Box::new(FormattablePropertyTemplate::new(property))
        }
        match self {
            CommitTemplatePropertyKind::Core(property) => property.into_template(),
            CommitTemplatePropertyKind::CommitOrChangeId(property) => wrap(property),
            CommitTemplatePropertyKind::ShortestIdPrefix(property) => wrap(property),
        }
    }
}

fn build_commit_keyword<'a>(
    language: &CommitTemplateLanguage<'a, '_>,
    name: &str,
    span: pest::Span,
) -> TemplateParseResult<CommitTemplatePropertyKind<'a>> {
    fn wrap_fn<'a, O>(
        f: impl Fn(&Commit) -> O + 'a,
    ) -> Box<dyn TemplateProperty<Commit, Output = O> + 'a> {
        Box::new(TemplatePropertyFn(f))
    }
    let repo = language.repo;
    let property = match name {
        "description" => language.wrap_string(wrap_fn(|commit| {
            cli_util::complete_newline(commit.description())
        })),
        "change_id" => language.wrap_commit_or_change_id(wrap_fn(move |commit| {
            CommitOrChangeId::change_id(repo, commit.change_id())
        })),
        "commit_id" => language.wrap_commit_or_change_id(wrap_fn(move |commit| {
            CommitOrChangeId::commit_id(repo, commit.id())
        })),
        "author" => language.wrap_signature(wrap_fn(|commit| commit.author().clone())),
        "committer" => language.wrap_signature(wrap_fn(|commit| commit.committer().clone())),
        "working_copies" => language.wrap_string(Box::new(WorkingCopiesProperty { repo })),
        "current_working_copy" => {
            let workspace_id = language.workspace_id.clone();
            language.wrap_boolean(wrap_fn(move |commit| {
                Some(commit.id()) == repo.view().get_wc_commit_id(&workspace_id)
            }))
        }
        "branches" => language.wrap_string(Box::new(BranchProperty { repo })),
        "tags" => language.wrap_string(Box::new(TagProperty { repo })),
        "git_refs" => language.wrap_string(Box::new(GitRefsProperty { repo })),
        "git_head" => language.wrap_string(Box::new(GitHeadProperty::new(repo))),
        "divergent" => language.wrap_boolean(wrap_fn(move |commit| {
            // The given commit could be hidden in e.g. obslog.
            let maybe_entries = repo.resolve_change_id(commit.change_id());
            maybe_entries.map_or(0, |entries| entries.len()) > 1
        })),
        "conflict" => language.wrap_boolean(wrap_fn(|commit| commit.tree().has_conflict())),
        "empty" => language.wrap_boolean(wrap_fn(move |commit| {
            commit.tree().id() == rewrite::merge_commit_trees(repo, &commit.parents()).id()
        })),
        _ => return Err(TemplateParseError::no_such_keyword(name, span)),
    };
    Ok(property)
}

fn build_commit_or_change_id_method<'a>(
    language: &CommitTemplateLanguage<'a, '_>,
    self_property: impl TemplateProperty<Commit, Output = CommitOrChangeId<'a>> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<CommitTemplatePropertyKind<'a>> {
    let parse_optional_integer = |function| -> Result<Option<_>, TemplateParseError> {
        let ([], [len_node]) = template_parser::expect_arguments(function)?;
        len_node
            .map(|node| {
                template_parser::build_expression(language, node).and_then(|p| {
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
            language.wrap_string(template_parser::chain_properties(
                (self_property, len_property),
                TemplatePropertyFn(|(id, len): &(CommitOrChangeId, Option<i64>)| {
                    id.short(len.and_then(|l| l.try_into().ok()).unwrap_or(12))
                }),
            ))
        }
        "shortest" => {
            let len_property = parse_optional_integer(function)?;
            language.wrap_shortest_id_prefix(template_parser::chain_properties(
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

fn build_shortest_id_prefix_method<'a>(
    language: &CommitTemplateLanguage<'a, '_>,
    self_property: impl TemplateProperty<Commit, Output = ShortestIdPrefix> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<CommitTemplatePropertyKind<'a>> {
    let property = match function.name {
        "prefix" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_string(template_parser::chain_properties(
                self_property,
                TemplatePropertyFn(|id: &ShortestIdPrefix| id.prefix.clone()),
            ))
        }
        "rest" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_string(template_parser::chain_properties(
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

pub fn parse<'a>(
    repo: &'a dyn Repo,
    workspace_id: &WorkspaceId,
    template_text: &str,
    aliases_map: &TemplateAliasesMap,
) -> TemplateParseResult<Box<dyn Template<Commit> + 'a>> {
    let language = CommitTemplateLanguage { repo, workspace_id };
    let node = template_parser::parse_template(template_text)?;
    let node = template_parser::expand_aliases(node, aliases_map)?;
    let expression = template_parser::build_expression(&language, &node)?;
    Ok(expression.into_template())
}
