// Copyright 2020 The Jujutsu Authors
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

use std::cell::RefCell;
use std::rc::Rc;
use std::{error, io, iter};

use jj_lib::backend::{Signature, Timestamp};

use crate::formatter::{FormatRecorder, Formatter, PlainTextFormatter};
use crate::time_util;

/// Represents printable type or compiled template containing placeholder value.
pub trait Template {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()>;
}

/// Template that supports list-like behavior.
pub trait ListTemplate: Template {
    /// Concatenates items with the given separator.
    fn join<'a>(self: Box<Self>, separator: Box<dyn Template + 'a>) -> Box<dyn Template + 'a>
    where
        Self: 'a;

    /// Upcasts to the template type.
    fn into_template<'a>(self: Box<Self>) -> Box<dyn Template + 'a>
    where
        Self: 'a;
}

pub trait IntoTemplate<'a> {
    fn into_template(self) -> Box<dyn Template + 'a>;
}

impl<T: Template + ?Sized> Template for &T {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        <T as Template>::format(self, formatter)
    }
}

impl<T: Template + ?Sized> Template for Box<T> {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        <T as Template>::format(self, formatter)
    }
}

// All optional printable types should be printable, and it's unlikely to
// implement different formatting per type.
impl<T: Template> Template for Option<T> {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        self.as_ref().map_or(Ok(()), |t| t.format(formatter))
    }
}

impl Template for Signature {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        write!(formatter.labeled("name"), "{}", self.name)?;
        if !self.name.is_empty() && !self.email.is_empty() {
            write!(formatter, " ")?;
        }
        if !self.email.is_empty() {
            write!(formatter, "<")?;
            write!(formatter.labeled("email"), "{}", self.email)?;
            write!(formatter, ">")?;
        }
        Ok(())
    }
}

impl Template for String {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        write!(formatter, "{self}")
    }
}

impl Template for &str {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        write!(formatter, "{self}")
    }
}

impl Template for Timestamp {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        match time_util::format_absolute_timestamp(self) {
            Ok(formatted) => write!(formatter, "{formatted}"),
            Err(err) => format_error_inline(formatter, &err),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimestampRange {
    // Could be aliased to Range<Timestamp> if needed.
    pub start: Timestamp,
    pub end: Timestamp,
}

impl TimestampRange {
    // TODO: Introduce duration type, and move formatting to it.
    pub fn duration(&self) -> Result<String, time_util::TimestampOutOfRange> {
        let mut f = timeago::Formatter::new();
        f.min_unit(timeago::TimeUnit::Microseconds).ago("");
        let duration = time_util::format_duration(&self.start, &self.end, &f)?;
        if duration == "now" {
            Ok("less than a microsecond".to_owned())
        } else {
            Ok(duration)
        }
    }
}

impl Template for TimestampRange {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        self.start.format(formatter)?;
        write!(formatter, " - ")?;
        self.end.format(formatter)?;
        Ok(())
    }
}

impl Template for Vec<String> {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        format_joined(formatter, self, " ")
    }
}

impl Template for bool {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        let repr = if *self { "true" } else { "false" };
        write!(formatter, "{repr}")
    }
}

impl Template for i64 {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        write!(formatter, "{self}")
    }
}

pub struct LabelTemplate<T, L> {
    content: T,
    labels: L,
}

impl<T, L> LabelTemplate<T, L> {
    pub fn new(content: T, labels: L) -> Self
    where
        T: Template,
        L: TemplateProperty<Output = Vec<String>>,
    {
        LabelTemplate { content, labels }
    }
}

impl<T, L> Template for LabelTemplate<T, L>
where
    T: Template,
    L: TemplateProperty<Output = Vec<String>>,
{
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        let labels = match self.labels.extract() {
            Ok(labels) => labels,
            Err(err) => return err.format(formatter),
        };
        for label in &labels {
            formatter.push_label(label)?;
        }
        self.content.format(formatter)?;
        for _label in &labels {
            formatter.pop_label()?;
        }
        Ok(())
    }
}

pub struct ConcatTemplate<T>(pub Vec<T>);

impl<T: Template> Template for ConcatTemplate<T> {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        for template in &self.0 {
            template.format(formatter)?
        }
        Ok(())
    }
}

/// Renders the content to buffer, and transforms it without losing labels.
pub struct ReformatTemplate<T, F> {
    content: T,
    reformat: F,
}

