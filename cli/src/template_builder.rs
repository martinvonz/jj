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

use std::collections::HashMap;

use itertools::Itertools as _;
use jj_lib::backend::{Signature, Timestamp};

use crate::template_parser::{
    self, ExpressionKind, ExpressionNode, FunctionCallNode, MethodCallNode, TemplateParseError,
    TemplateParseResult,
};
use crate::templater::{
    ConcatTemplate, ConditionalTemplate, IntoTemplate, LabelTemplate, ListPropertyTemplate,
    ListTemplate, Literal, PlainTextFormattedProperty, PropertyPlaceholder, ReformatTemplate,
    SeparateTemplate, Template, TemplateFunction, TemplateProperty, TimestampRange,
};
use crate::{text_util, time_util};

/// Callbacks to build language-specific evaluation objects from AST nodes.
pub trait TemplateLanguage<'a> {
    type Context: 'a;
    type Property: IntoTemplateProperty<'a, Self::Context>;

    fn wrap_string(
        &self,
        property: impl TemplateProperty<Self::Context, Output = String> + 'a,
    ) -> Self::Property;
    fn wrap_string_list(
        &self,
        property: impl TemplateProperty<Self::Context, Output = Vec<String>> + 'a,
    ) -> Self::Property;
    fn wrap_boolean(
        &self,
        property: impl TemplateProperty<Self::Context, Output = bool> + 'a,
    ) -> Self::Property;
    fn wrap_integer(
        &self,
        property: impl TemplateProperty<Self::Context, Output = i64> + 'a,
    ) -> Self::Property;
    fn wrap_signature(
        &self,
        property: impl TemplateProperty<Self::Context, Output = Signature> + 'a,
    ) -> Self::Property;
    fn wrap_timestamp(
        &self,
        property: impl TemplateProperty<Self::Context, Output = Timestamp> + 'a,
    ) -> Self::Property;
    fn wrap_timestamp_range(
        &self,
        property: impl TemplateProperty<Self::Context, Output = TimestampRange> + 'a,
    ) -> Self::Property;

    fn wrap_template(&self, template: Box<dyn Template<Self::Context> + 'a>) -> Self::Property;
    fn wrap_list_template(
        &self,
        template: Box<dyn ListTemplate<Self::Context> + 'a>,
    ) -> Self::Property;

    fn build_keyword(&self, name: &str, span: pest::Span) -> TemplateParseResult<Self::Property>;
    fn build_method(
        &self,
        build_ctx: &BuildContext<Self::Property>,
        property: Self::Property,
        function: &FunctionCallNode,
    ) -> TemplateParseResult<Self::Property>;
}

/// Implements `TemplateLanguage::wrap_<type>()` functions.
///
/// - `impl_core_wrap_property_fns('a)` for `CoreTemplatePropertyKind`,
/// - `impl_core_wrap_property_fns('a, MyKind::Core)` for `MyKind::Core(..)`.
macro_rules! impl_core_wrap_property_fns {
    ($a:lifetime) => {
        $crate::template_builder::impl_core_wrap_property_fns!($a, std::convert::identity);
    };
    ($a:lifetime, $outer:path) => {
        $crate::template_builder::impl_wrap_property_fns!(
            $a, $crate::template_builder::CoreTemplatePropertyKind, $outer, {
                wrap_string(String) => String,
                wrap_string_list(Vec<String>) => StringList,
                wrap_boolean(bool) => Boolean,
                wrap_integer(i64) => Integer,
                wrap_signature(jj_lib::backend::Signature) => Signature,
                wrap_timestamp(jj_lib::backend::Timestamp) => Timestamp,
                wrap_timestamp_range($crate::templater::TimestampRange) => TimestampRange,
            }
        );
        fn wrap_template(
            &self,
            template: Box<dyn $crate::templater::Template<Self::Context> + $a>,
        ) -> Self::Property {
            use $crate::template_builder::CoreTemplatePropertyKind as Kind;
            $outer(Kind::Template(template))
        }
        fn wrap_list_template(
            &self,
            template: Box<dyn $crate::templater::ListTemplate<Self::Context> + $a>,
        ) -> Self::Property {
            use $crate::template_builder::CoreTemplatePropertyKind as Kind;
            $outer(Kind::ListTemplate(template))
        }
    };
}

