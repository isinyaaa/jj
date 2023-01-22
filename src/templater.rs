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

use std::borrow::BorrowMut;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::ops::AddAssign;

use itertools::Itertools;
use jujutsu_lib::backend::{ChangeId, ObjectId, Signature, Timestamp};
use jujutsu_lib::commit::Commit;
use jujutsu_lib::op_store::WorkspaceId;
use jujutsu_lib::repo::RepoRef;
use jujutsu_lib::revset::RevsetExpression;
use jujutsu_lib::rewrite::merge_commit_trees;

use crate::formatter::{Formatter, PlainTextFormatter};

pub trait Template<C> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()>;
}

// TODO: Extract a trait for this type?
pub struct TemplateFormatter<'f, 't: 'f, C> {
    template: Box<dyn Template<C> + 't>,
    formatter: &'f mut dyn Formatter,
}

impl<'f, 't: 'f, C> TemplateFormatter<'f, 't, C> {
    pub fn new(template: Box<dyn Template<C> + 't>, formatter: &'f mut dyn Formatter) -> Self {
        TemplateFormatter {
            template,
            formatter,
        }
    }

    pub fn format<'c, 'a: 'c>(&'a mut self, context: &'c C) -> io::Result<()> {
        self.template.format(context, self.formatter.borrow_mut())
    }
}

pub struct LiteralTemplate(pub String);

impl<C> Template<C> for LiteralTemplate {
    fn format(&self, _context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        formatter.write_str(&self.0)
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
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        let text = self.property.extract(context);
        formatter.write_str(&text)
    }
}

#[derive(Debug, Clone)]
pub enum FormattedString {
    Plain(String),
    Labeled {
        str: Box<FormattedString>,
        labels: VecDeque<String>,
    },
    Concat(Vec<FormattedString>),
}
impl FormattedString {
    pub fn to_template(self) -> Box<dyn Template<()>> {
        match self {
            FormattedString::Plain(s) => Box::new(LiteralTemplate(s)),
            FormattedString::Labeled { str, labels } => {
                Box::new(LabelTemplate::new(str.to_template(), labels.into()))
            }
            FormattedString::Concat(strs) => Box::new(ListTemplate(
                strs.into_iter().map(|fs| fs.to_template()).collect_vec(),
            )),
        }
    }
    pub fn format(self, formatter: &mut dyn Formatter) -> io::Result<()> {
        self.to_template().format(&(), formatter)
    }

    /// Prepends `label` to the list of labels of every portion of this
    /// formatted string
    pub fn prepend_label(self, label: String) -> Self {
        match self {
            Self::Labeled { str, mut labels } => {
                labels.push_front(label);
                Self::Labeled { str, labels }
            }
            _ => Self::Labeled {
                str: Box::new(self),
                labels: VecDeque::from(vec![label]),
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            FormattedString::Plain(s) => s.is_empty(),
            FormattedString::Labeled { str, .. } => str.is_empty(),
            FormattedString::Concat(strs) => strs.iter().all(|s| s.is_empty()),
        }
    }

    pub fn concat(strings: Vec<FormattedString>) -> Self {
        Self::Concat(strings)
    }

    pub fn join(strs: Vec<FormattedString>, with: FormattedString) -> FormattedString {
        FormattedString::concat(itertools::intersperse(strs.into_iter(), with).collect_vec())
    }
}
impl Default for FormattedString {
    fn default() -> FormattedString {
        Self::from("".to_string())
    }
}
impl From<String> for FormattedString {
    fn from(s: String) -> FormattedString {
        Self::Plain(s)
    }
}
impl From<&str> for FormattedString {
    fn from(s: &str) -> FormattedString {
        Self::Plain(s.to_string())
    }
}
impl From<FormattedString> for String {
    fn from(fs: FormattedString) -> String {
        let mut result = vec![];
        fs.to_template()
            .format(&(), &mut PlainTextFormatter::new(&mut result))
            .unwrap();
        String::from_utf8_lossy(&result).to_string()
    }
}
pub struct FormattedStringPropertyTemplate<'a, C> {
    pub property: Box<dyn TemplateProperty<C, FormattedString> + 'a>,
}

