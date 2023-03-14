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

use itertools::Itertools as _;
use jujutsu_lib::backend::{Signature, Timestamp};

use crate::template_parser::{
    self, ExpressionKind, ExpressionNode, FunctionCallNode, MethodCallNode, TemplateParseError,
    TemplateParseErrorKind, TemplateParseResult,
};
use crate::templater::{
    ConcatTemplate, ConditionalTemplate, FormattablePropertyListTemplate, IntoTemplate,
    LabelTemplate, Literal, PlainTextFormattedProperty, ReformatTemplate, SeparateTemplate,
    Template, TemplateFunction, TemplateProperty, TimestampRange,
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
    fn wrap_template(&self, template: impl Template<Self::Context> + 'a) -> Self::Property;

    fn build_keyword(&self, name: &str, span: pest::Span) -> TemplateParseResult<Self::Property>;
    fn build_method(
        &self,
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
                wrap_signature(jujutsu_lib::backend::Signature) => Signature,
                wrap_timestamp(jujutsu_lib::backend::Timestamp) => Timestamp,
                wrap_timestamp_range($crate::templater::TimestampRange) => TimestampRange,
            }
        );
        fn wrap_template(
            &self,
            template: impl $crate::templater::Template<Self::Context> + $a,
        ) -> Self::Property {
            use $crate::template_builder::CoreTemplatePropertyKind as Kind;
            $outer(Kind::Template(Box::new(template)))
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
pub trait IntoTemplateProperty<'a, C>: IntoTemplate<'a, C> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<C, Output = bool> + 'a>>;
    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<C, Output = i64> + 'a>>;

    fn into_plain_text(self) -> Box<dyn TemplateProperty<C, Output = String> + 'a>;
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

    fn into_plain_text(self) -> Box<dyn TemplateProperty<I, Output = String> + 'a> {
        match self {
            CoreTemplatePropertyKind::String(property) => property,
            _ => Box::new(PlainTextFormattedProperty::new(self.into_template())),
        }
    }
}

impl<'a, I: 'a> IntoTemplate<'a, I> for CoreTemplatePropertyKind<'a, I> {
    fn into_template(self) -> Box<dyn Template<I> + 'a> {
        match self {
            CoreTemplatePropertyKind::String(property) => property.into_template(),
            CoreTemplatePropertyKind::StringList(property) => property.into_template(),
            CoreTemplatePropertyKind::Boolean(property) => property.into_template(),
            CoreTemplatePropertyKind::Integer(property) => property.into_template(),
            CoreTemplatePropertyKind::Signature(property) => property.into_template(),
            CoreTemplatePropertyKind::Timestamp(property) => property.into_template(),
            CoreTemplatePropertyKind::TimestampRange(property) => property.into_template(),
            CoreTemplatePropertyKind::Template(template) => template,
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

    pub fn into_plain_text<'a, C: 'a>(self) -> Box<dyn TemplateProperty<C, Output = String> + 'a>
    where
        P: IntoTemplateProperty<'a, C>,
    {
        self.property.into_plain_text()
    }
}

impl<'a, C: 'a, P: IntoTemplate<'a, C>> IntoTemplate<'a, C> for Expression<P> {
    fn into_template(self) -> Box<dyn Template<C> + 'a> {
        let template = self.property.into_template();
        if self.labels.is_empty() {
            template
        } else {
            Box::new(LabelTemplate::new(template, Literal(self.labels)))
        }
    }
}

fn build_method_call<'a, L: TemplateLanguage<'a>>(
    language: &L,
    method: &MethodCallNode,
) -> TemplateParseResult<Expression<L::Property>> {
    let mut expression = build_expression(language, &method.object)?;
    expression.property = language.build_method(expression.property, &method.function)?;
    expression.labels.push(method.function.name.to_owned());
    Ok(expression)
}

pub fn build_core_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    property: CoreTemplatePropertyKind<'a, L::Context>,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    match property {
        CoreTemplatePropertyKind::String(property) => {
            build_string_method(language, property, function)
        }
        CoreTemplatePropertyKind::StringList(property) => {
            build_list_method(language, property, function)
        }
        CoreTemplatePropertyKind::Boolean(property) => {
            build_boolean_method(language, property, function)
        }
        CoreTemplatePropertyKind::Integer(property) => {
            build_integer_method(language, property, function)
        }
        CoreTemplatePropertyKind::Signature(property) => {
            build_signature_method(language, property, function)
        }
        CoreTemplatePropertyKind::Timestamp(property) => {
            build_timestamp_method(language, property, function)
        }
        CoreTemplatePropertyKind::TimestampRange(property) => {
            build_timestamp_range_method(language, property, function)
        }
        CoreTemplatePropertyKind::Template(_) => {
            Err(TemplateParseError::no_such_method("Template", function))
        }
    }
}