macro_rules! impl_wrap_property_fns {
    ($a:lifetime, $kind:path, $outer:path, { $( $func:ident($ty:ty) => $var:ident, )+ }) => {
        $(
            fn $func(
                &self,
                property: impl $crate::templater::TemplateProperty<
                    Self::Context, Output = $ty> + $a,
            ) -> Self::Property {
                use $kind as Kind; // https://github.com/rust-lang/rust/issues/48067
                $outer(Kind::$var(Box::new(property)))
            }
        )+
    };
}

pub(crate) use {impl_core_wrap_property_fns, impl_wrap_property_fns};

/// Provides access to basic template property types.
pub trait IntoTemplateProperty<'a, C> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<C, Output = bool> + 'a>>;
    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<C, Output = i64> + 'a>>;

    fn try_into_plain_text(self) -> Option<Box<dyn TemplateProperty<C, Output = String> + 'a>>;
    fn try_into_template(self) -> Option<Box<dyn Template<C> + 'a>>;
}

pub enum CoreTemplatePropertyKind<'a, I> {
    String(Box<dyn TemplateProperty<I, Output = String> + 'a>),
    StringList(Box<dyn TemplateProperty<I, Output = Vec<String>> + 'a>),
    Boolean(Box<dyn TemplateProperty<I, Output = bool> + 'a>),
    Integer(Box<dyn TemplateProperty<I, Output = i64> + 'a>),
    Signature(Box<dyn TemplateProperty<I, Output = Signature> + 'a>),
    Timestamp(Box<dyn TemplateProperty<I, Output = Timestamp> + 'a>),
    TimestampRange(Box<dyn TemplateProperty<I, Output = TimestampRange> + 'a>),

    // Similar to `TemplateProperty<I, Output = Box<dyn Template<()> + 'a>`, but doesn't
    // capture `I` to produce `Template<()>`. The context `I` would have to be cloned
    // to convert `Template<I>` to `Template<()>`.
    Template(Box<dyn Template<I> + 'a>),
    ListTemplate(Box<dyn ListTemplate<I> + 'a>),
}

impl<'a, I: 'a> IntoTemplateProperty<'a, I> for CoreTemplatePropertyKind<'a, I> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<I, Output = bool> + 'a>> {
        match self {
            CoreTemplatePropertyKind::String(property) => {
                Some(Box::new(TemplateFunction::new(property, |s| !s.is_empty())))
            }
            // TODO: should we allow implicit cast of List type?
            CoreTemplatePropertyKind::Boolean(property) => Some(property),
            _ => None,
        }
    }

    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<I, Output = i64> + 'a>> {
        match self {
            CoreTemplatePropertyKind::Integer(property) => Some(property),
            _ => None,
        }
    }

    fn try_into_plain_text(self) -> Option<Box<dyn TemplateProperty<I, Output = String> + 'a>> {
        match self {
            CoreTemplatePropertyKind::String(property) => Some(property),
            _ => {
                let template = self.try_into_template()?;
                Some(Box::new(PlainTextFormattedProperty::new(template)))
            }
        }
    }

    fn try_into_template(self) -> Option<Box<dyn Template<I> + 'a>> {
        match self {
            CoreTemplatePropertyKind::String(property) => Some(property.into_template()),
            CoreTemplatePropertyKind::StringList(property) => Some(property.into_template()),
            CoreTemplatePropertyKind::Boolean(property) => Some(property.into_template()),
            CoreTemplatePropertyKind::Integer(property) => Some(property.into_template()),
            CoreTemplatePropertyKind::Signature(property) => Some(property.into_template()),
            CoreTemplatePropertyKind::Timestamp(property) => Some(property.into_template()),
            CoreTemplatePropertyKind::TimestampRange(property) => Some(property.into_template()),
            CoreTemplatePropertyKind::Template(template) => Some(template),
            CoreTemplatePropertyKind::ListTemplate(template) => Some(template.into_template()),
        }
    }
}

/// Opaque struct that represents a template value.
pub struct Expression<P> {
    property: P,
    labels: Vec<String>,
}

impl<P> Expression<P> {
    fn unlabeled(property: P) -> Self {
        let labels = vec![];
        Expression { property, labels }
    }

    fn with_label(property: P, label: impl Into<String>) -> Self {
        let labels = vec![label.into()];
        Expression { property, labels }
    }

    pub fn try_into_boolean<'a, C: 'a>(
        self,
    ) -> Option<Box<dyn TemplateProperty<C, Output = bool> + 'a>>
    where
        P: IntoTemplateProperty<'a, C>,
    {
        self.property.try_into_boolean()
    }

    pub fn try_into_integer<'a, C: 'a>(
        self,
    ) -> Option<Box<dyn TemplateProperty<C, Output = i64> + 'a>>
    where
        P: IntoTemplateProperty<'a, C>,
    {
        self.property.try_into_integer()
    }

    pub fn try_into_plain_text<'a, C: 'a>(
        self,
    ) -> Option<Box<dyn TemplateProperty<C, Output = String> + 'a>>
    where
        P: IntoTemplateProperty<'a, C>,
    {
        self.property.try_into_plain_text()
    }

    pub fn try_into_template<'a, C: 'a>(self) -> Option<Box<dyn Template<C> + 'a>>
    where
        P: IntoTemplateProperty<'a, C>,
    {
        let template = self.property.try_into_template()?;
        if self.labels.is_empty() {
            Some(template)
        } else {
            Some(Box::new(LabelTemplate::new(template, Literal(self.labels))))
        }
    }
}

pub struct BuildContext<'i, P> {
    /// Map of functions to create `L::Property`.
    local_variables: HashMap<&'i str, &'i (dyn Fn() -> P)>,
}

fn build_method_call<'a, L: TemplateLanguage<'a>>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    method: &MethodCallNode,
) -> TemplateParseResult<Expression<L::Property>> {
    let mut expression = build_expression(language, build_ctx, &method.object)?;
    expression.property =
        language.build_method(build_ctx, expression.property, &method.function)?;
    expression.labels.push(method.function.name.to_owned());
    Ok(expression)
}

pub fn build_core_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    property: CoreTemplatePropertyKind<'a, L::Context>,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    match property {
        CoreTemplatePropertyKind::String(property) => {
            build_string_method(language, build_ctx, property, function)
        }
        CoreTemplatePropertyKind::StringList(property) => {
            build_formattable_list_method(language, build_ctx, property, function, |item| {
                language.wrap_string(item)
            })
        }
        CoreTemplatePropertyKind::Boolean(property) => {
            build_boolean_method(language, build_ctx, property, function)
        }
        CoreTemplatePropertyKind::Integer(property) => {
            build_integer_method(language, build_ctx, property, function)
        }
        CoreTemplatePropertyKind::Signature(property) => {
            build_signature_method(language, build_ctx, property, function)
        }
        CoreTemplatePropertyKind::Timestamp(property) => {
            build_timestamp_method(language, build_ctx, property, function)
        }
        CoreTemplatePropertyKind::TimestampRange(property) => {
            build_timestamp_range_method(language, build_ctx, property, function)
        }
        CoreTemplatePropertyKind::Template(_) => {
            Err(TemplateParseError::no_such_method("Template", function))
        }
        CoreTemplatePropertyKind::ListTemplate(template) => {
            build_list_template_method(language, build_ctx, template, function)
        }
    }
}

