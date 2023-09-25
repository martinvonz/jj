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
use std::collections::HashMap;
use std::io;
use std::rc::Rc;

use itertools::Itertools as _;
use jj_lib::backend::{ChangeId, CommitId, ObjectId as _};
use jj_lib::commit::Commit;
use jj_lib::hex_util::to_reverse_hex;
use jj_lib::id_prefix::IdPrefixContext;
use jj_lib::op_store::{RefTarget, WorkspaceId};
use jj_lib::repo::Repo;
use jj_lib::rewrite;
use once_cell::unsync::OnceCell;

use crate::formatter::Formatter;
use crate::template_builder::{
    self, BuildContext, CoreTemplatePropertyKind, IntoTemplateProperty, TemplateLanguage,
};
use crate::template_parser::{
    self, FunctionCallNode, TemplateAliasesMap, TemplateParseError, TemplateParseResult,
};
use crate::templater::{
    IntoTemplate, PlainTextFormattedProperty, Template, TemplateFunction, TemplateProperty,
    TemplatePropertyFn,
};
use crate::text_util;

struct CommitTemplateLanguage<'repo, 'b> {
    repo: &'repo dyn Repo,
    workspace_id: &'b WorkspaceId,
    id_prefix_context: &'repo IdPrefixContext,
    keyword_cache: CommitKeywordCache,
}

impl<'repo> TemplateLanguage<'repo> for CommitTemplateLanguage<'repo, '_> {
    type Context = Commit;
    type Property = CommitTemplatePropertyKind<'repo>;

    template_builder::impl_core_wrap_property_fns!('repo, CommitTemplatePropertyKind::Core);

    fn build_keyword(&self, name: &str, span: pest::Span) -> TemplateParseResult<Self::Property> {
        build_commit_keyword(self, name, span)
    }

    fn build_method(
        &self,
        build_ctx: &BuildContext<Self::Property>,
        property: Self::Property,
        function: &FunctionCallNode,
    ) -> TemplateParseResult<Self::Property> {
        match property {
            CommitTemplatePropertyKind::Core(property) => {
                template_builder::build_core_method(self, build_ctx, property, function)
            }
            CommitTemplatePropertyKind::Commit(property) => {
                build_commit_method(self, build_ctx, property, function)
            }
            CommitTemplatePropertyKind::CommitList(property) => {
                template_builder::build_unformattable_list_method(
                    self,
                    build_ctx,
                    property,
                    function,
                    |item| self.wrap_commit(item),
                )
            }
            CommitTemplatePropertyKind::CommitOrChangeId(property) => {
                build_commit_or_change_id_method(self, build_ctx, property, function)
            }
            CommitTemplatePropertyKind::ShortestIdPrefix(property) => {
                build_shortest_id_prefix_method(self, build_ctx, property, function)
            }
        }
    }
}

// If we need to add multiple languages that support Commit types, this can be
// turned into a trait which extends TemplateLanguage.
impl<'repo> CommitTemplateLanguage<'repo, '_> {
    fn wrap_commit(
        &self,
        property: impl TemplateProperty<Commit, Output = Commit> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::Commit(Box::new(property))
    }

    fn wrap_commit_list(
        &self,
        property: impl TemplateProperty<Commit, Output = Vec<Commit>> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::CommitList(Box::new(property))
    }

    fn wrap_commit_or_change_id(
        &self,
        property: impl TemplateProperty<Commit, Output = CommitOrChangeId> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::CommitOrChangeId(Box::new(property))
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
    Commit(Box<dyn TemplateProperty<Commit, Output = Commit> + 'repo>),
    CommitList(Box<dyn TemplateProperty<Commit, Output = Vec<Commit>> + 'repo>),
    CommitOrChangeId(Box<dyn TemplateProperty<Commit, Output = CommitOrChangeId> + 'repo>),
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

    fn try_into_plain_text(
        self,
    ) -> Option<Box<dyn TemplateProperty<Commit, Output = String> + 'repo>> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.try_into_plain_text(),
            _ => {
                let template = self.try_into_template()?;
                Some(Box::new(PlainTextFormattedProperty::new(template)))
            }
        }
    }

    fn try_into_template(self) -> Option<Box<dyn Template<Commit> + 'repo>> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.try_into_template(),
            CommitTemplatePropertyKind::Commit(_) => None,
            CommitTemplatePropertyKind::CommitList(_) => None,
            CommitTemplatePropertyKind::CommitOrChangeId(property) => {
                Some(property.into_template())
            }
            CommitTemplatePropertyKind::ShortestIdPrefix(property) => {
                Some(property.into_template())
            }
        }
    }
}

