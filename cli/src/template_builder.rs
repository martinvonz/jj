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
    self, BinaryOp, ExpressionKind, ExpressionNode, FunctionCallNode, TemplateAliasesMap,
    TemplateParseError, TemplateParseErrorKind, TemplateParseResult, UnaryOp,
};
use crate::templater::{
    CoalesceTemplate, ConcatTemplate, ConditionalTemplate, IntoTemplate, LabelTemplate,
    ListPropertyTemplate, ListTemplate, Literal, PlainTextFormattedProperty, PropertyPlaceholder,
    ReformatTemplate, SeparateTemplate, Template, TemplateProperty, TemplatePropertyError,
    TemplatePropertyExt as _, TemplateRenderer, TimestampRange,
};
use crate::{text_util, time_util};

/// Callbacks to build language-specific evaluation objects from AST nodes.
pub trait TemplateLanguage<'a> {
    type Property: IntoTemplateProperty<'a>;

    fn wrap_string(property: impl TemplateProperty<Output = String> + 'a) -> Self::Property;
    fn wrap_string_list(
        property: impl TemplateProperty<Output = Vec<String>> + 'a,
    ) -> Self::Property;
    fn wrap_boolean(property: impl TemplateProperty<Output = bool> + 'a) -> Self::Property;
    fn wrap_integer(property: impl TemplateProperty<Output = i64> + 'a) -> Self::Property;
    fn wrap_signature(property: impl TemplateProperty<Output = Signature> + 'a) -> Self::Property;
    fn wrap_timestamp(property: impl TemplateProperty<Output = Timestamp> + 'a) -> Self::Property;
    fn wrap_timestamp_range(
        property: impl TemplateProperty<Output = TimestampRange> + 'a,
    ) -> Self::Property;

    fn wrap_template(template: Box<dyn Template + 'a>) -> Self::Property;
    fn wrap_list_template(template: Box<dyn ListTemplate + 'a>) -> Self::Property;

    /// Translates the given global `function` call to a property.
    ///
    /// This should be delegated to
    /// `CoreTemplateBuildFnTable::build_function()`.
    fn build_function(
        &self,
        build_ctx: &BuildContext<Self::Property>,
        function: &FunctionCallNode,
    ) -> TemplateParseResult<Self::Property>;

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
            template: Box<dyn $crate::templater::Template + $a>,
        ) -> Self::Property {
            use $crate::template_builder::CoreTemplatePropertyKind as Kind;
            $outer(Kind::Template(template))
        }
        fn wrap_list_template(
            template: Box<dyn $crate::templater::ListTemplate + $a>,
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
                property: impl $crate::templater::TemplateProperty<Output = $ty> + $a,
            ) -> Self::Property {
                use $kind as Kind; // https://github.com/rust-lang/rust/issues/48067
                $outer(Kind::$var(Box::new(property)))
            }
        )+
    };
}

pub(crate) use {impl_core_wrap_property_fns, impl_wrap_property_fns};

/// Provides access to basic template property types.
pub trait IntoTemplateProperty<'a> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<Output = bool> + 'a>>;
    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<Output = i64> + 'a>>;

    fn try_into_plain_text(self) -> Option<Box<dyn TemplateProperty<Output = String> + 'a>>;
    fn try_into_template(self) -> Option<Box<dyn Template + 'a>>;
}

pub enum CoreTemplatePropertyKind<'a> {
    String(Box<dyn TemplateProperty<Output = String> + 'a>),
    StringList(Box<dyn TemplateProperty<Output = Vec<String>> + 'a>),
    Boolean(Box<dyn TemplateProperty<Output = bool> + 'a>),
    Integer(Box<dyn TemplateProperty<Output = i64> + 'a>),
    Signature(Box<dyn TemplateProperty<Output = Signature> + 'a>),
    Timestamp(Box<dyn TemplateProperty<Output = Timestamp> + 'a>),
    TimestampRange(Box<dyn TemplateProperty<Output = TimestampRange> + 'a>),

    // Both TemplateProperty and Template can represent a value to be evaluated
    // dynamically, which suggests that `Box<dyn Template + 'a>` could be
    // composed as `Box<dyn TemplateProperty<Output = Box<dyn Template ..`.
    // However, there's a subtle difference: TemplateProperty is strict on
    // error, whereas Template is usually lax and prints an error inline. If
    // `concat(x, y)` were a property returning Template, and if `y` failed to
    // evaluate, the whole expression would fail. In this example, a partial
    // evaluation output is more useful. That's one reason why Template isn't
    // wrapped in a TemplateProperty. Another reason is that the outermost
    // caller expects a Template, not a TemplateProperty of Template output.
    Template(Box<dyn Template + 'a>),
    ListTemplate(Box<dyn ListTemplate + 'a>),
}

impl<'a> IntoTemplateProperty<'a> for CoreTemplatePropertyKind<'a> {
    fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<Output = bool> + 'a>> {
        match self {
            CoreTemplatePropertyKind::String(property) => {
                Some(Box::new(property.map(|s| !s.is_empty())))
            }
            CoreTemplatePropertyKind::StringList(property) => {
                Some(Box::new(property.map(|l| !l.is_empty())))
            }
            CoreTemplatePropertyKind::Boolean(property) => Some(property),
            CoreTemplatePropertyKind::Integer(_) => None,
            CoreTemplatePropertyKind::Signature(_) => None,
            CoreTemplatePropertyKind::Timestamp(_) => None,
            CoreTemplatePropertyKind::TimestampRange(_) => None,
            // Template types could also be evaluated to boolean, but it's less likely
            // to apply label() or .map() and use the result as conditional. It's also
            // unclear whether ListTemplate should behave as a "list" or a "template".
            CoreTemplatePropertyKind::Template(_) => None,
            CoreTemplatePropertyKind::ListTemplate(_) => None,
        }
    }

    fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<Output = i64> + 'a>> {
        match self {
            CoreTemplatePropertyKind::Integer(property) => Some(property),
            _ => None,
        }
    }

    fn try_into_plain_text(self) -> Option<Box<dyn TemplateProperty<Output = String> + 'a>> {
        match self {
            CoreTemplatePropertyKind::String(property) => Some(property),
            _ => {
                let template = self.try_into_template()?;
                Some(Box::new(PlainTextFormattedProperty::new(template)))
            }
        }
    }

    fn try_into_template(self) -> Option<Box<dyn Template + 'a>> {
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

/// Function that translates global function call node.
// The lifetime parameter 'a could be replaced with for<'a> to keep the method
// table away from a certain lifetime. That's technically more correct, but I
// couldn't find an easy way to expand that to the core template methods, which
// are defined for L: TemplateLanguage<'a>. That's why the build fn table is
// bound to a named lifetime, and therefore can't be cached statically.
pub type TemplateBuildFunctionFn<'a, L> =
    fn(
        &L,
        &BuildContext<<L as TemplateLanguage<'a>>::Property>,
        &FunctionCallNode,
    ) -> TemplateParseResult<<L as TemplateLanguage<'a>>::Property>;

/// Function that translates method call node of self type `T`.
pub type TemplateBuildMethodFn<'a, L, T> =
    fn(
        &L,
        &BuildContext<<L as TemplateLanguage<'a>>::Property>,
        Box<dyn TemplateProperty<Output = T> + 'a>,
        &FunctionCallNode,
    ) -> TemplateParseResult<<L as TemplateLanguage<'a>>::Property>;

/// Table of functions that translate global function call node.
pub type TemplateBuildFunctionFnMap<'a, L> = HashMap<&'static str, TemplateBuildFunctionFn<'a, L>>;

/// Table of functions that translate method call node of self type `T`.
pub type TemplateBuildMethodFnMap<'a, L, T> =
    HashMap<&'static str, TemplateBuildMethodFn<'a, L, T>>;

/// Symbol table of functions and methods available in the core template.
pub struct CoreTemplateBuildFnTable<'a, L: TemplateLanguage<'a> + ?Sized> {
    pub functions: TemplateBuildFunctionFnMap<'a, L>,
    pub string_methods: TemplateBuildMethodFnMap<'a, L, String>,
    pub boolean_methods: TemplateBuildMethodFnMap<'a, L, bool>,
    pub integer_methods: TemplateBuildMethodFnMap<'a, L, i64>,
    pub signature_methods: TemplateBuildMethodFnMap<'a, L, Signature>,
    pub timestamp_methods: TemplateBuildMethodFnMap<'a, L, Timestamp>,
    pub timestamp_range_methods: TemplateBuildMethodFnMap<'a, L, TimestampRange>,
}

pub fn merge_fn_map<'s, F>(base: &mut HashMap<&'s str, F>, extension: HashMap<&'s str, F>) {
    for (name, function) in extension {
        if base.insert(name, function).is_some() {
            panic!("Conflicting template definitions for '{name}' function");
        }
    }
}

impl<'a, L: TemplateLanguage<'a> + ?Sized> CoreTemplateBuildFnTable<'a, L> {
    /// Creates new symbol table containing the builtin functions and methods.
    pub fn builtin() -> Self {
        CoreTemplateBuildFnTable {
            functions: builtin_functions(),
            string_methods: builtin_string_methods(),
            boolean_methods: HashMap::new(),
            integer_methods: HashMap::new(),
            signature_methods: builtin_signature_methods(),
            timestamp_methods: builtin_timestamp_methods(),
            timestamp_range_methods: builtin_timestamp_range_methods(),
        }
    }

    pub fn empty() -> Self {
        CoreTemplateBuildFnTable {
            functions: HashMap::new(),
            string_methods: HashMap::new(),
            boolean_methods: HashMap::new(),
            integer_methods: HashMap::new(),
            signature_methods: HashMap::new(),
            timestamp_methods: HashMap::new(),
            timestamp_range_methods: HashMap::new(),
        }
    }

    pub fn merge(&mut self, extension: CoreTemplateBuildFnTable<'a, L>) {
        let CoreTemplateBuildFnTable {
            functions,
            string_methods,
            boolean_methods,
            integer_methods,
            signature_methods,
            timestamp_methods,
            timestamp_range_methods,
        } = extension;

        merge_fn_map(&mut self.functions, functions);
        merge_fn_map(&mut self.string_methods, string_methods);
        merge_fn_map(&mut self.boolean_methods, boolean_methods);
        merge_fn_map(&mut self.integer_methods, integer_methods);
        merge_fn_map(&mut self.signature_methods, signature_methods);
        merge_fn_map(&mut self.timestamp_methods, timestamp_methods);
        merge_fn_map(&mut self.timestamp_range_methods, timestamp_range_methods);
    }

    /// Translates the function call node `function` by using this symbol table.
    pub fn build_function(
        &self,
        language: &L,
        build_ctx: &BuildContext<L::Property>,
        function: &FunctionCallNode,
    ) -> TemplateParseResult<L::Property> {
        let table = &self.functions;
        let build = template_parser::lookup_function(table, function)?;
        build(language, build_ctx, function)
    }

    /// Applies the method call node `function` to the given `property` by using
    /// this symbol table.
    pub fn build_method(
        &self,
        language: &L,
        build_ctx: &BuildContext<L::Property>,
        property: CoreTemplatePropertyKind<'a>,
        function: &FunctionCallNode,
    ) -> TemplateParseResult<L::Property> {
        match property {
            CoreTemplatePropertyKind::String(property) => {
                let table = &self.string_methods;
                let build = template_parser::lookup_method("String", table, function)?;
                build(language, build_ctx, property, function)
            }
            CoreTemplatePropertyKind::StringList(property) => {
                // TODO: migrate to table?
                build_formattable_list_method(language, build_ctx, property, function, |item| {
                    L::wrap_string(item)
                })
            }
            CoreTemplatePropertyKind::Boolean(property) => {
                let table = &self.boolean_methods;
                let build = template_parser::lookup_method("Boolean", table, function)?;
                build(language, build_ctx, property, function)
            }
            CoreTemplatePropertyKind::Integer(property) => {
                let table = &self.integer_methods;
                let build = template_parser::lookup_method("Integer", table, function)?;
                build(language, build_ctx, property, function)
            }
            CoreTemplatePropertyKind::Signature(property) => {
                let table = &self.signature_methods;
                let build = template_parser::lookup_method("Signature", table, function)?;
                build(language, build_ctx, property, function)
            }
            CoreTemplatePropertyKind::Timestamp(property) => {
                let table = &self.timestamp_methods;
                let build = template_parser::lookup_method("Timestamp", table, function)?;
                build(language, build_ctx, property, function)
            }
            CoreTemplatePropertyKind::TimestampRange(property) => {
                let table = &self.timestamp_range_methods;
                let build = template_parser::lookup_method("TimestampRange", table, function)?;
                build(language, build_ctx, property, function)
            }
            CoreTemplatePropertyKind::Template(_) => {
                // TODO: migrate to table?
                Err(TemplateParseError::no_such_method("Template", function))
            }
            CoreTemplatePropertyKind::ListTemplate(template) => {
                // TODO: migrate to table?
                build_list_template_method(language, build_ctx, template, function)
            }
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
}

impl<'a, P: IntoTemplateProperty<'a>> Expression<P> {
    pub fn try_into_boolean(self) -> Option<Box<dyn TemplateProperty<Output = bool> + 'a>> {
        self.property.try_into_boolean()
    }

    pub fn try_into_integer(self) -> Option<Box<dyn TemplateProperty<Output = i64> + 'a>> {
        self.property.try_into_integer()
    }

    pub fn try_into_plain_text(self) -> Option<Box<dyn TemplateProperty<Output = String> + 'a>> {
        self.property.try_into_plain_text()
    }

    pub fn try_into_template(self) -> Option<Box<dyn Template + 'a>> {
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
    /// Function to create `L::Property` representing `self`.
    ///
    /// This could be `local_variables["self"]`, but keyword lookup shouldn't be
    /// overridden by a user-defined `self` variable.
    self_variable: &'i (dyn Fn() -> P),
}

fn build_keyword<'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    name: &str,
    name_span: pest::Span<'_>,
) -> TemplateParseResult<L::Property> {
    // Keyword is a 0-ary method on the "self" property
    let self_property = (build_ctx.self_variable)();
    let function = FunctionCallNode {
        name,
        name_span,
        args: vec![],
        args_span: name_span.end_pos().span(&name_span.end_pos()),
    };
    language
        .build_method(build_ctx, self_property, &function)
        .map_err(|err| match err.kind() {
            TemplateParseErrorKind::NoSuchMethod { candidates, .. } => {
                let kind = TemplateParseErrorKind::NoSuchKeyword {
                    name: name.to_owned(),
                    // TODO: filter methods by arity?
                    candidates: candidates.clone(),
                };
                TemplateParseError::with_span(kind, name_span)
            }
            // Since keyword is a 0-ary method, any argument errors mean there's
            // no such keyword.
            TemplateParseErrorKind::InvalidArguments { .. } => {
                let kind = TemplateParseErrorKind::NoSuchKeyword {
                    name: name.to_owned(),
                    // TODO: might be better to phrase the error differently
                    candidates: vec![format!("self.{name}(..)")],
                };
                TemplateParseError::with_span(kind, name_span)
            }
            // The keyword function may fail with the other reasons.
            _ => err,
        })
}

fn build_unary_operation<'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    op: UnaryOp,
    arg_node: &ExpressionNode,
) -> TemplateParseResult<L::Property> {
    match op {
        UnaryOp::LogicalNot => {
            let arg = expect_boolean_expression(language, build_ctx, arg_node)?;
            Ok(L::wrap_boolean(arg.map(|v| !v)))
        }
        UnaryOp::Negate => {
            let arg = expect_integer_expression(language, build_ctx, arg_node)?;
            Ok(L::wrap_integer(arg.and_then(|v| {
                v.checked_neg()
                    .ok_or_else(|| TemplatePropertyError("Attempt to negate with overflow".into()))
            })))
        }
    }
}

fn build_binary_operation<'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    op: BinaryOp,
    lhs_node: &ExpressionNode,
    rhs_node: &ExpressionNode,
) -> TemplateParseResult<L::Property> {
    match op {
        BinaryOp::LogicalOr => {
            let lhs = expect_boolean_expression(language, build_ctx, lhs_node)?;
            let rhs = expect_boolean_expression(language, build_ctx, rhs_node)?;
            let out = lhs.and_then(move |l| Ok(l || rhs.extract()?));
            Ok(L::wrap_boolean(out))
        }
        BinaryOp::LogicalAnd => {
            let lhs = expect_boolean_expression(language, build_ctx, lhs_node)?;
            let rhs = expect_boolean_expression(language, build_ctx, rhs_node)?;
            let out = lhs.and_then(move |l| Ok(l && rhs.extract()?));
            Ok(L::wrap_boolean(out))
        }
    }
}

fn builtin_string_methods<'a, L: TemplateLanguage<'a> + ?Sized>(
) -> TemplateBuildMethodFnMap<'a, L, String> {
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map = TemplateBuildMethodFnMap::<L, String>::new();
    map.insert("len", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.and_then(|s| Ok(s.len().try_into()?));
        Ok(L::wrap_integer(out_property))
    });
    map.insert(
        "contains",
        |language, build_ctx, self_property, function| {
            let [needle_node] = template_parser::expect_exact_arguments(function)?;
            // TODO: or .try_into_string() to disable implicit type cast?
            let needle_property = expect_plain_text_expression(language, build_ctx, needle_node)?;
            let out_property = (self_property, needle_property)
                .map(|(haystack, needle)| haystack.contains(&needle));
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert(
        "starts_with",
        |language, build_ctx, self_property, function| {
            let [needle_node] = template_parser::expect_exact_arguments(function)?;
            let needle_property = expect_plain_text_expression(language, build_ctx, needle_node)?;
            let out_property = (self_property, needle_property)
                .map(|(haystack, needle)| haystack.starts_with(&needle));
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert(
        "ends_with",
        |language, build_ctx, self_property, function| {
            let [needle_node] = template_parser::expect_exact_arguments(function)?;
            let needle_property = expect_plain_text_expression(language, build_ctx, needle_node)?;
            let out_property = (self_property, needle_property)
                .map(|(haystack, needle)| haystack.ends_with(&needle));
            Ok(L::wrap_boolean(out_property))
        },
    );
    map.insert(
        "remove_prefix",
        |language, build_ctx, self_property, function| {
            let [needle_node] = template_parser::expect_exact_arguments(function)?;
            let needle_property = expect_plain_text_expression(language, build_ctx, needle_node)?;
            let out_property = (self_property, needle_property).map(|(haystack, needle)| {
                haystack
                    .strip_prefix(&needle)
                    .map(ToOwned::to_owned)
                    .unwrap_or(haystack)
            });
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert(
        "remove_suffix",
        |language, build_ctx, self_property, function| {
            let [needle_node] = template_parser::expect_exact_arguments(function)?;
            let needle_property = expect_plain_text_expression(language, build_ctx, needle_node)?;
            let out_property = (self_property, needle_property).map(|(haystack, needle)| {
                haystack
                    .strip_suffix(&needle)
                    .map(ToOwned::to_owned)
                    .unwrap_or(haystack)
            });
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert("substr", |language, build_ctx, self_property, function| {
        let [start_idx, end_idx] = template_parser::expect_exact_arguments(function)?;
        let start_idx_property = expect_isize_expression(language, build_ctx, start_idx)?;
        let end_idx_property = expect_isize_expression(language, build_ctx, end_idx)?;
        let out_property =
            (self_property, start_idx_property, end_idx_property).map(|(s, start_idx, end_idx)| {
                let start_idx = string_index_to_char_boundary(&s, start_idx);
                let end_idx = string_index_to_char_boundary(&s, end_idx);
                s.get(start_idx..end_idx).unwrap_or_default().to_owned()
            });
        Ok(L::wrap_string(out_property))
    });
    map.insert(
        "first_line",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property =
                self_property.map(|s| s.lines().next().unwrap_or_default().to_string());
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert("lines", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|s| s.lines().map(|l| l.to_owned()).collect());
        Ok(L::wrap_string_list(out_property))
    });
    map.insert("upper", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|s| s.to_uppercase());
        Ok(L::wrap_string(out_property))
    });
    map.insert("lower", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|s| s.to_lowercase());
        Ok(L::wrap_string(out_property))
    });
    map
}

/// Clamps and aligns the given index `i` to char boundary.
///
/// Negative index counts from the end. If the index isn't at a char boundary,
/// it will be rounded towards 0 (left or right depending on the sign.)
fn string_index_to_char_boundary(s: &str, i: isize) -> usize {
    // TODO: use floor/ceil_char_boundary() if get stabilized
    let magnitude = i.unsigned_abs();
    if i < 0 {
        let p = s.len().saturating_sub(magnitude);
        (p..=s.len()).find(|&p| s.is_char_boundary(p)).unwrap()
    } else {
        let p = magnitude.min(s.len());
        (0..=p).rev().find(|&p| s.is_char_boundary(p)).unwrap()
    }
}

fn builtin_signature_methods<'a, L: TemplateLanguage<'a> + ?Sized>(
) -> TemplateBuildMethodFnMap<'a, L, Signature> {
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map = TemplateBuildMethodFnMap::<L, Signature>::new();
    map.insert("name", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|signature| signature.name);
        Ok(L::wrap_string(out_property))
    });
    map.insert("email", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|signature| signature.email);
        Ok(L::wrap_string(out_property))
    });
    map.insert(
        "username",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property = self_property.map(|signature| {
                let (username, _) = text_util::split_email(&signature.email);
                username.to_owned()
            });
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert(
        "timestamp",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property = self_property.map(|signature| signature.timestamp);
            Ok(L::wrap_timestamp(out_property))
        },
    );
    map
}

