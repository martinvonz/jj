// Copyright 2023 The Jujutsu Authors
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
use std::collections::HashMap;
use std::io;

use itertools::Itertools as _;
use jj_lib::extensions_map::ExtensionsMap;
use jj_lib::object_id::ObjectId;
use jj_lib::op_store::OperationId;
use jj_lib::operation::Operation;

use crate::template_builder::{
    self, merge_fn_map, BuildContext, CoreTemplateBuildFnTable, CoreTemplatePropertyKind,
    IntoTemplateProperty, TemplateBuildMethodFnMap, TemplateLanguage,
};
use crate::template_parser::{self, FunctionCallNode, TemplateParseResult};
use crate::templater::{
    IntoTemplate, PlainTextFormattedProperty, Template, TemplateFormatter, TemplateProperty,
    TemplatePropertyExt as _, TimestampRange,
};

pub trait OperationTemplateLanguageExtension {
    fn build_fn_table(&self) -> OperationTemplateBuildFnTable;

    fn build_cache_extensions(&self, extensions: &mut ExtensionsMap);
}

pub struct OperationTemplateLanguage {
    root_op_id: OperationId,
    current_op_id: Option<OperationId>,
    build_fn_table: OperationTemplateBuildFnTable,
    cache_extensions: ExtensionsMap,
}

impl OperationTemplateLanguage {
    /// Sets up environment where operation template will be transformed to
    /// evaluation tree.
    pub fn new(
        root_op_id: &OperationId,
        current_op_id: Option<&OperationId>,
        extension: Option<&dyn OperationTemplateLanguageExtension>,
    ) -> Self {
        let mut build_fn_table = OperationTemplateBuildFnTable::builtin();
        let mut cache_extensions = ExtensionsMap::empty();
        if let Some(extension) = extension {
            let ext_table = extension.build_fn_table();
            build_fn_table.merge(ext_table);
            extension.build_cache_extensions(&mut cache_extensions);
        }

        OperationTemplateLanguage {
            root_op_id: root_op_id.clone(),
            current_op_id: current_op_id.cloned(),
            build_fn_table,
            cache_extensions,
        }
    }
}

impl TemplateLanguage<'static> for OperationTemplateLanguage {
    type Property = OperationTemplatePropertyKind;

    template_builder::impl_core_wrap_property_fns!('static, OperationTemplatePropertyKind::Core);

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
            OperationTemplatePropertyKind::Core(property) => {
                let table = &self.build_fn_table.core;
                table.build_method(self, build_ctx, property, function)
            }
            OperationTemplatePropertyKind::Operation(property) => {
                let table = &self.build_fn_table.operation_methods;
                let build = template_parser::lookup_method("Operation", table, function)?;
                build(self, build_ctx, property, function)
            }
            OperationTemplatePropertyKind::OperationId(property) => {
                let table = &self.build_fn_table.operation_id_methods;
                let build = template_parser::lookup_method("OperationId", table, function)?;
                build(self, build_ctx, property, function)
            }
        }
    }
}

impl OperationTemplateLanguage {
    pub fn cache_extension<T: Any>(&self) -> Option<&T> {
        self.cache_extensions.get::<T>()
    }

    pub fn wrap_operation(
        property: impl TemplateProperty<Output = Operation> + 'static,
    ) -> OperationTemplatePropertyKind {
        OperationTemplatePropertyKind::Operation(Box::new(property))
    }

    pub fn wrap_operation_id(
        property: impl TemplateProperty<Output = OperationId> + 'static,
    ) -> OperationTemplatePropertyKind {
        OperationTemplatePropertyKind::OperationId(Box::new(property))
    }
}

pub enum OperationTemplatePropertyKind {
    Core(CoreTemplatePropertyKind<'static>),
    Operation(Box<dyn TemplateProperty<Output = Operation>>),
    OperationId(Box<dyn TemplateProperty<Output = OperationId>>),
}

impl IntoTemplateProperty<'static> for OperationTemplatePropertyKind {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<Output = bool>>> {
        match self {
            OperationTemplatePropertyKind::Core(property) => property.try_into_boolean(),
            OperationTemplatePropertyKind::Operation(_) => None,
            OperationTemplatePropertyKind::OperationId(_) => None,
        }
    }

    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<Output = i64>>> {
        match self {
            OperationTemplatePropertyKind::Core(property) => property.try_into_integer(),
            _ => None,
        }
    }

    fn try_into_plain_text(self) -> Option<Box<dyn TemplateProperty<Output = String>>> {
        match self {
            OperationTemplatePropertyKind::Core(property) => property.try_into_plain_text(),
            _ => {
                let template = self.try_into_template()?;
                Some(Box::new(PlainTextFormattedProperty::new(template)))
            }
        }
    }

    fn try_into_template(self) -> Option<Box<dyn Template>> {
        match self {
            OperationTemplatePropertyKind::Core(property) => property.try_into_template(),
            OperationTemplatePropertyKind::Operation(_) => None,
            OperationTemplatePropertyKind::OperationId(property) => Some(property.into_template()),
        }
    }
}

