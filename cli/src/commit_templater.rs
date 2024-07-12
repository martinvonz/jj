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
use jj_lib::git;
use jj_lib::hex_util::to_reverse_hex;
use jj_lib::id_prefix::IdPrefixContext;
use jj_lib::object_id::ObjectId as _;
use jj_lib::op_store::{RefTarget, RemoteRef, WorkspaceId};
use jj_lib::repo::Repo;
use jj_lib::revset::{self, Revset, RevsetExpression, RevsetModifier, RevsetParseContext};
use once_cell::unsync::OnceCell;

use crate::template_builder::{
    self, merge_fn_map, BuildContext, CoreTemplateBuildFnTable, CoreTemplatePropertyKind,
    IntoTemplateProperty, TemplateBuildMethodFnMap, TemplateLanguage,
};
use crate::template_parser::{self, FunctionCallNode, TemplateParseError, TemplateParseResult};
use crate::templater::{
    self, PlainTextFormattedProperty, SizeHint, Template, TemplateFormatter, TemplateProperty,
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
    keyword_cache: CommitKeywordCache<'repo>,
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
        extensions: &[impl AsRef<dyn CommitTemplateLanguageExtension>],
    ) -> Self {
        let mut build_fn_table = CommitTemplateBuildFnTable::builtin();
        let mut cache_extensions = ExtensionsMap::empty();

        for extension in extensions {
            build_fn_table.merge(extension.as_ref().build_fn_table());
            extension
                .as_ref()
                .build_cache_extensions(&mut cache_extensions);
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
        let type_name = property.type_name();
        match property {
            CommitTemplatePropertyKind::Core(property) => {
                let table = &self.build_fn_table.core;
                table.build_method(self, build_ctx, property, function)
            }
            CommitTemplatePropertyKind::Commit(property) => {
                let table = &self.build_fn_table.commit_methods;
                let build = template_parser::lookup_method(type_name, table, function)?;
                build(self, build_ctx, property, function)
            }
            CommitTemplatePropertyKind::CommitOpt(property) => {
                let type_name = "Commit";
                let table = &self.build_fn_table.commit_methods;
                let build = template_parser::lookup_method(type_name, table, function)?;
                let inner_property = property.try_unwrap(type_name);
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
                let build = template_parser::lookup_method(type_name, table, function)?;
                build(self, build_ctx, property, function)
            }
            CommitTemplatePropertyKind::RefNameOpt(property) => {
                let type_name = "RefName";
                let table = &self.build_fn_table.ref_name_methods;
                let build = template_parser::lookup_method(type_name, table, function)?;
                let inner_property = property.try_unwrap(type_name);
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
                let build = template_parser::lookup_method(type_name, table, function)?;
                build(self, build_ctx, property, function)
            }
            CommitTemplatePropertyKind::ShortestIdPrefix(property) => {
                let table = &self.build_fn_table.shortest_id_prefix_methods;
                let build = template_parser::lookup_method(type_name, table, function)?;
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

    pub fn keyword_cache(&self) -> &CommitKeywordCache<'repo> {
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
        property: impl TemplateProperty<Output = Rc<RefName>> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::RefName(Box::new(property))
    }

    pub fn wrap_ref_name_opt(
        property: impl TemplateProperty<Output = Option<Rc<RefName>>> + 'repo,
    ) -> CommitTemplatePropertyKind<'repo> {
        CommitTemplatePropertyKind::RefNameOpt(Box::new(property))
    }

    pub fn wrap_ref_name_list(
        property: impl TemplateProperty<Output = Vec<Rc<RefName>>> + 'repo,
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
    RefName(Box<dyn TemplateProperty<Output = Rc<RefName>> + 'repo>),
    RefNameOpt(Box<dyn TemplateProperty<Output = Option<Rc<RefName>>> + 'repo>),
    RefNameList(Box<dyn TemplateProperty<Output = Vec<Rc<RefName>>> + 'repo>),
    CommitOrChangeId(Box<dyn TemplateProperty<Output = CommitOrChangeId> + 'repo>),
    ShortestIdPrefix(Box<dyn TemplateProperty<Output = ShortestIdPrefix> + 'repo>),
}

impl<'repo> IntoTemplateProperty<'repo> for CommitTemplatePropertyKind<'repo> {
    fn type_name(&self) -> &'static str {
        match self {
            CommitTemplatePropertyKind::Core(property) => property.type_name(),
            CommitTemplatePropertyKind::Commit(_) => "Commit",
            CommitTemplatePropertyKind::CommitOpt(_) => "Option<Commit>",
            CommitTemplatePropertyKind::CommitList(_) => "List<Commit>",
            CommitTemplatePropertyKind::RefName(_) => "RefName",
            CommitTemplatePropertyKind::RefNameOpt(_) => "Option<RefName>",
            CommitTemplatePropertyKind::RefNameList(_) => "List<RefName>",
            CommitTemplatePropertyKind::CommitOrChangeId(_) => "CommitOrChangeId",
            CommitTemplatePropertyKind::ShortestIdPrefix(_) => "ShortestIdPrefix",
        }
    }

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
    pub ref_name_methods: CommitTemplateBuildMethodFnMap<'repo, Rc<RefName>>,
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

#[derive(Default)]
pub struct CommitKeywordCache<'repo> {
    // Build index lazily, and Rc to get away from &self lifetime.
    branches_index: OnceCell<Rc<RefNamesIndex>>,
    tags_index: OnceCell<Rc<RefNamesIndex>>,
    git_refs_index: OnceCell<Rc<RefNamesIndex>>,
    is_immutable_fn: OnceCell<Rc<RevsetContainingFn<'repo>>>,
}

impl<'repo> CommitKeywordCache<'repo> {
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

    pub fn is_immutable_fn(
        &self,
        language: &CommitTemplateLanguage<'repo>,
        span: pest::Span<'_>,
    ) -> TemplateParseResult<&Rc<RevsetContainingFn<'repo>>> {
        self.is_immutable_fn.get_or_try_init(|| {
            let revset = evaluate_immutable_revset(language, span)?;
            Ok(revset.containing_fn().into())
        })
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
            function.expect_no_arguments()?;
            let out_property =
                self_property.map(|commit| text_util::complete_newline(commit.description()));
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert(
        "change_id",
        |_language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let out_property =
                self_property.map(|commit| CommitOrChangeId::Change(commit.change_id().to_owned()));
            Ok(L::wrap_commit_or_change_id(out_property))
        },
    );
    map.insert(
        "commit_id",
        |_language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let out_property =
                self_property.map(|commit| CommitOrChangeId::Commit(commit.id().to_owned()));
            Ok(L::wrap_commit_or_change_id(out_property))
        },
    );
    map.insert(
        "parents",
        |_language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let out_property =
                self_property.and_then(|commit| Ok(commit.parents().try_collect()?));
            Ok(L::wrap_commit_list(out_property))
        },
    );
    map.insert("author", |language, _build_ctx, self_property, function| {
        function.expect_no_arguments()?;
        let mailmap = language.revset_parse_context.mailmap().clone();
        let out_property = self_property.map(move |commit| mailmap.author(&commit));
        Ok(L::wrap_signature(out_property))
    });
    map.insert(
        "author_raw",
        |_language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let out_property = self_property.map(|commit| commit.author_raw().clone());
            Ok(L::wrap_signature(out_property))
        },
    );
    map.insert(
        "committer",
        |language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let mailmap = language.revset_parse_context.mailmap().clone();
            let out_property = self_property.map(move |commit| mailmap.committer(&commit));
            Ok(L::wrap_signature(out_property))
        },
    );
    map.insert(
        "committer_raw",
        |_language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let out_property = self_property.map(|commit| commit.committer_raw().clone());
            Ok(L::wrap_signature(out_property))
        },
    );
    map.insert("mine", |language, _build_ctx, self_property, function| {
        function.expect_no_arguments()?;
        let mailmap = language.revset_parse_context.mailmap().clone();
        let user_email = language.revset_parse_context.user_email().to_owned();
        let out_property =
            self_property.map(move |commit| mailmap.author(&commit).email == user_email);
        Ok(L::wrap_boolean(out_property))
    });
    map.insert(
        "working_copies",
        |language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let repo = language.repo;
            let out_property = self_property.map(|commit| extract_working_copies(repo, &commit));
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert(
        "current_working_copy",
        |language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
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
            function.expect_no_arguments()?;
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
            function.expect_no_arguments()?;
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
            function.expect_no_arguments()?;
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
        function.expect_no_arguments()?;
        let index = language.keyword_cache.tags_index(language.repo).clone();
        let out_property = self_property.map(move |commit| index.get(commit.id()).to_vec());
        Ok(L::wrap_ref_name_list(out_property))
    });
    map.insert(
        "git_refs",
        |language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let index = language.keyword_cache.git_refs_index(language.repo).clone();
            let out_property = self_property.map(move |commit| index.get(commit.id()).to_vec());
            Ok(L::wrap_ref_name_list(out_property))
        },
    );
    map.insert(
        "git_head",
        |language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let repo = language.repo;
            let out_property = self_property.map(|commit| extract_git_head(repo, &commit));
            Ok(L::wrap_ref_name_opt(out_property))
        },
    );
    map.insert(
        "divergent",
        |language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
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
        function.expect_no_arguments()?;
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
            function.expect_no_arguments()?;
            let is_immutable = language
                .keyword_cache
                .is_immutable_fn(language, function.name_span)?
                .clone();
            let out_property = self_property.map(move |commit| is_immutable(commit.id()));
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert(
        "contained_in",
        |language, _build_ctx, self_property, function| {
            let [revset_node] = function.expect_exact_arguments()?;

            let is_contained =
                template_parser::expect_string_literal_with(revset_node, |revset, span| {
                    Ok(evaluate_user_revset(language, span, revset)?.containing_fn())
                })?;

            let out_property = self_property.map(move |commit| is_contained(commit.id()));
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert(
        "conflict",
        |_language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let out_property = self_property.and_then(|commit| Ok(commit.has_conflict()?));
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert("empty", |language, _build_ctx, self_property, function| {
        function.expect_no_arguments()?;
        let repo = language.repo;
        let out_property = self_property.and_then(|commit| Ok(commit.is_empty(repo)?));
        Ok(L::wrap_boolean(out_property))
    });
    map.insert("root", |language, _build_ctx, self_property, function| {
        function.expect_no_arguments()?;
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

type RevsetContainingFn<'repo> = dyn Fn(&CommitId) -> bool + 'repo;

fn evaluate_revset_expression<'repo>(
    language: &CommitTemplateLanguage<'repo>,
    span: pest::Span<'_>,
    expression: Rc<RevsetExpression>,
) -> Result<Box<dyn Revset + 'repo>, TemplateParseError> {
    let symbol_resolver = revset_util::default_symbol_resolver(
        language.repo,
        language.revset_parse_context.symbol_resolvers(),
        language.id_prefix_context,
    );
    let revset =
        revset_util::evaluate(language.repo, &symbol_resolver, expression).map_err(|err| {
            TemplateParseError::expression("Failed to evaluate revset", span).with_source(err)
        })?;
    Ok(revset)
}

fn evaluate_immutable_revset<'repo>(
    language: &CommitTemplateLanguage<'repo>,
    span: pest::Span<'_>,
) -> Result<Box<dyn Revset + 'repo>, TemplateParseError> {
    // Alternatively, a negated (i.e. visible mutable) set could be computed.
    // It's usually smaller than the immutable set. The revset engine can also
    // optimize "::<recent_heads>" query to use bitset-based implementation.
    let expression = revset_util::parse_immutable_expression(&language.revset_parse_context)
        .map_err(|err| {
            TemplateParseError::expression("Failed to parse revset", span).with_source(err)
        })?;

    evaluate_revset_expression(language, span, expression)
}

fn evaluate_user_revset<'repo>(
    language: &CommitTemplateLanguage<'repo>,
    span: pest::Span<'_>,
    revset: &str,
) -> Result<Box<dyn Revset + 'repo>, TemplateParseError> {
    let (expression, modifier) =
        revset::parse_with_modifier(revset, &language.revset_parse_context).map_err(|err| {
            TemplateParseError::expression("Failed to parse revset", span).with_source(err)
        })?;
    let (None | Some(RevsetModifier::All)) = modifier;

    evaluate_revset_expression(language, span, expression)
}

