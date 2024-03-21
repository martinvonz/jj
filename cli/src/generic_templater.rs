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
/// the self type `C`.) The self type `C` is usually a tuple or struct of value
/// types. It's cloned several times internally. Keyword functions need to be
/// registered to extract properties from the self object.
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
    /// by `TemplateFunction`.
    ///
    /// ```ignore
    /// language.add_keyword("name", |self_property| {
    ///     let out_property = self_property.map(|v| v.to_string());
    ///     Ok(GenericTemplateLanguage::wrap_string(out_property))
    /// });
    /// ```
    pub fn add_keyword<F>(&mut self, name: &'static str, build: F)
    where
        F: Fn(
                Box<dyn TemplateProperty<Output = C> + 'a>,
            ) -> TemplateParseResult<GenericTemplatePropertyKind<'a, C>>
            + 'a,
    {
        self.build_fn_table.keywords.insert(name, Box::new(build));
    }
}

impl<'a, C: 'a> TemplateLanguage<'a> for GenericTemplateLanguage<'a, C> {
    type Property = GenericTemplatePropertyKind<'a, C>;

    template_builder::impl_core_wrap_property_fns!('a, GenericTemplatePropertyKind::Core);

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
            GenericTemplatePropertyKind::Core(property) => {
                let table = &self.build_fn_table.core;
                table.build_method(self, build_ctx, property, function)
            }
            GenericTemplatePropertyKind::Self_(property) => {
                let table = &self.build_fn_table.keywords;
                let build = template_parser::lookup_method("Self", table, function)?;
                // For simplicity, only 0-ary method is supported.
                template_parser::expect_no_arguments(function)?;
                build(property)
            }
        }
    }
}

impl<'a, C> GenericTemplateLanguage<'a, C> {
    pub fn wrap_self(
        property: impl TemplateProperty<Output = C> + 'a,
    ) -> GenericTemplatePropertyKind<'a, C> {
        GenericTemplatePropertyKind::Self_(Box::new(property))
    }
}

pub enum GenericTemplatePropertyKind<'a, C> {
    Core(CoreTemplatePropertyKind<'a>),
    Self_(Box<dyn TemplateProperty<Output = C> + 'a>),
}

impl<'a, C: 'a> IntoTemplateProperty<'a> for GenericTemplatePropertyKind<'a, C> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<Output = bool> + 'a>> {
        match self {
            GenericTemplatePropertyKind::Core(property) => property.try_into_boolean(),
            GenericTemplatePropertyKind::Self_(_) => None,
        }
    }

    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<Output = i64> + 'a>> {
        match self {
            GenericTemplatePropertyKind::Core(property) => property.try_into_integer(),
            GenericTemplatePropertyKind::Self_(_) => None,
        }
    }

    fn try_into_plain_text(self) -> Option<Box<dyn TemplateProperty<Output = String> + 'a>> {
        match self {
            GenericTemplatePropertyKind::Core(property) => property.try_into_plain_text(),
            GenericTemplatePropertyKind::Self_(_) => None,
        }
    }

    fn try_into_template(self) -> Option<Box<dyn Template + 'a>> {
        match self {
            GenericTemplatePropertyKind::Core(property) => property.try_into_template(),
            GenericTemplatePropertyKind::Self_(_) => None,
        }
    }
}

/// Function that translates keyword (or 0-ary method call node of the self type
/// `C`.)
///
/// Because the `GenericTemplateLanguage` doesn't provide a way to pass around
/// global resources, the keyword function is allowed to capture resources.
pub type GenericTemplateBuildKeywordFn<'a, C> = Box<
    dyn Fn(
            Box<dyn TemplateProperty<Output = C> + 'a>,
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