fn build_string_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    self_property: impl TemplateProperty<L::Context, Output = String> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "contains" => {
            let [needle_node] = template_parser::expect_exact_arguments(function)?;
            // TODO: or .try_into_string() to disable implicit type cast?
            let needle_property = expect_plain_text_expression(language, build_ctx, needle_node)?;
            language.wrap_boolean(TemplateFunction::new(
                (self_property, needle_property),
                |(haystack, needle)| haystack.contains(&needle),
            ))
        }
        "starts_with" => {
            let [needle_node] = template_parser::expect_exact_arguments(function)?;
            let needle_property = expect_plain_text_expression(language, build_ctx, needle_node)?;
            language.wrap_boolean(TemplateFunction::new(
                (self_property, needle_property),
                move |(haystack, needle)| haystack.starts_with(&needle),
            ))
        }
        "ends_with" => {
            let [needle_node] = template_parser::expect_exact_arguments(function)?;
            let needle_property = expect_plain_text_expression(language, build_ctx, needle_node)?;
            language.wrap_boolean(TemplateFunction::new(
                (self_property, needle_property),
                move |(haystack, needle)| haystack.ends_with(&needle),
            ))
        }
        "remove_prefix" => {
            let [needle_node] = template_parser::expect_exact_arguments(function)?;
            let needle_property = expect_plain_text_expression(language, build_ctx, needle_node)?;
            language.wrap_string(TemplateFunction::new(
                (self_property, needle_property),
                move |(haystack, needle)| {
                    haystack
                        .strip_prefix(&needle)
                        .map(ToOwned::to_owned)
                        .unwrap_or(haystack)
                },
            ))
        }
        "remove_suffix" => {
            let [needle_node] = template_parser::expect_exact_arguments(function)?;
            let needle_property = expect_plain_text_expression(language, build_ctx, needle_node)?;
            language.wrap_string(TemplateFunction::new(
                (self_property, needle_property),
                move |(haystack, needle)| {
                    haystack
                        .strip_suffix(&needle)
                        .map(ToOwned::to_owned)
                        .unwrap_or(haystack)
                },
            ))
        }
        "substr" => {
            let [start_idx, end_idx] = template_parser::expect_exact_arguments(function)?;
            let start_idx_property = expect_integer_expression(language, build_ctx, start_idx)?;
            let end_idx_property = expect_integer_expression(language, build_ctx, end_idx)?;
            language.wrap_string(TemplateFunction::new(
                (self_property, start_idx_property, end_idx_property),
                |(s, start_idx, end_idx)| {
                    // TODO: If we add .len() method, we'll expose bytes-based and char-based APIs.
                    // Having different index units would be confusing, so we might want to change
                    // .substr() to bytes-based and round up/down towards char or grapheme-cluster
                    // boundary.
                    let to_idx = |i: i64| -> usize {
                        let magnitude = usize::try_from(i.unsigned_abs()).unwrap_or(usize::MAX);
                        if i < 0 {
                            s.chars().count().saturating_sub(magnitude)
                        } else {
                            magnitude
                        }
                    };
                    let start_idx = to_idx(start_idx);
                    let end_idx = to_idx(end_idx);
                    if start_idx >= end_idx {
                        String::new()
                    } else {
                        s.chars()
                            .skip(start_idx)
                            .take(end_idx - start_idx)
                            .collect()
                    }
                },
            ))
        }
        "first_line" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |s| {
                s.lines().next().unwrap_or_default().to_string()
            }))
        }
        "lines" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_string_list(TemplateFunction::new(self_property, |s| {
                s.lines().map(|l| l.to_owned()).collect()
            }))
        }
        "upper" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |s| s.to_uppercase()))
        }
        "lower" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |s| s.to_lowercase()))
        }
        _ => return Err(TemplateParseError::no_such_method("String", function)),
    };
    Ok(property)
}

