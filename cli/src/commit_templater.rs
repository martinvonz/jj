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

use std::any::Any;
use std::cmp::max;
use std::collections::HashMap;
use std::io;
use std::rc::Rc;

use itertools::Itertools as _;
use jj_lib::backend::{ChangeId, CommitId};
use jj_lib::commit::Commit;
use jj_lib::extensions_map::ExtensionsMap;
use jj_lib::hex_util::to_reverse_hex;
use jj_lib::id_prefix::IdPrefixContext;
use jj_lib::object_id::ObjectId as _;
use jj_lib::op_store::{RefTarget, WorkspaceId};
use jj_lib::repo::Repo;
use jj_lib::revset::{Revset, RevsetParseContext};
use jj_lib::{git, rewrite};
use once_cell::unsync::OnceCell;

use crate::template_builder::{
    self, merge_fn_map, BuildContext, CoreTemplateBuildFnTable, CoreTemplatePropertyKind,
    IntoTemplateProperty, TemplateBuildMethodFnMap, TemplateLanguage,
};
use crate::template_parser::{self, FunctionCallNode, TemplateParseError, TemplateParseResult};
use crate::templater::{
    self, IntoTemplate, PlainTextFormattedProperty, Template, TemplateFormatter, TemplateProperty,
    TemplatePropertyError, TemplatePropertyExt as _,
};
use crate::{revset_util, text_util};

pub trait CommitTemplateLanguageExtension {
    fn build_fn_table<'repo>(&self) -> CommitTemplateBuildFnTable<'repo>;

    fn build_cache_extensions(&self, extensions: &mut ExtensionsMap);
}

pub struct CommitTemplateLanguage<'repo> {
    repo: &'repo dyn Repo,
    workspace_id: WorkspaceId,
    // RevsetParseContext doesn't borrow a repo, but we'll need 'repo lifetime
    // anyway to capture it to evaluate dynamically-constructed user expression
    // such as `revset("ancestors(" ++ commit_id ++ ")")`.
    // TODO: Maybe refactor context structs? WorkspaceId is contained in
    // RevsetParseContext for example.
    revset_parse_context: RevsetParseContext<'repo>,
    id_prefix_context: &'repo IdPrefixContext,
    build_fn_table: CommitTemplateBuildFnTable<'repo>,
    keyword_cache: CommitKeywordCache,
    cache_extensions: ExtensionsMap,
}

impl<'repo> CommitTemplateLanguage<'repo> {
    /// Sets up environment where commit template will be transformed to
    /// evaluation tree.
    pub fn new(
        repo: &'repo dyn Repo,
        workspace_id: &WorkspaceId,
        revset_parse_context: RevsetParseContext<'repo>,
        id_prefix_context: &'repo IdPrefixContext,
        extension: Option<&dyn CommitTemplateLanguageExtension>,
    ) -> Self {
        let mut build_fn_table = CommitTemplateBuildFnTable::builtin();
        let mut cache_extensions = ExtensionsMap::empty();

        // TODO: Extension methods should be refactored to be plural, to support
        // multiple extensions in a dynamic load environment
        if let Some(extension) = extension {
            build_fn_table.merge(extension.build_fn_table());
            extension.build_cache_extensions(&mut cache_extensions);
        }

        CommitTemplateLanguage {
            repo,
            workspace_id: workspace_id.clone(),
            revset_parse_context,
            id_prefix_context,
            build_fn_table,
            keyword_cache: CommitKeywordCache::default(),
            cache_extensions,
        }
    }
}