#[derive(Debug, Default)]
struct CommitKeywordCache {
    // Build index lazily, and Rc to get away from &self lifetime.
    branches_index: OnceCell<Rc<RefNamesIndex>>,
    tags_index: OnceCell<Rc<RefNamesIndex>>,
    git_refs_index: OnceCell<Rc<RefNamesIndex>>,
}

impl CommitKeywordCache {
    fn branches_index(&self, repo: &dyn Repo) -> &Rc<RefNamesIndex> {
        self.branches_index
            .get_or_init(|| Rc::new(build_branches_index(repo)))
    }

    fn tags_index(&self, repo: &dyn Repo) -> &Rc<RefNamesIndex> {
        self.tags_index
            .get_or_init(|| Rc::new(build_ref_names_index(repo.view().tags())))
    }

    fn git_refs_index(&self, repo: &dyn Repo) -> &Rc<RefNamesIndex> {
        self.git_refs_index
            .get_or_init(|| Rc::new(build_ref_names_index(repo.view().git_refs())))
    }
}

fn build_commit_keyword<'repo>(
    language: &CommitTemplateLanguage<'repo, '_>,
    name: &str,
    span: pest::Span,
) -> TemplateParseResult<CommitTemplatePropertyKind<'repo>> {
    // Commit object is lightweight (a few Arc + CommitId), so just clone it
    // to turn into a property type. Abstraction over "for<'a> (&'a T) -> &'a T"
    // and "(&T) -> T" wouldn't be simple. If we want to remove Clone/Rc/Arc,
    // maybe we can add an abstraction that takes "Fn(&Commit) -> O" and returns
    // "TemplateProperty<Commit, Output = O>".
    let property = TemplatePropertyFn(|commit: &Commit| commit.clone());
    build_commit_keyword_opt(language, property, name)
        .ok_or_else(|| TemplateParseError::no_such_keyword(name, span))
}

fn build_commit_method<'repo>(
    language: &CommitTemplateLanguage<'repo, '_>,
    _build_ctx: &BuildContext<CommitTemplatePropertyKind<'repo>>,
    self_property: impl TemplateProperty<Commit, Output = Commit> + 'repo,
    function: &FunctionCallNode,
) -> TemplateParseResult<CommitTemplatePropertyKind<'repo>> {
    if let Some(property) = build_commit_keyword_opt(language, self_property, function.name) {
        template_parser::expect_no_arguments(function)?;
        Ok(property)
    } else {
        Err(TemplateParseError::no_such_method("Commit", function))
    }
}