fn build_boolean_method<'a, L: TemplateLanguage<'a>>(
    _language: &L,
    _build_ctx: &BuildContext<L::Property>,
    _self_property: impl TemplateProperty<L::Context, Output = bool> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    Err(TemplateParseError::no_such_method("Boolean", function))
}

fn build_integer_method<'a, L: TemplateLanguage<'a>>(
    _language: &L,
    _build_ctx: &BuildContext<L::Property>,
    _self_property: impl TemplateProperty<L::Context, Output = i64> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    Err(TemplateParseError::no_such_method("Integer", function))
}

fn build_signature_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    _build_ctx: &BuildContext<L::Property>,
    self_property: impl TemplateProperty<L::Context, Output = Signature> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "name" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |signature| {
                signature.name
            }))
        }
        "email" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |signature| {
                signature.email
            }))
        }
        "username" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |signature| {
                let (username, _) = text_util::split_email(&signature.email);
                username.to_owned()
            }))
        }
        "timestamp" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_timestamp(TemplateFunction::new(self_property, |signature| {
                signature.timestamp
            }))
        }
        _ => return Err(TemplateParseError::no_such_method("Signature", function)),
    };
    Ok(property)
}

fn build_timestamp_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    _build_ctx: &BuildContext<L::Property>,
    self_property: impl TemplateProperty<L::Context, Output = Timestamp> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "ago" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |timestamp| {
                time_util::format_timestamp_relative_to_now(&timestamp)
            }))
        }
        "format" => {
            // No dynamic string is allowed as the templater has no runtime error type.
            let [format_node] = template_parser::expect_exact_arguments(function)?;
            let format =
                template_parser::expect_string_literal_with(format_node, |format, span| {
                    time_util::FormattingItems::parse(format).ok_or_else(|| {
                        TemplateParseError::unexpected_expression("Invalid time format", span)
                    })
                })?
                .into_owned();
            language.wrap_string(TemplateFunction::new(self_property, move |timestamp| {
                time_util::format_absolute_timestamp_with(&timestamp, &format)
            }))
        }
        "utc" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_timestamp(TemplateFunction::new(self_property, |mut timestamp| {
                timestamp.tz_offset = 0;
                timestamp
            }))
        }
        _ => return Err(TemplateParseError::no_such_method("Timestamp", function)),
    };
    Ok(property)
}