impl<'repo> TemplateLanguage<'repo> for CommitTemplateLanguage<'repo> {
    type Property = CommitTemplatePropertyKind<'repo>;

    template_builder::impl_core_wrap_property_fns!('repo, CommitTemplatePropertyKind::Core);

    fn build_function(
        &self,
        build_ctx: &BuildContext<Self::Property>,
        function: &FunctionCallNode,
    ) -> TemplateParseResult<Self::Property> {
        let table = &self.build_fn_table.core;
        table.build_function(self, build_ctx, function)
    }

    fn build_method(
        &self,
        build_ctx: &BuildContext<Self::Property>,
        property: Self::Property,
        function: &FunctionCallNode,
    ) -> TemplateParseResult<Self::Property> {
        match property {
            CommitTemplatePropertyKind::Core(property) => {
                let table = &self.build_fn_table.core;
                table.build_method(self, build_ctx, property, function)
            }
            CommitTemplatePropertyKind::Commit(property) => {
                let table = &self.build_fn_table.commit_methods;
                let build = template_parser::lookup_method("Commit", table, function)?;
                build(self, build_ctx, property, function)
            }
            CommitTemplatePropertyKind::CommitOpt(property) => {
                let table = &self.build_fn_table.commit_methods;
                let build = template_parser::lookup_method("Commit", table, function)?;
                let inner_property = property.and_then(|opt| {
                    opt.ok_or_else(|| TemplatePropertyError("No commit available".into()))
                });
                build(self, build_ctx, Box::new(inner_property), function)
            }
            CommitTemplatePropertyKind::CommitList(property) => {
                // TODO: migrate to table?
                template_builder::build_unformattable_list_method(
                    self,
                    build_ctx,
                    property,
                    function,
                    Self::wrap_commit,
                )
            }
            CommitTemplatePropertyKind::RefName(property) => {
                let table = &self.build_fn_table.ref_name_methods;
                let build = template_parser::lookup_method("RefName", table, function)?;
                build(self, build_ctx, property, function)
            }
            CommitTemplatePropertyKind::RefNameOpt(property) => {
                let table = &self.build_fn_table.ref_name_methods;
                let build = template_parser::lookup_method("RefName", table, function)?;
                let inner_property = property.and_then(|opt| {
                    opt.ok_or_else(|| TemplatePropertyError("No RefName available".into()))
                });
                build(self, build_ctx, Box::new(inner_property), function)
            }
            CommitTemplatePropertyKind::RefNameList(property) => {
                // TODO: migrate to table?
                template_builder::build_formattable_list_method(
                    self,
                    build_ctx,
                    property,
                    function,
                    Self::wrap_ref_name,
                )
            }
            CommitTemplatePropertyKind::CommitOrChangeId(property) => {
                let table = &self.build_fn_table.commit_or_change_id_methods;
                let build = template_parser::lookup_method("CommitOrChangeId", table, function)?;
                build(self, build_ctx, property, function)
            }
            CommitTemplatePropertyKind::ShortestIdPrefix(property) => {
                let table = &self.build_fn_table.shortest_id_prefix_methods;
                let build = template_parser::lookup_method("ShortestIdPrefix", table, function)?;
                build(self, build_ctx, property, function)
            }
        }
    }
}

// If we need to add multiple languages that support Commit types, this can be
// turned into a trait which extends TemplateLanguage.
impl<'repo> CommitTemplateLanguage<'repo> {
    pub fn repo(&self) -> &'repo dyn Repo {
        self.repo
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.workspace_id
    }

    pub fn keyword_cache(&self) -> &CommitKeywordCache {
        &self.keyword_cache
    }

    pub fn cache_extension<T: Any>(&self) -> Option<&T> {
        self.cache_extensions.get::<T>()
    }

    pub fn wrap_commit(
        property: impl TemplateProperty<Output = Commit> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::Commit(Box::new(property))
    }

    pub fn wrap_commit_opt(
        property: impl TemplateProperty<Output = Option<Commit>> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::CommitOpt(Box::new(property))
    }

    pub fn wrap_commit_list(
        property: impl TemplateProperty<Output = Vec<Commit>> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::CommitList(Box::new(property))
    }

    pub fn wrap_ref_name(
        property: impl TemplateProperty<Output = RefName> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::RefName(Box::new(property))
    }

    pub fn wrap_ref_name_opt(
        property: impl TemplateProperty<Output = Option<RefName>> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::RefNameOpt(Box::new(property))
    }

    pub fn wrap_ref_name_list(
        property: impl TemplateProperty<Output = Vec<RefName>> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::RefNameList(Box::new(property))
    }

    pub fn wrap_commit_or_change_id(
        property: impl TemplateProperty<Output = CommitOrChangeId> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::CommitOrChangeId(Box::new(property))
    }

    pub fn wrap_shortest_id_prefix(
        property: impl TemplateProperty<Output = ShortestIdPrefix> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::ShortestIdPrefix(Box::new(property))
    }
}

