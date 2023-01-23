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

use std::collections::{HashMap, HashSet};
use std::io;
use std::ops::AddAssign;

use itertools::Itertools;
use jujutsu_lib::backend::{ChangeId, ObjectId, Signature, Timestamp};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::op_store::WorkspaceId;
use jujutsu_lib::repo::RepoRef;
use jujutsu_lib::revset::RevsetExpression;
use jujutsu_lib::rewrite::merge_commit_trees;

use crate::formatter::Formatter;
use crate::time_util;

pub trait Template<C> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()>;
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

impl Template<()> for Timestamp {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(&time_util::format_absolute_timestamp(self))
    }
}

impl Template<()> for bool {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(if *self { "true" } else { "false" })
    }
}

// TODO: figure out why this lifetime is needed
pub struct LabelTemplate<'a, C> {
    content: Box<dyn Template<C> + 'a>,
    labels: Vec<String>,
}

impl<'a, C> LabelTemplate<'a, C> {
    pub fn new(content: Box<dyn Template<C> + 'a>, labels: Vec<String>) -> Self {
        LabelTemplate { content, labels }
    }
}

impl<'a, C> Template<C> for LabelTemplate<'a, C> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        for label in &self.labels {
            formatter.push_label(label)?;
        }
        self.content.format(context, formatter)?;
        for _label in &self.labels {
            formatter.pop_label()?;
        }
        Ok(())
    }
}

pub type DynamicLabelFunction<'a, C> = Box<dyn Fn(&C) -> Vec<String> + 'a>;

// TODO: figure out why this lifetime is needed
pub struct DynamicLabelTemplate<'a, C> {
    content: Box<dyn Template<C> + 'a>,
    label_property: DynamicLabelFunction<'a, C>,
}

impl<'a, C> DynamicLabelTemplate<'a, C> {
    pub fn new(
        content: Box<dyn Template<C> + 'a>,
        label_property: DynamicLabelFunction<'a, C>,
    ) -> Self {
        DynamicLabelTemplate {
            content,
            label_property,
        }
    }
}

impl<'a, C> Template<C> for DynamicLabelTemplate<'a, C> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        let labels = self.label_property.as_ref()(context);
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

// TODO: figure out why this lifetime is needed
pub struct ListTemplate<'a, C>(pub Vec<Box<dyn Template<C> + 'a>>);

impl<'a, C> Template<C> for ListTemplate<'a, C> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        for template in &self.0 {
            template.format(context, formatter)?
        }
        Ok(())
    }
}

pub trait TemplateProperty<C> {
    type Output;

    fn extract(&self, context: &C) -> Self::Output;
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

/// Adapter to extract context-less template value from property for displaying.
pub struct FormattablePropertyTemplate<'a, C, O> {
    property: Box<dyn TemplateProperty<C, Output = O> + 'a>,
}

impl<'a, C, O> FormattablePropertyTemplate<'a, C, O> {
    pub fn new(property: Box<dyn TemplateProperty<C, Output = O> + 'a>) -> Self {
        FormattablePropertyTemplate { property }
    }
}

impl<C, O> Template<C> for FormattablePropertyTemplate<'_, C, O>
where
    O: Template<()>,
{
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        let template = self.property.extract(context);
        template.format(&(), formatter)
    }
}

pub struct DescriptionProperty;

impl TemplateProperty<Commit> for DescriptionProperty {
    type Output = String;

    fn extract(&self, context: &Commit) -> Self::Output {
        match context.description() {
            s if s.is_empty() => "(no description set)\n".to_owned(),
            s if s.ends_with('\n') => s.to_owned(),
            s => format!("{s}\n"),
        }
    }
}

pub struct AuthorProperty;

impl TemplateProperty<Commit> for AuthorProperty {
    type Output = Signature;

    fn extract(&self, context: &Commit) -> Self::Output {
        context.author().clone()
    }
}

pub struct CommitterProperty;

impl TemplateProperty<Commit> for CommitterProperty {
    type Output = Signature;

    fn extract(&self, context: &Commit) -> Self::Output {
        context.committer().clone()
    }
}

