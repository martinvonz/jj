use std::collections::HashMap;
use std::io::Write as _;

use bstr::ByteVec as _;
use indexmap::IndexMap;
use indoc::indoc;
use itertools::Itertools;
use jj_lib::backend::CommitId;
use jj_lib::commit::Commit;
use jj_lib::repo::ReadonlyRepo;
use jj_lib::settings::UserSettings;
use thiserror::Error;

use crate::cli_util::edit_temp_file;
use crate::cli_util::short_commit_hash;
use crate::cli_util::WorkspaceCommandTransaction;
use crate::command_error::CommandError;
use crate::formatter::PlainTextFormatter;
use crate::text_util;

/// Cleanup a description by normalizing line endings, and removing leading and
/// trailing blank lines.
fn cleanup_description_lines<I>(lines: I) -> String
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let description = lines
        .into_iter()
        .filter(|line| !line.as_ref().starts_with("JJ: "))
        .fold(String::new(), |acc, line| acc + line.as_ref() + "\n");
    text_util::complete_newline(description.trim_matches('\n'))
}

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

    Ok(cleanup_description_lines(description.lines()))
}

/// Edits the descriptions of the given commits in a single editor session.
pub fn edit_multiple_descriptions(
    tx: &mut WorkspaceCommandTransaction,
    repo: &ReadonlyRepo,
    commits: &[(&CommitId, Commit)],
    settings: &UserSettings,
) -> Result<ParsedBulkEditMessage<CommitId>, CommandError> {
    let mut commits_map = IndexMap::new();
    let mut bulk_message = String::new();

    bulk_message.push_str(indoc! {r#"
        JJ: Enter or edit commit descriptions after the `JJ: describe` lines.
        JJ: Warning:
        JJ: - The text you enter will be lost on a syntax error.
        JJ: - The syntax of the separator lines may change in the future.

    "#});
    for (commit_id, temp_commit) in commits.iter() {
        let commit_hash = short_commit_hash(commit_id);
        bulk_message.push_str("JJ: describe ");
        bulk_message.push_str(&commit_hash);
        bulk_message.push_str(" -------\n");
        commits_map.insert(commit_hash, *commit_id);
        let template = description_template(tx, "", temp_commit)?;
        bulk_message.push_str(&template);
        bulk_message.push('\n');
    }
    bulk_message.push_str("JJ: Lines starting with \"JJ: \" (like this one) will be removed.\n");

    let bulk_message = edit_temp_file(
        "description",
        ".jjdescription",
        repo.repo_path(),
        &bulk_message,
        settings,
    )?;

    Ok(parse_bulk_edit_message(&bulk_message, &commits_map)?)
}

#[derive(Debug)]
pub struct ParsedBulkEditMessage<T> {
    /// The parsed, formatted descriptions.
    pub descriptions: HashMap<T, String>,
    /// Commit IDs that were expected while parsing the edited messages, but
    /// which were not found.
    pub missing: Vec<String>,
    /// Commit IDs that were found multiple times while parsing the edited
    /// messages.
    pub duplicates: Vec<String>,
    /// Commit IDs that were found while parsing the edited messages, but which
    /// were not originally being edited.
    pub unexpected: Vec<String>,
}

#[derive(Debug, Error, PartialEq)]
pub enum ParseBulkEditMessageError {
    #[error(r#"Found the following line without a commit header: "{0}""#)]
    LineWithoutCommitHeader(String),
}

/// Parse the bulk message of edited commit descriptions.
fn parse_bulk_edit_message<T>(
    message: &str,
    commit_ids_map: &IndexMap<String, &T>,
) -> Result<ParsedBulkEditMessage<T>, ParseBulkEditMessageError>
where
    T: Eq + std::hash::Hash + Clone,
{
    let mut descriptions = HashMap::new();
    let mut duplicates = Vec::new();
    let mut unexpected = Vec::new();

    let mut messages: Vec<(&str, Vec<&str>)> = vec![];
    for line in message.lines() {
        if let Some(commit_id_prefix) = line.strip_prefix("JJ: describe ") {
            let commit_id_prefix =
                commit_id_prefix.trim_end_matches(|c: char| c.is_ascii_whitespace() || c == '-');
            messages.push((commit_id_prefix, vec![]));
        } else if let Some((_, lines)) = messages.last_mut() {
            lines.push(line);
        }
        // Do not allow lines without a commit header, except for empty lines or comments.
        else if !line.trim().is_empty() && !line.starts_with("JJ: ") {
            return Err(ParseBulkEditMessageError::LineWithoutCommitHeader(
                line.to_owned(),
            ));
        };
    }

    for (commit_id_prefix, description_lines) in messages {
        let Some(&commit_id) = commit_ids_map.get(commit_id_prefix) else {
            unexpected.push(commit_id_prefix.to_string());
            continue;
        };
        if descriptions.contains_key(commit_id) {
            duplicates.push(commit_id_prefix.to_string());
            continue;
        }
        descriptions.insert(
            commit_id.clone(),
            cleanup_description_lines(&description_lines),
        );
    }

    let missing: Vec<_> = commit_ids_map
        .iter()
        .filter(|(_, commit_id)| !descriptions.contains_key(*commit_id))
        .map(|(commit_id_prefix, _)| commit_id_prefix.to_string())
        .collect();

    Ok(ParsedBulkEditMessage {
        descriptions,
        missing,
        duplicates,
        unexpected,
    })
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

#[cfg(test)]
mod tests {
    use indexmap::indexmap;
    use indoc::indoc;
    use maplit::hashmap;

    use super::parse_bulk_edit_message;
    use crate::description_util::ParseBulkEditMessageError;

    #[test]
    fn test_parse_complete_bulk_edit_message() {
        let result = parse_bulk_edit_message(
            indoc! {"
                JJ: describe 1 -------
                Description 1

                JJ: describe 2
                Description 2

                JJ: describe 3 --
                Description 3
            "},
            &indexmap! {
                "1".to_string() => &1,
                "2".to_string() => &2,
                "3".to_string() => &3,
            },
        )
        .unwrap();
        assert_eq!(
            result.descriptions,
            hashmap! {
                1 => "Description 1\n".to_string(),
                2 => "Description 2\n".to_string(),
                3 => "Description 3\n".to_string(),
            }
        );
        assert!(result.missing.is_empty());
        assert!(result.duplicates.is_empty());
        assert!(result.unexpected.is_empty());
    }

    #[test]
    fn test_parse_bulk_edit_message_with_missing_descriptions() {
        let result = parse_bulk_edit_message(
            indoc! {"
                JJ: describe 1 -------
                Description 1
            "},
            &indexmap! {
                "1".to_string() => &1,
                "2".to_string() => &2,
            },
        )
        .unwrap();
        assert_eq!(
            result.descriptions,
            hashmap! {
                1 => "Description 1\n".to_string(),
            }
        );
        assert_eq!(result.missing, vec!["2".to_string()]);
        assert!(result.duplicates.is_empty());
        assert!(result.unexpected.is_empty());
    }

    #[test]
    fn test_parse_bulk_edit_message_with_duplicate_descriptions() {
        let result = parse_bulk_edit_message(
            indoc! {"
                JJ: describe 1 -------
                Description 1

                JJ: describe 1 -------
                Description 1 (repeated)
            "},
            &indexmap! {
                "1".to_string() => &1,
            },
        )
        .unwrap();
        assert_eq!(
            result.descriptions,
            hashmap! {
                1 => "Description 1\n".to_string(),
            }
        );
        assert!(result.missing.is_empty());
        assert_eq!(result.duplicates, vec!["1".to_string()]);
        assert!(result.unexpected.is_empty());
    }

    #[test]
    fn test_parse_bulk_edit_message_with_unexpected_descriptions() {
        let result = parse_bulk_edit_message(
            indoc! {"
                JJ: describe 1 -------
                Description 1

                JJ: describe 3 -------
                Description 3 (unexpected)
            "},
            &indexmap! {
                "1".to_string() => &1,
            },
        )
        .unwrap();
        assert_eq!(
            result.descriptions,
            hashmap! {
                1 => "Description 1\n".to_string(),
            }
        );
        assert!(result.missing.is_empty());
        assert!(result.duplicates.is_empty());
        assert_eq!(result.unexpected, vec!["3".to_string()]);
    }

    #[test]
    fn test_parse_bulk_edit_message_with_no_header() {
        let result = parse_bulk_edit_message(
            indoc! {"
                Description 1
            "},
            &indexmap! {
                "1".to_string() => &1,
            },
        );
        assert_eq!(
            result.unwrap_err(),
            ParseBulkEditMessageError::LineWithoutCommitHeader("Description 1".to_string())
        );
    }

    #[test]
    fn test_parse_bulk_edit_message_with_comment_before_header() {
        let result = parse_bulk_edit_message(
            indoc! {"
                JJ: Custom comment and empty lines below should be accepted


                JJ: describe 1 -------
                Description 1
            "},
            &indexmap! {
                "1".to_string() => &1,
            },
        )
        .unwrap();
        assert_eq!(
            result.descriptions,
            hashmap! {
                1 => "Description 1\n".to_string(),
            }
        );
        assert!(result.missing.is_empty());
        assert!(result.duplicates.is_empty());
        assert!(result.unexpected.is_empty());
    }
}