pub enum CommitTemplatePropertyKind<'repo> {
    Core(CoreTemplatePropertyKind<'repo>),
    Commit(Box<dyn TemplateProperty<Output = Commit> + 'repo>),
    CommitOpt(Box<dyn TemplateProperty<Output = Option<Commit>> + 'repo>),
    CommitList(Box<dyn TemplateProperty<Output = Vec<Commit>> + 'repo>),
    RefName(Box<dyn TemplateProperty<Output = RefName> + 'repo>),
    RefNameOpt(Box<dyn TemplateProperty<Output = Option<RefName>> + 'repo>),
    RefNameList(Box<dyn TemplateProperty<Output = Vec<RefName>> + 'repo>),
    CommitOrChangeId(Box<dyn TemplateProperty<Output = CommitOrChangeId> + 'repo>),
    ShortestIdPrefix(Box<dyn TemplateProperty<Output = ShortestIdPrefix> + 'repo>),
}

impl<'repo> IntoTemplateProperty<'repo> for CommitTemplatePropertyKind<'repo> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<Output = bool> + 'repo>> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.try_into_boolean(),
            CommitTemplatePropertyKind::Commit(_) => None,
            CommitTemplatePropertyKind::CommitOpt(property) => {
                Some(Box::new(property.map(|opt| opt.is_some())))
            }
            CommitTemplatePropertyKind::CommitList(property) => {
                Some(Box::new(property.map(|l| !l.is_empty())))
            }
            CommitTemplatePropertyKind::RefName(_) => None,
            CommitTemplatePropertyKind::RefNameOpt(property) => {
                Some(Box::new(property.map(|opt| opt.is_some())))
            }
            CommitTemplatePropertyKind::RefNameList(property) => {
                Some(Box::new(property.map(|l| !l.is_empty())))
            }
            CommitTemplatePropertyKind::CommitOrChangeId(_) => None,
            CommitTemplatePropertyKind::ShortestIdPrefix(_) => None,
        }
    }

    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<Output = i64> + 'repo>> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.try_into_integer(),
            _ => None,
        }
    }

    fn try_into_plain_text(self) -> Option<Box<dyn TemplateProperty<Output = String> + 'repo>> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.try_into_plain_text(),
            _ => {
                let template = self.try_into_template()?;
                Some(Box::new(PlainTextFormattedProperty::new(template)))
            }
        }
    }

    fn try_into_template(self) -> Option<Box<dyn Template + 'repo>> {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.try_into_template(),
            CommitTemplatePropertyKind::Commit(_) => None,
            CommitTemplatePropertyKind::CommitOpt(_) => None,
            CommitTemplatePropertyKind::CommitList(_) => None,
            CommitTemplatePropertyKind::RefName(property) => Some(property.into_template()),
            CommitTemplatePropertyKind::RefNameOpt(property) => Some(property.into_template()),
            CommitTemplatePropertyKind::RefNameList(property) => Some(property.into_template()),
            CommitTemplatePropertyKind::CommitOrChangeId(property) => {
                Some(property.into_template())
            }
            CommitTemplatePropertyKind::ShortestIdPrefix(property) => {
                Some(property.into_template())
            }
        }
    }
}

/// Table of functions that translate method call node of self type `T`.
pub type CommitTemplateBuildMethodFnMap<'repo, T> =
    TemplateBuildMethodFnMap<'repo, CommitTemplateLanguage<'repo>, T>;

