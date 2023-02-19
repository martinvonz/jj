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

use std::cmp::max;
use std::io;

use itertools::Itertools as _;
use jujutsu_lib::backend::{ChangeId, CommitId, ObjectId as _, Signature, Timestamp};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::hex_util::to_reverse_hex;
use jujutsu_lib::op_store::WorkspaceId;
use jujutsu_lib::repo::Repo;
use jujutsu_lib::rewrite;

use crate::cli_util;
use crate::formatter::Formatter;
use crate::template_parser::{
    self, CoreTemplatePropertyKind, FunctionCallNode, IntoTemplateProperty, TemplateAliasesMap,
    TemplateLanguage, TemplateParseError, TemplateParseResult,
};
use crate::templater::{
    FormattablePropertyTemplate, PlainTextFormattedProperty, Template, TemplateProperty,
    TemplatePropertyFn,
};

struct CommitTemplateLanguage<'repo, 'b> {
    repo: &'repo dyn Repo,
    workspace_id: &'b WorkspaceId,
}

impl<'repo> TemplateLanguage<'repo> for CommitTemplateLanguage<'repo, '_> {
    type Context = Commit;
    type Property = CommitTemplatePropertyKind<'repo>;

    // TODO: maybe generate wrap_<type>() by macro?
    fn wrap_string(
        &self,
        property: Box<dyn TemplateProperty<Self::Context, Output = String> + 'repo>,
    ) -> Self::Property {
        CommitTemplatePropertyKind::Core(CoreTemplatePropertyKind::String(property))
    }
    fn wrap_boolean(
        &self,
        property: Box<dyn TemplateProperty<Self::Context, Output = bool> + 'repo>,
    ) -> Self::Property {
        CommitTemplatePropertyKind::Core(CoreTemplatePropertyKind::Boolean(property))
    }
    fn wrap_integer(
        &self,
        property: Box<dyn TemplateProperty<Self::Context, Output = i64> + 'repo>,
    ) -> Self::Property {
        CommitTemplatePropertyKind::Core(CoreTemplatePropertyKind::Integer(property))
    }
    fn wrap_signature(
        &self,
        property: Box<dyn TemplateProperty<Self::Context, Output = Signature> + 'repo>,
    ) -> Self::Property {
        CommitTemplatePropertyKind::Core(CoreTemplatePropertyKind::Signature(property))
    }
    fn wrap_timestamp(
        &self,
        property: Box<dyn TemplateProperty<Self::Context, Output = Timestamp> + 'repo>,
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
impl<'repo> CommitTemplateLanguage<'repo, '_> {
    fn wrap_commit_or_change_id(
        &self,
        property: Box<dyn TemplateProperty<Commit, Output = CommitOrChangeId<'repo>> + 'repo>,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::CommitOrChangeId(property)
    }

    fn wrap_shortest_id_prefix(
        &self,
        property: Box<dyn TemplateProperty<Commit, Output = ShortestIdPrefix> + 'repo>,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::ShortestIdPrefix(property)
    }
}

enum CommitTemplatePropertyKind<'repo> {
    Core(CoreTemplatePropertyKind<'repo, Commit>),
    CommitOrChangeId(Box<dyn TemplateProperty<Commit, Output = CommitOrChangeId<'repo>> + 'repo>),
    ShortestIdPrefix(Box<dyn TemplateProperty<Commit, Output = ShortestIdPrefix> + 'repo>),
}

impl<'repo> IntoTemplateProperty<'repo, Commit> for CommitTemplatePropertyKind<'repo> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<Commit, Output = bool> + 'repo>> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.try_into_boolean(),
            _ => None,
        }
    }

    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<Commit, Output = i64> + 'repo>> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.try_into_integer(),
            _ => None,
        }
    }

    fn into_plain_text(self) -> Box<dyn TemplateProperty<Commit, Output = String> + 'repo> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.into_plain_text(),
            _ => Box::new(PlainTextFormattedProperty::new(self.into_template())),
        }
    }

    fn into_template(self) -> Box<dyn Template<Commit> + 'repo> {
        fn wrap<'repo, O: Template<()> + 'repo>(
            property: Box<dyn TemplateProperty<Commit, Output = O> + 'repo>,
        ) -> Box<dyn Template<Commit> + 'repo> {
            Box::new(FormattablePropertyTemplate::new(property))
        }
        match self {
            CommitTemplatePropertyKind::Core(property) => property.into_template(),
            CommitTemplatePropertyKind::CommitOrChangeId(property) => wrap(property),
            CommitTemplatePropertyKind::ShortestIdPrefix(property) => wrap(property),
        }
    }
}

