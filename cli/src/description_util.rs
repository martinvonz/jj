use itertools::Itertools;
use jj_lib::commit::Commit;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::merged_tree::MergedTree;
use jj_lib::repo::ReadonlyRepo;
use jj_lib::settings::UserSettings;

use crate::cli_util::{edit_temp_file, WorkspaceCommandHelper};
use crate::command_error::CommandError;
use crate::diff_util::{self, DiffFormat};
use crate::formatter::PlainTextFormatter;
use crate::text_util;
use crate::ui::Ui;

pub fn edit_description(
    repo: &ReadonlyRepo,
    description: &str,
    settings: &UserSettings,
) -> Result<String, CommandError> {
    let description = format!(
        r#"{}
JJ: Lines starting with "JJ: " (like this one) will be removed.
"#,
        description
    );

    let description = edit_temp_file(
        "description",
        ".jjdescription",
        repo.repo_path(),
        &description,
        settings,
    )?;

    // Normalize line ending, remove leading and trailing blank lines.
    let description = description
        .lines()
        .filter(|line| !line.starts_with("JJ: "))
        .join("\n");
    Ok(text_util::complete_newline(description.trim_matches('\n')))
}

pub fn combine_messages(
    repo: &ReadonlyRepo,
    source: &Commit,
    destination: &Commit,
    settings: &UserSettings,
    abandon_source: bool,
) -> Result<String, CommandError> {
    let description = if abandon_source {
        if source.description().is_empty() {
            destination.description().to_string()
        } else if destination.description().is_empty() {
            source.description().to_string()
        } else {
            let combined = "JJ: Enter a description for the combined commit.\n".to_string()
                + "JJ: Description from the destination commit:\n"
                + destination.description()
                + "\nJJ: Description from the source commit:\n"
                + source.description();
            edit_description(repo, &combined, settings)?
        }
    } else {
        destination.description().to_string()
    };
    Ok(description)
}

/// Create a description from a list of paragraphs.
///
/// Based on the Git CLI behavior. See `opt_parse_m()` and `cleanup_mode` in
/// `git/builtin/commit.c`.
pub fn join_message_paragraphs(paragraphs: &[String]) -> String {
    // Ensure each paragraph ends with a newline, then add another newline between
    // paragraphs.
    paragraphs
        .iter()
        .map(|p| text_util::complete_newline(p.as_str()))
        .join("\n")
}

pub fn description_template_for_describe(
    ui: &Ui,
    settings: &UserSettings,
    workspace_command: &WorkspaceCommandHelper,
    commit: &Commit,
) -> Result<String, CommandError> {
    let mut diff_summary_bytes = Vec::new();
    diff_util::show_patch(
        ui,
        &mut PlainTextFormatter::new(&mut diff_summary_bytes),
        workspace_command,
        commit,
        &EverythingMatcher,
        &[DiffFormat::Summary],
    )?;
    let description = if commit.description().is_empty() {
        settings.default_description()
    } else {
        commit.description().to_owned()
    };
    if diff_summary_bytes.is_empty() {
        Ok(description)
    } else {
        Ok(description + "\n" + &diff_summary_to_description(&diff_summary_bytes))
    }
}

pub fn description_template_for_commit(
    ui: &Ui,
    settings: &UserSettings,
    workspace_command: &WorkspaceCommandHelper,
    intro: &str,
    overall_commit_description: &str,
    from_tree: &MergedTree,
    to_tree: &MergedTree,
) -> Result<String, CommandError> {
    let mut diff_summary_bytes = Vec::new();
    diff_util::show_diff(
        ui,
        &mut PlainTextFormatter::new(&mut diff_summary_bytes),
        workspace_command,
        from_tree,
        to_tree,
        &EverythingMatcher,
        &[DiffFormat::Summary],
    )?;
    let mut template_chunks = Vec::new();
    if !intro.is_empty() {
        template_chunks.push(format!("JJ: {intro}\n"));
    }
    template_chunks.push(if overall_commit_description.is_empty() {
        settings.default_description()
    } else {
        overall_commit_description.to_owned()
    });
    if !diff_summary_bytes.is_empty() {
        template_chunks.push("\n".to_owned());
        template_chunks.push(diff_summary_to_description(&diff_summary_bytes));
    }
    Ok(template_chunks.concat())
}

pub fn diff_summary_to_description(bytes: &[u8]) -> String {
    let text = std::str::from_utf8(bytes).expect(
        "Summary diffs and repo paths must always be valid UTF8.",
        // Double-check this assumption for diffs that include file content.
    );
    "JJ: This commit contains the following changes:\n".to_owned()
        + &textwrap::indent(text, "JJ:     ")
}