fn builtin_timestamp_methods<'a, L: TemplateLanguage<'a> + ?Sized>(
) -> TemplateBuildMethodFnMap<'a, L, Timestamp> {
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map = TemplateBuildMethodFnMap::<L, Timestamp>::new();
    map.insert("ago", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let now = Timestamp::now();
        let format = timeago::Formatter::new();
        let out_property = self_property
            .and_then(move |timestamp| Ok(time_util::format_duration(&timestamp, &now, &format)?));
        Ok(L::wrap_string(out_property))
    });
    map.insert(
        "format",
        |_language, _build_ctx, self_property, function| {
            // No dynamic string is allowed as the templater has no runtime error type.
            let [format_node] = template_parser::expect_exact_arguments(function)?;
            let format =
                template_parser::expect_string_literal_with(format_node, |format, span| {
                    time_util::FormattingItems::parse(format)
                        .ok_or_else(|| TemplateParseError::expression("Invalid time format", span))
                })?
                .into_owned();
            let out_property = self_property.and_then(move |timestamp| {
                Ok(time_util::format_absolute_timestamp_with(
                    &timestamp, &format,
                )?)
            });
            Ok(L::wrap_string(out_property))
        },
    );
    map.insert("utc", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|mut timestamp| {
            timestamp.tz_offset = 0;
            timestamp
        });
        Ok(L::wrap_timestamp(out_property))
    });
    map.insert("local", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let tz_offset = std::env::var("JJ_TZ_OFFSET_MINS")
            .ok()
            .and_then(|tz_string| tz_string.parse::<i32>().ok())
            .unwrap_or_else(|| chrono::Local::now().offset().local_minus_utc() / 60);
        let out_property = self_property.map(move |mut timestamp| {
            timestamp.tz_offset = tz_offset;
            timestamp
        });
        Ok(L::wrap_timestamp(out_property))
    });
    map
}

