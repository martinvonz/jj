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

pub trait Template<C> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()>;
}

/// Template that supports list-like behavior.
pub trait ListTemplate<C>: Template<C> {
    /// Concatenates items with the given separator.
    fn join<'a>(self: Box<Self>, separator: Box<dyn Template<C> + 'a>) -> Box<dyn Template<C> + 'a>
    where
        Self: 'a,
        C: 'a;

    /// Upcasts to the template type.
    fn into_template<'a>(self: Box<Self>) -> Box<dyn Template<C> + 'a>
    where
        Self: 'a;
}

pub trait IntoTemplate<'a, C> {
    fn into_template(self) -> Box<dyn Template<C> + 'a>;
}

impl<C, T: Template<C> + ?Sized> Template<C> for &T {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        <T as Template<C>>::format(self, context, formatter)
    }
}

impl<C, T: Template<C> + ?Sized> Template<C> for Box<T> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        <T as Template<C>>::format(self, context, formatter)
    }
}

impl Template<()> for Signature {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
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

impl Template<()> for String {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(self)
    }
}

impl Template<()> for &str {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(self)
    }
}

impl Template<()> for Timestamp {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(&time_util::format_absolute_timestamp(self))
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
    pub fn duration(&self) -> String {
        let mut f = timeago::Formatter::new();
        f.min_unit(timeago::TimeUnit::Microseconds).ago("");
        let duration = time_util::format_duration(&self.start, &self.end, &f);
        if duration == "now" {
            "less than a microsecond".to_owned()
        } else {
            duration
        }
    }
}

impl Template<()> for TimestampRange {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        self.start.format(&(), formatter)?;
        write!(formatter, " - ")?;
        self.end.format(&(), formatter)?;
        Ok(())
    }
}

impl Template<()> for Vec<String> {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        format_joined(&(), formatter, self, " ")
    }
}

impl Template<()> for bool {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(if *self { "true" } else { "false" })
    }
}

impl Template<()> for i64 {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        write!(formatter, "{self}")
    }
}

pub struct LabelTemplate<T, L> {
    content: T,
    labels: L,
}

impl<T, L> LabelTemplate<T, L> {
    pub fn new<C>(content: T, labels: L) -> Self
    where
        T: Template<C>,
        L: TemplateProperty<C, Output = Vec<String>>,
    {
        LabelTemplate { content, labels }
    }
}

impl<C, T, L> Template<C> for LabelTemplate<T, L>
where
    T: Template<C>,
    L: TemplateProperty<C, Output = Vec<String>>,
{
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        let labels = match self.labels.extract(context) {
            Ok(labels) => labels,
            Err(err) => return err.format(&(), formatter),
        };
        for label in &labels {
            formatter.push_label(label)?;
        }
        self.content.format(context, formatter)?;
        for _label in &labels {
            formatter.pop_label()?;
        }
        Ok(())
    }
}

pub struct ConcatTemplate<T>(pub Vec<T>);

impl<C, T: Template<C>> Template<C> for ConcatTemplate<T> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        for template in &self.0 {
            template.format(context, formatter)?
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
    pub fn new<C>(content: T, reformat: F) -> Self
    where
        T: Template<C>,
        F: Fn(&C, &mut dyn Formatter, &FormatRecorder) -> io::Result<()>,
    {
        ReformatTemplate { content, reformat }
    }
}

impl<C, T, F> Template<C> for ReformatTemplate<T, F>
where
    T: Template<C>,
    F: Fn(&C, &mut dyn Formatter, &FormatRecorder) -> io::Result<()>,
{
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        let mut recorder = FormatRecorder::new();
        self.content.format(context, &mut recorder)?;
        (self.reformat)(context, formatter, &recorder)
    }
}

/// Like `ConcatTemplate`, but inserts a separator between non-empty templates.
pub struct SeparateTemplate<S, T> {
    separator: S,
    contents: Vec<T>,
}

impl<S, T> SeparateTemplate<S, T> {
    pub fn new<C>(separator: S, contents: Vec<T>) -> Self
    where
        S: Template<C>,
        T: Template<C>,
    {
        SeparateTemplate {
            separator,
            contents,
        }
    }
}

