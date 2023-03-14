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

use std::io;

use jujutsu_lib::backend::{Signature, Timestamp};

use crate::formatter::{FormatRecorder, Formatter, PlainTextFormatter};
use crate::time_util;

pub trait Template<C> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()>;
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
        write!(formatter, " <")?;
        write!(formatter.labeled("email"), "{}", self.email)?;
        write!(formatter, ">")?;
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
        let labels = self.labels.extract(context);
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

pub trait TemplateProperty<C> {
    type Output;

    fn extract(&self, context: &C) -> Self::Output;
}

impl<C, P: TemplateProperty<C> + ?Sized> TemplateProperty<C> for Box<P> {
    type Output = <P as TemplateProperty<C>>::Output;

    fn extract(&self, context: &C) -> Self::Output {
        <P as TemplateProperty<C>>::extract(self, context)
    }
}

impl<C, P: TemplateProperty<C>> TemplateProperty<C> for Option<P> {
    type Output = Option<P::Output>;

    fn extract(&self, context: &C) -> Self::Output {
        self.as_ref().map(|property| property.extract(context))
    }
}

// Implement TemplateProperty for tuples
macro_rules! tuple_impls {
    ($( ( $($n:tt $T:ident),+ ) )+) => {
        $(
            impl<C, $($T: TemplateProperty<C>,)+> TemplateProperty<C> for ($($T,)+) {
                type Output = ($($T::Output,)+);

                fn extract(&self, context: &C) -> Self::Output {
                    ($(self.$n.extract(context),)+)
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

    fn extract(&self, _context: &C) -> O {
        self.0.clone()
    }
}

/// Adapter to turn closure into property.
pub struct TemplatePropertyFn<F>(pub F);

impl<C, O, F: Fn(&C) -> O> TemplateProperty<C> for TemplatePropertyFn<F> {
    type Output = O;

    fn extract(&self, context: &C) -> Self::Output {
        (self.0)(context)
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
        let template = self.property.extract(context);
        template.format(&(), formatter)
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

    fn extract(&self, context: &C) -> Self::Output {
        let mut output = vec![];
        self.template
            .format(context, &mut PlainTextFormatter::new(&mut output))
            .expect("write() to PlainTextFormatter should never fail");
        // TODO: Use from_utf8_lossy() if we added template that embeds file content
        String::from_utf8(output).expect("template output should be utf-8 bytes")
    }
}

/// Renders a list of template properties with the given separator.
///
/// Each template property can be extracted as a context-less value, but
/// the separator takes a context of type `C`.
pub struct FormattablePropertyListTemplate<P, S> {
    property: P,
    separator: S,
}

impl<P, S> FormattablePropertyListTemplate<P, S> {
    pub fn new<C>(property: P, separator: S) -> Self
    where
        P: TemplateProperty<C>,
        P::Output: IntoIterator,
        <P::Output as IntoIterator>::Item: Template<()>,
        S: Template<C>,
    {
        FormattablePropertyListTemplate {
            property,
            separator,
        }
    }
}

impl<C, P, S> Template<C> for FormattablePropertyListTemplate<P, S>
where
    P: TemplateProperty<C>,
    P::Output: IntoIterator,
    <P::Output as IntoIterator>::Item: Template<()>,
    S: Template<C>,
{
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        let contents = self.property.extract(context);
        format_joined_with(
            context,
            formatter,
            contents,
            &self.separator,
            |_, formatter, item| item.format(&(), formatter),
        )
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
        if self.condition.extract(context) {
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

    fn extract(&self, context: &C) -> Self::Output {
        (self.function)(self.property.extract(context))
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
