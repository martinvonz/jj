// Copyright 2024 The Jujutsu Authors
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

use std::collections::HashMap;

use crate::template_builder::{
    self, BuildContext, CoreTemplateBuildFnTable, CoreTemplatePropertyKind, IntoTemplateProperty,
    TemplateLanguage,
};
use crate::template_parser::{self, FunctionCallNode, TemplateParseResult};
use crate::templater::{Template, TemplateProperty};

/// General-purpose template language for basic value types.
///
/// This template language only supports the core template property types (plus
/// the context type `C`.) The context type `C` is usually a tuple or struct of
/// value types. Keyword functions need to be registered to extract properties
/// from the context object.
pub struct GenericTemplateLanguage<'a, C> {
    build_fn_table: GenericTemplateBuildFnTable<'a, C>,
}

impl<'a, C> GenericTemplateLanguage<'a, C> {
    /// Sets up environment with no keywords.
    ///
    /// New keyword functions can be registered by `add_keyword()`.
    // It's not "Default" in a way that the core methods table is NOT empty.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self::with_keywords(HashMap::new())
    }

    /// Sets up environment with the given `keywords` table.
    pub fn with_keywords(keywords: GenericTemplateBuildKeywordFnMap<'a, C>) -> Self {
        GenericTemplateLanguage {
            build_fn_table: GenericTemplateBuildFnTable {
                core: CoreTemplateBuildFnTable::builtin(),
                keywords,
            },
        }
    }

    /// Registers new function that translates keyword to property.
    ///
    /// A keyword function returns `Self::Property`, which is basically a
    /// closure tagged by its return type. The inner closure is usually wrapped
    /// by `TemplatePropertyFn`.
    ///
    /// ```ignore
    /// language.add_keyword("name", |language| {
    ///     let property = TemplatePropertyFn(|v: &C| Ok(v.to_string()));
    ///     Ok(language.wrap_string(property))
    /// });
    /// ```
    pub fn add_keyword<F>(&mut self, name: &'static str, build: F)
    where
        F: Fn(&Self) -> TemplateParseResult<GenericTemplatePropertyKind<'a, C>> + 'a,
    {
        self.build_fn_table.keywords.insert(name, Box::new(build));
    }
}

impl<'a, C: 'a> TemplateLanguage<'a> for GenericTemplateLanguage<'a, C> {
    type Context = C;
    type Property = GenericTemplatePropertyKind<'a, C>;

    template_builder::impl_core_wrap_property_fns!('a, GenericTemplatePropertyKind::Core);

    fn build_self(&self) -> Self::Property {
        // No need to clone the context object because there are no other
        // objects of "Self" type, and the context is available everywhere.
        GenericTemplatePropertyKind::Self_
    }

    fn build_function(
        &self,
        build_ctx: &BuildContext<Self::Property>,
        function: &FunctionCallNode,
    ) -> TemplateParseResult<Self::Property> {
        template_builder::build_global_function(self, build_ctx, function)
    }

    fn build_method(
        &self,
        build_ctx: &BuildContext<Self::Property>,
        property: Self::Property,
        function: &FunctionCallNode,
    ) -> TemplateParseResult<Self::Property> {
        match property {
            GenericTemplatePropertyKind::Core(property) => {
                let table = &self.build_fn_table.core;
                table.build_method(self, build_ctx, property, function)
            }
            GenericTemplatePropertyKind::Self_ => {
                let table = &self.build_fn_table.keywords;
                let build = template_parser::lookup_method("Self", table, function)?;
                // For simplicity, only 0-ary method is supported.
                template_parser::expect_no_arguments(function)?;
                build(self)
            }
        }
    }
}

pub enum GenericTemplatePropertyKind<'a, C> {
    Core(CoreTemplatePropertyKind<'a, C>),
    Self_,
}

impl<'a, C: 'a> IntoTemplateProperty<'a, C> for GenericTemplatePropertyKind<'a, C> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<C, Output = bool> + 'a>> {
        match self {
            GenericTemplatePropertyKind::Core(property) => property.try_into_boolean(),
            GenericTemplatePropertyKind::Self_ => None,
        }
    }

    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<C, Output = i64> + 'a>> {
        match self {
            GenericTemplatePropertyKind::Core(property) => property.try_into_integer(),
            GenericTemplatePropertyKind::Self_ => None,
        }
    }

    fn try_into_plain_text(self) -> Option<Box<dyn TemplateProperty<C, Output = String> + 'a>> {
        match self {
            GenericTemplatePropertyKind::Core(property) => property.try_into_plain_text(),
            GenericTemplatePropertyKind::Self_ => None,
        }
    }

    fn try_into_template(self) -> Option<Box<dyn Template<C> + 'a>> {
        match self {
            GenericTemplatePropertyKind::Core(property) => property.try_into_template(),
            GenericTemplatePropertyKind::Self_ => None,
        }
    }
}

/// Function that translates keyword (or 0-ary method call node of the context
/// type `C`.)
///
/// Because the `GenericTemplateLanguage` doesn't provide a way to pass around
/// global resources, the keyword function is allowed to capture resources.
pub type GenericTemplateBuildKeywordFn<'a, C> = Box<
    dyn Fn(
            &GenericTemplateLanguage<'a, C>,
        ) -> TemplateParseResult<GenericTemplatePropertyKind<'a, C>>
        + 'a,
>;

/// Table of functions that translate keyword node.
pub type GenericTemplateBuildKeywordFnMap<'a, C> =
    HashMap<&'static str, GenericTemplateBuildKeywordFn<'a, C>>;

/// Symbol table of methods available in the general-purpose template.
struct GenericTemplateBuildFnTable<'a, C: 'a> {
    core: CoreTemplateBuildFnTable<'a, GenericTemplateLanguage<'a, C>>,
    keywords: GenericTemplateBuildKeywordFnMap<'a, C>,
}