fn build_commit_keyword<'repo>(
    language: &CommitTemplateLanguage<'repo, '_>,
    name: &str,
    span: pest::Span,
) -> TemplateParseResult<CommitTemplatePropertyKind<'repo>> {
    fn wrap_fn<'repo, O>(
        f: impl Fn(&Commit) -> O + 'repo,
    ) -> Box<dyn TemplateProperty<Commit, Output = O> + 'repo> {
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

struct WorkingCopiesProperty<'repo> {
    pub repo: &'repo dyn Repo,
}

impl TemplateProperty<Commit> for WorkingCopiesProperty<'_> {
    type Output = String;

    fn extract(&self, context: &Commit) -> Self::Output {
        let wc_commit_ids = self.repo.view().wc_commit_ids();
        if wc_commit_ids.len() <= 1 {
            return "".to_string();
        }
        let mut names = vec![];
        for (workspace_id, wc_commit_id) in wc_commit_ids.iter().sorted() {
            if wc_commit_id == context.id() {
                names.push(format!("{}@", workspace_id.as_str()));
            }
        }
        names.join(" ")
    }
}

struct BranchProperty<'repo> {
    pub repo: &'repo dyn Repo,
}

impl TemplateProperty<Commit> for BranchProperty<'_> {
    type Output = String;

    fn extract(&self, context: &Commit) -> Self::Output {
        let mut names = vec![];
        for (branch_name, branch_target) in self.repo.view().branches() {
            let local_target = branch_target.local_target.as_ref();
            if let Some(local_target) = local_target {
                if local_target.has_add(context.id()) {
                    if local_target.is_conflict() {
                        names.push(format!("{branch_name}??"));
                    } else if branch_target
                        .remote_targets
                        .values()
                        .any(|remote_target| remote_target != local_target)
                    {
                        names.push(format!("{branch_name}*"));
                    } else {
                        names.push(branch_name.clone());
                    }
                }
            }
            for (remote_name, remote_target) in &branch_target.remote_targets {
                if Some(remote_target) != local_target && remote_target.has_add(context.id()) {
                    if remote_target.is_conflict() {
                        names.push(format!("{branch_name}@{remote_name}?"));
                    } else {
                        names.push(format!("{branch_name}@{remote_name}"));
                    }
                }
            }
        }
        names.join(" ")
    }
}

struct TagProperty<'repo> {
    pub repo: &'repo dyn Repo,
}

impl TemplateProperty<Commit> for TagProperty<'_> {
    type Output = String;

    fn extract(&self, context: &Commit) -> Self::Output {
        let mut names = vec![];
        for (tag_name, target) in self.repo.view().tags() {
            if target.has_add(context.id()) {
                if target.is_conflict() {
                    names.push(format!("{tag_name}?"));
                } else {
                    names.push(tag_name.clone());
                }
            }
        }
        names.join(" ")
    }
}

struct GitRefsProperty<'repo> {
    pub repo: &'repo dyn Repo,
}

impl TemplateProperty<Commit> for GitRefsProperty<'_> {
    type Output = String;

    fn extract(&self, context: &Commit) -> Self::Output {
        // TODO: We should keep a map from commit to ref names so we don't have to walk
        // all refs here.
        let mut names = vec![];
        for (name, target) in self.repo.view().git_refs() {
            if target.has_add(context.id()) {
                if target.is_conflict() {
                    names.push(format!("{name}?"));
                } else {
                    names.push(name.clone());
                }
            }
        }
        names.join(" ")
    }
}

struct GitHeadProperty<'repo> {
    repo: &'repo dyn Repo,
}

impl<'repo> GitHeadProperty<'repo> {
    pub fn new(repo: &'repo dyn Repo) -> Self {
        Self { repo }
    }
}