fn build_timestamp_range_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    _build_ctx: &BuildContext<L::Property>,
    self_property: impl TemplateProperty<L::Context, Output = TimestampRange> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "start" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_timestamp(TemplateFunction::new(self_property, |time_range| {
                time_range.start
            }))
        }
        "end" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_timestamp(TemplateFunction::new(self_property, |time_range| {
                time_range.end
            }))
        }
        "duration" => {
            template_parser::expect_no_arguments(function)?;
            language.wrap_string(TemplateFunction::new(self_property, |time_range| {
                time_range.duration()
            }))
        }
        _ => {
            return Err(TemplateParseError::no_such_method(
                "TimestampRange",
                function,
            ))
        }
    };
    Ok(property)
}

fn build_list_template_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    self_template: Box<dyn ListTemplate<L::Context> + 'a>,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "join" => {
            let [separator_node] = template_parser::expect_exact_arguments(function)?;
            let separator = expect_template_expression(language, build_ctx, separator_node)?;
            language.wrap_template(self_template.join(separator))
        }
        _ => return Err(TemplateParseError::no_such_method("ListTemplate", function)),
    };
    Ok(property)
}

/// Builds method call expression for printable list property.
pub fn build_formattable_list_method<'a, L, O>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    self_property: impl TemplateProperty<L::Context, Output = Vec<O>> + 'a,
    function: &FunctionCallNode,
    // TODO: Generic L: WrapProperty<L::Context, O> trait might be needed to support more
    // list operations such as first()/slice(). For .map(), a simple callback works.
    wrap_item: impl Fn(PropertyPlaceholder<O>) -> L::Property,
) -> TemplateParseResult<L::Property>
where
    L: TemplateLanguage<'a>,
    O: Template<()> + Clone + 'a,
{
    let property = match function.name {
        "join" => {
            let [separator_node] = template_parser::expect_exact_arguments(function)?;
            let separator = expect_template_expression(language, build_ctx, separator_node)?;
            let template =
                ListPropertyTemplate::new(self_property, separator, |_, formatter, item| {
                    item.format(&(), formatter)
                });
            language.wrap_template(Box::new(template))
        }
        "map" => build_map_operation(language, build_ctx, self_property, function, wrap_item)?,
        _ => return Err(TemplateParseError::no_such_method("List", function)),
    };
    Ok(property)
}

pub fn build_unformattable_list_method<'a, L, O>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    self_property: impl TemplateProperty<L::Context, Output = Vec<O>> + 'a,
    function: &FunctionCallNode,
    wrap_item: impl Fn(PropertyPlaceholder<O>) -> L::Property,
) -> TemplateParseResult<L::Property>
where
    L: TemplateLanguage<'a>,
    O: Clone + 'a,
{
    let property = match function.name {
        // No "join"
        "map" => build_map_operation(language, build_ctx, self_property, function, wrap_item)?,
        _ => return Err(TemplateParseError::no_such_method("List", function)),
    };
    Ok(property)
}

/// Builds expression that extracts iterable property and applies template to
/// each item.
///
/// `wrap_item()` is the function to wrap a list item of type `O` as a property.
fn build_map_operation<'a, L, O, P>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    self_property: P,
    function: &FunctionCallNode,
    wrap_item: impl Fn(PropertyPlaceholder<O>) -> L::Property,
) -> TemplateParseResult<L::Property>
where
    L: TemplateLanguage<'a>,
    P: TemplateProperty<L::Context> + 'a,
    P::Output: IntoIterator<Item = O>,
    O: Clone + 'a,
{
    // Build an item template with placeholder property, then evaluate it
    // for each item.
    //
    // It would be nice if we could build a template of (L::Context, O)
    // input, but doing that for a generic item type wouldn't be easy. It's
    // also invalid to convert &C to &(C, _).
    let [lambda_node] = template_parser::expect_exact_arguments(function)?;
    let item_placeholder = PropertyPlaceholder::new();
    let item_template = template_parser::expect_lambda_with(lambda_node, |lambda, _span| {
        let item_fn = || wrap_item(item_placeholder.clone());
        let mut local_variables = build_ctx.local_variables.clone();
        if let [name] = lambda.params.as_slice() {
            local_variables.insert(name, &item_fn);
        } else {
            return Err(TemplateParseError::unexpected_expression(
                "Expected 1 lambda parameters",
                lambda.params_span,
            ));
        }
        let build_ctx = BuildContext { local_variables };
        expect_template_expression(language, &build_ctx, &lambda.body)
    })?;
    let list_template = ListPropertyTemplate::new(
        self_property,
        Literal(" "), // separator
        move |context, formatter, item| {
            item_placeholder.with_value(item, || item_template.format(context, formatter))
        },
    );
    Ok(language.wrap_list_template(Box::new(list_template)))
}