impl<T, F> ReformatTemplate<T, F> {
    pub fn new(content: T, reformat: F) -> Self
    where
        T: Template,
        F: Fn(&mut dyn Formatter, &FormatRecorder) -> io::Result<()>,
    {
        ReformatTemplate { content, reformat }
    }
}

impl<T, F> Template for ReformatTemplate<T, F>
where
    T: Template,
    F: Fn(&mut dyn Formatter, &FormatRecorder) -> io::Result<()>,
{
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        let mut recorder = FormatRecorder::new();
        self.content.format(&mut recorder)?;
        (self.reformat)(formatter, &recorder)
    }
}

/// Like `ConcatTemplate`, but inserts a separator between non-empty templates.
pub struct SeparateTemplate<S, T> {
    separator: S,
    contents: Vec<T>,
}

impl<S, T> SeparateTemplate<S, T> {
    pub fn new(separator: S, contents: Vec<T>) -> Self
    where
        S: Template,
        T: Template,
    {
        SeparateTemplate {
            separator,
            contents,
        }
    }
}

impl<S, T> Template for SeparateTemplate<S, T>
where
    S: Template,
    T: Template,
{
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        let mut content_recorders = self
            .contents
            .iter()
            .filter_map(|template| {
                let mut recorder = FormatRecorder::new();
                match template.format(&mut recorder) {
                    Ok(()) if recorder.data().is_empty() => None, // omit empty content
                    Ok(()) => Some(Ok(recorder)),
                    Err(e) => Some(Err(e)),
                }
            })
            .fuse();
        if let Some(recorder) = content_recorders.next() {
            recorder?.replay(formatter)?;
        }
        for recorder in content_recorders {
            self.separator.format(formatter)?;
            recorder?.replay(formatter)?;
        }
        Ok(())
    }
}

/// Wrapper around an error occurred during template evaluation.
#[derive(Debug)]
pub struct TemplatePropertyError(pub Box<dyn error::Error + Send + Sync>);

// Implements conversion from any error type to support `expr?` in function
// binding. This type doesn't implement `std::error::Error` instead.
// https://github.com/dtolnay/anyhow/issues/25#issuecomment-544140480
impl<E> From<E> for TemplatePropertyError
where
    E: error::Error + Send + Sync + 'static,
{
    fn from(err: E) -> Self {
        TemplatePropertyError(err.into())
    }
}

/// Prints the evaluation error as inline template output.
impl Template for TemplatePropertyError {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        format_error_inline(formatter, &*self.0)
    }
}

pub trait TemplateProperty {
    type Output;

    fn extract(&self) -> Result<Self::Output, TemplatePropertyError>;
}

impl<P: TemplateProperty + ?Sized> TemplateProperty for Box<P> {
    type Output = <P as TemplateProperty>::Output;

    fn extract(&self) -> Result<Self::Output, TemplatePropertyError> {
        <P as TemplateProperty>::extract(self)
    }
}

impl<P: TemplateProperty> TemplateProperty for Option<P> {
    type Output = Option<P::Output>;

    fn extract(&self) -> Result<Self::Output, TemplatePropertyError> {
        self.as_ref().map(|property| property.extract()).transpose()
    }
}

// Implement TemplateProperty for tuples
macro_rules! tuple_impls {
    ($( ( $($n:tt $T:ident),+ ) )+) => {
        $(
            impl<$($T: TemplateProperty,)+> TemplateProperty for ($($T,)+) {
                type Output = ($($T::Output,)+);

                fn extract(&self) -> Result<Self::Output, TemplatePropertyError> {
                    Ok(($(self.$n.extract()?,)+))
                }
            }
        )+
    }
}

tuple_impls! {
    (0 T0)
    (0 T0, 1 T1)
    (0 T0, 1 T1, 2 T2)
    (0 T0, 1 T1, 2 T2, 3 T3)
}

/// `TemplateProperty` adapters that are useful when implementing methods.
pub trait TemplatePropertyExt: TemplateProperty {
    /// Translates to a property that will apply fallible `function` to an
    /// extracted value.
    fn and_then<O, F>(self, function: F) -> TemplateFunction<Self, F>
    where
        Self: Sized,
        F: Fn(Self::Output) -> Result<O, TemplatePropertyError>,
    {
        TemplateFunction::new(self, function)
    }

    /// Translates to a property that will apply `function` to an extracted
    /// value, leaving `Err` untouched.
    fn map<O, F>(self, function: F) -> impl TemplateProperty<Output = O>
    where
        Self: Sized,
        F: Fn(Self::Output) -> O,
    {
        TemplateFunction::new(self, move |value| Ok(function(value)))
    }
}