fn builtin_timestamp_range_methods<'a, L: TemplateLanguage<'a> + ?Sized>(
) -> TemplateBuildMethodFnMap<'a, L, TimestampRange> {
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map = TemplateBuildMethodFnMap::<L, TimestampRange>::new();
    map.insert("start", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|time_range| time_range.start);
        Ok(L::wrap_timestamp(out_property))
    });
    map.insert("end", |_language, _build_ctx, self_property, function| {
        template_parser::expect_no_arguments(function)?;
        let out_property = self_property.map(|time_range| time_range.end);
        Ok(L::wrap_timestamp(out_property))
    });
    map.insert(
        "duration",
        |_language, _build_ctx, self_property, function| {
            template_parser::expect_no_arguments(function)?;
            let out_property = self_property.and_then(|time_range| Ok(time_range.duration()?));
            Ok(L::wrap_string(out_property))
        },
    );
    map
}

fn build_list_template_method<'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    self_template: Box<dyn ListTemplate + 'a>,
    function: &FunctionCallNode,
) -> TemplateParseResult<L::Property> {
    let property = match function.name {
        "join" => {
            let [separator_node] = template_parser::expect_exact_arguments(function)?;
            let separator = expect_template_expression(language, build_ctx, separator_node)?;
            L::wrap_template(self_template.join(separator))
        }
        _ => return Err(TemplateParseError::no_such_method("ListTemplate", function)),
    };
    Ok(property)
}

/// Builds method call expression for printable list property.
pub fn build_formattable_list_method<'a, L, O>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    self_property: impl TemplateProperty<Output = Vec<O>> + 'a,
    function: &FunctionCallNode,
    // TODO: Generic L: WrapProperty<O> trait might be needed to support more
    // list operations such as first()/slice(). For .map(), a simple callback works.
    wrap_item: impl Fn(PropertyPlaceholder<O>) -> L::Property,
) -> TemplateParseResult<L::Property>
where
    L: TemplateLanguage<'a> + ?Sized,
    O: Template + Clone + 'a,
{
    let property = match function.name {
        "len" => {
            template_parser::expect_no_arguments(function)?;
            let out_property = self_property.and_then(|items| Ok(items.len().try_into()?));
            L::wrap_integer(out_property)
        }
        "join" => {
            let [separator_node] = template_parser::expect_exact_arguments(function)?;
            let separator = expect_template_expression(language, build_ctx, separator_node)?;
            let template =
                ListPropertyTemplate::new(self_property, separator, |formatter, item| {
                    item.format(formatter)
                });
            L::wrap_template(Box::new(template))
        }
        "map" => build_map_operation(language, build_ctx, self_property, function, wrap_item)?,
        _ => return Err(TemplateParseError::no_such_method("List", function)),
    };
    Ok(property)
}