fn build_commit_keyword_opt<'repo>(
    language: &CommitTemplateLanguage<'repo, '_>,
    property: impl TemplateProperty<Commit, Output = Commit> + 'repo,
    name: &str,
) -> Option<CommitTemplatePropertyKind<'repo>> {
    fn wrap_fn<'repo, O>(
        property: impl TemplateProperty<Commit, Output = Commit> + 'repo,
        f: impl Fn(&Commit) -> O + 'repo,
    ) -> impl TemplateProperty<Commit, Output = O> + 'repo {
        TemplateFunction::new(property, move |commit| f(&commit))
    }
    fn wrap_repo_fn<'repo, O>(
        repo: &'repo dyn Repo,
        property: impl TemplateProperty<Commit, Output = Commit> + 'repo,
        f: impl Fn(&dyn Repo, &Commit) -> O + 'repo,
    ) -> impl TemplateProperty<Commit, Output = O> + 'repo {
        TemplateFunction::new(property, move |commit| f(repo, &commit))
    }

    let repo = language.repo;
    let cache = &language.keyword_cache;
    let property = match name {
        "description" => language.wrap_string(wrap_fn(property, |commit| {
            text_util::complete_newline(commit.description())
        })),
        "change_id" => language.wrap_commit_or_change_id(wrap_fn(property, |commit| {
            CommitOrChangeId::Change(commit.change_id().to_owned())
        })),
        "commit_id" => language.wrap_commit_or_change_id(wrap_fn(property, |commit| {
            CommitOrChangeId::Commit(commit.id().to_owned())
        })),
        "parents" => language.wrap_commit_list(wrap_fn(property, |commit| commit.parents())),
        "author" => language.wrap_signature(wrap_fn(property, |commit| commit.author().clone())),
        "committer" => {
            language.wrap_signature(wrap_fn(property, |commit| commit.committer().clone()))
        }
        "working_copies" => {
            language.wrap_string(wrap_repo_fn(repo, property, extract_working_copies))
        }
        "current_working_copy" => {
            let workspace_id = language.workspace_id.clone();
            language.wrap_boolean(wrap_fn(property, move |commit| {
                Some(commit.id()) == repo.view().get_wc_commit_id(&workspace_id)
            }))
        }
        "branches" => {
            let index = cache.branches_index(repo).clone();
            language.wrap_string(wrap_fn(property, move |commit| {
                index.get(commit.id()).join(" ") // TODO: return Vec<Branch>?
            }))
        }
        "tags" => {
            let index = cache.tags_index(repo).clone();
            language.wrap_string(wrap_fn(property, move |commit| {
                index.get(commit.id()).join(" ") // TODO: return Vec<NameRef>?
            }))
        }
        "git_refs" => {
            let index = cache.git_refs_index(repo).clone();
            language.wrap_string(wrap_fn(property, move |commit| {
                index.get(commit.id()).join(" ") // TODO: return Vec<NameRef>?
            }))
        }
        "git_head" => language.wrap_string(wrap_repo_fn(repo, property, extract_git_head)),
        "divergent" => language.wrap_boolean(wrap_fn(property, |commit| {
            // The given commit could be hidden in e.g. obslog.
            let maybe_entries = repo.resolve_change_id(commit.change_id());
            maybe_entries.map_or(0, |entries| entries.len()) > 1
        })),
        "hidden" => language.wrap_boolean(wrap_fn(property, |commit| {
            let maybe_entries = repo.resolve_change_id(commit.change_id());
            maybe_entries.map_or(true, |entries| !entries.contains(commit.id()))
        })),
        "conflict" => {
            language.wrap_boolean(wrap_fn(property, |commit| commit.has_conflict().unwrap()))
        }
        "empty" => language.wrap_boolean(wrap_fn(property, |commit| {
            if let [parent] = &commit.parents()[..] {
                return parent.tree_id() == commit.tree_id();
            }
            let parent_tree = rewrite::merge_commit_trees(repo, &commit.parents()).unwrap();
            *commit.tree_id() == parent_tree.id()
        })),
        "root" => language.wrap_boolean(wrap_fn(property, move |commit| {
            commit.id() == repo.store().root_commit_id()
        })),
        _ => return None,
    };
    Some(property)
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

/// Cache for reverse lookup refs.
#[derive(Clone, Debug, Default)]
struct RefNamesIndex {
    // TODO: store Branch/NameRef type in place of String?
    index: HashMap<CommitId, Vec<String>>,
}

impl RefNamesIndex {
    fn insert<'a>(&mut self, ids: impl IntoIterator<Item = &'a CommitId>, name: String) {
        for id in ids {
            let ref_names = self.index.entry(id.clone()).or_default();
            ref_names.push(name.clone());
        }
    }

    fn get(&self, id: &CommitId) -> &[String] {
        if let Some(names) = self.index.get(id) {
            names
        } else {
            &[]
        }
    }
}

fn build_branches_index(repo: &dyn Repo) -> RefNamesIndex {
    let mut index = RefNamesIndex::default();
    for (branch_name, branch_target) in repo.view().branches() {
        let local_target = &branch_target.local_target;
        let mut unsynced_remote_targets = branch_target
            .remote_targets
            .iter()
            .filter(|&(_, target)| target != local_target)
            .peekable();
        if local_target.is_present() {
            let decorated_name = if local_target.has_conflict() {
                format!("{branch_name}??")
            } else if unsynced_remote_targets.peek().is_some() {
                format!("{branch_name}*")
            } else {
                branch_name.clone()
            };
            index.insert(local_target.added_ids(), decorated_name);
        }
        for (remote_name, target) in unsynced_remote_targets {
            let decorated_name = if target.has_conflict() {
                format!("{branch_name}@{remote_name}?")
            } else {
                format!("{branch_name}@{remote_name}")
            };
            index.insert(target.added_ids(), decorated_name);
        }
    }
    index
}