pub struct WorkingCopiesProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit> for WorkingCopiesProperty<'_> {
    type Output = String;

    fn extract(&self, context: &Commit) -> Self::Output {
        let wc_commit_ids = self.repo.view().wc_commit_ids();
        if wc_commit_ids.len() <= 1 {
            return "".to_string();
        }
        let mut names = vec![];
        for (workspace_id, wc_commit_id) in wc_commit_ids.iter().sorted() {
            if wc_commit_id == context.id() {
                names.push(format!("{}@", workspace_id.as_str()));
            }
        }
        names.join(" ")
    }
}

pub struct IsWorkingCopyProperty<'a> {
    pub repo: RepoRef<'a>,
    pub workspace_id: WorkspaceId,
}

impl TemplateProperty<Commit> for IsWorkingCopyProperty<'_> {
    type Output = bool;

    fn extract(&self, context: &Commit) -> Self::Output {
        Some(context.id()) == self.repo.view().get_wc_commit_id(&self.workspace_id)
    }
}

pub struct BranchProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit> for BranchProperty<'_> {
    type Output = String;

    fn extract(&self, context: &Commit) -> Self::Output {
        let mut names = vec![];
        for (branch_name, branch_target) in self.repo.view().branches() {
            let local_target = branch_target.local_target.as_ref();
            if let Some(local_target) = local_target {
                if local_target.has_add(context.id()) {
                    if local_target.is_conflict() {
                        names.push(format!("{branch_name}??"));
                    } else if branch_target
                        .remote_targets
                        .values()
                        .any(|remote_target| remote_target != local_target)
                    {
                        names.push(format!("{branch_name}*"));
                    } else {
                        names.push(branch_name.clone());
                    }
                }
            }
            for (remote_name, remote_target) in &branch_target.remote_targets {
                if Some(remote_target) != local_target && remote_target.has_add(context.id()) {
                    if remote_target.is_conflict() {
                        names.push(format!("{branch_name}@{remote_name}?"));
                    } else {
                        names.push(format!("{branch_name}@{remote_name}"));
                    }
                }
            }
        }
        names.join(" ")
    }
}

pub struct TagProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit> for TagProperty<'_> {
    type Output = String;

    fn extract(&self, context: &Commit) -> Self::Output {
        let mut names = vec![];
        for (tag_name, target) in self.repo.view().tags() {
            if target.has_add(context.id()) {
                if target.is_conflict() {
                    names.push(format!("{tag_name}?"));
                } else {
                    names.push(tag_name.clone());
                }
            }
        }
        names.join(" ")
    }
}

pub struct GitRefsProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit> for GitRefsProperty<'_> {
    type Output = String;

    fn extract(&self, context: &Commit) -> Self::Output {
        // TODO: We should keep a map from commit to ref names so we don't have to walk
        // all refs here.
        let mut names = vec![];
        for (name, target) in self.repo.view().git_refs() {
            if target.has_add(context.id()) {
                if target.is_conflict() {
                    names.push(format!("{name}?"));
                } else {
                    names.push(name.clone());
                }
            }
        }
        names.join(" ")
    }
}

pub struct IsGitHeadProperty<'a> {
    repo: RepoRef<'a>,
}

impl<'a> IsGitHeadProperty<'a> {
    pub fn new(repo: RepoRef<'a>) -> Self {
        Self { repo }
    }
}

impl TemplateProperty<Commit> for IsGitHeadProperty<'_> {
    type Output = bool;

    fn extract(&self, context: &Commit) -> Self::Output {
        self.repo.view().git_head().as_ref() == Some(context.id())
    }
}

pub struct DivergentProperty {
    divergent_changes: HashSet<ChangeId>,
}

impl DivergentProperty {
    pub fn new(repo: RepoRef) -> Self {
        // TODO: Create a persistent index from change id to commit ids.
        let mut commit_count_by_change: HashMap<ChangeId, i32> = HashMap::new();
        for index_entry in RevsetExpression::all().evaluate(repo, None).unwrap().iter() {
            let change_id = index_entry.change_id();
            commit_count_by_change
                .entry(change_id)
                .or_default()
                .add_assign(1);
        }
        let mut divergent_changes = HashSet::new();
        for (change_id, count) in commit_count_by_change {
            if count > 1 {
                divergent_changes.insert(change_id);
            }
        }
        Self { divergent_changes }
    }
}

impl TemplateProperty<Commit> for DivergentProperty {
    type Output = bool;

    fn extract(&self, context: &Commit) -> Self::Output {
        self.divergent_changes.contains(context.change_id())
    }
}