/// Branch or tag name with metadata.
#[derive(Debug)]
pub struct RefName {
    /// Local name.
    name: String,
    /// Remote name if this is a remote or Git-tracking ref.
    remote: Option<String>,
    /// Target commit ids.
    target: RefTarget,
    /// Local ref metadata which tracks this remote ref.
    tracking_ref: Option<TrackingRef>,
    /// Local ref is synchronized with all tracking remotes, or tracking remote
    /// ref is synchronized with the local.
    synced: bool,
}

#[derive(Debug)]
struct TrackingRef {
    /// Local ref target which tracks the other remote ref.
    target: RefTarget,
    /// Number of commits ahead of the tracking `target`.
    ahead_count: OnceCell<SizeHint>,
    /// Number of commits behind of the tracking `target`.
    behind_count: OnceCell<SizeHint>,
}

impl RefName {
    // RefName is wrapped by Rc<T> to make it cheaply cloned and share
    // lazy-evaluation results across clones.

    /// Creates local ref representation which might track some of the
    /// `remote_refs`.
    pub fn local<'a>(
        name: impl Into<String>,
        target: RefTarget,
        remote_refs: impl IntoIterator<Item = &'a RemoteRef>,
    ) -> Rc<Self> {
        let synced = remote_refs
            .into_iter()
            .all(|remote_ref| !remote_ref.is_tracking() || remote_ref.target == target);
        Rc::new(RefName {
            name: name.into(),
            remote: None,
            target,
            tracking_ref: None,
            synced,
        })
    }

    /// Creates local ref representation which doesn't track any remote refs.
    pub fn local_only(name: impl Into<String>, target: RefTarget) -> Rc<Self> {
        Self::local(name, target, [])
    }

    /// Creates remote ref representation which might be tracked by a local ref
    /// pointing to the `local_target`.
    pub fn remote(
        name: impl Into<String>,
        remote_name: impl Into<String>,
        remote_ref: RemoteRef,
        local_target: &RefTarget,
    ) -> Rc<Self> {
        let synced = remote_ref.is_tracking() && remote_ref.target == *local_target;
        let tracking_ref = remote_ref.is_tracking().then(|| {
            let count = if synced {
                OnceCell::from((0, Some(0))) // fast path for synced remotes
            } else {
                OnceCell::new()
            };
            TrackingRef {
                target: local_target.clone(),
                ahead_count: count.clone(),
                behind_count: count,
            }
        });
        Rc::new(RefName {
            name: name.into(),
            remote: Some(remote_name.into()),
            target: remote_ref.target,
            tracking_ref,
            synced,
        })
    }

    /// Creates remote ref representation which isn't tracked by a local ref.
    pub fn remote_only(
        name: impl Into<String>,
        remote_name: impl Into<String>,
        target: RefTarget,
    ) -> Rc<Self> {
        Rc::new(RefName {
            name: name.into(),
            remote: Some(remote_name.into()),
            target,
            tracking_ref: None,
            synced: false, // has no local counterpart
        })
    }

    fn is_local(&self) -> bool {
        self.remote.is_none()
    }

    fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    fn is_present(&self) -> bool {
        self.target.is_present()
    }

    /// Whether the ref target has conflicts.
    fn has_conflict(&self) -> bool {
        self.target.has_conflict()
    }

    /// Returns true if this ref is tracked by a local ref. The local ref might
    /// have been deleted (but not pushed yet.)
    fn is_tracked(&self) -> bool {
        self.tracking_ref.is_some()
    }

    /// Returns true if this ref is tracked by a local ref, and if the local ref
    /// is present.
    fn is_tracking_present(&self) -> bool {
        self.tracking_ref
            .as_ref()
            .map_or(false, |tracking| tracking.target.is_present())
    }

    /// Number of commits ahead of the tracking local ref.
    fn tracking_ahead_count(&self, repo: &dyn Repo) -> Result<SizeHint, TemplatePropertyError> {
        let Some(tracking) = &self.tracking_ref else {
            return Err(TemplatePropertyError("Not a tracked remote ref".into()));
        };
        tracking
            .ahead_count
            .get_or_try_init(|| {
                let self_ids = self.target.added_ids().cloned().collect_vec();
                let other_ids = tracking.target.added_ids().cloned().collect_vec();
                Ok(revset::walk_revs(repo, &self_ids, &other_ids)?.count_estimate())
            })
            .copied()
    }

    /// Number of commits behind of the tracking local ref.
    fn tracking_behind_count(&self, repo: &dyn Repo) -> Result<SizeHint, TemplatePropertyError> {
        let Some(tracking) = &self.tracking_ref else {
            return Err(TemplatePropertyError("Not a tracked remote ref".into()));
        };
        tracking
            .behind_count
            .get_or_try_init(|| {
                let self_ids = self.target.added_ids().cloned().collect_vec();
                let other_ids = tracking.target.added_ids().cloned().collect_vec();
                Ok(revset::walk_revs(repo, &other_ids, &self_ids)?.count_estimate())
            })
            .copied()
    }
}