fn build_global_function<'a, L: TemplateLanguage<'a>>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    function: &FunctionCallNode,
) -> TemplateParseResult<Expression<L::Property>> {
    let property = match function.name {
        "fill" => {
            let [width_node, content_node] = template_parser::expect_exact_arguments(function)?;
            let width = expect_integer_expression(language, build_ctx, width_node)?;
            let content = expect_template_expression(language, build_ctx, content_node)?;
            let template = ReformatTemplate::new(content, move |context, formatter, recorded| {
                let width = width.extract(context).try_into().unwrap_or(0);
                text_util::write_wrapped(formatter, recorded, width)
            });
            language.wrap_template(Box::new(template))
        }
        "indent" => {
            let [prefix_node, content_node] = template_parser::expect_exact_arguments(function)?;
            let prefix = expect_template_expression(language, build_ctx, prefix_node)?;
            let content = expect_template_expression(language, build_ctx, content_node)?;
            let template = ReformatTemplate::new(content, move |context, formatter, recorded| {
                text_util::write_indented(formatter, recorded, |formatter| {
                    // If Template::format() returned a custom error type, we would need to
                    // handle template evaluation error out of this closure:
                    //   prefix.format(context, &mut prefix_recorder)?;
                    //   write_indented(formatter, recorded, |formatter| {
                    //       prefix_recorder.replay(formatter)
                    //   })
                    prefix.format(context, formatter)
                })
            });
            language.wrap_template(Box::new(template))
        }
        "label" => {
            let [label_node, content_node] = template_parser::expect_exact_arguments(function)?;
            let label_property = expect_plain_text_expression(language, build_ctx, label_node)?;
            let content = expect_template_expression(language, build_ctx, content_node)?;
            let labels = TemplateFunction::new(label_property, |s| {
                s.split_whitespace().map(ToString::to_string).collect()
            });
            language.wrap_template(Box::new(LabelTemplate::new(content, labels)))
        }
        "if" => {
            let ([condition_node, true_node], [false_node]) =
                template_parser::expect_arguments(function)?;
            let condition = expect_boolean_expression(language, build_ctx, condition_node)?;
            let true_template = expect_template_expression(language, build_ctx, true_node)?;
            let false_template = false_node
                .map(|node| expect_template_expression(language, build_ctx, node))
                .transpose()?;
            let template = ConditionalTemplate::new(condition, true_template, false_template);
            language.wrap_template(Box::new(template))
        }
        "concat" => {
            let contents = function
                .args
                .iter()
                .map(|node| expect_template_expression(language, build_ctx, node))
                .try_collect()?;
            language.wrap_template(Box::new(ConcatTemplate(contents)))
        }
        "separate" => {
            let ([separator_node], content_nodes) =
                template_parser::expect_some_arguments(function)?;
            let separator = expect_template_expression(language, build_ctx, separator_node)?;
            let contents = content_nodes
                .iter()
                .map(|node| expect_template_expression(language, build_ctx, node))
                .try_collect()?;
            language.wrap_template(Box::new(SeparateTemplate::new(separator, contents)))
        }
        _ => return Err(TemplateParseError::no_such_function(function)),
    };
    Ok(Expression::unlabeled(property))
}