/// Symbol table of methods available in the commit template.
pub struct CommitTemplateBuildFnTable<'repo> {
    pub core: CoreTemplateBuildFnTable<'repo, CommitTemplateLanguage<'repo>>,
    pub commit_methods: CommitTemplateBuildMethodFnMap<'repo, Commit>,
    pub ref_name_methods: CommitTemplateBuildMethodFnMap<'repo, RefName>,
    pub commit_or_change_id_methods: CommitTemplateBuildMethodFnMap<'repo, CommitOrChangeId>,
    pub shortest_id_prefix_methods: CommitTemplateBuildMethodFnMap<'repo, ShortestIdPrefix>,
}

impl<'repo> CommitTemplateBuildFnTable<'repo> {
    /// Creates new symbol table containing the builtin methods.
    fn builtin() -> Self {
        CommitTemplateBuildFnTable {
            core: CoreTemplateBuildFnTable::builtin(),
            commit_methods: builtin_commit_methods(),
            ref_name_methods: builtin_ref_name_methods(),
            commit_or_change_id_methods: builtin_commit_or_change_id_methods(),
            shortest_id_prefix_methods: builtin_shortest_id_prefix_methods(),
        }
    }

    pub fn empty() -> Self {
        CommitTemplateBuildFnTable {
            core: CoreTemplateBuildFnTable::empty(),
            commit_methods: HashMap::new(),
            ref_name_methods: HashMap::new(),
            commit_or_change_id_methods: HashMap::new(),
            shortest_id_prefix_methods: HashMap::new(),
        }
    }

    fn merge(&mut self, extension: CommitTemplateBuildFnTable<'repo>) {
        let CommitTemplateBuildFnTable {
            core,
            commit_methods,
            ref_name_methods,
            commit_or_change_id_methods,
            shortest_id_prefix_methods,
        } = extension;

        self.core.merge(core);
        merge_fn_map(&mut self.commit_methods, commit_methods);
        merge_fn_map(&mut self.ref_name_methods, ref_name_methods);
        merge_fn_map(
            &mut self.commit_or_change_id_methods,
            commit_or_change_id_methods,
        );
        merge_fn_map(
            &mut self.shortest_id_prefix_methods,
            shortest_id_prefix_methods,
        );
    }
}

#[derive(Debug, Default)]
pub struct CommitKeywordCache {
    // Build index lazily, and Rc to get away from &self lifetime.
    branches_index: OnceCell<Rc<RefNamesIndex>>,
    tags_index: OnceCell<Rc<RefNamesIndex>>,
    git_refs_index: OnceCell<Rc<RefNamesIndex>>,
}

impl CommitKeywordCache {
    pub fn branches_index(&self, repo: &dyn Repo) -> &Rc<RefNamesIndex> {
        self.branches_index
            .get_or_init(|| Rc::new(build_branches_index(repo)))
    }

    pub fn tags_index(&self, repo: &dyn Repo) -> &Rc<RefNamesIndex> {
        self.tags_index
            .get_or_init(|| Rc::new(build_ref_names_index(repo.view().tags())))
    }

    pub fn git_refs_index(&self, repo: &dyn Repo) -> &Rc<RefNamesIndex> {
        self.git_refs_index
            .get_or_init(|| Rc::new(build_ref_names_index(repo.view().git_refs())))
    }
}