pub struct ConflictProperty;

impl TemplateProperty<Commit> for ConflictProperty {
    type Output = bool;

    fn extract(&self, context: &Commit) -> Self::Output {
        context.tree().has_conflict()
    }
}

pub struct ConditionalTemplate<'a, C> {
    pub condition: Box<dyn TemplateProperty<C, Output = bool> + 'a>,
    pub true_template: Box<dyn Template<C> + 'a>,
    pub false_template: Option<Box<dyn Template<C> + 'a>>,
}

// TODO: figure out why this lifetime is needed
impl<'a, C> ConditionalTemplate<'a, C> {
    pub fn new(
        condition: Box<dyn TemplateProperty<C, Output = bool> + 'a>,
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
pub struct TemplateFunction<'a, C, I, O> {
    pub property: Box<dyn TemplateProperty<C, Output = I> + 'a>,
    pub function: Box<dyn Fn(I) -> O + 'a>,
}

// TODO: figure out why this lifetime is needed
impl<'a, C, I, O> TemplateFunction<'a, C, I, O> {
    pub fn new(
        template: Box<dyn TemplateProperty<C, Output = I> + 'a>,
        function: Box<dyn Fn(I) -> O + 'a>,
    ) -> Self {
        TemplateFunction {
            property: template,
            function,
        }
    }
}

impl<'a, C, I, O> TemplateProperty<C> for TemplateFunction<'a, C, I, O> {
    type Output = O;

    fn extract(&self, context: &C) -> Self::Output {
        (self.function)(self.property.extract(context))
    }
}

/// Type-erased `CommitId`/`ChangeId`.
#[derive(Debug, Clone)]
pub struct CommitOrChangeId(Vec<u8>);

impl CommitOrChangeId {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn hex(&self) -> String {
        hex::encode(&self.0)
    }

    pub fn short(&self) -> String {
        let mut hex = self.hex();
        hex.truncate(12);
        hex
    }

    pub fn short_prefix_and_brackets(&self, repo: RepoRef) -> String {
        highlight_shortest_prefix(self, 12, repo)
    }
}

impl Template<()> for CommitOrChangeId {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(&self.hex())
    }
}

fn highlight_shortest_prefix(id: &CommitOrChangeId, total_len: usize, repo: RepoRef) -> String {
    let prefix_len = repo
        .base_repo()
        .shortest_unique_prefix_length(id.as_bytes());
    let mut hex = id.hex();
    if prefix_len < total_len - 2 {
        format!(
            "{}[{}]",
            &hex[0..prefix_len],
            &hex[prefix_len..total_len - 2]
        )
    } else {
        hex.truncate(total_len);
        hex
    }
}

pub struct CommitOrChangeIdShort<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<CommitOrChangeId> for CommitOrChangeIdShort<'_> {
    type Output = String;

    fn extract(&self, context: &CommitOrChangeId) -> Self::Output {
        context.short()
    }
}

pub struct CommitOrChangeIdShortPrefixAndBrackets<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<CommitOrChangeId> for CommitOrChangeIdShortPrefixAndBrackets<'_> {
    type Output = String;

    fn extract(&self, context: &CommitOrChangeId) -> Self::Output {
        context.short_prefix_and_brackets(self.repo)
    }
}

pub struct CommitIdProperty;

impl TemplateProperty<Commit> for CommitIdProperty {
    type Output = CommitOrChangeId;

    fn extract(&self, context: &Commit) -> Self::Output {
        CommitOrChangeId(context.id().to_bytes())
    }
}

pub struct ChangeIdProperty;

impl TemplateProperty<Commit> for ChangeIdProperty {
    type Output = CommitOrChangeId;

    fn extract(&self, context: &Commit) -> Self::Output {
        CommitOrChangeId(context.change_id().to_bytes())
    }
}

pub struct SignatureTimestamp;

impl TemplateProperty<Signature> for SignatureTimestamp {
    type Output = Timestamp;

    fn extract(&self, context: &Signature) -> Self::Output {
        context.timestamp.clone()
    }
}

pub struct EmptyProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit> for EmptyProperty<'_> {
    type Output = bool;

    fn extract(&self, context: &Commit) -> Self::Output {
        context.tree().id() == merge_commit_trees(self.repo, &context.parents()).id()
    }
}
