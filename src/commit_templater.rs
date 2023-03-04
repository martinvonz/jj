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
use jujutsu_lib::backend::{ChangeId, CommitId, ObjectId as _};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::hex_util::to_reverse_hex;
use jujutsu_lib::op_store::WorkspaceId;
use jujutsu_lib::repo::Repo;
use jujutsu_lib::rewrite;

use crate::formatter::Formatter;
use crate::template_parser::{
    self, CoreTemplatePropertyKind, FunctionCallNode, IntoTemplateProperty, TemplateAliasesMap,
    TemplateLanguage, TemplateParseError, TemplateParseResult,
};
use crate::templater::{
    self, IntoTemplate, PlainTextFormattedProperty, Template, TemplateFunction, TemplateProperty,
    TemplatePropertyFn,
};
use crate::text_util;

struct CommitTemplateLanguage<'repo, 'b> {
    repo: &'repo dyn Repo,
    workspace_id: &'b WorkspaceId,
}

impl<'repo> TemplateLanguage<'repo> for CommitTemplateLanguage<'repo, '_> {
    type Context = Commit;
    type Property = CommitTemplatePropertyKind<'repo>;

    template_parser::impl_core_wrap_property_fns!('repo, CommitTemplatePropertyKind::Core);

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
            CommitTemplatePropertyKind::CommitOrChangeIdList(property) => {
                template_parser::build_list_method(self, property, function)
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
        property: impl TemplateProperty<Commit, Output = CommitOrChangeId<'repo>> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::CommitOrChangeId(Box::new(property))
    }

    fn wrap_commit_or_change_id_list(
        &self,
        property: impl TemplateProperty<Commit, Output = Vec<CommitOrChangeId<'repo>>> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::CommitOrChangeIdList(Box::new(property))
    }

    fn wrap_shortest_id_prefix(
        &self,
        property: impl TemplateProperty<Commit, Output = ShortestIdPrefix> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::ShortestIdPrefix(Box::new(property))
    }
}

enum CommitTemplatePropertyKind<'repo> {
    Core(CoreTemplatePropertyKind<'repo, Commit>),
    CommitOrChangeId(Box<dyn TemplateProperty<Commit, Output = CommitOrChangeId<'repo>> + 'repo>),
    CommitOrChangeIdList(
        Box<dyn TemplateProperty<Commit, Output = Vec<CommitOrChangeId<'repo>>> + 'repo>,
    ),
    ShortestIdPrefix(Box<dyn TemplateProperty<Commit, Output = ShortestIdPrefix> + 'repo>),
}

impl<'repo> IntoTemplateProperty<'repo, Commit> for CommitTemplatePropertyKind<'repo> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<Commit, Output = bool> + 'repo>> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.try_into_boolean(),
            // TODO: should we allow implicit cast of List type?
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
}

impl<'repo> IntoTemplate<'repo, Commit> for CommitTemplatePropertyKind<'repo> {
    fn into_template(self) -> Box<dyn Template<Commit> + 'repo> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.into_template(),
            CommitTemplatePropertyKind::CommitOrChangeId(property) => property.into_template(),
            CommitTemplatePropertyKind::CommitOrChangeIdList(property) => property.into_template(),
            CommitTemplatePropertyKind::ShortestIdPrefix(property) => property.into_template(),
        }
    }
}

