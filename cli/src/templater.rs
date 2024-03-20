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
use std::{error, fmt, io, iter};

use jj_lib::backend::{Signature, Timestamp};

use crate::formatter::{FormatRecorder, Formatter, LabeledWriter, PlainTextFormatter};
use crate::time_util;

/// Represents printable type or compiled template containing placeholder value.
pub trait Template {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()>;
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
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        <T as Template>::format(self, formatter)
    }
}

impl<T: Template + ?Sized> Template for Box<T> {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        <T as Template>::format(self, formatter)
    }
}

// All optional printable types should be printable, and it's unlikely to
// implement different formatting per type.
impl<T: Template> Template for Option<T> {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        self.as_ref().map_or(Ok(()), |t| t.format(formatter))
    }
}

impl Template for Signature {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
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
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        write!(formatter, "{self}")
    }
}

impl Template for &str {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        write!(formatter, "{self}")
    }
}

impl Template for Timestamp {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        match time_util::format_absolute_timestamp(self) {
            Ok(formatted) => write!(formatter, "{formatted}"),
            Err(err) => formatter.handle_error(err.into()),
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
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        self.start.format(formatter)?;
        write!(formatter, " - ")?;
        self.end.format(formatter)?;
        Ok(())
    }
}

impl Template for Vec<String> {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        format_joined(formatter, self, " ")
    }
}

impl Template for bool {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        let repr = if *self { "true" } else { "false" };
        write!(formatter, "{repr}")
    }
}

impl Template for i64 {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
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
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        let labels = match self.labels.extract() {
            Ok(labels) => labels,
            Err(err) => return formatter.handle_error(err),
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

/// Renders contents in order, and returns the first non-empty output.
pub struct CoalesceTemplate<T>(pub Vec<T>);

impl<T: Template> Template for CoalesceTemplate<T> {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        let Some((last, contents)) = self.0.split_last() else {
            return Ok(());
        };
        let record_non_empty = record_non_empty_fn(formatter);
        if let Some(recorder) = contents.iter().find_map(record_non_empty) {
            recorder?.replay(formatter.as_mut())
        } else {
            last.format(formatter) // no need to capture the last content
        }
    }
}

pub struct ConcatTemplate<T>(pub Vec<T>);

impl<T: Template> Template for ConcatTemplate<T> {
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
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
        F: Fn(&mut TemplateFormatter, &FormatRecorder) -> io::Result<()>,
    {
        ReformatTemplate { content, reformat }
    }
}

impl<T, F> Template for ReformatTemplate<T, F>
where
    T: Template,
    F: Fn(&mut TemplateFormatter, &FormatRecorder) -> io::Result<()>,
{
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        let rewrap = formatter.rewrap_fn();
        let mut recorder = FormatRecorder::new();
        self.content.format(&mut rewrap(&mut recorder))?;
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
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        let record_non_empty = record_non_empty_fn(formatter);
        let mut content_recorders = self.contents.iter().filter_map(record_non_empty).fuse();
        if let Some(recorder) = content_recorders.next() {
            recorder?.replay(formatter.as_mut())?;
        }
        for recorder in content_recorders {
            self.separator.format(formatter)?;
            recorder?.replay(formatter.as_mut())?;
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
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
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
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        match self.property.extract() {
            Ok(template) => template.format(formatter),
            Err(err) => formatter.handle_error(err),
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
        let mut formatter = PlainTextFormatter::new(&mut output);
        let mut wrapper = TemplateFormatter::new(&mut formatter, propagate_property_error);
        self.template.format(&mut wrapper)?;
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
        F: Fn(&mut TemplateFormatter, O) -> io::Result<()>,
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
    F: Fn(&mut TemplateFormatter, O) -> io::Result<()>,
{
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        let contents = match self.property.extract() {
            Ok(contents) => contents,
            Err(err) => return formatter.handle_error(err),
        };
        format_joined_with(formatter, contents, &self.separator, &self.format_item)
    }
}

impl<O, P, S, F> ListTemplate for ListPropertyTemplate<P, S, F>
where
    P: TemplateProperty,
    P::Output: IntoIterator<Item = O>,
    S: Template,
    F: Fn(&mut TemplateFormatter, O) -> io::Result<()>,
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
    fn format(&self, formatter: &mut TemplateFormatter) -> io::Result<()> {
        let condition = match self.condition.extract() {
            Ok(condition) => condition,
            Err(err) => return formatter.handle_error(err),
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
        if let Some(value) = self.value.borrow().as_ref() {
            Ok(value.clone())
        } else {
            Err(TemplatePropertyError("Placeholder value is not set".into()))
        }
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
        let mut wrapper = TemplateFormatter::new(formatter, format_property_error_inline);
        self.placeholder
            .with_value(context.clone(), || self.template.format(&mut wrapper))
    }
}

/// Wrapper to pass around `Formatter` and error handler.
pub struct TemplateFormatter<'a> {
    formatter: &'a mut dyn Formatter,
    error_handler: PropertyErrorHandler,
}

impl<'a> TemplateFormatter<'a> {
    fn new(formatter: &'a mut dyn Formatter, error_handler: PropertyErrorHandler) -> Self {
        TemplateFormatter {
            formatter,
            error_handler,
        }
    }

    /// Returns function that wraps another `Formatter` with the current error
    /// handling strategy.
    ///
    /// This does not borrow `self` so the underlying formatter can be mutably
    /// borrowed.
    pub fn rewrap_fn(&self) -> impl Fn(&mut dyn Formatter) -> TemplateFormatter<'_> {
        let error_handler = self.error_handler;
        move |formatter| TemplateFormatter::new(formatter, error_handler)
    }

    pub fn labeled<S: AsRef<str>>(
        &mut self,
        label: S,
    ) -> LabeledWriter<&mut (dyn Formatter + 'a), S> {
        self.formatter.labeled(label)
    }

    pub fn push_label(&mut self, label: &str) -> io::Result<()> {
        self.formatter.push_label(label)
    }

    pub fn pop_label(&mut self) -> io::Result<()> {
        self.formatter.pop_label()
    }

    pub fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        self.formatter.write_fmt(args)
    }