impl<C, S, T> Template<C> for SeparateTemplate<S, T>
where
    S: Template<C>,
    T: Template<C>,
{
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        let mut content_recorders = self
            .contents
            .iter()
            .filter_map(|template| {
                let mut recorder = FormatRecorder::new();
                match template.format(context, &mut recorder) {
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
            self.separator.format(context, formatter)?;
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
impl Template<()> for TemplatePropertyError {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        format_error_inline(formatter, &*self.0)
    }
}

pub trait TemplateProperty<C> {
    type Output;

    fn extract(&self, context: &C) -> Result<Self::Output, TemplatePropertyError>;
}

impl<C, P: TemplateProperty<C> + ?Sized> TemplateProperty<C> for Box<P> {
    type Output = <P as TemplateProperty<C>>::Output;

    fn extract(&self, context: &C) -> Result<Self::Output, TemplatePropertyError> {
        <P as TemplateProperty<C>>::extract(self, context)
    }
}

impl<C, P: TemplateProperty<C>> TemplateProperty<C> for Option<P> {
    type Output = Option<P::Output>;

    fn extract(&self, context: &C) -> Result<Self::Output, TemplatePropertyError> {
        self.as_ref()
            .map(|property| property.extract(context))
            .transpose()
    }
}

// Implement TemplateProperty for tuples
macro_rules! tuple_impls {
    ($( ( $($n:tt $T:ident),+ ) )+) => {
        $(
            impl<C, $($T: TemplateProperty<C>,)+> TemplateProperty<C> for ($($T,)+) {
                type Output = ($($T::Output,)+);

                fn extract(&self, context: &C) -> Result<Self::Output, TemplatePropertyError> {
                    Ok(($(self.$n.extract(context)?,)+))
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

/// Adapter to drop template context.
pub struct Literal<O>(pub O);

impl<C, O: Template<()>> Template<C> for Literal<O> {
    fn format(&self, _context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        self.0.format(&(), formatter)
    }
}

impl<C, O: Clone> TemplateProperty<C> for Literal<O> {
    type Output = O;

    fn extract(&self, _context: &C) -> Result<Self::Output, TemplatePropertyError> {
        Ok(self.0.clone())
    }
}

/// Adapter to turn closure into property.
pub struct TemplatePropertyFn<F>(pub F);

impl<C, O, F: Fn(&C) -> O> TemplateProperty<C> for TemplatePropertyFn<F> {
    type Output = O;

    fn extract(&self, context: &C) -> Result<Self::Output, TemplatePropertyError> {
        Ok((self.0)(context))
    }
}

/// Adapter to extract context-less template value from property for displaying.
pub struct FormattablePropertyTemplate<P> {
    property: P,
}

impl<P> FormattablePropertyTemplate<P> {
    pub fn new<C>(property: P) -> Self
    where
        P: TemplateProperty<C>,
        P::Output: Template<()>,
    {
        FormattablePropertyTemplate { property }
    }
}

impl<C, P> Template<C> for FormattablePropertyTemplate<P>
where
    P: TemplateProperty<C>,
    P::Output: Template<()>,
{
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        match self.property.extract(context) {
            Ok(template) => template.format(&(), formatter),
            Err(err) => err.format(&(), formatter),
        }
    }
}

impl<'a, C: 'a, O> IntoTemplate<'a, C> for Box<dyn TemplateProperty<C, Output = O> + 'a>
where
    O: Template<()> + 'a,
{
    fn into_template(self) -> Box<dyn Template<C> + 'a> {
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

impl<C, T: Template<C>> TemplateProperty<C> for PlainTextFormattedProperty<T> {
    type Output = String;

    fn extract(&self, context: &C) -> Result<Self::Output, TemplatePropertyError> {
        let mut output = vec![];
        self.template
            .format(context, &mut PlainTextFormatter::new(&mut output))
            .expect("write() to PlainTextFormatter should never fail");
        // TODO: Use from_utf8_lossy() if we added template that embeds file content
        Ok(String::from_utf8(output).expect("template output should be utf-8 bytes"))
    }
}

/// Renders template property of list type with the given separator.
///
/// Each list item will be formatted by the given `format_item()` function.
/// The separator takes a context of type `C`.
pub struct ListPropertyTemplate<P, S, F> {
    property: P,
    separator: S,
    format_item: F,
}

impl<P, S, F> ListPropertyTemplate<P, S, F> {
    pub fn new<C, O>(property: P, separator: S, format_item: F) -> Self
    where
        P: TemplateProperty<C>,
        P::Output: IntoIterator<Item = O>,
        S: Template<C>,
        F: Fn(&C, &mut dyn Formatter, O) -> io::Result<()>,
    {
        ListPropertyTemplate {
            property,
            separator,
            format_item,
        }
    }
}

impl<C, O, P, S, F> Template<C> for ListPropertyTemplate<P, S, F>
where
    P: TemplateProperty<C>,
    P::Output: IntoIterator<Item = O>,
    S: Template<C>,
    F: Fn(&C, &mut dyn Formatter, O) -> io::Result<()>,
{
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        let contents = match self.property.extract(context) {
            Ok(contents) => contents,
            Err(err) => return err.format(&(), formatter),
        };
        format_joined_with(
            context,
            formatter,
            contents,
            &self.separator,
            &self.format_item,
        )
    }
}

impl<C, O, P, S, F> ListTemplate<C> for ListPropertyTemplate<P, S, F>
where
    P: TemplateProperty<C>,
    P::Output: IntoIterator<Item = O>,
    S: Template<C>,
    F: Fn(&C, &mut dyn Formatter, O) -> io::Result<()>,
{
    fn join<'a>(self: Box<Self>, separator: Box<dyn Template<C> + 'a>) -> Box<dyn Template<C> + 'a>
    where
        Self: 'a,
        C: 'a,
    {
        // Once join()-ed, list-like API should be dropped. This is guaranteed by
        // the return type.
        Box::new(ListPropertyTemplate::new(
            self.property,
            separator,
            self.format_item,
        ))
    }

    fn into_template<'a>(self: Box<Self>) -> Box<dyn Template<C> + 'a>
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
    pub fn new<C>(condition: P, true_template: T, false_template: Option<U>) -> Self
    where
        P: TemplateProperty<C, Output = bool>,
        T: Template<C>,
        U: Template<C>,
    {
        ConditionalTemplate {
            condition,
            true_template,
            false_template,
        }
    }
}

impl<C, P, T, U> Template<C> for ConditionalTemplate<P, T, U>
where
    P: TemplateProperty<C, Output = bool>,
    T: Template<C>,
    U: Template<C>,
{
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        let condition = match self.condition.extract(context) {
            Ok(condition) => condition,
            Err(err) => return err.format(&(), formatter),
        };
        if condition {
            self.true_template.format(context, formatter)?;
        } else if let Some(false_template) = &self.false_template {
            false_template.format(context, formatter)?;
        }
        Ok(())
    }
}

// TODO: If needed, add a ContextualTemplateFunction where the function also
// gets the context
pub struct TemplateFunction<P, F> {
    pub property: P,
    pub function: F,
}

impl<P, F> TemplateFunction<P, F> {
    pub fn new<C, O>(property: P, function: F) -> Self
    where
        P: TemplateProperty<C>,
        F: Fn(P::Output) -> O,
    {
        TemplateFunction { property, function }
    }
}

impl<C, O, P, F> TemplateProperty<C> for TemplateFunction<P, F>
where
    P: TemplateProperty<C>,
    F: Fn(P::Output) -> O,
{
    type Output = O;

    fn extract(&self, context: &C) -> Result<Self::Output, TemplatePropertyError> {
        Ok((self.function)(self.property.extract(context)?))
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

impl<C, O: Clone> TemplateProperty<C> for PropertyPlaceholder<O> {
    type Output = O;

    fn extract(&self, _: &C) -> Result<Self::Output, TemplatePropertyError> {
        Ok(self
            .value
            .borrow()
            .as_ref()
            .expect("placeholder value must be set before evaluating template")
            .clone())
    }
}

pub fn format_joined<C, I, S>(
    context: &C,
    formatter: &mut dyn Formatter,
    contents: I,
    separator: S,
) -> io::Result<()>
where
    I: IntoIterator,
    I::Item: Template<C>,
    S: Template<C>,
{
    format_joined_with(
        context,
        formatter,
        contents,
        separator,
        |context, formatter, item| item.format(context, formatter),
    )
}

fn format_joined_with<C, I, S, F>(
    context: &C,
    formatter: &mut dyn Formatter,
    contents: I,
    separator: S,
    mut format_item: F,
) -> io::Result<()>
where
    I: IntoIterator,
    S: Template<C>,
    F: FnMut(&C, &mut dyn Formatter, I::Item) -> io::Result<()>,
{
    let mut contents_iter = contents.into_iter().fuse();
    if let Some(item) = contents_iter.next() {
        format_item(context, formatter, item)?;
    }
    for item in contents_iter {
        separator.format(context, formatter)?;
        format_item(context, formatter, item)?;
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