fn build_ref_names_index<'a>(
    ref_pairs: impl IntoIterator<Item = (&'a String, &'a RefTarget)>,
) -> RefNamesIndex {
    let mut index = RefNamesIndex::default();
    for (name, target) in ref_pairs {
        let decorated_name = if target.has_conflict() {
            format!("{name}?")
        } else {
            name.clone()
        };
        index.insert(target.added_ids(), decorated_name);
    }
    index
}

// TODO: return NameRef?
fn extract_git_head(repo: &dyn Repo, commit: &Commit) -> String {
    let target = repo.view().git_head();
    if target.added_ids().contains(commit.id()) {
        if target.has_conflict() {
            "HEAD@git?".to_string()
        } else {
            "HEAD@git".to_string()
        }
    } else {
        "".to_string()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CommitOrChangeId {
    Commit(CommitId),
    Change(ChangeId),
}

impl CommitOrChangeId {
    pub fn hex(&self) -> String {
        match self {
            CommitOrChangeId::Commit(id) => id.hex(),
            CommitOrChangeId::Change(id) => {
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
    pub fn shortest(
        &self,
        repo: &dyn Repo,
        id_prefix_context: &IdPrefixContext,
        total_len: usize,
    ) -> ShortestIdPrefix {
        let mut hex = self.hex();
        let prefix_len = match self {
            CommitOrChangeId::Commit(id) => id_prefix_context.shortest_commit_prefix_len(repo, id),
            CommitOrChangeId::Change(id) => id_prefix_context.shortest_change_prefix_len(repo, id),
        };
        hex.truncate(max(prefix_len, total_len));
        let rest = hex.split_off(prefix_len);
        ShortestIdPrefix { prefix: hex, rest }
    }
}

impl Template<()> for CommitOrChangeId {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(&self.hex())
    }
}

fn build_commit_or_change_id_method<'repo>(
    language: &CommitTemplateLanguage<'repo, '_>,
    build_ctx: &BuildContext<CommitTemplatePropertyKind<'repo>>,
    self_property: impl TemplateProperty<Commit, Output = CommitOrChangeId> + 'repo,
    function: &FunctionCallNode,
) -> TemplateParseResult<CommitTemplatePropertyKind<'repo>> {
    let parse_optional_integer = |function| -> Result<Option<_>, TemplateParseError> {
        let ([], [len_node]) = template_parser::expect_arguments(function)?;
        len_node
            .map(|node| template_builder::expect_integer_expression(language, build_ctx, node))
            .transpose()
    };
    let property = match function.name {
        "short" => {
            let len_property = parse_optional_integer(function)?;
            language.wrap_string(TemplateFunction::new(
                (self_property, len_property),
                |(id, len)| id.short(len.map_or(12, |l| l.try_into().unwrap_or(0))),
            ))
        }
        "shortest" => {
            let id_prefix_context = &language.id_prefix_context;
            let len_property = parse_optional_integer(function)?;
            language.wrap_shortest_id_prefix(TemplateFunction::new(
                (self_property, len_property),
                |(id, len)| {
                    id.shortest(
                        language.repo,
                        id_prefix_context,
                        len.and_then(|l| l.try_into().ok()).unwrap_or(0),
                    )
                },
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
    _build_ctx: &BuildContext<CommitTemplatePropertyKind<'repo>>,
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
    id_prefix_context: &'repo IdPrefixContext,
    template_text: &str,
    aliases_map: &TemplateAliasesMap,
) -> TemplateParseResult<Box<dyn Template<Commit> + 'repo>> {
    let language = CommitTemplateLanguage {
        repo,
        workspace_id,
        id_prefix_context,
        keyword_cache: CommitKeywordCache::default(),
    };
    let node = template_parser::parse(template_text, aliases_map)?;
    template_builder::build(&language, &node)
}