// If wrapping with Rc<T> becomes common, add generic impl for Rc<T>.
impl Template for Rc<RefName> {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        write!(formatter.labeled("name"), "{}", self.name)?;
        if let Some(remote) = &self.remote {
            write!(formatter, "@")?;
            write!(formatter.labeled("remote"), "{remote}")?;
        }
        // Don't show both conflict and unsynced sigils as conflicted ref wouldn't
        // be pushed.
        if self.has_conflict() {
            write!(formatter, "??")?;
        } else if self.is_local() && !self.synced {
            write!(formatter, "*")?;
        }
        Ok(())
    }
}

impl Template for Vec<Rc<RefName>> {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        templater::format_joined(formatter, self, " ")
    }
}

fn builtin_ref_name_methods<'repo>() -> CommitTemplateBuildMethodFnMap<'repo, Rc<RefName>> {
    type L<'repo> = CommitTemplateLanguage<'repo>;
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map = CommitTemplateBuildMethodFnMap::<Rc<RefName>>::new();
    map.insert("name", |_language, _build_ctx, self_property, function| {
        function.expect_no_arguments()?;
        let out_property = self_property.map(|ref_name| ref_name.name.clone());
        Ok(L::wrap_string(out_property))
    });
    map.insert(
        "remote",
        |_language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let out_property =
                self_property.map(|ref_name| ref_name.remote.clone().unwrap_or_default());
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert(
        "present",
        |_language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let out_property = self_property.map(|ref_name| ref_name.is_present());
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert(
        "conflict",
        |_language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let out_property = self_property.map(|ref_name| ref_name.has_conflict());
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert(
        "normal_target",
        |language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let repo = language.repo;
            let out_property = self_property.and_then(|ref_name| {
                let maybe_id = ref_name.target.as_normal();
                Ok(maybe_id.map(|id| repo.store().get_commit(id)).transpose()?)
            });
            Ok(L::wrap_commit_opt(out_property))
        },
    );
    map.insert(
        "removed_targets",
        |language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let repo = language.repo;
            let out_property = self_property.and_then(|ref_name| {
                let ids = ref_name.target.removed_ids();
                Ok(ids.map(|id| repo.store().get_commit(id)).try_collect()?)
            });
            Ok(L::wrap_commit_list(out_property))
        },
    );
    map.insert(
        "added_targets",
        |language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let repo = language.repo;
            let out_property = self_property.and_then(|ref_name| {
                let ids = ref_name.target.added_ids();
                Ok(ids.map(|id| repo.store().get_commit(id)).try_collect()?)
            });
            Ok(L::wrap_commit_list(out_property))
        },
    );
    map.insert(
        "tracked",
        |_language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let out_property = self_property.map(|ref_name| ref_name.is_tracked());
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert(
        "tracking_present",
        |_language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let out_property = self_property.map(|ref_name| ref_name.is_tracking_present());
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert(
        "tracking_ahead_count",
        |language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let repo = language.repo;
            let out_property =
                self_property.and_then(|ref_name| ref_name.tracking_ahead_count(repo));
            Ok(L::wrap_size_hint(out_property))
        },
    );
    map.insert(
        "tracking_behind_count",
        |language, _build_ctx, self_property, function| {
            function.expect_no_arguments()?;
            let repo = language.repo;
            let out_property =
                self_property.and_then(|ref_name| ref_name.tracking_behind_count(repo));
            Ok(L::wrap_size_hint(out_property))
        },
    );
    map
}

/// Cache for reverse lookup refs.
#[derive(Clone, Debug, Default)]
pub struct RefNamesIndex {
    index: HashMap<CommitId, Vec<Rc<RefName>>>,
}

impl RefNamesIndex {
    fn insert<'a>(&mut self, ids: impl IntoIterator<Item = &'a CommitId>, name: Rc<RefName>) {
        for id in ids {
            let ref_names = self.index.entry(id.clone()).or_default();
            ref_names.push(name.clone());
        }
    }

    pub fn get(&self, id: &CommitId) -> &[Rc<RefName>] {
        self.index.get(id).map_or(&[], |names: &Vec<_>| names)
    }
}

