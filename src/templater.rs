// Copyright 2020 Google LLC
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

use std::borrow::BorrowMut;
use std::io;
use std::ops::Add;

use jujutsu_lib::commit::Commit;
use jujutsu_lib::repo::RepoRef;
use jujutsu_lib::store::{CommitId, Signature};

use crate::styler::Styler;

pub trait Template<C> {
    fn format(&self, context: &C, styler: &mut dyn Styler) -> io::Result<()>;
}

// TODO: Extract a trait for this type?
pub struct TemplateFormatter<'s, 't: 's, C> {
    template: Box<dyn Template<C> + 't>,
    styler: &'s mut dyn Styler,
}

impl<'s, 't: 's, C> TemplateFormatter<'s, 't, C> {
    pub fn new(template: Box<dyn Template<C> + 't>, styler: &'s mut dyn Styler) -> Self {
        TemplateFormatter { template, styler }
    }

    pub fn format<'c, 'a: 'c>(&'a mut self, context: &'c C) -> io::Result<()> {
        self.template.format(context, self.styler.borrow_mut())
    }
}

pub struct LiteralTemplate(pub String);

impl<C> Template<C> for LiteralTemplate {
    fn format(&self, _context: &C, styler: &mut dyn Styler) -> io::Result<()> {
        styler.write_str(&self.0)
    }
}

// TODO: figure out why this lifetime is needed
pub struct LabelTemplate<'a, C> {
    content: Box<dyn Template<C> + 'a>,
    labels: Vec<String>,
}

impl<'a, C> LabelTemplate<'a, C> {
    pub fn new(content: Box<dyn Template<C> + 'a>, labels: String) -> Self {
        let labels: Vec<String> = labels
            .split_whitespace()
            .map(|label| label.to_string())
            .collect();
        LabelTemplate { content, labels }
    }
}

impl<'a, C> Template<C> for LabelTemplate<'a, C> {
    fn format(&self, context: &C, styler: &mut dyn Styler) -> io::Result<()> {
        for label in &self.labels {
            styler.add_label(label.clone())?;
        }
        self.content.format(context, styler)?;
        for _label in &self.labels {
            styler.remove_label()?;
        }
        Ok(())
    }
}

// TODO: figure out why this lifetime is needed
pub struct DynamicLabelTemplate<'a, C> {
    content: Box<dyn Template<C> + 'a>,
    label_property: Box<dyn Fn(&C) -> String + 'a>,
}

impl<'a, C> DynamicLabelTemplate<'a, C> {
    pub fn new(
        content: Box<dyn Template<C> + 'a>,
        label_property: Box<dyn Fn(&C) -> String + 'a>,
    ) -> Self {
        DynamicLabelTemplate {
            content,
            label_property,
        }
    }
}

impl<'a, C> Template<C> for DynamicLabelTemplate<'a, C> {
    fn format(&self, context: &C, styler: &mut dyn Styler) -> io::Result<()> {
        let labels = self.label_property.as_ref()(context);
        let labels: Vec<String> = labels
            .split_whitespace()
            .map(|label| label.to_string())
            .collect();
        for label in &labels {
            styler.add_label(label.clone())?;
        }
        self.content.format(context, styler)?;
        for _label in &labels {
            styler.remove_label()?;
        }
        Ok(())
    }
}

// TODO: figure out why this lifetime is needed
pub struct ListTemplate<'a, C>(pub Vec<Box<dyn Template<C> + 'a>>);

impl<'a, C> Template<C> for ListTemplate<'a, C> {
    fn format(&self, context: &C, styler: &mut dyn Styler) -> io::Result<()> {
        for template in &self.0 {
            template.format(context, styler)?
        }
        Ok(())
    }
}

pub trait TemplateProperty<C, O> {
    fn extract(&self, context: &C) -> O;
}

pub struct ConstantTemplateProperty<O> {
    pub output: O,
}

impl<C, O: Clone> TemplateProperty<C, O> for ConstantTemplateProperty<O> {
    fn extract(&self, _context: &C) -> O {
        self.output.clone()
    }
}

// TODO: figure out why this lifetime is needed
pub struct StringPropertyTemplate<'a, C> {
    pub property: Box<dyn TemplateProperty<C, String> + 'a>,
}

impl<'a, C> Template<C> for StringPropertyTemplate<'a, C> {
    fn format(&self, context: &C, styler: &mut dyn Styler) -> io::Result<()> {
        let text = self.property.extract(context);
        styler.write_str(&text)
    }
}

pub struct ChangeIdProperty;

impl<'r> TemplateProperty<Commit, String> for ChangeIdProperty {
    fn extract(&self, context: &Commit) -> String {
        context.change_id().hex()
    }
}

pub struct DescriptionProperty;