/// Builds intermediate expression tree from AST nodes.
pub fn build_expression<'a, L: TemplateLanguage<'a>>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    node: &ExpressionNode,
) -> TemplateParseResult<Expression<L::Property>> {
    match &node.kind {
        ExpressionKind::Identifier(name) => {
            if let Some(make) = build_ctx.local_variables.get(name) {
                // Don't label a local variable with its name
                Ok(Expression::unlabeled(make()))
            } else {
                let property = language.build_keyword(name, node.span)?;
                Ok(Expression::with_label(property, *name))
            }
        }
        ExpressionKind::Boolean(value) => {
            let property = language.wrap_boolean(Literal(*value));
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::Integer(value) => {
            let property = language.wrap_integer(Literal(*value));
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::String(value) => {
            let property = language.wrap_string(Literal(value.clone()));
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::Concat(nodes) => {
            let templates = nodes
                .iter()
                .map(|node| expect_template_expression(language, build_ctx, node))
                .try_collect()?;
            let property = language.wrap_template(Box::new(ConcatTemplate(templates)));
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::FunctionCall(function) => {
            build_global_function(language, build_ctx, function)
        }
        ExpressionKind::MethodCall(method) => build_method_call(language, build_ctx, method),
        ExpressionKind::Lambda(_) => Err(TemplateParseError::unexpected_expression(
            "Lambda cannot be defined here",
            node.span,
        )),
        ExpressionKind::AliasExpanded(id, subst) => build_expression(language, build_ctx, subst)
            .map_err(|e| e.within_alias_expansion(*id, node.span)),
    }
}

/// Builds template evaluation tree from AST nodes, with fresh build context.
pub fn build<'a, L: TemplateLanguage<'a>>(
    language: &L,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn Template<L::Context> + 'a>> {
    let build_ctx = BuildContext {
        local_variables: HashMap::new(),
    };
    expect_template_expression(language, &build_ctx, node)
}

pub fn expect_boolean_expression<'a, L: TemplateLanguage<'a>>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn TemplateProperty<L::Context, Output = bool> + 'a>> {
    build_expression(language, build_ctx, node)?
        .try_into_boolean()
        .ok_or_else(|| TemplateParseError::expected_type("Boolean", node.span))
}

pub fn expect_integer_expression<'a, L: TemplateLanguage<'a>>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn TemplateProperty<L::Context, Output = i64> + 'a>> {
    build_expression(language, build_ctx, node)?
        .try_into_integer()
        .ok_or_else(|| TemplateParseError::expected_type("Integer", node.span))
}

pub fn expect_plain_text_expression<'a, L: TemplateLanguage<'a>>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn TemplateProperty<L::Context, Output = String> + 'a>> {
    // Since any formattable type can be converted to a string property,
    // the expected type is not a String, but a Template.
    build_expression(language, build_ctx, node)?
        .try_into_plain_text()
        .ok_or_else(|| TemplateParseError::expected_type("Template", node.span))
}

pub fn expect_template_expression<'a, L: TemplateLanguage<'a>>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn Template<L::Context> + 'a>> {
    build_expression(language, build_ctx, node)?
        .try_into_template()
        .ok_or_else(|| TemplateParseError::expected_type("Template", node.span))
}

#[cfg(test)]
mod tests {
    use std::iter;

    use jj_lib::backend::MillisSinceEpoch;

    use super::*;
    use crate::formatter::{self, ColorFormatter};
    use crate::template_parser::TemplateAliasesMap;

    /// Minimal template language for testing.
    #[derive(Clone, Default)]
    struct TestTemplateLanguage {
        keywords: HashMap<&'static str, TestTemplateKeywordFn>,
    }

    impl TemplateLanguage<'static> for TestTemplateLanguage {
        type Context = ();
        type Property = CoreTemplatePropertyKind<'static, ()>;

        impl_core_wrap_property_fns!('static);

        fn build_keyword(
            &self,
            name: &str,
            span: pest::Span,
        ) -> TemplateParseResult<Self::Property> {
            self.keywords
                .get(name)
                .map(|f| f(self))
                .ok_or_else(|| TemplateParseError::no_such_keyword(name, span))
        }

        fn build_method(
            &self,
            build_ctx: &BuildContext<Self::Property>,
            property: Self::Property,
            function: &FunctionCallNode,
        ) -> TemplateParseResult<Self::Property> {
            build_core_method(self, build_ctx, property, function)
        }
    }

    type TestTemplateKeywordFn = fn(&TestTemplateLanguage) -> CoreTemplatePropertyKind<'static, ()>;