fn builtin_commit_methods<'repo>() -> CommitTemplateBuildMethodFnMap<'repo, Commit> {
    type L<'repo> = CommitTemplateLanguage<'repo>;
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map = CommitTemplateBuildMethodFnMap::<Commit>::new();
    map.insert(
        "description",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property =
                self_property.map(|commit| text_util::complete_newline(commit.description()));
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert(
        "change_id",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property =
                self_property.map(|commit| CommitOrChangeId::Change(commit.change_id().to_owned()));
            Ok(L::wrap_commit_or_change_id(out_property))
        },
    );
    map.insert(
        "commit_id",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property =
                self_property.map(|commit| CommitOrChangeId::Commit(commit.id().to_owned()));
            Ok(L::wrap_commit_or_change_id(out_property))
        },
    );
    map.insert(
        "parents",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property = self_property.map(|commit| commit.parents());
            Ok(L::wrap_commit_list(out_property))
        },
    );
    map.insert(
        "author",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property = self_property.map(|commit| commit.author().clone());
            Ok(L::wrap_signature(out_property))
        },
    );
    map.insert(
        "committer",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property = self_property.map(|commit| commit.committer().clone());
            Ok(L::wrap_signature(out_property))
        },
    );
    map.insert(
        "working_copies",
        |language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let repo = language.repo;
            let out_property = self_property.map(|commit| extract_working_copies(repo, &commit));
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert(
        "current_working_copy",
        |language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let repo = language.repo;
            let workspace_id = language.workspace_id.clone();
            let out_property = self_property.map(move |commit| {
                Some(commit.id()) == repo.view().get_wc_commit_id(&workspace_id)
            });
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert(
        "branches",
        |language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let index = language.keyword_cache.branches_index(language.repo).clone();
            let out_property = self_property.map(move |commit| {
                index
                    .get(commit.id())
                    .iter()
                    .filter(|ref_name| ref_name.is_local() || !ref_name.synced)
                    .cloned()
                    .collect()
            });
            Ok(L::wrap_ref_name_list(out_property))
        },
    );
    map.insert(
        "local_branches",
        |language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let index = language.keyword_cache.branches_index(language.repo).clone();
            let out_property = self_property.map(move |commit| {
                index
                    .get(commit.id())
                    .iter()
                    .filter(|ref_name| ref_name.is_local())
                    .cloned()
                    .collect()
            });
            Ok(L::wrap_ref_name_list(out_property))
        },
    );
    map.insert(
        "remote_branches",
        |language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let index = language.keyword_cache.branches_index(language.repo).clone();
            let out_property = self_property.map(move |commit| {
                index
                    .get(commit.id())
                    .iter()
                    .filter(|ref_name| ref_name.is_remote())
                    .cloned()
                    .collect()
            });
            Ok(L::wrap_ref_name_list(out_property))
        },
    );
    map.insert("tags", |language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let index = language.keyword_cache.tags_index(language.repo).clone();
        let out_property = self_property.map(move |commit| index.get(commit.id()).to_vec());
        Ok(L::wrap_ref_name_list(out_property))
    });
    map.insert(
        "git_refs",
        |language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let index = language.keyword_cache.git_refs_index(language.repo).clone();
            let out_property = self_property.map(move |commit| index.get(commit.id()).to_vec());
            Ok(L::wrap_ref_name_list(out_property))
        },
    );
    map.insert(
        "git_head",
        |language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let repo = language.repo;
            let out_property = self_property.map(|commit| extract_git_head(repo, &commit));
            Ok(L::wrap_ref_name_opt(out_property))
        },
    );
    map.insert(
        "divergent",
        |language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let repo = language.repo;
            let out_property = self_property.map(|commit| {
                // The given commit could be hidden in e.g. obslog.
                let maybe_entries = repo.resolve_change_id(commit.change_id());
                maybe_entries.map_or(0, |entries| entries.len()) > 1
            });
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert("hidden", |language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let repo = language.repo;
        let out_property = self_property.map(|commit| {
            let maybe_entries = repo.resolve_change_id(commit.change_id());
            maybe_entries.map_or(true, |entries| !entries.contains(commit.id()))
        });
        Ok(L::wrap_boolean(out_property))
    });
    map.insert(
        "immutable",
        |language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let revset = evaluate_immutable_revset(language, function.name_span)?;
            let is_immutable = revset.containing_fn();
            let out_property = self_property.map(move |commit| is_immutable(commit.id()));
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert(
        "conflict",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property = self_property.and_then(|commit| Ok(commit.has_conflict()?));
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert("empty", |language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let repo = language.repo;
        let out_property = self_property.and_then(|commit| {
            if let [parent] = &commit.parents()[..] {
                return Ok(parent.tree_id() == commit.tree_id());
            }
            let parent_tree = rewrite::merge_commit_trees(repo, &commit.parents())?;
            Ok(*commit.tree_id() == parent_tree.id())
        });
        Ok(L::wrap_boolean(out_property))
    });
    map.insert("root", |language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let repo = language.repo;
        let out_property = self_property.map(|commit| commit.id() == repo.store().root_commit_id());
        Ok(L::wrap_boolean(out_property))
    });
    map
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

