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

use crate::formatter::{Formatter, PlainTextFormatter};
use crate::time_util;

pub trait Template<C> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()>;
    /// Returns true if `format()` will generate output other than labels.
    fn has_content(&self, context: &C) -> bool;
}

pub trait IntoTemplate<'a, C> {
    fn into_template(self) -> Box<dyn Template<C> + 'a>;
}

impl<C, T: Template<C> + ?Sized> Template<C> for Box<T> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        <T as Template<C>>::format(self, context, formatter)
    }

    fn has_content(&self, context: &C) -> bool {
        <T as Template<C>>::has_content(self, context)
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

    fn has_content(&self, _: &()) -> bool {
        true
    }
}

impl Template<()> for String {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(self)
    }

    fn has_content(&self, _: &()) -> bool {
        !self.is_empty()
    }
}

impl Template<()> for Timestamp {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(&time_util::format_absolute_timestamp(self))
    }

    fn has_content(&self, _: &()) -> bool {
        true
    }
}

impl Template<()> for bool {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(if *self { "true" } else { "false" })
    }

    fn has_content(&self, _: &()) -> bool {
        true
    }
}

impl Template<()> for i64 {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        write!(formatter, "{self}")
    }

    fn has_content(&self, _: &()) -> bool {
        true
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

    fn has_content(&self, context: &C) -> bool {
        self.content.has_content(context)
    }
}

pub struct ListTemplate<T>(pub Vec<T>);

impl<C, T: Template<C>> Template<C> for ListTemplate<T> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        for template in &self.0 {
            template.format(context, formatter)?
        }
        Ok(())
    }

    fn has_content(&self, context: &C) -> bool {
        self.0.iter().any(|template| template.has_content(context))
    }
}

/// Like `ListTemplate`, but inserts a separator between non-empty templates.
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
        // TemplateProperty may be evaluated twice, by has_content() and format().
        // If that's too expensive, we can instead create a buffered formatter
        // inheriting the state, and write to it to test the emptiness. In this case,
        // the formatter should guarantee push/pop_label() is noop without content.
        let mut content_templates = self
            .contents
            .iter()
            .filter(|template| template.has_content(context))
            .fuse();
        if let Some(template) = content_templates.next() {
            template.format(context, formatter)?;
        }
        for template in content_templates {
            self.separator.format(context, formatter)?;
            template.format(context, formatter)?;
        }
        Ok(())
    }

    fn has_content(&self, context: &C) -> bool {
        self.contents
            .iter()
            .any(|template| template.has_content(context))
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

    fn has_content(&self, _context: &C) -> bool {
        self.0.has_content(&())
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

    fn has_content(&self, context: &C) -> bool {
        let template = self.property.extract(context);
        template.has_content(&())
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

    fn has_content(&self, context: &C) -> bool {
        if self.condition.extract(context) {
            self.true_template.has_content(context)
        } else if let Some(false_template) = &self.false_template {
            false_template.has_content(context)
        } else {
            false
        }
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