impl<'r> TemplateProperty<Commit, String> for DescriptionProperty {
    fn extract(&self, context: &Commit) -> String {
        let description = context.description().to_owned();
        if description.ends_with('\n') {
            description
        } else {
            description.add("\n")
        }
    }
}

pub struct AuthorProperty;

impl<'r> TemplateProperty<Commit, Signature> for AuthorProperty {
    fn extract(&self, context: &Commit) -> Signature {
        context.author().clone()
    }
}

pub struct CommitterProperty;

impl<'r> TemplateProperty<Commit, Signature> for CommitterProperty {
    fn extract(&self, context: &Commit) -> Signature {
        context.committer().clone()
    }
}

pub struct OpenProperty;

impl<'r> TemplateProperty<Commit, bool> for OpenProperty {
    fn extract(&self, context: &Commit) -> bool {
        context.is_open()
    }
}

pub struct PrunedProperty;

impl TemplateProperty<Commit, bool> for PrunedProperty {
    fn extract(&self, context: &Commit) -> bool {
        context.is_pruned()
    }
}

pub struct CurrentCheckoutProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit, bool> for CurrentCheckoutProperty<'_> {
    fn extract(&self, context: &Commit) -> bool {
        context.id() == self.repo.view().checkout()
    }
}

pub struct GitRefsProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit, String> for GitRefsProperty<'_> {
    fn extract(&self, context: &Commit) -> String {
        let refs: Vec<_> = self
            .repo
            .view()
            .git_refs()
            .iter()
            .filter(|(_name, id)| *id == context.id())
            .map(|(name, _id)| name.clone())
            .collect();
        refs.join(" ")
    }
}

pub struct ObsoleteProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit, bool> for ObsoleteProperty<'_> {
    fn extract(&self, context: &Commit) -> bool {
        self.repo.evolution().is_obsolete(context.id())
    }
}

pub struct OrphanProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit, bool> for OrphanProperty<'_> {
    fn extract(&self, context: &Commit) -> bool {
        self.repo.evolution().is_orphan(context.id())
    }
}

pub struct DivergentProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit, bool> for DivergentProperty<'_> {
    fn extract(&self, context: &Commit) -> bool {
        self.repo.evolution().is_divergent(context.change_id())
    }
}

pub struct ConflictProperty;

impl TemplateProperty<Commit, bool> for ConflictProperty {
    fn extract(&self, context: &Commit) -> bool {
        context.tree().has_conflict()
    }
}

pub struct ConditionalTemplate<'a, C> {
    pub condition: Box<dyn TemplateProperty<C, bool> + 'a>,
    pub true_template: Box<dyn Template<C> + 'a>,
    pub false_template: Option<Box<dyn Template<C> + 'a>>,
}

// TODO: figure out why this lifetime is needed
impl<'a, C> ConditionalTemplate<'a, C> {
    pub fn new(
        condition: Box<dyn TemplateProperty<C, bool> + 'a>,
        true_template: Box<dyn Template<C> + 'a>,
        false_template: Option<Box<dyn Template<C> + 'a>>,
    ) -> Self {
        ConditionalTemplate {
            condition,
            true_template,
            false_template,
        }
    }
}

impl<'a, C> Template<C> for ConditionalTemplate<'a, C> {
    fn format(&self, context: &C, styler: &mut dyn Styler) -> io::Result<()> {
        if self.condition.extract(context) {
            self.true_template.format(context, styler)?;
        } else if let Some(false_template) = &self.false_template {
            false_template.format(context, styler)?;
        }
        Ok(())
    }
}

// TODO: If needed, add a ContextualTemplateFunction where the function also
// gets the context
pub struct TemplateFunction<'a, C, I, O> {
    pub property: Box<dyn TemplateProperty<C, I> + 'a>,
    pub function: Box<dyn Fn(I) -> O + 'a>,
}

// TODO: figure out why this lifetime is needed
impl<'a, C, I, O> TemplateFunction<'a, C, I, O> {
    pub fn new(
        template: Box<dyn TemplateProperty<C, I> + 'a>,
        function: Box<dyn Fn(I) -> O + 'a>,
    ) -> Self {
        TemplateFunction {
            property: template,
            function,
        }
    }
}

impl<'a, C, I, O> TemplateProperty<C, O> for TemplateFunction<'a, C, I, O> {
    fn extract(&self, context: &C) -> O {
        (self.function)(self.property.extract(context))
    }
}

pub struct CommitIdKeyword;

impl CommitIdKeyword {
    pub fn default_format(commit_id: CommitId) -> String {
        commit_id.hex()
    }

    pub fn shortest_format(commit_id: CommitId) -> String {
        // TODO: make this actually be the shortest unambiguous prefix
        commit_id.hex()[..12].to_string()
    }
}

impl<'r> TemplateProperty<Commit, CommitId> for CommitIdKeyword {
    fn extract(&self, context: &Commit) -> CommitId {
        context.id().clone()
    }
}