fn build_branches_index(repo: &dyn Repo) -> RefNamesIndex {
    let mut index = RefNamesIndex::default();
    for (branch_name, branch_target) in repo.view().branches() {
        let local_target = branch_target.local_target;
        let remote_refs = branch_target.remote_refs;
        if local_target.is_present() {
            let ref_name = RefName::local(
                branch_name,
                local_target.clone(),
                remote_refs.iter().map(|&(_, remote_ref)| remote_ref),
            );
            index.insert(local_target.added_ids(), ref_name);
        }
        for &(remote_name, remote_ref) in &remote_refs {
            let ref_name =
                RefName::remote(branch_name, remote_name, remote_ref.clone(), local_target);
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
        let ref_name = RefName::local_only(name, target.clone());
        index.insert(target.added_ids(), ref_name);
    }
    index
}

fn extract_git_head(repo: &dyn Repo, commit: &Commit) -> Option<Rc<RefName>> {
    let target = repo.view().git_head();
    target
        .added_ids()
        .contains(commit.id())
        .then(|| RefName::remote_only("HEAD", git::REMOTE_NAME_FOR_LOCAL_GIT_REPO, target.clone()))
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
        let ([], [len_node]) = function.expect_arguments()?;
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
            let ([], [len_node]) = function.expect_arguments()?;
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
            function.expect_no_arguments()?;
            let out_property = self_property.map(|id| id.prefix);
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert("rest", |_language, _build_ctx, self_property, function| {
        function.expect_no_arguments()?;
        let out_property = self_property.map(|id| id.rest);
        Ok(L::wrap_string(out_property))
    });
    map.insert("upper", |_language, _build_ctx, self_property, function| {
        function.expect_no_arguments()?;
        let out_property = self_property.map(|id| id.to_upper());
        Ok(L::wrap_shortest_id_prefix(out_property))
    });
    map.insert("lower", |_language, _build_ctx, self_property, function| {
        function.expect_no_arguments()?;
        let out_property = self_property.map(|id| id.to_lower());
        Ok(L::wrap_shortest_id_prefix(out_property))
    });
    map
}