impl TemplateProperty<Commit> for GitHeadProperty<'_> {
    type Output = String;

    fn extract(&self, context: &Commit) -> String {
        match self.repo.view().git_head() {
            Some(ref_target) if ref_target.has_add(context.id()) => {
                if ref_target.is_conflict() {
                    "HEAD@git?".to_string()
                } else {
                    "HEAD@git".to_string()
                }
            }
            _ => "".to_string(),
        }
    }
}

/// Type-erased `CommitId`/`ChangeId`.
#[derive(Clone)]
struct CommitOrChangeId<'repo> {
    repo: &'repo dyn Repo,
    id_bytes: Vec<u8>,
    is_commit_id: bool,
}

impl<'repo> CommitOrChangeId<'repo> {
    pub fn commit_id(repo: &'repo dyn Repo, id: &CommitId) -> Self {
        CommitOrChangeId {
            repo,
            id_bytes: id.to_bytes(),
            is_commit_id: true,
        }
    }

    pub fn change_id(repo: &'repo dyn Repo, id: &ChangeId) -> Self {
        CommitOrChangeId {
            repo,
            id_bytes: id.to_bytes(),
            is_commit_id: false,
        }
    }

    pub fn hex(&self) -> String {
        if self.is_commit_id {
            hex::encode(&self.id_bytes)
        } else {
            // TODO: We can avoid the unwrap() and make this more efficient by converting
            // straight from bytes.
            to_reverse_hex(&hex::encode(&self.id_bytes)).unwrap()
        }
    }

    pub fn short(&self, total_len: usize) -> String {
        let mut hex = self.hex();
        hex.truncate(total_len);
        hex
    }

    /// The length of the id printed will be the maximum of `total_len` and the
    /// length of the shortest unique prefix
    pub fn shortest(&self, total_len: usize) -> ShortestIdPrefix {
        let mut hex = self.hex();
        let prefix_len = if self.is_commit_id {
            self.repo
                .index()
                .shortest_unique_commit_id_prefix_len(&CommitId::from_bytes(&self.id_bytes))
        } else {
            self.repo
                .shortest_unique_change_id_prefix_len(&ChangeId::from_bytes(&self.id_bytes))
        };
        hex.truncate(max(prefix_len, total_len));
        let rest = hex.split_off(prefix_len);
        ShortestIdPrefix { prefix: hex, rest }
    }
}

impl Template<()> for CommitOrChangeId<'_> {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(&self.hex())
    }

    fn has_content(&self, _: &()) -> bool {
        !self.id_bytes.is_empty()
    }
}

fn build_commit_or_change_id_method<'repo>(
    language: &CommitTemplateLanguage<'repo, '_>,
    self_property: impl TemplateProperty<Commit, Output = CommitOrChangeId<'repo>> + 'repo,
    function: &FunctionCallNode,
) -> TemplateParseResult<CommitTemplatePropertyKind<'repo>> {
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

struct ShortestIdPrefix {
    pub prefix: String,
    pub rest: String,
}

impl Template<()> for ShortestIdPrefix {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.with_label("prefix", |fmt| fmt.write_str(&self.prefix))?;
        formatter.with_label("rest", |fmt| fmt.write_str(&self.rest))
    }

    fn has_content(&self, _: &()) -> bool {
        !self.prefix.is_empty() || !self.rest.is_empty()
    }
}

fn build_shortest_id_prefix_method<'repo>(
    language: &CommitTemplateLanguage<'repo, '_>,
    self_property: impl TemplateProperty<Commit, Output = ShortestIdPrefix> + 'repo,
    function: &FunctionCallNode,
) -> TemplateParseResult<CommitTemplatePropertyKind<'repo>> {
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

pub fn parse<'repo>(
    repo: &'repo dyn Repo,
    workspace_id: &WorkspaceId,
    template_text: &str,
    aliases_map: &TemplateAliasesMap,
) -> TemplateParseResult<Box<dyn Template<Commit> + 'repo>> {
    let language = CommitTemplateLanguage { repo, workspace_id };
    let node = template_parser::parse_template(template_text)?;
    let node = template_parser::expand_aliases(node, aliases_map)?;
    let expression = template_parser::build_expression(&language, &node)?;
    Ok(expression.into_template())
}