pub fn build_unformattable_list_method<'a, L, O>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    self_property: impl TemplateProperty<Output = Vec<O>> + 'a,
    function: &FunctionCallNode,
    wrap_item: impl Fn(PropertyPlaceholder<O>) -> L::Property,
) -> TemplateParseResult<L::Property>
where
    L: TemplateLanguage<'a> + ?Sized,
    O: Clone + 'a,
{
    let property = match function.name {
        "len" => {
            template_parser::expect_no_arguments(function)?;
            let out_property = self_property.and_then(|items| Ok(items.len().try_into()?));
            L::wrap_integer(out_property)
        }
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
    L: TemplateLanguage<'a> + ?Sized,
    P: TemplateProperty + 'a,
    P::Output: IntoIterator<Item = O>,
    O: Clone + 'a,
{
    // Build an item template with placeholder property, then evaluate it
    // for each item.
    let [lambda_node] = template_parser::expect_exact_arguments(function)?;
    let item_placeholder = PropertyPlaceholder::new();
    let item_template = template_parser::expect_lambda_with(lambda_node, |lambda, _span| {
        let item_fn = || wrap_item(item_placeholder.clone());
        let mut local_variables = build_ctx.local_variables.clone();
        if let [name] = lambda.params.as_slice() {
            local_variables.insert(name, &item_fn);
        } else {
            return Err(TemplateParseError::expression(
                "Expected 1 lambda parameters",
                lambda.params_span,
            ));
        }
        let inner_build_ctx = BuildContext {
            local_variables,
            self_variable: build_ctx.self_variable,
        };
        expect_template_expression(language, &inner_build_ctx, &lambda.body)
    })?;
    let list_template = ListPropertyTemplate::new(
        self_property,
        Literal(" "), // separator
        move |formatter, item| {
            item_placeholder.with_value(item, || item_template.format(formatter))
        },
    );
    Ok(L::wrap_list_template(Box::new(list_template)))
}

fn builtin_functions<'a, L: TemplateLanguage<'a> + ?Sized>() -> TemplateBuildFunctionFnMap<'a, L> {
    // Not using maplit::hashmap!{} or custom declarative macro here because
    // code completion inside macro is quite restricted.
    let mut map = TemplateBuildFunctionFnMap::<L>::new();
    map.insert("fill", |language, build_ctx, function| {
        let [width_node, content_node] = template_parser::expect_exact_arguments(function)?;
        let width = expect_usize_expression(language, build_ctx, width_node)?;
        let content = expect_template_expression(language, build_ctx, content_node)?;
        let template =
            ReformatTemplate::new(content, move |formatter, recorded| match width.extract() {
                Ok(width) => text_util::write_wrapped(formatter.as_mut(), recorded, width),
                Err(err) => formatter.handle_error(err),
            });
        Ok(L::wrap_template(Box::new(template)))
    });
    map.insert("indent", |language, build_ctx, function| {
        let [prefix_node, content_node] = template_parser::expect_exact_arguments(function)?;
        let prefix = expect_template_expression(language, build_ctx, prefix_node)?;
        let content = expect_template_expression(language, build_ctx, content_node)?;
        let template = ReformatTemplate::new(content, move |formatter, recorded| {
            let rewrap = formatter.rewrap_fn();
            text_util::write_indented(formatter.as_mut(), recorded, |formatter| {
                prefix.format(&mut rewrap(formatter))
            })
        });
        Ok(L::wrap_template(Box::new(template)))
    });
    map.insert("label", |language, build_ctx, function| {
        let [label_node, content_node] = template_parser::expect_exact_arguments(function)?;
        let label_property = expect_plain_text_expression(language, build_ctx, label_node)?;
        let content = expect_template_expression(language, build_ctx, content_node)?;
        let labels =
            label_property.map(|s| s.split_whitespace().map(ToString::to_string).collect());
        Ok(L::wrap_template(Box::new(LabelTemplate::new(
            content, labels,
        ))))
    });
    map.insert("if", |language, build_ctx, function| {
        let ([condition_node, true_node], [false_node]) =
            template_parser::expect_arguments(function)?;
        let condition = expect_boolean_expression(language, build_ctx, condition_node)?;
        let true_template = expect_template_expression(language, build_ctx, true_node)?;
        let false_template = false_node
            .map(|node| expect_template_expression(language, build_ctx, node))
            .transpose()?;
        let template = ConditionalTemplate::new(condition, true_template, false_template);
        Ok(L::wrap_template(Box::new(template)))
    });
    map.insert("coalesce", |language, build_ctx, function| {
        let contents = function
            .args
            .iter()
            .map(|node| expect_template_expression(language, build_ctx, node))
            .try_collect()?;
        Ok(L::wrap_template(Box::new(CoalesceTemplate(contents))))
    });
    map.insert("concat", |language, build_ctx, function| {
        let contents = function
            .args
            .iter()
            .map(|node| expect_template_expression(language, build_ctx, node))
            .try_collect()?;
        Ok(L::wrap_template(Box::new(ConcatTemplate(contents))))
    });
    map.insert("separate", |language, build_ctx, function| {
        let ([separator_node], content_nodes) = template_parser::expect_some_arguments(function)?;
        let separator = expect_template_expression(language, build_ctx, separator_node)?;
        let contents = content_nodes
            .iter()
            .map(|node| expect_template_expression(language, build_ctx, node))
            .try_collect()?;
        Ok(L::wrap_template(Box::new(SeparateTemplate::new(
            separator, contents,
        ))))
    });
    map.insert("surround", |language, build_ctx, function| {
        let [prefix_node, suffix_node, content_node] =
            template_parser::expect_exact_arguments(function)?;
        let prefix = expect_template_expression(language, build_ctx, prefix_node)?;
        let suffix = expect_template_expression(language, build_ctx, suffix_node)?;
        let content = expect_template_expression(language, build_ctx, content_node)?;
        let template = ReformatTemplate::new(content, move |formatter, recorded| {
            if recorded.data().is_empty() {
                return Ok(());
            }
            prefix.format(formatter)?;
            recorded.replay(formatter.as_mut())?;
            suffix.format(formatter)?;
            Ok(())
        });
        Ok(L::wrap_template(Box::new(template)))
    });
    map
}

/// Builds intermediate expression tree from AST nodes.
pub fn build_expression<'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    node: &ExpressionNode,
) -> TemplateParseResult<Expression<L::Property>> {
    match &node.kind {
        ExpressionKind::Identifier(name) => {
            if let Some(make) = build_ctx.local_variables.get(name) {
                // Don't label a local variable with its name
                Ok(Expression::unlabeled(make()))
            } else if *name == "self" {
                // "self" is a special variable, so don't label it
                let make = build_ctx.self_variable;
                Ok(Expression::unlabeled(make()))
            } else {
                let property =
                    build_keyword(language, build_ctx, name, node.span).map_err(|err| {
                        err.extend_keyword_candidates(itertools::chain(
                            build_ctx.local_variables.keys().copied(),
                            ["self"],
                        ))
                    })?;
                Ok(Expression::with_label(property, *name))
            }
        }
        ExpressionKind::Boolean(value) => {
            let property = L::wrap_boolean(Literal(*value));
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::Integer(value) => {
            let property = L::wrap_integer(Literal(*value));
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::String(value) => {
            let property = L::wrap_string(Literal(value.clone()));
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::Unary(op, arg_node) => {
            let property = build_unary_operation(language, build_ctx, *op, arg_node)?;
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::Binary(op, lhs_node, rhs_node) => {
            let property = build_binary_operation(language, build_ctx, *op, lhs_node, rhs_node)?;
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::Concat(nodes) => {
            let templates = nodes
                .iter()
                .map(|node| expect_template_expression(language, build_ctx, node))
                .try_collect()?;
            let property = L::wrap_template(Box::new(ConcatTemplate(templates)));
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::FunctionCall(function) => {
            let property = language.build_function(build_ctx, function)?;
            Ok(Expression::unlabeled(property))
        }
        ExpressionKind::MethodCall(method) => {
            let mut expression = build_expression(language, build_ctx, &method.object)?;
            expression.property =
                language.build_method(build_ctx, expression.property, &method.function)?;
            expression.labels.push(method.function.name.to_owned());
            Ok(expression)
        }
        ExpressionKind::Lambda(_) => Err(TemplateParseError::expression(
            "Lambda cannot be defined here",
            node.span,
        )),
        ExpressionKind::AliasExpanded(id, subst) => build_expression(language, build_ctx, subst)
            .map_err(|e| e.within_alias_expansion(*id, node.span)),
    }
}

/// Builds template evaluation tree from AST nodes, with fresh build context.
///
/// `wrap_self` specifies the type of the top-level property, which should be
/// one of the `L::wrap_*()` functions.
pub fn build<'a, C: Clone + 'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    node: &ExpressionNode,
    // TODO: Generic L: WrapProperty<C> trait might be better. See the
    // comment in build_formattable_list_method().
    wrap_self: impl Fn(PropertyPlaceholder<C>) -> L::Property,
) -> TemplateParseResult<TemplateRenderer<'a, C>> {
    let self_placeholder = PropertyPlaceholder::new();
    let build_ctx = BuildContext {
        local_variables: HashMap::new(),
        self_variable: &|| wrap_self(self_placeholder.clone()),
    };
    let template = expect_template_expression(language, &build_ctx, node)?;
    Ok(TemplateRenderer::new(template, self_placeholder))
}

/// Parses text, expands aliases, then builds template evaluation tree.
pub fn parse<'a, C: Clone + 'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    template_text: &str,
    aliases_map: &TemplateAliasesMap,
    wrap_self: impl Fn(PropertyPlaceholder<C>) -> L::Property,
) -> TemplateParseResult<TemplateRenderer<'a, C>> {
    let node = template_parser::parse(template_text, aliases_map)?;
    build(language, &node, wrap_self).map_err(|err| err.extend_alias_candidates(aliases_map))
}

pub fn expect_boolean_expression<'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn TemplateProperty<Output = bool> + 'a>> {
    build_expression(language, build_ctx, node)?
        .try_into_boolean()
        .ok_or_else(|| TemplateParseError::expected_type("Boolean", node.span))
}

pub fn expect_integer_expression<'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn TemplateProperty<Output = i64> + 'a>> {
    build_expression(language, build_ctx, node)?
        .try_into_integer()
        .ok_or_else(|| TemplateParseError::expected_type("Integer", node.span))
}

/// If the given expression `node` is of `Integer` type, converts it to `isize`.
pub fn expect_isize_expression<'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn TemplateProperty<Output = isize> + 'a>> {
    let i64_property = expect_integer_expression(language, build_ctx, node)?;
    let isize_property = i64_property.and_then(|v| Ok(isize::try_from(v)?));
    Ok(Box::new(isize_property))
}

/// If the given expression `node` is of `Integer` type, converts it to `usize`.
pub fn expect_usize_expression<'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn TemplateProperty<Output = usize> + 'a>> {
    let i64_property = expect_integer_expression(language, build_ctx, node)?;
    let usize_property = i64_property.and_then(|v| Ok(usize::try_from(v)?));
    Ok(Box::new(usize_property))
}

pub fn expect_plain_text_expression<'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn TemplateProperty<Output = String> + 'a>> {
    // Since any formattable type can be converted to a string property,
    // the expected type is not a String, but a Template.
    build_expression(language, build_ctx, node)?
        .try_into_plain_text()
        .ok_or_else(|| TemplateParseError::expected_type("Template", node.span))
}

pub fn expect_template_expression<'a, L: TemplateLanguage<'a> + ?Sized>(
    language: &L,
    build_ctx: &BuildContext<L::Property>,
    node: &ExpressionNode,
) -> TemplateParseResult<Box<dyn Template + 'a>> {
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
    use crate::generic_templater::GenericTemplateLanguage;

    type L = GenericTemplateLanguage<'static, ()>;
    type TestTemplatePropertyKind = <L as TemplateLanguage<'static>>::Property;

    /// Helper to set up template evaluation environment.
    struct TestTemplateEnv {
        language: L,
        aliases_map: TemplateAliasesMap,
        color_rules: Vec<(Vec<String>, formatter::Style)>,
    }

    impl TestTemplateEnv {
        fn new() -> Self {
            TestTemplateEnv {
                language: L::new(),
                aliases_map: TemplateAliasesMap::new(),
                color_rules: Vec::new(),
            }
        }
    }

    impl TestTemplateEnv {
        fn add_keyword<F>(&mut self, name: &'static str, build: F)
        where
            F: Fn() -> TestTemplatePropertyKind + 'static,
        {
            self.language.add_keyword(name, move |_| Ok(build()));
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

        fn parse(&self, template: &str) -> TemplateParseResult<TemplateRenderer<'static, ()>> {
            parse(&self.language, template, &self.aliases_map, L::wrap_self)
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
            drop(formatter);
            String::from_utf8(output).unwrap()
        }
    }

    fn new_error_property<O>(message: &str) -> impl TemplateProperty<Output = O> + '_ {
        Literal(()).and_then(|()| Err(TemplatePropertyError(message.into())))
    }

    fn new_signature(name: &str, email: &str) -> Signature {
        Signature {
            name: name.to_owned(),
            email: email.to_owned(),
            timestamp: new_timestamp(0, 0),
        }
    }

    fn new_timestamp(msec: i64, tz_offset: i32) -> Timestamp {
        Timestamp {
            timestamp: MillisSinceEpoch(msec),
            tz_offset,
        }
    }

    #[test]
    fn test_parsed_tree() {
        let mut env = TestTemplateEnv::new();
        env.add_keyword("divergent", || L::wrap_boolean(Literal(false)));
        env.add_keyword("empty", || L::wrap_boolean(Literal(true)));
        env.add_keyword("hello", || L::wrap_string(Literal("Hello".to_owned())));

        // Empty
        insta::assert_snapshot!(env.render_ok(r#"  "#), @"");

        // Single term with whitespace
        insta::assert_snapshot!(env.render_ok(r#"  hello.upper()  "#), @"HELLO");

        // Multiple terms
        insta::assert_snapshot!(env.render_ok(r#"  hello.upper()  ++ true "#), @"HELLOtrue");

        // Parenthesized single term
        insta::assert_snapshot!(env.render_ok(r#"(hello.upper())"#), @"HELLO");

        // Parenthesized multiple terms and concatenation
        insta::assert_snapshot!(env.render_ok(r#"(hello.upper() ++ " ") ++ empty"#), @"HELLO true");

        // Parenthesized "if" condition
        insta::assert_snapshot!(env.render_ok(r#"if((divergent), "t", "f")"#), @"f");

        // Parenthesized method chaining
        insta::assert_snapshot!(env.render_ok(r#"(hello).upper()"#), @"HELLO");
    }

    #[test]
    fn test_parse_error() {
        let mut env = TestTemplateEnv::new();
        env.add_keyword("description", || L::wrap_string(Literal("".to_owned())));
        env.add_keyword("empty", || L::wrap_boolean(Literal(true)));

        insta::assert_snapshot!(env.parse_err(r#"description ()"#), @r###"
         --> 1:13
          |
        1 | description ()
          |             ^---
          |
          = expected <EOI>, `++`, `||`, or `&&`
        "###);

        insta::assert_snapshot!(env.parse_err(r#"foo"#), @r###"
         --> 1:1
          |
        1 | foo
          | ^-^
          |
          = Keyword "foo" doesn't exist
        "###);

        insta::assert_snapshot!(env.parse_err(r#"foo()"#), @r###"
         --> 1:1
          |
        1 | foo()
          | ^-^
          |
          = Function "foo" doesn't exist
        "###);
        insta::assert_snapshot!(env.parse_err(r#"false()"#), @r###"
         --> 1:1
          |
        1 | false()
          | ^---^
          |
          = Expected identifier
        "###);

        insta::assert_snapshot!(env.parse_err(r#"!foo"#), @r###"
         --> 1:2
          |
        1 | !foo
          |  ^-^
          |
          = Keyword "foo" doesn't exist
        "###);
        insta::assert_snapshot!(env.parse_err(r#"true && 123"#), @r###"
         --> 1:9
          |
        1 | true && 123
          |         ^-^
          |
          = Expected expression of type "Boolean"
        "###);

        insta::assert_snapshot!(env.parse_err(r#"description.first_line().foo()"#), @r###"
         --> 1:26
          |
        1 | description.first_line().foo()
          |                          ^-^
          |
          = Method "foo" doesn't exist for type "String"
        "###);

        insta::assert_snapshot!(env.parse_err(r#"10000000000000000000"#), @r###"
         --> 1:1
          |
        1 | 10000000000000000000
          | ^------------------^
          |
          = Invalid integer literal
        "###);
        insta::assert_snapshot!(env.parse_err(r#"42.foo()"#), @r###"
         --> 1:4
          |
        1 | 42.foo()
          |    ^-^
          |
          = Method "foo" doesn't exist for type "Integer"
        "###);
        insta::assert_snapshot!(env.parse_err(r#"(-empty)"#), @r###"
         --> 1:3
          |
        1 | (-empty)
          |   ^---^
          |
          = Expected expression of type "Integer"
        "###);

        insta::assert_snapshot!(env.parse_err(r#"("foo" ++ "bar").baz()"#), @r###"
         --> 1:18
          |
        1 | ("foo" ++ "bar").baz()
          |                  ^-^
          |
          = Method "baz" doesn't exist for type "Template"
        "###);

        insta::assert_snapshot!(env.parse_err(r#"description.contains()"#), @r###"
         --> 1:22
          |
        1 | description.contains()
          |                      ^
          |
          = Function "contains": Expected 1 arguments
        "###);

        insta::assert_snapshot!(env.parse_err(r#"description.first_line("foo")"#), @r###"
         --> 1:24
          |
        1 | description.first_line("foo")
          |                        ^---^
          |
          = Function "first_line": Expected 0 arguments
        "###);

        insta::assert_snapshot!(env.parse_err(r#"label()"#), @r###"
         --> 1:7
          |
        1 | label()
          |       ^
          |
          = Function "label": Expected 2 arguments
        "###);
        insta::assert_snapshot!(env.parse_err(r#"label("foo", "bar", "baz")"#), @r###"
         --> 1:7
          |
        1 | label("foo", "bar", "baz")
          |       ^-----------------^
          |
          = Function "label": Expected 2 arguments
        "###);

        insta::assert_snapshot!(env.parse_err(r#"if()"#), @r###"
         --> 1:4
          |
        1 | if()
          |    ^
          |
          = Function "if": Expected 2 to 3 arguments
        "###);
        insta::assert_snapshot!(env.parse_err(r#"if("foo", "bar", "baz", "quux")"#), @r###"
         --> 1:4
          |
        1 | if("foo", "bar", "baz", "quux")
          |    ^-------------------------^
          |
          = Function "if": Expected 2 to 3 arguments
        "###);

        insta::assert_snapshot!(env.parse_err(r#"if(label("foo", "bar"), "baz")"#), @r###"
         --> 1:4
          |
        1 | if(label("foo", "bar"), "baz")
          |    ^-----------------^
          |
          = Expected expression of type "Boolean"
        "###);

        insta::assert_snapshot!(env.parse_err(r#"|x| description"#), @r###"
         --> 1:1
          |
        1 | |x| description
          | ^-------------^
          |
          = Lambda cannot be defined here
        "###);
    }

    #[test]
    fn test_self_keyword() {
        let mut env = TestTemplateEnv::new();
        env.add_keyword("say_hello", || L::wrap_string(Literal("Hello".to_owned())));

        insta::assert_snapshot!(env.render_ok(r#"self.say_hello()"#), @"Hello");
        insta::assert_snapshot!(env.parse_err(r#"self"#), @r###"
         --> 1:1
          |
        1 | self
          | ^--^
          |
          = Expected expression of type "Template"
        "###);
    }

    #[test]
    fn test_boolean_cast() {
        let mut env = TestTemplateEnv::new();

        insta::assert_snapshot!(env.render_ok(r#"if("", true, false)"#), @"false");
        insta::assert_snapshot!(env.render_ok(r#"if("a", true, false)"#), @"true");

        env.add_keyword("sl0", || {
            L::wrap_string_list(Literal::<Vec<String>>(vec![]))
        });
        env.add_keyword("sl1", || L::wrap_string_list(Literal(vec!["".to_owned()])));
        insta::assert_snapshot!(env.render_ok(r#"if(sl0, true, false)"#), @"false");
        insta::assert_snapshot!(env.render_ok(r#"if(sl1, true, false)"#), @"true");

        // No implicit cast of integer
        insta::assert_snapshot!(env.parse_err(r#"if(0, true, false)"#), @r###"
         --> 1:4
          |
        1 | if(0, true, false)
          |    ^
          |
          = Expected expression of type "Boolean"
        "###);

        insta::assert_snapshot!(env.parse_err(r#"if(label("", ""), true, false)"#), @r###"
         --> 1:4
          |
        1 | if(label("", ""), true, false)
          |    ^-----------^
          |
          = Expected expression of type "Boolean"
        "###);
        insta::assert_snapshot!(env.parse_err(r#"if(sl0.map(|x| x), true, false)"#), @r###"
         --> 1:4
          |
        1 | if(sl0.map(|x| x), true, false)
          |    ^------------^
          |
          = Expected expression of type "Boolean"
        "###);
    }

    #[test]
    fn test_arithmetic_operation() {
        let mut env = TestTemplateEnv::new();
        env.add_keyword("i64_min", || L::wrap_integer(Literal(i64::MIN)));

        insta::assert_snapshot!(env.render_ok(r#"-1"#), @"-1");
        insta::assert_snapshot!(env.render_ok(r#"--2"#), @"2");
        insta::assert_snapshot!(env.render_ok(r#"-(3)"#), @"-3");

        // No panic on integer overflow.
        insta::assert_snapshot!(
            env.render_ok(r#"-i64_min"#),
            @"<Error: Attempt to negate with overflow>");
    }

    #[test]
    fn test_logical_operation() {
        let mut env = TestTemplateEnv::new();

        insta::assert_snapshot!(env.render_ok(r#"!false"#), @"true");
        insta::assert_snapshot!(env.render_ok(r#"false || !false"#), @"true");
        insta::assert_snapshot!(env.render_ok(r#"false && true"#), @"false");

        insta::assert_snapshot!(env.render_ok(r#" !"" "#), @"true");
        insta::assert_snapshot!(env.render_ok(r#" "" || "a".lines() "#), @"true");

        // Short-circuiting
        env.add_keyword("bad_bool", || L::wrap_boolean(new_error_property("Bad")));
        insta::assert_snapshot!(env.render_ok(r#"false && bad_bool"#), @"false");
        insta::assert_snapshot!(env.render_ok(r#"true && bad_bool"#), @"<Error: Bad>");
        insta::assert_snapshot!(env.render_ok(r#"false || bad_bool"#), @"<Error: Bad>");
        insta::assert_snapshot!(env.render_ok(r#"true || bad_bool"#), @"true");
    }

    #[test]
    fn test_list_method() {
        let mut env = TestTemplateEnv::new();
        env.add_keyword("empty", || L::wrap_boolean(Literal(true)));
        env.add_keyword("sep", || L::wrap_string(Literal("sep".to_owned())));

        insta::assert_snapshot!(env.render_ok(r#""".lines().len()"#), @"0");
        insta::assert_snapshot!(env.render_ok(r#""a\nb\nc".lines().len()"#), @"3");

        insta::assert_snapshot!(env.render_ok(r#""".lines().join("|")"#), @"");
        insta::assert_snapshot!(env.render_ok(r#""a\nb\nc".lines().join("|")"#), @"a|b|c");
        // Null separator
        insta::assert_snapshot!(env.render_ok(r#""a\nb\nc".lines().join("\0")"#), @"a\0b\0c");
        // Keyword as separator
        insta::assert_snapshot!(
            env.render_ok(r#""a\nb\nc".lines().join(sep.upper())"#),
            @"aSEPbSEPc");

        insta::assert_snapshot!(
            env.render_ok(r#""a\nb\nc".lines().map(|s| s ++ s)"#),
            @"aa bb cc");
        // Global keyword in item template
        insta::assert_snapshot!(
            env.render_ok(r#""a\nb\nc".lines().map(|s| s ++ empty)"#),
            @"atrue btrue ctrue");
        // Global keyword in item template shadowing 'self'
        insta::assert_snapshot!(
            env.render_ok(r#""a\nb\nc".lines().map(|self| self ++ empty)"#),
            @"atrue btrue ctrue");
        // Override global keyword 'empty'
        insta::assert_snapshot!(
            env.render_ok(r#""a\nb\nc".lines().map(|empty| empty)"#),
            @"a b c");
        // Nested map operations
        insta::assert_snapshot!(
            env.render_ok(r#""a\nb\nc".lines().map(|s| "x\ny".lines().map(|t| s ++ t))"#),
            @"ax ay bx by cx cy");
        // Nested map/join operations
        insta::assert_snapshot!(
            env.render_ok(r#""a\nb\nc".lines().map(|s| "x\ny".lines().map(|t| s ++ t).join(",")).join(";")"#),
            @"ax,ay;bx,by;cx,cy");
        // Nested string operations
        insta::assert_snapshot!(
            env.render_ok(r#""!a\n!b\nc\nend".remove_suffix("end").lines().map(|s| s.remove_prefix("!"))"#),
            @"a b c");

        // Lambda expression in alias
        env.add_alias("identity", "|x| x");
        insta::assert_snapshot!(env.render_ok(r#""a\nb\nc".lines().map(identity)"#), @"a b c");

        // Not a lambda expression
        insta::assert_snapshot!(env.parse_err(r#""a".lines().map(empty)"#), @r###"
         --> 1:17
          |
        1 | "a".lines().map(empty)
          |                 ^---^
          |
          = Expected lambda expression
        "###);
        // Bad lambda parameter count
        insta::assert_snapshot!(env.parse_err(r#""a".lines().map(|| "")"#), @r###"
         --> 1:18
          |
        1 | "a".lines().map(|| "")
          |                  ^
          |
          = Expected 1 lambda parameters
        "###);
        insta::assert_snapshot!(env.parse_err(r#""a".lines().map(|a, b| "")"#), @r###"
         --> 1:18
          |
        1 | "a".lines().map(|a, b| "")
          |                  ^--^
          |
          = Expected 1 lambda parameters
        "###);
        // Error in lambda expression
        insta::assert_snapshot!(env.parse_err(r#""a".lines().map(|s| s.unknown())"#), @r###"
         --> 1:23
          |
        1 | "a".lines().map(|s| s.unknown())
          |                       ^-----^
          |
          = Method "unknown" doesn't exist for type "String"
        "###);
        // Error in lambda alias
        env.add_alias("too_many_params", "|x, y| x");
        insta::assert_snapshot!(env.parse_err(r#""a".lines().map(too_many_params)"#), @r###"
         --> 1:17
          |
        1 | "a".lines().map(too_many_params)
          |                 ^-------------^
          |
          = Alias "too_many_params" cannot be expanded
         --> 1:2
          |
        1 | |x, y| x
          |  ^--^
          |
          = Expected 1 lambda parameters
        "###);
    }

    #[test]
    fn test_string_method() {
        let mut env = TestTemplateEnv::new();
        env.add_keyword("description", || {
            L::wrap_string(Literal("description 1".to_owned()))
        });
        env.add_keyword("bad_string", || L::wrap_string(new_error_property("Bad")));

        insta::assert_snapshot!(env.render_ok(r#""".len()"#), @"0");
        insta::assert_snapshot!(env.render_ok(r#""foo".len()"#), @"3");
        insta::assert_snapshot!(env.render_ok(r#""💩".len()"#), @"4");

        insta::assert_snapshot!(env.render_ok(r#""fooo".contains("foo")"#), @"true");
        insta::assert_snapshot!(env.render_ok(r#""foo".contains("fooo")"#), @"false");
        insta::assert_snapshot!(env.render_ok(r#"description.contains("description")"#), @"true");
        insta::assert_snapshot!(
            env.render_ok(r#""description 123".contains(description.first_line())"#),
            @"true");

        // inner template error should propagate
        insta::assert_snapshot!(env.render_ok(r#""foo".contains(bad_string)"#), @"<Error: Bad>");
        insta::assert_snapshot!(
            env.render_ok(r#""foo".contains("f" ++ bad_string) ++ "bar""#), @"<Error: Bad>bar");
        insta::assert_snapshot!(
            env.render_ok(r#""foo".contains(separate("o", "f", bad_string))"#), @"<Error: Bad>");

        insta::assert_snapshot!(env.render_ok(r#""".first_line()"#), @"");
        insta::assert_snapshot!(env.render_ok(r#""foo\nbar".first_line()"#), @"foo");

        insta::assert_snapshot!(env.render_ok(r#""".lines()"#), @"");
        insta::assert_snapshot!(env.render_ok(r#""a\nb\nc\n".lines()"#), @"a b c");

        insta::assert_snapshot!(env.render_ok(r#""".starts_with("")"#), @"true");
        insta::assert_snapshot!(env.render_ok(r#""everything".starts_with("")"#), @"true");
        insta::assert_snapshot!(env.render_ok(r#""".starts_with("foo")"#), @"false");
        insta::assert_snapshot!(env.render_ok(r#""foo".starts_with("foo")"#), @"true");
        insta::assert_snapshot!(env.render_ok(r#""foobar".starts_with("foo")"#), @"true");
        insta::assert_snapshot!(env.render_ok(r#""foobar".starts_with("bar")"#), @"false");

        insta::assert_snapshot!(env.render_ok(r#""".ends_with("")"#), @"true");
        insta::assert_snapshot!(env.render_ok(r#""everything".ends_with("")"#), @"true");
        insta::assert_snapshot!(env.render_ok(r#""".ends_with("foo")"#), @"false");
        insta::assert_snapshot!(env.render_ok(r#""foo".ends_with("foo")"#), @"true");
        insta::assert_snapshot!(env.render_ok(r#""foobar".ends_with("foo")"#), @"false");
        insta::assert_snapshot!(env.render_ok(r#""foobar".ends_with("bar")"#), @"true");

        insta::assert_snapshot!(env.render_ok(r#""".remove_prefix("wip: ")"#), @"");
        insta::assert_snapshot!(
            env.render_ok(r#""wip: testing".remove_prefix("wip: ")"#),
            @"testing");

        insta::assert_snapshot!(
            env.render_ok(r#""bar@my.example.com".remove_suffix("@other.example.com")"#),
            @"bar@my.example.com");
        insta::assert_snapshot!(
            env.render_ok(r#""bar@other.example.com".remove_suffix("@other.example.com")"#),
            @"bar");

        insta::assert_snapshot!(env.render_ok(r#""foo".substr(0, 0)"#), @"");
        insta::assert_snapshot!(env.render_ok(r#""foo".substr(0, 1)"#), @"f");
        insta::assert_snapshot!(env.render_ok(r#""foo".substr(0, 3)"#), @"foo");
        insta::assert_snapshot!(env.render_ok(r#""foo".substr(0, 4)"#), @"foo");
        insta::assert_snapshot!(env.render_ok(r#""abcdef".substr(2, -1)"#), @"cde");
        insta::assert_snapshot!(env.render_ok(r#""abcdef".substr(-3, 99)"#), @"def");
        insta::assert_snapshot!(env.render_ok(r#""abcdef".substr(-6, 99)"#), @"abcdef");
        insta::assert_snapshot!(env.render_ok(r#""abcdef".substr(-7, 1)"#), @"a");

        // non-ascii characters
        insta::assert_snapshot!(env.render_ok(r#""abc💩".substr(2, -1)"#), @"c💩");
        insta::assert_snapshot!(env.render_ok(r#""abc💩".substr(3, -3)"#), @"💩");
        insta::assert_snapshot!(env.render_ok(r#""abc💩".substr(3, -4)"#), @"");
        insta::assert_snapshot!(env.render_ok(r#""abc💩".substr(6, -3)"#), @"💩");
        insta::assert_snapshot!(env.render_ok(r#""abc💩".substr(7, -3)"#), @"");
        insta::assert_snapshot!(env.render_ok(r#""abc💩".substr(3, 4)"#), @"");
        insta::assert_snapshot!(env.render_ok(r#""abc💩".substr(3, 6)"#), @"");
        insta::assert_snapshot!(env.render_ok(r#""abc💩".substr(3, 7)"#), @"💩");
        insta::assert_snapshot!(env.render_ok(r#""abc💩".substr(-1, 7)"#), @"");
        insta::assert_snapshot!(env.render_ok(r#""abc💩".substr(-3, 7)"#), @"");
        insta::assert_snapshot!(env.render_ok(r#""abc💩".substr(-4, 7)"#), @"💩");

        // ranges with end > start are empty
        insta::assert_snapshot!(env.render_ok(r#""abcdef".substr(4, 2)"#), @"");
        insta::assert_snapshot!(env.render_ok(r#""abcdef".substr(-2, -4)"#), @"");
    }

    #[test]
    fn test_signature() {
        let mut env = TestTemplateEnv::new();

        env.add_keyword("author", || {
            L::wrap_signature(Literal(new_signature("Test User", "test.user@example.com")))
        });
        insta::assert_snapshot!(env.render_ok(r#"author"#), @"Test User <test.user@example.com>");
        insta::assert_snapshot!(env.render_ok(r#"author.name()"#), @"Test User");
        insta::assert_snapshot!(env.render_ok(r#"author.email()"#), @"test.user@example.com");
        insta::assert_snapshot!(env.render_ok(r#"author.username()"#), @"test.user");

        env.add_keyword("author", || {
            L::wrap_signature(Literal(new_signature(
                "Another Test User",
                "test.user@example.com",
            )))
        });
        insta::assert_snapshot!(env.render_ok(r#"author"#), @"Another Test User <test.user@example.com>");
        insta::assert_snapshot!(env.render_ok(r#"author.name()"#), @"Another Test User");
        insta::assert_snapshot!(env.render_ok(r#"author.email()"#), @"test.user@example.com");
        insta::assert_snapshot!(env.render_ok(r#"author.username()"#), @"test.user");

        env.add_keyword("author", || {
            L::wrap_signature(Literal(new_signature(
                "Test User",
                "test.user@invalid@example.com",
            )))
        });
        insta::assert_snapshot!(env.render_ok(r#"author"#), @"Test User <test.user@invalid@example.com>");
        insta::assert_snapshot!(env.render_ok(r#"author.name()"#), @"Test User");
        insta::assert_snapshot!(env.render_ok(r#"author.email()"#), @"test.user@invalid@example.com");
        insta::assert_snapshot!(env.render_ok(r#"author.username()"#), @"test.user");

        env.add_keyword("author", || {
            L::wrap_signature(Literal(new_signature("Test User", "test.user")))
        });
        insta::assert_snapshot!(env.render_ok(r#"author"#), @"Test User <test.user>");
        insta::assert_snapshot!(env.render_ok(r#"author.email()"#), @"test.user");
        insta::assert_snapshot!(env.render_ok(r#"author.username()"#), @"test.user");

        env.add_keyword("author", || {
            L::wrap_signature(Literal(new_signature(
                "Test User",
                "test.user+tag@example.com",
            )))
        });
        insta::assert_snapshot!(env.render_ok(r#"author"#), @"Test User <test.user+tag@example.com>");
        insta::assert_snapshot!(env.render_ok(r#"author.email()"#), @"test.user+tag@example.com");
        insta::assert_snapshot!(env.render_ok(r#"author.username()"#), @"test.user+tag");

        env.add_keyword("author", || {
            L::wrap_signature(Literal(new_signature("Test User", "x@y")))
        });
        insta::assert_snapshot!(env.render_ok(r#"author"#), @"Test User <x@y>");
        insta::assert_snapshot!(env.render_ok(r#"author.email()"#), @"x@y");
        insta::assert_snapshot!(env.render_ok(r#"author.username()"#), @"x");

        env.add_keyword("author", || {
            L::wrap_signature(Literal(new_signature("", "test.user@example.com")))
        });
        insta::assert_snapshot!(env.render_ok(r#"author"#), @"<test.user@example.com>");
        insta::assert_snapshot!(env.render_ok(r#"author.name()"#), @"");
        insta::assert_snapshot!(env.render_ok(r#"author.email()"#), @"test.user@example.com");
        insta::assert_snapshot!(env.render_ok(r#"author.username()"#), @"test.user");

        env.add_keyword("author", || {
            L::wrap_signature(Literal(new_signature("Test User", "")))
        });
        insta::assert_snapshot!(env.render_ok(r#"author"#), @"Test User");
        insta::assert_snapshot!(env.render_ok(r#"author.name()"#), @"Test User");
        insta::assert_snapshot!(env.render_ok(r#"author.email()"#), @"");
        insta::assert_snapshot!(env.render_ok(r#"author.username()"#), @"");

        env.add_keyword("author", || {
            L::wrap_signature(Literal(new_signature("", "")))
        });
        insta::assert_snapshot!(env.render_ok(r#"author"#), @"");
        insta::assert_snapshot!(env.render_ok(r#"author.name()"#), @"");
        insta::assert_snapshot!(env.render_ok(r#"author.email()"#), @"");
        insta::assert_snapshot!(env.render_ok(r#"author.username()"#), @"");
    }

    #[test]
    fn test_timestamp_method() {
        let mut env = TestTemplateEnv::new();
        env.add_keyword("t0", || L::wrap_timestamp(Literal(new_timestamp(0, 0))));

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
    fn test_fill_function() {
        let mut env = TestTemplateEnv::new();
        env.add_color("error", crossterm::style::Color::DarkRed);

        insta::assert_snapshot!(
            env.render_ok(r#"fill(20, "The quick fox jumps over the " ++
                                  label("error", "lazy") ++ " dog\n")"#),
            @r###"
        The quick fox jumps
        over the [38;5;1mlazy[39m dog
        "###);

        // A low value will not chop words, but can chop a label by words
        insta::assert_snapshot!(
            env.render_ok(r#"fill(9, "Longlonglongword an some short words " ++
                                  label("error", "longlonglongword and short words") ++
                                  " back out\n")"#),
            @r###"
        Longlonglongword
        an some
        short
        words
        [38;5;1mlonglonglongword[39m
        [38;5;1mand short[39m
        [38;5;1mwords[39m
        back out
        "###);

        // Filling to 0 means breaking at every word
        insta::assert_snapshot!(
            env.render_ok(r#"fill(0, "The quick fox jumps over the " ++
                                  label("error", "lazy") ++ " dog\n")"#),
            @r###"
        The
        quick
        fox
        jumps
        over
        the
        [38;5;1mlazy[39m
        dog
        "###);

        // Filling to -0 is the same as 0
        insta::assert_snapshot!(
            env.render_ok(r#"fill(-0, "The quick fox jumps over the " ++
                                  label("error", "lazy") ++ " dog\n")"#),
            @r###"
        The
        quick
        fox
        jumps
        over
        the
        [38;5;1mlazy[39m
        dog
        "###);

        // Filling to negative width is an error
        insta::assert_snapshot!(
            env.render_ok(r#"fill(-10, "The quick fox jumps over the " ++
                                  label("error", "lazy") ++ " dog\n")"#),
            @"[38;5;1m<Error: out of range integral type conversion attempted>[39m");

        // Word-wrap, then indent
        insta::assert_snapshot!(
            env.render_ok(r#""START marker to help insta\n" ++
                             indent("    ", fill(20, "The quick fox jumps over the " ++
                                                 label("error", "lazy") ++ " dog\n"))"#),
            @r###"
        START marker to help insta
            The quick fox jumps
            over the [38;5;1mlazy[39m dog
        "###);

        // Word-wrap indented (no special handling for leading spaces)
        insta::assert_snapshot!(
            env.render_ok(r#""START marker to help insta\n" ++
                             fill(20, indent("    ", "The quick fox jumps over the " ++
                                             label("error", "lazy") ++ " dog\n"))"#),
            @r###"
        START marker to help insta
            The quick fox
        jumps over the [38;5;1mlazy[39m
        dog
        "###);
    }

    #[test]
    fn test_indent_function() {
        let mut env = TestTemplateEnv::new();
        env.add_color("error", crossterm::style::Color::DarkRed);
        env.add_color("warning", crossterm::style::Color::DarkYellow);
        env.add_color("hint", crossterm::style::Color::DarkCyan);

        // Empty line shouldn't be indented. Not using insta here because we test
        // whitespace existence.
        assert_eq!(env.render_ok(r#"indent("__", "")"#), "");
        assert_eq!(env.render_ok(r#"indent("__", "\n")"#), "\n");
        assert_eq!(env.render_ok(r#"indent("__", "a\n\nb")"#), "__a\n\n__b");

        // "\n" at end of labeled text
        insta::assert_snapshot!(
            env.render_ok(r#"indent("__", label("error", "a\n") ++ label("warning", "b\n"))"#),
            @r###"
        [38;5;1m__a[39m
        [38;5;3m__b[39m
        "###);

        // "\n" in labeled text
        insta::assert_snapshot!(
            env.render_ok(r#"indent("__", label("error", "a") ++ label("warning", "b\nc"))"#),
            @r###"
        [38;5;1m__a[39m[38;5;3mb[39m
        [38;5;3m__c[39m
        "###);

        // Labeled prefix + unlabeled content
        insta::assert_snapshot!(
            env.render_ok(r#"indent(label("error", "XX"), "a\nb\n")"#),
            @r###"
        [38;5;1mXX[39ma
        [38;5;1mXX[39mb
        "###);

        // Nested indent, silly but works
        insta::assert_snapshot!(
            env.render_ok(r#"indent(label("hint", "A"),
                                    label("warning", indent(label("hint", "B"),
                                                            label("error", "x\n") ++ "y")))"#),
            @r###"
        [38;5;6mAB[38;5;1mx[39m
        [38;5;6mAB[38;5;3my[39m
        "###);
    }

    #[test]
    fn test_label_function() {
        let mut env = TestTemplateEnv::new();
        env.add_keyword("empty", || L::wrap_boolean(Literal(true)));
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

    #[test]
    fn test_coalesce_function() {
        let mut env = TestTemplateEnv::new();
        env.add_keyword("bad_string", || L::wrap_string(new_error_property("Bad")));
        env.add_keyword("empty_string", || L::wrap_string(Literal("".to_owned())));
        env.add_keyword("non_empty_string", || {
            L::wrap_string(Literal("a".to_owned()))
        });

        insta::assert_snapshot!(env.render_ok(r#"coalesce()"#), @"");
        insta::assert_snapshot!(env.render_ok(r#"coalesce("")"#), @"");
        insta::assert_snapshot!(env.render_ok(r#"coalesce("", "a", "", "b")"#), @"a");
        insta::assert_snapshot!(
            env.render_ok(r#"coalesce(empty_string, "", non_empty_string)"#), @"a");

        // "false" is not empty
        insta::assert_snapshot!(env.render_ok(r#"coalesce(false, true)"#), @"false");

        // Error is not empty
        insta::assert_snapshot!(env.render_ok(r#"coalesce(bad_string, "a")"#), @"<Error: Bad>");
        // but can be short-circuited
        insta::assert_snapshot!(env.render_ok(r#"coalesce("a", bad_string)"#), @"a");
    }

    #[test]
    fn test_concat_function() {
        let mut env = TestTemplateEnv::new();
        env.add_keyword("empty", || L::wrap_boolean(Literal(true)));
        env.add_keyword("hidden", || L::wrap_boolean(Literal(false)));
        env.add_color("empty", crossterm::style::Color::DarkGreen);
        env.add_color("error", crossterm::style::Color::DarkRed);
        env.add_color("warning", crossterm::style::Color::DarkYellow);

        insta::assert_snapshot!(env.render_ok(r#"concat()"#), @"");
        insta::assert_snapshot!(
            env.render_ok(r#"concat(hidden, empty)"#),
            @"false[38;5;2mtrue[39m");
        insta::assert_snapshot!(
            env.render_ok(r#"concat(label("error", ""), label("warning", "a"), "b")"#),
            @"[38;5;3ma[39mb");
    }

    #[test]
    fn test_separate_function() {
        let mut env = TestTemplateEnv::new();
        env.add_keyword("description", || L::wrap_string(Literal("".to_owned())));
        env.add_keyword("empty", || L::wrap_boolean(Literal(true)));
        env.add_keyword("hidden", || L::wrap_boolean(Literal(false)));
        env.add_color("empty", crossterm::style::Color::DarkGreen);
        env.add_color("error", crossterm::style::Color::DarkRed);
        env.add_color("warning", crossterm::style::Color::DarkYellow);

        insta::assert_snapshot!(env.render_ok(r#"separate(" ")"#), @"");
        insta::assert_snapshot!(env.render_ok(r#"separate(" ", "")"#), @"");
        insta::assert_snapshot!(env.render_ok(r#"separate(" ", "a")"#), @"a");
        insta::assert_snapshot!(env.render_ok(r#"separate(" ", "a", "b")"#), @"a b");
        insta::assert_snapshot!(env.render_ok(r#"separate(" ", "a", "", "b")"#), @"a b");
        insta::assert_snapshot!(env.render_ok(r#"separate(" ", "a", "b", "")"#), @"a b");
        insta::assert_snapshot!(env.render_ok(r#"separate(" ", "", "a", "b")"#), @"a b");

        // Labeled
        insta::assert_snapshot!(
            env.render_ok(r#"separate(" ", label("error", ""), label("warning", "a"), "b")"#),
            @"[38;5;3ma[39m b");

        // List template
        insta::assert_snapshot!(env.render_ok(r#"separate(" ", "a", ("" ++ ""))"#), @"a");
        insta::assert_snapshot!(env.render_ok(r#"separate(" ", "a", ("" ++ "b"))"#), @"a b");

        // Nested separate
        insta::assert_snapshot!(
            env.render_ok(r#"separate(" ", "a", separate("|", "", ""))"#), @"a");
        insta::assert_snapshot!(
            env.render_ok(r#"separate(" ", "a", separate("|", "b", ""))"#), @"a b");
        insta::assert_snapshot!(
            env.render_ok(r#"separate(" ", "a", separate("|", "b", "c"))"#), @"a b|c");

        // Conditional template
        insta::assert_snapshot!(
            env.render_ok(r#"separate(" ", "a", if(true, ""))"#), @"a");
        insta::assert_snapshot!(
            env.render_ok(r#"separate(" ", "a", if(true, "", "f"))"#), @"a");
        insta::assert_snapshot!(
            env.render_ok(r#"separate(" ", "a", if(false, "t", ""))"#), @"a");
        insta::assert_snapshot!(
            env.render_ok(r#"separate(" ", "a", if(true, "t", "f"))"#), @"a t");

        // Separate keywords
        insta::assert_snapshot!(
            env.render_ok(r#"separate(" ", hidden, description, empty)"#),
            @"false [38;5;2mtrue[39m");

        // Keyword as separator
        insta::assert_snapshot!(
            env.render_ok(r#"separate(hidden, "X", "Y", "Z")"#),
            @"XfalseYfalseZ");
    }

    #[test]
    fn test_surround_function() {
        let mut env = TestTemplateEnv::new();
        env.add_keyword("lt", || L::wrap_string(Literal("<".to_owned())));
        env.add_keyword("gt", || L::wrap_string(Literal(">".to_owned())));
        env.add_keyword("content", || L::wrap_string(Literal("content".to_owned())));
        env.add_keyword("empty_content", || L::wrap_string(Literal("".to_owned())));
        env.add_color("error", crossterm::style::Color::DarkRed);
        env.add_color("paren", crossterm::style::Color::Cyan);

        insta::assert_snapshot!(env.render_ok(r#"surround("{", "}", "")"#), @"");
        insta::assert_snapshot!(env.render_ok(r#"surround("{", "}", "a")"#), @"{a}");

        // Labeled
        insta::assert_snapshot!(
            env.render_ok(
                r#"surround(label("paren", "("), label("paren", ")"), label("error", "a"))"#),
            @"[38;5;14m([39m[38;5;1ma[39m[38;5;14m)[39m");

        // Keyword
        insta::assert_snapshot!(
            env.render_ok(r#"surround(lt, gt, content)"#),
            @"<content>");
        insta::assert_snapshot!(
            env.render_ok(r#"surround(lt, gt, empty_content)"#),
            @"");

        // Conditional template as content
        insta::assert_snapshot!(
            env.render_ok(r#"surround(lt, gt, if(empty_content, "", "empty"))"#),
            @"<empty>");
        insta::assert_snapshot!(
            env.render_ok(r#"surround(lt, gt, if(empty_content, "not empty", ""))"#),
            @"");
    }
}