/// Table of functions that translate method call node of self type `T`.
pub type OperationTemplateBuildMethodFnMap<T> =
    TemplateBuildMethodFnMap<'static, OperationTemplateLanguage, T>;

/// Symbol table of methods available in the operation template.
pub struct OperationTemplateBuildFnTable {
    pub core: CoreTemplateBuildFnTable<'static, OperationTemplateLanguage>,
    pub operation_methods: OperationTemplateBuildMethodFnMap<Operation>,
    pub operation_id_methods: OperationTemplateBuildMethodFnMap<OperationId>,
}

impl OperationTemplateBuildFnTable {
    /// Creates new symbol table containing the builtin methods.
    fn builtin() -> Self {
        OperationTemplateBuildFnTable {
            core: CoreTemplateBuildFnTable::builtin(),
            operation_methods: builtin_operation_methods(),
            operation_id_methods: builtin_operation_id_methods(),
        }
    }

    pub fn empty() -> Self {
        OperationTemplateBuildFnTable {
            core: CoreTemplateBuildFnTable::empty(),
            operation_methods: HashMap::new(),
            operation_id_methods: HashMap::new(),
        }
    }

    fn merge(&mut self, other: OperationTemplateBuildFnTable) {
        let OperationTemplateBuildFnTable {
            core,
            operation_methods,
            operation_id_methods,
        } = other;

        self.core.merge(core);
        merge_fn_map(&mut self.operation_methods, operation_methods);
        merge_fn_map(&mut self.operation_id_methods, operation_id_methods);
    }
}

fn builtin_operation_methods() -> OperationTemplateBuildMethodFnMap<Operation> {
    type L = OperationTemplateLanguage;
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map = OperationTemplateBuildMethodFnMap::<Operation>::new();
    map.insert(
        "current_operation",
        |language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let current_op_id = language.current_op_id.clone();
            let out_property = self_property.map(move |op| Some(op.id()) == current_op_id.as_ref());
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert(
        "description",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property = self_property.map(|op| op.metadata().description.clone());
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert("id", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|op| op.id().clone());
        Ok(L::wrap_operation_id(out_property))
    });
    map.insert("tags", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|op| {
            // TODO: introduce map type
            op.metadata()
                .tags
                .iter()
                .map(|(key, value)| format!("{key}: {value}"))
                .join("\n")
        });
        Ok(L::wrap_string(out_property))
    });
    map.insert("time", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|op| TimestampRange {
            start: op.metadata().start_time.clone(),
            end: op.metadata().end_time.clone(),
        });
        Ok(L::wrap_timestamp_range(out_property))
    });
    map.insert("user", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|op| {
            // TODO: introduce dedicated type and provide accessors?
            format!("{}@{}", op.metadata().username, op.metadata().hostname)
        });
        Ok(L::wrap_string(out_property))
    });
    map.insert("root", |language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let root_op_id = language.root_op_id.clone();
        let out_property = self_property.map(move |op| op.id() == &root_op_id);
        Ok(L::wrap_boolean(out_property))
    });
    map
}

impl Template for OperationId {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        write!(formatter, "{}", self.hex())
    }
}

fn builtin_operation_id_methods() -> OperationTemplateBuildMethodFnMap<OperationId> {
    type L = OperationTemplateLanguage;
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map = OperationTemplateBuildMethodFnMap::<OperationId>::new();
    map.insert("short", |language, build_ctx, self_property, function| {
        let ([], [len_node]) = template_parser::expect_arguments(function)?;
        let len_property = len_node
            .map(|node| template_builder::expect_usize_expression(language, build_ctx, node))
            .transpose()?;
        let out_property = (self_property, len_property).map(|(id, len)| {
            let mut hex = id.hex();
            hex.truncate(len.unwrap_or(12));
            hex
        });
        Ok(L::wrap_string(out_property))
    });
    map
}
