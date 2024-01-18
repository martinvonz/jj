// Copyright 2024 The Jujutsu Authors
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

//! Parsing footer lines from commit messages.

/// A key-value pair representing a footer line in a commit message, of the
/// form `Key: Value`.
#[derive(Debug, PartialEq, Clone)]
pub struct FooterEntry(pub String, pub String);

/// Parse the footer lines from a commit message; these are simple key-value
/// pairs, separated by a colon, describing extra information in a commit
/// message; an example is the following:
///
/// ```text
/// chore: fix bug 1234
///
/// Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod
/// tempor incididunt ut labore et dolore magna aliqua.
///
/// Co-authored-by: Alice <alice@example.com>
/// Co-authored-by: Bob <bob@example.com>
/// Reviewed-by: Charlie <charlie@example.com>
/// Change-Id: I1234567890abcdef1234567890abcdef12345678
/// ```
///
/// In this case, there are four footer lines: two `Co-authored-by` lines, one
/// `Reviewed-by` line, and one `Change-Id` line.
pub fn get_footer_lines(body: &str) -> Vec<FooterEntry> {
    // a footer always comes at the end of a message; we can split the message
    // by newline, but we need to immediately reverse the order of the lines
    // to ensure we parse the footer in an unambiguous manner; this avoids cases
    // where a colon in the body of the message is mistaken for a footer line

    let lines = body.trim().lines().rev().collect::<Vec<&str>>();

    // short-circuit if there is only 1 line; this avoids a case where a commit
    // with a single-line description like 'cli: fix bug' does not have a
    // footer, but would otherwise be mistaken for a footer line
    if lines.len() <= 1 {
        return vec![];
    }

    let mut footer: Vec<FooterEntry> = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(": ") {
            let key = key.trim();
            let value = value.trim();
            footer.push(FooterEntry(key.to_string(), value.to_string()));
        } else {
            break;
        }
    }

    // reverse the insert order, since we parsed the footer in reverse
    footer.reverse();

    if footer.is_empty() {
        vec![]
    } else {
        footer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_footer_lines() {
        let body = r#"chore: fix bug 1234

Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed
do eiusmod tempor incididunt ut labore et dolore magna aliqua.

Acked-by: Austin Seipp <aseipp@pobox.com>
Reviewed-by: Yuya Nishihara <yuya@tcha.org>
Reviewed-by: Martin von Zweigbergk <martinvonz@gmail.com>
Change-Id: I1234567890abcdef1234567890abcdef12345678"#;

        let footer = get_footer_lines(body);
        assert_eq!(footer.len(), 4);

        assert_eq!(footer.first().unwrap().1, "Austin Seipp <aseipp@pobox.com>");
        assert_eq!(footer.get(1).unwrap().1, "Yuya Nishihara <yuya@tcha.org>");
        assert_eq!(
            footer.get(2).unwrap().1,
            "Martin von Zweigbergk <martinvonz@gmail.com>"
        );
        assert_eq!(
            footer.get(3).unwrap().1,
            "I1234567890abcdef1234567890abcdef12345678"
        );
    }

    #[test]
    fn test_footer_lines_with_colon_in_body() {
        let body = r#"chore: fix bug 1234

Summary: Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod
tempor incididunt ut labore et dolore magna aliqua.

Change-Id: I1234567890abcdef1234567890abcdef12345678"#;

        let footer = get_footer_lines(body);

        // should only have Change-Id
        assert_eq!(footer.len(), 1);
        assert_eq!(footer.first().unwrap().0, "Change-Id");
    }

    #[test]
    fn test_footer_lines_with_single_line_description() {
        let body = r#"chore: fix bug 1234"#;
        let footer = get_footer_lines(body);
        assert_eq!(footer.len(), 0);
    }
}