impl<'a, C> Template<C> for FormattedStringPropertyTemplate<'a, C> {
    fn format(&self, context: &C, formatter: &mut dyn Formatter) -> io::Result<()> {
        let text = self.property.extract(context);
        text.format(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formatter::ColorFormatter;

    fn config_from_string(text: &str) -> config::Config {
        config::Config::builder()
            .add_source(config::File::from_str(text, config::FileFormat::Toml))
            .build()
            .unwrap()
    }

    #[test]
    fn test_color_formatter_sibling() {
        // A partial match on one rule does not eliminate other rules.
        let config = config_from_string(
            r#"
        colors."first second" = "blue"
        colors."second first" = "red"
        colors."second" = "green"
        "#,
        );
        let mut output: Vec<u8> = vec![];
        let mut formatter = ColorFormatter::for_config(&mut output, &config);
        let input = vec![
            FormattedString::from(r#"prepend_label("first") => no color"#)
                .prepend_label("first".to_string()),
            FormattedString::from(r#"prepend_label("second") => green"#)
                .prepend_label("second".to_string()),
            FormattedString::from(r#"prepend_label("first").prepend_label("second") => red"#)
                .prepend_label("first".to_string())
                .prepend_label("second".to_string()),
            FormattedString::from(r#"prepend_label("second").prepend_label("first") => blue"#)
                .prepend_label("second".to_string())
                .prepend_label("first".to_string()),
        ];
        FormattedString::join(input, FormattedString::from("\n"))
            .format(&mut formatter)
            .unwrap(); // TODO: Replace concat with join
        insta::assert_snapshot!(String::from_utf8(output).unwrap(),
        @r###"
        prepend_label("first") => no color
        [38;5;2mprepend_label("second") => green[39m
        [38;5;1mprepend_label("first").prepend_label("second") => red[39m
        [38;5;4mprepend_label("second").prepend_label("first") => blue[39m
        "###);
    }
}

pub struct DescriptionProperty;

impl TemplateProperty<Commit, String> for DescriptionProperty {
    fn extract(&self, context: &Commit) -> String {
        match context.description() {
            s if s.is_empty() => "(no description set)\n".to_owned(),
            s if s.ends_with('\n') => s.to_owned(),
            s => format!("{s}\n"),
        }
    }
}

pub struct AuthorProperty;

impl TemplateProperty<Commit, Signature> for AuthorProperty {
    fn extract(&self, context: &Commit) -> Signature {
        context.author().clone()
    }
}

pub struct CommitterProperty;

impl TemplateProperty<Commit, Signature> for CommitterProperty {
    fn extract(&self, context: &Commit) -> Signature {
        context.committer().clone()
    }
}

pub struct WorkingCopiesProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit, String> for WorkingCopiesProperty<'_> {
    fn extract(&self, context: &Commit) -> String {
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

impl TemplateProperty<Commit, bool> for IsWorkingCopyProperty<'_> {
    fn extract(&self, context: &Commit) -> bool {
        Some(context.id()) == self.repo.view().get_wc_commit_id(&self.workspace_id)
    }
}

pub struct BranchProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit, FormattedString> for BranchProperty<'_> {
    fn extract(&self, context: &Commit) -> FormattedString {
        let mut names = vec![];
        for (branch_name, branch_target) in self.repo.view().branches() {
            let local_target = branch_target.local_target.as_ref();
            if let Some(local_target) = local_target {
                if local_target.has_add(context.id()) {
                    let mut pieces = vec![];
                    pieces.push(FormattedString::from(branch_name.clone()));
                    if local_target.is_conflict() {
                        pieces.push(
                            FormattedString::from("??".to_string())
                                .prepend_label("divergent".to_string()),
                        );
                    } else if branch_target
                        .remote_targets
                        .values()
                        .any(|remote_target| remote_target != local_target)
                    {
                        pieces.push(
                            FormattedString::from("*".to_string())
                                .prepend_label("diff".to_string())
                                .prepend_label("remote".to_string()),
                        );
                    }
                    names.push(FormattedString::concat(pieces));
                }
            }
            for (remote_name, remote_target) in &branch_target.remote_targets {
                if Some(remote_target) != local_target && remote_target.has_add(context.id()) {
                    let mut pieces = vec![
                        FormattedString::from(branch_name.clone()),
                        FormattedString::from("@").prepend_label("separator".to_string()),
                        // TODO: This doesn't seem to affect HEAD@git
                        FormattedString::from(remote_name.clone())
                            .prepend_label("remote".to_string()),
                    ];
                    if remote_target.is_conflict() {
                        pieces.push(
                            FormattedString::from("??").prepend_label("divergent".to_string()),
                        );
                    }
                    names.push(FormattedString::concat(pieces));
                }
            }
        }
        FormattedString::join(names, FormattedString::from(" "))
    }
}

pub struct TagProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit, String> for TagProperty<'_> {
    fn extract(&self, context: &Commit) -> String {
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

impl TemplateProperty<Commit, String> for GitRefsProperty<'_> {
    fn extract(&self, context: &Commit) -> String {
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

impl TemplateProperty<Commit, bool> for IsGitHeadProperty<'_> {
    fn extract(&self, context: &Commit) -> bool {
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

impl TemplateProperty<Commit, bool> for DivergentProperty {
    fn extract(&self, context: &Commit) -> bool {
        self.divergent_changes.contains(context.change_id())
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

impl TemplateProperty<CommitOrChangeId, String> for CommitOrChangeIdShort<'_> {
    fn extract(&self, context: &CommitOrChangeId) -> String {
        context.short()
    }
}

pub struct CommitOrChangeIdShortPrefixAndBrackets<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<CommitOrChangeId, String> for CommitOrChangeIdShortPrefixAndBrackets<'_> {
    fn extract(&self, context: &CommitOrChangeId) -> String {
        context.short_prefix_and_brackets(self.repo)
    }
}

pub struct CommitIdProperty;

impl TemplateProperty<Commit, CommitOrChangeId> for CommitIdProperty {
    fn extract(&self, context: &Commit) -> CommitOrChangeId {
        CommitOrChangeId(context.id().to_bytes())
    }
}

pub struct ChangeIdProperty;

impl TemplateProperty<Commit, CommitOrChangeId> for ChangeIdProperty {
    fn extract(&self, context: &Commit) -> CommitOrChangeId {
        CommitOrChangeId(context.change_id().to_bytes())
    }
}

pub struct SignatureTimestamp;

impl TemplateProperty<Signature, Timestamp> for SignatureTimestamp {
    fn extract(&self, context: &Signature) -> Timestamp {
        context.timestamp.clone()
    }
}

pub struct EmptyProperty<'a> {
    pub repo: RepoRef<'a>,
}

impl TemplateProperty<Commit, bool> for EmptyProperty<'_> {
    fn extract(&self, context: &Commit) -> bool {
        context.tree().id() == merge_commit_trees(self.repo, &context.parents()).id()
    }
}