impl<P: TemplateProperty + ?Sized> TemplatePropertyExt for P {}

/// Adapter that wraps literal value in `TemplateProperty`.
pub struct Literal<O>(pub O);

impl<O: Template> Template for Literal<O> {
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        self.0.format(formatter)
    }
}

impl<O: Clone> TemplateProperty for Literal<O> {
    type Output = O;

    fn extract(&self) -> Result<Self::Output, TemplatePropertyError> {
        Ok(self.0.clone())
    }
}

/// Adapter to extract template value from property for displaying.
pub struct FormattablePropertyTemplate<P> {
    property: P,
}

impl<P> FormattablePropertyTemplate<P> {
    pub fn new(property: P) -> Self
    where
        P: TemplateProperty,
        P::Output: Template,
    {
        FormattablePropertyTemplate { property }
    }
}

impl<P> Template for FormattablePropertyTemplate<P>
where
    P: TemplateProperty,
    P::Output: Template,
{
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        match self.property.extract() {
            Ok(template) => template.format(formatter),
            Err(err) => err.format(formatter),
        }
    }
}

impl<'a, O> IntoTemplate<'a> for Box<dyn TemplateProperty<Output = O> + 'a>
where
    O: Template + 'a,
{
    fn into_template(self) -> Box<dyn Template + 'a> {
        Box::new(FormattablePropertyTemplate::new(self))
    }
}

/// Adapter to turn template back to string property.
pub struct PlainTextFormattedProperty<T> {
    template: T,
}

impl<T> PlainTextFormattedProperty<T> {
    pub fn new(template: T) -> Self {
        PlainTextFormattedProperty { template }
    }
}

impl<T: Template> TemplateProperty for PlainTextFormattedProperty<T> {
    type Output = String;

    fn extract(&self) -> Result<Self::Output, TemplatePropertyError> {
        let mut output = vec![];
        self.template
            .format(&mut PlainTextFormatter::new(&mut output))
            .expect("write() to PlainTextFormatter should never fail");
        Ok(String::from_utf8(output).map_err(|err| err.utf8_error())?)
    }
}

/// Renders template property of list type with the given separator.
///
/// Each list item will be formatted by the given `format_item()` function.
pub struct ListPropertyTemplate<P, S, F> {
    property: P,
    separator: S,
    format_item: F,
}

impl<P, S, F> ListPropertyTemplate<P, S, F> {
    pub fn new<O>(property: P, separator: S, format_item: F) -> Self
    where
        P: TemplateProperty,
        P::Output: IntoIterator<Item = O>,
        S: Template,
        F: Fn(&mut dyn Formatter, O) -> io::Result<()>,
    {
        ListPropertyTemplate {
            property,
            separator,
            format_item,
        }
    }
}

impl<O, P, S, F> Template for ListPropertyTemplate<P, S, F>
where
    P: TemplateProperty,
    P::Output: IntoIterator<Item = O>,
    S: Template,
    F: Fn(&mut dyn Formatter, O) -> io::Result<()>,
{
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        let contents = match self.property.extract() {
            Ok(contents) => contents,
            Err(err) => return err.format(formatter),
        };
        format_joined_with(formatter, contents, &self.separator, &self.format_item)
    }
}

impl<O, P, S, F> ListTemplate for ListPropertyTemplate<P, S, F>
where
    P: TemplateProperty,
    P::Output: IntoIterator<Item = O>,
    S: Template,
    F: Fn(&mut dyn Formatter, O) -> io::Result<()>,
{
    fn join<'a>(self: Box<Self>, separator: Box<dyn Template + 'a>) -> Box<dyn Template + 'a>
    where
        Self: 'a,
    {
        // Once join()-ed, list-like API should be dropped. This is guaranteed by
        // the return type.
        Box::new(ListPropertyTemplate::new(
            self.property,
            separator,
            self.format_item,
        ))
    }

    fn into_template<'a>(self: Box<Self>) -> Box<dyn Template + 'a>
    where
        Self: 'a,
    {
        self
    }
}

pub struct ConditionalTemplate<P, T, U> {
    pub condition: P,
    pub true_template: T,
    pub false_template: Option<U>,
}

impl<P, T, U> ConditionalTemplate<P, T, U> {
    pub fn new(condition: P, true_template: T, false_template: Option<U>) -> Self
    where
        P: TemplateProperty<Output = bool>,
        T: Template,
        U: Template,
    {
        ConditionalTemplate {
            condition,
            true_template,
            false_template,
        }
    }
}

