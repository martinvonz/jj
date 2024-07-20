use std::io::Write as _;

use bstr::ByteVec as _;
use itertools::Itertools;
use jj_lib::commit::Commit;
use jj_lib::repo::ReadonlyRepo;
use jj_lib::settings::UserSettings;

use crate::cli_util::{edit_temp_file, WorkspaceCommandTransaction};
use crate::command_error::CommandError;
use crate::formatter::PlainTextFormatter;
use crate::text_util;

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

/// Combines the descriptions from the input commits. If only one is non-empty,
/// then that one is used. Otherwise we concatenate the messages and ask the
/// user to edit the result in their editor.
pub fn combine_messages(
    repo: &ReadonlyRepo,
    sources: &[&Commit],
    destination: &Commit,
    settings: &UserSettings,
) -> Result<String, CommandError> {
    let non_empty = sources
        .iter()
        .chain(std::iter::once(&destination))
        .filter(|c| !c.description().is_empty())
        .take(2)
        .collect_vec();
    match *non_empty.as_slice() {
        [] => {
            return Ok(String::new());
        }
        [commit] => {
            return Ok(commit.description().to_owned());
        }
        _ => {}
    }
    // Produce a combined description with instructions for the user to edit.
    // Include empty descriptins too, so the user doesn't have to wonder why they
    // only see 2 descriptions when they combined 3 commits.
    let mut combined = "JJ: Enter a description for the combined commit.".to_string();
    combined.push_str("\nJJ: Description from the destination commit:\n");
    combined.push_str(destination.description());
    for commit in sources {
        combined.push_str("\nJJ: Description from source commit:\n");
        combined.push_str(commit.description());
    }
    edit_description(repo, &combined, settings)
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

/// Renders commit description template, which will be edited by user.
pub fn description_template(
    tx: &WorkspaceCommandTransaction,
    intro: &str,
    commit: &Commit,
) -> Result<String, CommandError> {
    // TODO: Should "ui.default-description" be deprecated?
    // We might want default description templates per command instead. For
    // example, "backout_description" template will be rendered against the
    // commit to be backed out, and the generated description could be set
    // without spawning editor.

    // Named as "draft" because the output can contain "JJ: " comment lines.
    let template_key = "templates.draft_commit_description";
    let template_text = tx.settings().config().get_string(template_key)?;
    let template = tx.parse_commit_template(&template_text)?;

    let mut output = Vec::new();
    if !intro.is_empty() {
        writeln!(output, "JJ: {intro}").unwrap();
    }
    template
        .format(commit, &mut PlainTextFormatter::new(&mut output))
        .expect("write() to vec backed formatter should never fail");
    // Template output is usually UTF-8, but it can contain file content.
    Ok(output.into_string_lossy())
}