fn build_commit_keyword<'repo>(
    language: &CommitTemplateLanguage<'repo, '_>,
    name: &str,
    span: pest::Span,
) -> TemplateParseResult<CommitTemplatePropertyKind<'repo>> {
    fn wrap_fn<O, F: Fn(&Commit) -> O>(f: F) -> TemplatePropertyFn<F> {
        TemplatePropertyFn(f)
    }
    fn wrap_repo_fn<'repo, O>(
        repo: &'repo dyn Repo,
        f: impl Fn(&dyn Repo, &Commit) -> O + 'repo,
    ) -> impl TemplateProperty<Commit, Output = O> + 'repo {
        wrap_fn(move |commit| f(repo, commit))
    }

    let repo = language.repo;
    let property = match name {
        "description" => language.wrap_string(wrap_fn(|commit| {
            text_util::complete_newline(commit.description())
        })),
        "change_id" => language.wrap_commit_or_change_id(wrap_fn(move |commit| {
            CommitOrChangeId::new(repo, IdKind::Change(commit.change_id().to_owned()))
        })),
        "commit_id" => language.wrap_commit_or_change_id(wrap_fn(move |commit| {
            CommitOrChangeId::new(repo, IdKind::Commit(commit.id().to_owned()))
        })),
        "parent_commit_ids" => language.wrap_commit_or_change_id_list(wrap_fn(move |commit| {
            commit
                .parent_ids()
                .iter()
                .map(|id| CommitOrChangeId::new(repo, IdKind::Commit(id.to_owned())))
                .collect()
        })),
        "author" => language.wrap_signature(wrap_fn(|commit| commit.author().clone())),
        "committer" => language.wrap_signature(wrap_fn(|commit| commit.committer().clone())),
        "working_copies" => language.wrap_string(wrap_repo_fn(repo, extract_working_copies)),
        "current_working_copy" => {
            let workspace_id = language.workspace_id.clone();
            language.wrap_boolean(wrap_fn(move |commit| {
                Some(commit.id()) == repo.view().get_wc_commit_id(&workspace_id)
            }))
        }
        "branches" => language.wrap_string(wrap_repo_fn(repo, extract_branches)),
        "tags" => language.wrap_string(wrap_repo_fn(repo, extract_tags)),
        "git_refs" => language.wrap_string(wrap_repo_fn(repo, extract_git_refs)),
        "git_head" => language.wrap_string(wrap_repo_fn(repo, extract_git_head)),
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

// TODO: return Vec<String>
fn extract_working_copies(repo: &dyn Repo, commit: &Commit) -> String {
    let wc_commit_ids = repo.view().wc_commit_ids();
    if wc_commit_ids.len() <= 1 {
        return "".to_string();
    }
    let mut names = vec![];
    for (workspace_id, wc_commit_id) in wc_commit_ids.iter().sorted() {
        if wc_commit_id == commit.id() {
            names.push(format!("{}@", workspace_id.as_str()));
        }
    }
    names.join(" ")
}

// TODO: return Vec<Branch>?
fn extract_branches(repo: &dyn Repo, commit: &Commit) -> String {
    let mut names = vec![];
    for (branch_name, branch_target) in repo.view().branches() {
        let local_target = branch_target.local_target.as_ref();
        if let Some(local_target) = local_target {
            if local_target.has_add(commit.id()) {
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
            if Some(remote_target) != local_target && remote_target.has_add(commit.id()) {
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

// TODO: return Vec<NameRef>?
fn extract_tags(repo: &dyn Repo, commit: &Commit) -> String {
    let mut names = vec![];
    for (tag_name, target) in repo.view().tags() {
        if target.has_add(commit.id()) {
            if target.is_conflict() {
                names.push(format!("{tag_name}?"));
            } else {
                names.push(tag_name.clone());
            }
        }
    }
    names.join(" ")
}

// TODO: return Vec<NameRef>?
fn extract_git_refs(repo: &dyn Repo, commit: &Commit) -> String {
    // TODO: We should keep a map from commit to ref names so we don't have to walk
    // all refs here.
    let mut names = vec![];
    for (name, target) in repo.view().git_refs() {
        if target.has_add(commit.id()) {
            if target.is_conflict() {
                names.push(format!("{name}?"));
            } else {
                names.push(name.clone());
            }
        }
    }
    names.join(" ")
}

// TODO: return NameRef?
fn extract_git_head(repo: &dyn Repo, commit: &Commit) -> String {
    match repo.view().git_head() {
        Some(ref_target) if ref_target.has_add(commit.id()) => {
            if ref_target.is_conflict() {
                "HEAD@git?".to_string()
            } else {
                "HEAD@git".to_string()
            }
        }
        _ => "".to_string(),
    }
}

#[derive(Clone)]
struct CommitOrChangeId<'repo> {
    repo: &'repo dyn Repo,
    id: IdKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum IdKind {
    Commit(CommitId),
    Change(ChangeId),
}

impl<'repo> CommitOrChangeId<'repo> {
    pub fn new(repo: &'repo dyn Repo, id: IdKind) -> Self {
        CommitOrChangeId { repo, id }
    }

    pub fn hex(&self) -> String {
        match &self.id {
            IdKind::Commit(id) => id.hex(),
            IdKind::Change(id) => {
                // TODO: We can avoid the unwrap() and make this more efficient by converting
                // straight from bytes.
                to_reverse_hex(&id.hex()).unwrap()
            }
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
        let prefix_len = match &self.id {
            IdKind::Commit(id) => self.repo.index().shortest_unique_commit_id_prefix_len(id),
            IdKind::Change(id) => self.repo.shortest_unique_change_id_prefix_len(id),
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
}

impl Template<()> for Vec<CommitOrChangeId<'_>> {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        templater::format_joined(&(), formatter, self, " ")
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
            .map(|node| template_parser::expect_integer_expression(language, node))
            .transpose()
    };
    let property = match function.name {
        "short" => {
            let len_property = parse_optional_integer(function)?;
            language.wrap_string(TemplateFunction::new(
                (self_property, len_property),
                |(id, len)| id.short(len.and_then(|l| l.try_into().ok()).unwrap_or(12)),
            ))
        }
        "shortest" => {
            let len_property = parse_optional_integer(function)?;
            language.wrap_shortest_id_prefix(TemplateFunction::new(
                (self_property, len_property),
                |(id, len)| id.shortest(len.and_then(|l| l.try_into().ok()).unwrap_or(0)),
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
}

impl ShortestIdPrefix {
    fn to_upper(&self) -> Self {
        Self {
            prefix: self.prefix.to_ascii_uppercase(),
            rest: self.rest.to_ascii_uppercase(),
        }
    }
    fn to_lower(&self) -> Self {
        Self {
            prefix: self.prefix.to_ascii_lowercase(),
            rest: self.rest.to_ascii_lowercase(),
        }
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
            language.wrap_string(TemplateFunction::new(self_property, |id| id.prefix))
        }
        "rest" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |id| id.rest))
        }
        "upper" => {
            template_parser::expect_no_arguments(function)?;
            language
                .wrap_shortest_id_prefix(TemplateFunction::new(self_property, |id| id.to_upper()))
        }
        "lower" => {
            template_parser::expect_no_arguments(function)?;
            language
                .wrap_shortest_id_prefix(TemplateFunction::new(self_property, |id| id.to_lower()))
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