fn evaluate_immutable_revset<'repo>(
    language: &CommitTemplateLanguage<'repo>,
    span: pest::Span<'_>,
) -> Result<Box<dyn Revset + 'repo>, TemplateParseError> {
    let repo = language.repo;
    // Alternatively, a negated (i.e. visible mutable) set could be computed.
    // It's usually smaller than the immutable set. The revset engine can also
    // optimize "::<recent_heads>" query to use bitset-based implementation.
    let expression = revset_util::parse_immutable_expression(&language.revset_parse_context)
        .map_err(|err| {
            TemplateParseError::expression("Failed to parse revset", span).with_source(err)
        })?;
    let symbol_resolver = revset_util::default_symbol_resolver(repo, language.id_prefix_context);
    let revset = revset_util::evaluate(repo, &symbol_resolver, expression).map_err(|err| {
        TemplateParseError::expression("Failed to evaluate revset", span).with_source(err)
    })?;
    Ok(revset)
}

/// Branch or tag name with metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefName {
    /// Local name.
    name: String,
    /// Remote name if this is a remote or Git-tracking ref.
    remote: Option<String>,
    /// Ref target has conflicts.
    conflict: bool,
    /// Local ref is synchronized with all tracking remotes, or tracking remote
    /// ref is synchronized with the local.
    synced: bool,
}

impl RefName {
    fn is_local(&self) -> bool {
        self.remote.is_none()
    }

    fn is_remote(&self) -> bool {
        self.remote.is_some()
    }
}

impl Template for RefName {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        write!(formatter.labeled("name"), "{}", self.name)?;
        if let Some(remote) = &self.remote {
            write!(formatter, "@")?;
            write!(formatter.labeled("remote"), "{remote}")?;
        }
        // Don't show both conflict and unsynced sigils as conflicted ref wouldn't
        // be pushed.
        if self.conflict {
            write!(formatter, "??")?;
        } else if self.is_local() && !self.synced {
            write!(formatter, "*")?;
        }
        Ok(())
    }
}

impl Template for Vec<RefName> {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        templater::format_joined(formatter, self, " ")
    }
}

fn builtin_ref_name_methods<'repo>() -> CommitTemplateBuildMethodFnMap<'repo, RefName> {
    type L<'repo> = CommitTemplateLanguage<'repo>;
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map = CommitTemplateBuildMethodFnMap::<RefName>::new();
    map.insert("name", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|ref_name| ref_name.name);
        Ok(L::wrap_string(out_property))
    });
    map.insert(
        "remote",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property = self_property.map(|ref_name| ref_name.remote.unwrap_or_default());
            Ok(L::wrap_string(out_property))
        },
    );
    map
}

/// Cache for reverse lookup refs.
#[derive(Clone, Debug, Default)]
pub struct RefNamesIndex {
    index: HashMap<CommitId, Vec<RefName>>,
}

impl RefNamesIndex {
    fn insert<'a>(&mut self, ids: impl IntoIterator<Item = &'a CommitId>, name: RefName) {
        for id in ids {
            let ref_names = self.index.entry(id.clone()).or_default();
            ref_names.push(name.clone());
        }
    }

    pub fn get(&self, id: &CommitId) -> &[RefName] {
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
        let local_target = branch_target.local_target;
        let remote_refs = branch_target.remote_refs;
        if local_target.is_present() {
            let ref_name = RefName {
                name: branch_name.to_owned(),
                remote: None,
                conflict: local_target.has_conflict(),
                synced: remote_refs.iter().all(|&(_, remote_ref)| {
                    !remote_ref.is_tracking() || remote_ref.target == *local_target
                }),
            };
            index.insert(local_target.added_ids(), ref_name);
        }
        for &(remote_name, remote_ref) in &remote_refs {
            let ref_name = RefName {
                name: branch_name.to_owned(),
                remote: Some(remote_name.to_owned()),
                conflict: remote_ref.target.has_conflict(),
                synced: remote_ref.is_tracking() && remote_ref.target == *local_target,
            };
            index.insert(remote_ref.target.added_ids(), ref_name);
        }
    }
    index
}