    /// Helper to set up template evaluation environment.
    #[derive(Clone, Default)]
    struct TestTemplateEnv {
        language: TestTemplateLanguage,
        aliases_map: TemplateAliasesMap,
        color_rules: Vec<(Vec<String>, formatter::Style)>,
    }

    impl TestTemplateEnv {
        fn add_keyword(&mut self, name: &'static str, f: TestTemplateKeywordFn) {
            self.language.keywords.insert(name, f);
        }

        fn add_alias(&mut self, decl: impl AsRef<str>, defn: impl Into<String>) {
            self.aliases_map.insert(decl, defn).unwrap();
        }

        fn add_color(&mut self, label: &str, fg_color: crossterm::style::Color) {
            let labels = label.split_whitespace().map(|s| s.to_owned()).collect();
            let style = formatter::Style {
                fg_color: Some(fg_color),
                ..Default::default()
            };
            self.color_rules.push((labels, style));
        }

        fn parse(&self, template: &str) -> TemplateParseResult<Box<dyn Template<()>>> {
            let node = template_parser::parse(template, &self.aliases_map)?;
            build(&self.language, &node)
        }

        fn parse_err(&self, template: &str) -> String {
            let err = self.parse(template).err().unwrap();
            iter::successors(Some(&err), |e| e.origin()).join("\n")
        }

        fn render_ok(&self, template: &str) -> String {
            let template = self.parse(template).unwrap();
            let mut output = Vec::new();
            let mut formatter = ColorFormatter::new(&mut output, self.color_rules.clone().into());
            template.format(&(), &mut formatter).unwrap();
            String::from_utf8(output).unwrap()
        }
    }

    fn new_timestamp(msec: i64, tz_offset: i32) -> Timestamp {
        Timestamp {
            timestamp: MillisSinceEpoch(msec),
            tz_offset,
        }
    }

    #[test]
    fn test_timestamp_method() {
        let mut env = TestTemplateEnv::default();
        env.add_keyword("t0", |language| {
            language.wrap_timestamp(Literal(new_timestamp(0, 0)))
        });

        insta::assert_snapshot!(
            env.render_ok(r#"t0.format("%Y%m%d %H:%M:%S")"#),
            @"19700101 00:00:00");

        // Invalid format string
        insta::assert_snapshot!(env.parse_err(r#"t0.format("%_")"#), @r###"
         --> 1:11
          |
        1 | t0.format("%_")
          |           ^--^
          |
          = Invalid time format
        "###);

        // Invalid type
        insta::assert_snapshot!(env.parse_err(r#"t0.format(0)"#), @r###"
         --> 1:11
          |
        1 | t0.format(0)
          |           ^
          |
          = Expected string literal
        "###);

        // Dynamic string isn't supported yet
        insta::assert_snapshot!(env.parse_err(r#"t0.format("%Y" ++ "%m")"#), @r###"
         --> 1:11
          |
        1 | t0.format("%Y" ++ "%m")
          |           ^----------^
          |
          = Expected string literal
        "###);

        // Literal alias expansion
        env.add_alias("time_format", r#""%Y-%m-%d""#);
        env.add_alias("bad_time_format", r#""%_""#);
        insta::assert_snapshot!(env.render_ok(r#"t0.format(time_format)"#), @"1970-01-01");
        insta::assert_snapshot!(env.parse_err(r#"t0.format(bad_time_format)"#), @r###"
         --> 1:11
          |
        1 | t0.format(bad_time_format)
          |           ^-------------^
          |
          = Alias "bad_time_format" cannot be expanded
         --> 1:1
          |
        1 | "%_"
          | ^--^
          |
          = Invalid time format
        "###);
    }

    #[test]
    fn test_label_function() {
        let mut env = TestTemplateEnv::default();
        env.add_keyword("empty", |language| language.wrap_boolean(Literal(true)));
        env.add_color("error", crossterm::style::Color::DarkRed);
        env.add_color("warning", crossterm::style::Color::DarkYellow);

        // Literal
        insta::assert_snapshot!(
            env.render_ok(r#"label("error", "text")"#),
            @"[38;5;1mtext[39m");

        // Evaluated property
        insta::assert_snapshot!(
            env.render_ok(r#"label("error".first_line(), "text")"#),
            @"[38;5;1mtext[39m");

        // Template
        insta::assert_snapshot!(
            env.render_ok(r#"label(if(empty, "error", "warning"), "text")"#),
            @"[38;5;1mtext[39m");
    }
}
