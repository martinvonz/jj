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

use std::cmp::{max, min};
use std::io;

use itertools::Itertools;
use jujutsu_lib::backend::{ObjectId, Signature, Timestamp};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::op_store::WorkspaceId;
use jujutsu_lib::repo::RepoRef;
use jujutsu_lib::rewrite::merge_commit_trees;

use crate::formatter::Formatter;
use crate::time_util;

pub trait Template<C> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()>;
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

pub struct LabelTemplate<T> {
    content: T,
    labels: Vec<String>,
}

impl<T> LabelTemplate<T> {
    pub fn new<C>(content: T, labels: Vec<String>) -> Self
    where
        T: Template<C>,
    {
        LabelTemplate { content, labels }
    }
}

impl<C, T: Template<C>> Template<C> for LabelTemplate<T> {
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

pub struct DynamicLabelTemplate<T, F> {
    content: T,
    label_property: F,
}

impl<T, F> DynamicLabelTemplate<T, F> {
    pub fn new<C>(content: T, label_property: F) -> Self
    where
        T: Template<C>,
        F: Fn(&C) -> Vec<String>,
    {
        DynamicLabelTemplate {
            content,
            label_property,
        }
    }
}

impl<C, T, F> Template<C> for DynamicLabelTemplate<T, F>
where
    T: Template<C>,
    F: Fn(&C) -> Vec<String>,
{
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        let labels = (self.label_property)(context);
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

pub struct ListTemplate<T>(pub Vec<T>);

impl<C, T: Template<C>> Template<C> for ListTemplate<T> {
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

impl<C, P: TemplateProperty<C> + ?Sized> TemplateProperty<C> for Box<P> {
    type Output = <P as TemplateProperty<C>>::Output;

    fn extract(&self, context: &C) -> Self::Output {
        <P as TemplateProperty<C>>::extract(self, context)
    }
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

pub struct GitHeadProperty<'a> {
    repo: RepoRef<'a>,
}

impl<'a> GitHeadProperty<'a> {
    pub fn new(repo: RepoRef<'a>) -> Self {
        Self { repo }
    }
}

impl TemplateProperty<Commit> for GitHeadProperty<'_> {
    type Output = String;

    fn extract(&self, context: &Commit) -> String {
        match self.repo.view().git_head() {
            Some(ref_target) if ref_target.has_add(context.id()) => {
                if ref_target.is_conflict() {
                    "HEAD@git?".to_string()
                } else {
                    "HEAD@git".to_string()
                }
            }
            _ => "".to_string(),
        }
    }
}

pub struct DivergentProperty<'a> {
    repo: RepoRef<'a>,
}

impl<'a> DivergentProperty<'a> {
    pub fn new(repo: RepoRef<'a>) -> Self {
        DivergentProperty { repo }
    }
}

impl TemplateProperty<Commit> for DivergentProperty<'_> {
    type Output = bool;

    fn extract(&self, context: &Commit) -> Self::Output {
        // The given commit could be hidden in e.g. obslog.
        let maybe_entries = self.repo.resolve_change_id(context.change_id());
        maybe_entries.map_or(0, |entries| entries.len()) > 1
    }
}

pub struct ConflictProperty;

impl TemplateProperty<Commit> for ConflictProperty {
    type Output = bool;

    fn extract(&self, context: &Commit) -> Self::Output {
        context.tree().has_conflict()
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

/// Type-erased `CommitId`/`ChangeId`.
#[derive(Clone)]
pub struct CommitOrChangeId<'a> {
    repo: RepoRef<'a>,
    id_bytes: Vec<u8>,
}

impl CommitOrChangeId<'_> {
    pub fn as_bytes(&self) -> &[u8] {
        &self.id_bytes
    }

    pub fn hex(&self) -> String {
        hex::encode(&self.id_bytes)
    }

    pub fn short(&self) -> String {
        let mut hex = self.hex();
        hex.truncate(12);
        hex
    }

    pub fn short_prefix_and_brackets(&self) -> String {
        highlight_shortest_prefix_brackets(self, 12)
    }
}

impl Template<()> for CommitOrChangeId<'_> {
    fn format(&self, _: &(), formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(&self.hex())
    }
}

/// This function supports short `total_len` by ensuring that the entire
/// unique prefix is always printed
fn extract_entire_prefix_and_trimmed_tail(
    s: &str,
    prefix_len: usize,
    total_len: usize,
) -> (&str, &str) {
    let prefix_len = min(prefix_len, s.len());
    let total_len = max(prefix_len, min(total_len, s.len()));
    (&s[0..prefix_len], &s[prefix_len..total_len])
}

#[cfg(test)]
mod tests {
    use super::extract_entire_prefix_and_trimmed_tail;

    #[test]
    fn test_prefix() {
        let s = "0123456789";
        insta::assert_debug_snapshot!(extract_entire_prefix_and_trimmed_tail(s, 2, 5), @r###"
        (
            "01",
            "234",
        )
        "###);
        insta::assert_debug_snapshot!(extract_entire_prefix_and_trimmed_tail(s, 2, 11), @r###"
        (
            "01",
            "23456789",
        )
        "###);
        insta::assert_debug_snapshot!(extract_entire_prefix_and_trimmed_tail(s, 11, 2), @r###"
        (
            "0123456789",
            "",
        )
        "###);
        insta::assert_debug_snapshot!(extract_entire_prefix_and_trimmed_tail(s, 11, 11), @r###"
        (
            "0123456789",
            "",
        )
        "###);
    }
}

fn highlight_shortest_prefix_brackets(id: &CommitOrChangeId, total_len: usize) -> String {
    let hex = id.hex();
    let (prefix, rest) = extract_entire_prefix_and_trimmed_tail(
        &hex,
        id.repo.shortest_unique_id_prefix_len(id.as_bytes()),
        total_len - 2,
    );
    if rest.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}[{rest}]")
    }
}

pub struct CommitOrChangeIdShort;

impl TemplateProperty<CommitOrChangeId<'_>> for CommitOrChangeIdShort {
    type Output = String;

    fn extract(&self, context: &CommitOrChangeId) -> Self::Output {
        context.short()
    }
}

pub struct CommitOrChangeIdShortPrefixAndBrackets;

impl TemplateProperty<CommitOrChangeId<'_>> for CommitOrChangeIdShortPrefixAndBrackets {
    type Output = String;

    fn extract(&self, context: &CommitOrChangeId) -> Self::Output {
        context.short_prefix_and_brackets()
    }
}

pub struct CommitIdProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl<'a> TemplateProperty<Commit> for CommitIdProperty<'a> {
    type Output = CommitOrChangeId<'a>;

    fn extract(&self, context: &Commit) -> Self::Output {
        CommitOrChangeId {
            repo: self.repo,
            id_bytes: context.id().to_bytes(),
        }
    }
}

pub struct ChangeIdProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl<'a> TemplateProperty<Commit> for ChangeIdProperty<'a> {
    type Output = CommitOrChangeId<'a>;

    fn extract(&self, context: &Commit) -> Self::Output {
        CommitOrChangeId {
            repo: self.repo,
            id_bytes: context.change_id().to_bytes(),
        }
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