impl<P, T, U> Template for ConditionalTemplate<P, T, U>
where
    P: TemplateProperty<Output = bool>,
    T: Template,
    U: Template,
{
    fn format(&self, formatter: &mut dyn Formatter) -> io::Result<()> {
        let condition = match self.condition.extract() {
            Ok(condition) => condition,
            Err(err) => return err.format(formatter),
        };
        if condition {
            self.true_template.format(formatter)?;
        } else if let Some(false_template) = &self.false_template {
            false_template.format(formatter)?;
        }
        Ok(())
    }
}

/// Adapter to apply fallible `function` to the `property`.
///
/// This is usually created by `TemplatePropertyExt::and_then()`/`map()`.
pub struct TemplateFunction<P, F> {
    pub property: P,
    pub function: F,
}

impl<P, F> TemplateFunction<P, F> {
    pub fn new<O>(property: P, function: F) -> Self
    where
        P: TemplateProperty,
        F: Fn(P::Output) -> Result<O, TemplatePropertyError>,
    {
        TemplateFunction { property, function }
    }
}

impl<O, P, F> TemplateProperty for TemplateFunction<P, F>
where
    P: TemplateProperty,
    F: Fn(P::Output) -> Result<O, TemplatePropertyError>,
{
    type Output = O;

    fn extract(&self) -> Result<Self::Output, TemplatePropertyError> {
        (self.function)(self.property.extract()?)
    }
}

/// Property which will be compiled into template once, and substituted later.
#[derive(Clone, Debug)]
pub struct PropertyPlaceholder<O> {
    value: Rc<RefCell<Option<O>>>,
}

impl<O> PropertyPlaceholder<O> {
    pub fn new() -> Self {
        PropertyPlaceholder {
            value: Rc::new(RefCell::new(None)),
        }
    }

    pub fn set(&self, value: O) {
        *self.value.borrow_mut() = Some(value);
    }

    pub fn take(&self) -> Option<O> {
        self.value.borrow_mut().take()
    }

    pub fn with_value<R>(&self, value: O, f: impl FnOnce() -> R) -> R {
        self.set(value);
        let result = f();
        self.take();
        result
    }
}

impl<O> Default for PropertyPlaceholder<O> {
    fn default() -> Self {
        Self::new()
    }
}

impl<O: Clone> TemplateProperty for PropertyPlaceholder<O> {
    type Output = O;

    fn extract(&self) -> Result<Self::Output, TemplatePropertyError> {
        Ok(self
            .value
            .borrow()
            .as_ref()
            .expect("placeholder value must be set before evaluating template")
            .clone())
    }
}

/// Adapter that renders compiled `template` with the `placeholder` value set.
pub struct TemplateRenderer<'a, C> {
    template: Box<dyn Template + 'a>,
    placeholder: PropertyPlaceholder<C>,
}

impl<'a, C: Clone> TemplateRenderer<'a, C> {
    pub fn new(template: Box<dyn Template + 'a>, placeholder: PropertyPlaceholder<C>) -> Self {
        TemplateRenderer {
            template,
            placeholder,
        }
    }

    pub fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        self.placeholder
            .with_value(context.clone(), || self.template.format(formatter))
    }
}

pub fn format_joined<I, S>(
    formatter: &mut dyn Formatter,
    contents: I,
    separator: S,
) -> io::Result<()>
where
    I: IntoIterator,
    I::Item: Template,
    S: Template,
{
    format_joined_with(formatter, contents, separator, |formatter, item| {
        item.format(formatter)
    })
}

fn format_joined_with<I, S, F>(
    formatter: &mut dyn Formatter,
    contents: I,
    separator: S,
    mut format_item: F,
) -> io::Result<()>
where
    I: IntoIterator,
    S: Template,
    F: FnMut(&mut dyn Formatter, I::Item) -> io::Result<()>,
{
    let mut contents_iter = contents.into_iter().fuse();
    if let Some(item) = contents_iter.next() {
        format_item(formatter, item)?;
    }
    for item in contents_iter {
        separator.format(formatter)?;
        format_item(formatter, item)?;
    }
    Ok(())
}

fn format_error_inline(formatter: &mut dyn Formatter, err: &dyn error::Error) -> io::Result<()> {
    formatter.with_label("error", |formatter| {
        write!(formatter, "<Error")?;
        for err in iter::successors(Some(err), |err| err.source()) {
            write!(formatter, ": {err}")?;
        }
        write!(formatter, ">")?;
        Ok(())
    })
}