fn build_string_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
    self_property: impl TemplateProperty<L::Context, Output = String> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "contains" => {
            let [needle_node] = template_parser::expect_exact_arguments(function)?;
            // TODO: or .try_into_string() to disable implicit type cast?
            let needle_property = build_expression(language, needle_node)?.into_plain_text();
            language.wrap_boolean(TemplateFunction::new(
                (self_property, needle_property),
                |(haystack, needle)| haystack.contains(&needle),
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
    _self_property: impl TemplateProperty<L::Context, Output = bool> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    Err(TemplateParseError::no_such_method("Boolean", function))
}

fn build_integer_method<'a, L: TemplateLanguage<'a>>(
    _language: &L,
    _self_property: impl TemplateProperty<L::Context, Output = i64> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    Err(TemplateParseError::no_such_method("Integer", function))
}

fn build_signature_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
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
                        let kind = TemplateParseErrorKind::InvalidTimeFormat;
                        TemplateParseError::with_span(kind, span)
                    })
                })?
                .into_owned();
            language.wrap_string(TemplateFunction::new(self_property, move |timestamp| {
                time_util::format_absolute_timestamp_with(&timestamp, &format)
            }))
        }
        _ => return Err(TemplateParseError::no_such_method("Timestamp", function)),
    };
    Ok(property)
}

fn build_timestamp_range_method<'a, L: TemplateLanguage<'a>>(
    language: &L,
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

pub fn build_list_method<'a, L: TemplateLanguage<'a>, P: Template<()> + 'a>(
    language: &L,
    self_property: impl TemplateProperty<L::Context, Output = Vec<P>> + 'a,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "join" => {
            let [separator_node] = template_parser::expect_exact_arguments(function)?;
            let separator = build_expression(language, separator_node)?.into_template();
            let template = FormattablePropertyListTemplate::new(self_property, separator);
            language.wrap_template(template)
        }
        // TODO: .map()
        _ => return Err(TemplateParseError::no_such_method("List", function)),
    };
    Ok(property)
}

fn build_global_function<'a, L: TemplateLanguage<'a>>(
    language: &L,
    function: &FunctionCallNode,
) -> TemplateParseResult<Expression<L::Property>> {
    let property = match function.name {
        "fill" => {
            let [width_node, content_node] = template_parser::expect_exact_arguments(function)?;
            let width = expect_integer_expression(language, width_node)?;
            let content = build_expression(language, content_node)?.into_template();
            let template = ReformatTemplate::new(content, move |context, formatter, recorded| {
                let width = width.extract(context).try_into().unwrap_or(0);
                text_util::write_wrapped(formatter, recorded, width)
            });
            language.wrap_template(template)
        }
        "indent" => {
            let [prefix_node, content_node] = template_parser::expect_exact_arguments(function)?;
            let prefix = build_expression(language, prefix_node)?.into_template();
            let content = build_expression(language, content_node)?.into_template();
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
            language.wrap_template(template)
        }
        "label" => {
            let [label_node, content_node] = template_parser::expect_exact_arguments(function)?;
            let label_property = build_expression(language, label_node)?.into_plain_text();
            let content = build_expression(language, content_node)?.into_template();
            let labels = TemplateFunction::new(label_property, |s| {
                s.split_whitespace().map(ToString::to_string).collect()
            });
            language.wrap_template(LabelTemplate::new(content, labels))
        }
        "if" => {
            let ([condition_node, true_node], [false_node]) =
                template_parser::expect_arguments(function)?;
            let condition = expect_boolean_expression(language, condition_node)?;
            let true_template = build_expression(language, true_node)?.into_template();
            let false_template = false_node
                .map(|node| build_expression(language, node))
                .transpose()?
                .map(|x| x.into_template());
            let template = ConditionalTemplate::new(condition, true_template, false_template);
            language.wrap_template(template)
        }
        "concat" => {
            let contents = function
                .args
                .iter()
                .map(|node| build_expression(language, node).map(|x| x.into_template()))
                .try_collect()?;
            language.wrap_template(ConcatTemplate(contents))
        }
        "separate" => {
            let ([separator_node], content_nodes) =
                template_parser::expect_some_arguments(function)?;
            let separator = build_expression(language, separator_node)?.into_template();
            let contents = content_nodes
                .iter()
                .map(|node| build_expression(language, node).map(|x| x.into_template()))
                .try_collect()?;
            language.wrap_template(SeparateTemplate::new(separator, contents))
        }
        _ => return Err(TemplateParseError::no_such_function(function)),
    };
    Ok(Expression::unlabeled(property))
}

/// Builds template evaluation tree from AST nodes.
pub fn build_expression<'a, L: TemplateLanguage<'a>>(
    language: &L,
    node: &ExpressionNode,
) -> TemplateParseResult<Expression<L::Property>> {
    match &node.kind {
        ExpressionKind::Identifier(name) => {
            let property = language.build_keyword(name, node.span)?;
            Ok(Expression::with_label(property, *name))
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
                .map(|node| build_expression(language, node).map(|x| x.into_template()))
                .try_collect()?;
            let property = language.wrap_template(ConcatTemplate(templates));
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::FunctionCall(function) => build_global_function(language, function),
        ExpressionKind::MethodCall(method) => build_method_call(language, method),
        ExpressionKind::AliasExpanded(id, subst) => {
            build_expression(language, subst).map_err(|e| e.within_alias_expansion(*id, node.span))
        }
    }
}

pub fn expect_boolean_expression<'a, L: TemplateLanguage<'a>>(
    language: &L,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn TemplateProperty<L::Context, Output = bool> + 'a>> {
    build_expression(language, node)?
        .try_into_boolean()
        .ok_or_else(|| TemplateParseError::invalid_argument_type("Boolean", node.span))
}

pub fn expect_integer_expression<'a, L: TemplateLanguage<'a>>(
    language: &L,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn TemplateProperty<L::Context, Output = i64> + 'a>> {
    build_expression(language, node)?
        .try_into_integer()
        .ok_or_else(|| TemplateParseError::invalid_argument_type("Integer", node.span))
}