    /// Handles the given template property evaluation error.
    ///
    /// This usually prints the given error inline, and returns `Ok`. It's up to
    /// caller to decide whether or not to continue template processing on `Ok`.
    /// For example, `if(cond, ..)` expression will terminate if the `cond`
    /// failed to evaluate, whereas `concat(x, y, ..)` will continue processing.
    ///
    /// If `Err` is returned, the error should be propagated.
    pub fn handle_error(&mut self, err: TemplatePropertyError) -> io::Result<()> {
        (self.error_handler)(self.formatter, err)
    }
}

impl<'a> AsMut<dyn Formatter + 'a> for TemplateFormatter<'a> {
    fn as_mut(&mut self) -> &mut (dyn Formatter + 'a) {
        self.formatter
    }
}

pub fn format_joined<I, S>(
    formatter: &mut TemplateFormatter,
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
    formatter: &mut TemplateFormatter,
    contents: I,
    separator: S,
    mut format_item: F,
) -> io::Result<()>
where
    I: IntoIterator,
    S: Template,
    F: FnMut(&mut TemplateFormatter, I::Item) -> io::Result<()>,
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

type PropertyErrorHandler = fn(&mut dyn Formatter, TemplatePropertyError) -> io::Result<()>;

/// Prints property evaluation error as inline template output.
fn format_property_error_inline(
    formatter: &mut dyn Formatter,
    err: TemplatePropertyError,
) -> io::Result<()> {
    let TemplatePropertyError(err) = &err;
    formatter.with_label("error", |formatter| {
        write!(formatter, "<")?;
        write!(formatter.labeled("heading"), "Error: ")?;
        write!(formatter, "{err}")?;
        for err in iter::successors(err.source(), |err| err.source()) {
            write!(formatter, ": {err}")?;
        }
        write!(formatter, ">")?;
        Ok(())
    })
}

fn propagate_property_error(
    _formatter: &mut dyn Formatter,
    err: TemplatePropertyError,
) -> io::Result<()> {
    Err(io::Error::other(err.0))
}

/// Creates function that renders a template to buffer and returns the buffer
/// only if it isn't empty.
///
/// This inherits the error handling strategy from the given `formatter`.
fn record_non_empty_fn<T: Template + ?Sized>(
    formatter: &TemplateFormatter,
) -> impl Fn(&T) -> Option<io::Result<FormatRecorder>> {
    let rewrap = formatter.rewrap_fn();
    move |template| {
        let mut recorder = FormatRecorder::new();
        match template.format(&mut rewrap(&mut recorder)) {
            Ok(()) if recorder.data().is_empty() => None, // omit empty content
            Ok(()) => Some(Ok(recorder)),
            Err(e) => Some(Err(e)),
        }
    }
}
