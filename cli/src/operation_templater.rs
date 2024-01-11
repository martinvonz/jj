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

use std::io;

use itertools::Itertools as _;
use jj_lib::object_id::ObjectId;
use jj_lib::op_store::{OperationId, OperationMetadata};
use jj_lib::operation::Operation;

use crate::formatter::Formatter;
use crate::template_builder::{
    self, BuildContext, CoreTemplatePropertyKind, IntoTemplateProperty, TemplateLanguage,
};
use crate::template_parser::{
    self, FunctionCallNode, TemplateAliasesMap, TemplateParseError, TemplateParseResult,
};
use crate::templater::{
    IntoTemplate, PlainTextFormattedProperty, Template, TemplateFunction, TemplateProperty,
    TemplatePropertyFn, TimestampRange,
};

struct OperationTemplateLanguage<'b> {
    current_op_id: Option<&'b OperationId>,
}

impl TemplateLanguage<'static> for OperationTemplateLanguage<'_> {
    type Context = Operation;
    type Property = OperationTemplatePropertyKind;

    template_builder::impl_core_wrap_property_fns!('static, OperationTemplatePropertyKind::Core);

    fn build_keyword(&self, name: &str, span: pest::Span) -> TemplateParseResult<Self::Property> {
        build_operation_keyword(self, name, span)
    }

    fn build_method(
        &self,
        build_ctx: &BuildContext<Self::Property>,
        property: Self::Property,
        function: &FunctionCallNode,
    ) -> TemplateParseResult<Self::Property> {
        match property {
            OperationTemplatePropertyKind::Core(property) => {
                template_builder::build_core_method(self, build_ctx, property, function)
            }
            OperationTemplatePropertyKind::OperationId(property) => {
                build_operation_id_method(self, build_ctx, property, function)
            }
        }
    }
}

impl OperationTemplateLanguage<'_> {
    fn wrap_operation_id(
        &self,
        property: impl TemplateProperty<Operation, Output = OperationId> + 'static,
    ) -> OperationTemplatePropertyKind {
        OperationTemplatePropertyKind::OperationId(Box::new(property))
    }
}

enum OperationTemplatePropertyKind {
    Core(CoreTemplatePropertyKind<'static, Operation>),
    OperationId(Box<dyn TemplateProperty<Operation, Output = OperationId>>),
}

impl IntoTemplateProperty<'static, Operation> for OperationTemplatePropertyKind {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<Operation, Output = bool>>> {
        match self {
            OperationTemplatePropertyKind::Core(property) => property.try_into_boolean(),
            OperationTemplatePropertyKind::OperationId(_) => None,
        }
    }

    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<Operation, Output = i64>>> {
        match self {
            OperationTemplatePropertyKind::Core(property) => property.try_into_integer(),
            _ => None,
        }
    }

    fn try_into_plain_text(self) -> Option<Box<dyn TemplateProperty<Operation, Output = String>>> {
        match self {
            OperationTemplatePropertyKind::Core(property) => property.try_into_plain_text(),
            _ => {
                let template = self.try_into_template()?;
                Some(Box::new(PlainTextFormattedProperty::new(template)))
            }
        }
    }

    fn try_into_template(self) -> Option<Box<dyn Template<Operation>>> {
        match self {
            OperationTemplatePropertyKind::Core(property) => property.try_into_template(),
            OperationTemplatePropertyKind::OperationId(property) => Some(property.into_template()),
        }
    }
}

fn build_operation_keyword(
    language: &OperationTemplateLanguage,
    name: &str,
    span: pest::Span,
) -> TemplateParseResult<OperationTemplatePropertyKind> {
    fn wrap_fn<O, F: Fn(&Operation) -> O>(f: F) -> TemplatePropertyFn<F> {
        TemplatePropertyFn(f)
    }
    fn wrap_metadata_fn<O>(
        f: impl Fn(&OperationMetadata) -> O + 'static,
    ) -> impl TemplateProperty<Operation, Output = O> {
        wrap_fn(move |op| f(&op.store_operation().metadata))
    }

    let property = match name {
        "current_operation" => {
            let current_op_id = language.current_op_id.cloned();
            language.wrap_boolean(wrap_fn(move |op| Some(op.id()) == current_op_id.as_ref()))
        }
        "description" => {
            language.wrap_string(wrap_metadata_fn(|metadata| metadata.description.clone()))
        }
        "id" => language.wrap_operation_id(wrap_fn(|op| op.id().clone())),
        "tags" => language.wrap_string(wrap_metadata_fn(|metadata| {
            // TODO: introduce map type
            metadata
                .tags
                .iter()
                .map(|(key, value)| format!("{key}: {value}"))
                .join("\n")
        })),
        "time" => language.wrap_timestamp_range(wrap_metadata_fn(|metadata| TimestampRange {
            start: metadata.start_time.clone(),
            end: metadata.end_time.clone(),
        })),
        "user" => language.wrap_string(wrap_metadata_fn(|metadata| {
            // TODO: introduce dedicated type and provide accessors?
            format!("{}@{}", metadata.username, metadata.hostname)
        })),
        _ => return Err(TemplateParseError::no_such_keyword(name, span)),
    };
    Ok(property)
}

impl Template<()> for OperationId {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(&self.hex())
    }
}

fn build_operation_id_method(
    language: &OperationTemplateLanguage,
    build_ctx: &BuildContext<OperationTemplatePropertyKind>,
    self_property: impl TemplateProperty<Operation, Output = OperationId> + 'static,
    function: &FunctionCallNode,
) -> TemplateParseResult<OperationTemplatePropertyKind> {
    let property = match function.name {
        "short" => {
            let ([], [len_node]) = template_parser::expect_arguments(function)?;
            let len_property = len_node
                .map(|node| template_builder::expect_integer_expression(language, build_ctx, node))
                .transpose()?;
            language.wrap_string(TemplateFunction::new(
                (self_property, len_property),
                |(id, len)| {
                    let mut hex = id.hex();
                    hex.truncate(len.map_or(12, |l| l.try_into().unwrap_or(0)));
                    hex
                },
            ))
        }
        _ => return Err(TemplateParseError::no_such_method("OperationId", function)),
    };
    Ok(property)
}

pub fn parse(
    current_op_id: Option<&OperationId>,
    template_text: &str,
    aliases_map: &TemplateAliasesMap,
) -> TemplateParseResult<Box<dyn Template<Operation>>> {
    let language = OperationTemplateLanguage { current_op_id };
    let node = template_parser::parse(template_text, aliases_map)?;
    template_builder::build(&language, &node)
}