fn build_ref_names_index<'a>(
    ref_pairs: impl IntoIterator<Item = (&'a String, &'a RefTarget)>,
) -> RefNamesIndex {
    let mut index = RefNamesIndex::default();
    for (name, target) in ref_pairs {
        let ref_name = RefName {
            name: name.to_owned(),
            remote: None,
            conflict: target.has_conflict(),
            synced: true, // has no tracking remotes
        };
        index.insert(target.added_ids(), ref_name);
    }
    index
}

fn extract_git_head(repo: &dyn Repo, commit: &Commit) -> Option<RefName> {
    let target = repo.view().git_head();
    target.added_ids().contains(commit.id()).then(|| {
        RefName {
            name: "HEAD".to_owned(),
            remote: Some(git::REMOTE_NAME_FOR_LOCAL_GIT_REPO.to_owned()),
            conflict: target.has_conflict(),
            synced: false, // has no local counterpart
        }
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommitOrChangeId {
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

impl Template for CommitOrChangeId {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        write!(formatter, "{}", self.hex())
    }
}

fn builtin_commit_or_change_id_methods<'repo>(
) -> CommitTemplateBuildMethodFnMap<'repo, CommitOrChangeId> {
    type L<'repo> = CommitTemplateLanguage<'repo>;
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map = CommitTemplateBuildMethodFnMap::<CommitOrChangeId>::new();
    map.insert("short", |language, build_ctx, self_property, function| {
        let ([], [len_node]) = template_parser::expect_arguments(function)?;
        let len_property = len_node
            .map(|node| template_builder::expect_usize_expression(language, build_ctx, node))
            .transpose()?;
        let out_property =
            (self_property, len_property).map(|(id, len)| id.short(len.unwrap_or(12)));
        Ok(L::wrap_string(out_property))
    });
    map.insert(
        "shortest",
        |language, build_ctx, self_property, function| {
            let id_prefix_context = &language.id_prefix_context;
            let ([], [len_node]) = template_parser::expect_arguments(function)?;
            let len_property = len_node
                .map(|node| template_builder::expect_usize_expression(language, build_ctx, node))
                .transpose()?;
            let out_property = (self_property, len_property)
                .map(|(id, len)| id.shortest(language.repo, id_prefix_context, len.unwrap_or(0)));
            Ok(L::wrap_shortest_id_prefix(out_property))
        },
    );
    map
}

pub struct ShortestIdPrefix {
    pub prefix: String,
    pub rest: String,
}

impl Template for ShortestIdPrefix {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        write!(formatter.labeled("prefix"), "{}", self.prefix)?;
        write!(formatter.labeled("rest"), "{}", self.rest)?;
        Ok(())
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

fn builtin_shortest_id_prefix_methods<'repo>(
) -> CommitTemplateBuildMethodFnMap<'repo, ShortestIdPrefix> {
    type L<'repo> = CommitTemplateLanguage<'repo>;
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map = CommitTemplateBuildMethodFnMap::<ShortestIdPrefix>::new();
    map.insert(
        "prefix",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property = self_property.map(|id| id.prefix);
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert("rest", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|id| id.rest);
        Ok(L::wrap_string(out_property))
    });
    map.insert("upper", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|id| id.to_upper());
        Ok(L::wrap_shortest_id_prefix(out_property))
    });
    map.insert("lower", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|id| id.to_lower());
        Ok(L::wrap_shortest_id_prefix(out_property))
    });
    map
}
