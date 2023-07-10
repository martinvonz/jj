// Copyright 2021 The Jujutsu Authors
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

#![allow(missing_docs)]

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use itertools::Itertools;
use regex::{escape as regex_escape, Regex};

#[derive(Debug)]
struct GitIgnoreLine {
    is_negative: bool,
    regex: Regex,
}

impl GitIgnoreLine {
    // Remove trailing spaces (unless backslash-escaped). Any character
    // can be backslash-escaped as well.
    fn remove_trailing_space(input: &str) -> &str {
        let input = input.strip_suffix('\r').unwrap_or(input);
        let mut it = input.char_indices().rev().peekable();
        while let Some((i, c)) = it.next() {
            if c != ' ' {
                return &input[..i + c.len_utf8()];
            }
            if matches!(it.peek(), Some((_, '\\'))) {
                if it.skip(1).take_while(|(_, b)| *b == '\\').count() % 2 == 1 {
                    return &input[..i];
                }
                return &input[..i + 1];
            }
        }
        ""
    }

    fn parse(prefix: &str, input: &str) -> Option<GitIgnoreLine> {
        assert!(prefix.is_empty() || prefix.ends_with('/'));
        if input.starts_with('#') {
            return None;
        }

        let input = GitIgnoreLine::remove_trailing_space(input);
        // Remove leading "!" before checking for empty to match git's implementation
        // (i.e. just "!" matching nothing, not everything).
        let (is_negative, input) = match input.strip_prefix('!') {
            None => (false, input),
            Some(rest) => (true, rest),
        };
        if input.is_empty() {
            return None;
        }

        let (matches_only_directory, input) = match input.strip_suffix('/') {
            None => (false, input),
            Some(rest) => (true, rest),
        };
        let (mut is_rooted, input) = match input.strip_prefix('/') {
            None => (false, input),
            Some(rest) => (true, rest),
        };
        is_rooted |= input.contains('/');

        let mut regex = String::new();
        regex.push('^');
        regex.push_str(prefix);
        if !is_rooted {
            regex.push_str("(.*/)?");
        }

        let components = input.split('/').collect_vec();
        for (i, component) in components.iter().enumerate() {
            if *component == "**" {
                if i == components.len() - 1 {
                    regex.push_str(".*");
                } else {
                    regex.push_str("(.*/)?");
                }
            } else {
                let mut in_escape = false;
                let mut character_class: Option<String> = None;
                for c in component.chars() {
                    if in_escape {
                        in_escape = false;
                        if !matches!(c, ' ' | '#' | '!' | '?' | '\\' | '*') {
                            regex.push_str(&regex_escape("\\"));
                        }
                        regex.push_str(&regex_escape(&c.to_string()));
                    } else if c == '\\' {
                        in_escape = true;
                    } else if let Some(characters) = &mut character_class {
                        if c == ']' {
                            regex.push('[');
                            regex.push_str(characters);
                            regex.push(']');
                            character_class = None;
                        } else {
                            characters.push(c);
                        }
                    } else {
                        in_escape = false;
                        if c == '?' {
                            regex.push_str("[^/]");
                        } else if c == '*' {
                            regex.push_str("[^/]*");
                        } else if c == '[' {
                            character_class = Some(String::new());
                        } else {
                            regex.push_str(&regex_escape(&c.to_string()));
                        }
                    }
                }
                if in_escape {
                    regex.push_str(&regex_escape("\\"));
                }
                if i < components.len() - 1 {
                    regex.push('/');
                }
            }
        }
        if matches_only_directory {
            regex.push_str("/.*");
        } else {
            regex.push_str("(/.*|$)");
        }
        let regex = Regex::new(&regex).unwrap();

        Some(GitIgnoreLine { is_negative, regex })
    }

    fn matches(&self, path: &str) -> bool {
        self.regex.is_match(path)
    }
}

#[derive(Debug)]
pub struct GitIgnoreFile {
    parent: Option<Arc<GitIgnoreFile>>,
    lines: Vec<GitIgnoreLine>,
}

impl GitIgnoreFile {
    pub fn empty() -> Arc<GitIgnoreFile> {
        Arc::new(GitIgnoreFile {
            parent: None,
            lines: vec![],
        })
    }

    pub fn chain(self: &Arc<GitIgnoreFile>, prefix: &str, input: &[u8]) -> Arc<GitIgnoreFile> {
        let mut lines = vec![];
        for input_line in input.split(|b| *b == b'\n') {
            // Skip non-utf8 lines
            if let Ok(line_string) = String::from_utf8(input_line.to_vec()) {
                if let Some(line) = GitIgnoreLine::parse(prefix, &line_string) {
                    lines.push(line);
                }
            }
        }

        Arc::new(GitIgnoreFile {
            parent: Some(self.clone()),
            lines,
        })
    }

    pub fn chain_with_file(
        self: &Arc<GitIgnoreFile>,
        prefix: &str,
        file: PathBuf,
    ) -> Arc<GitIgnoreFile> {
        if file.is_file() {
            let mut file = File::open(file).unwrap();
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).unwrap();
            self.chain(prefix, &buf)
        } else {
            self.clone()
        }
    }

    fn all_lines_reversed<'a>(&'a self) -> Box<dyn Iterator<Item = &GitIgnoreLine> + 'a> {
        if let Some(parent) = &self.parent {
            Box::new(self.lines.iter().rev().chain(parent.all_lines_reversed()))
        } else {
            Box::new(self.lines.iter().rev())
        }
    }

    pub fn matches_file(&self, path: &str) -> bool {
        // Later lines take precedence, so check them in reverse
        for line in self.all_lines_reversed() {
            if line.matches(path) {
                return !line.is_negative;
            }
        }
        false
    }

    pub fn matches_all_files_in(&self, dir: &str) -> bool {
        // Later lines take precedence, so check them in reverse
        assert!(dir.is_empty() || dir.ends_with('/'));
        for line in self.all_lines_reversed() {
            // Let's say there's a "/target/" pattern and then a "!interesting" pattern
            // after it, then we can't say for sure that all files in target/ match.
            // TODO: This can be smarter. For example, if there's a pattern "/foo/" followed
            // by "!/bar/", then we can answer "true" for "foo/". A more complex
            // case is if a pattern "/foo/" is followed "!/foo/bar/", then we
            // can say "false" for "foo/" and "true" for "foo/baz/".
            if line.is_negative {
                return false;
            }
            if line.matches(dir) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn matches_file(input: &[u8], path: &str) -> bool {
        let file = GitIgnoreFile::empty().chain("", input);
        file.matches_file(path)
    }

    fn matches_all_files_in(input: &[u8], path: &str) -> bool {
        let file = GitIgnoreFile::empty().chain("", input);
        file.matches_all_files_in(path)
    }

    #[test]
    fn test_gitignore_empty_file() {
        let file = GitIgnoreFile::empty();
        assert!(!file.matches_file("foo"));
    }

    #[test]
    fn test_gitignore_empty_file_with_prefix() {
        let file = GitIgnoreFile::empty().chain("dir/", b"");
        assert!(!file.matches_file("dir/foo"));
    }

    #[test]
    fn test_gitignore_literal() {
        let file = GitIgnoreFile::empty().chain("", b"foo\n");
        assert!(file.matches_file("foo"));
        assert!(file.matches_file("dir/foo"));
        assert!(file.matches_file("dir/subdir/foo"));
        assert!(!file.matches_file("food"));
        assert!(!file.matches_file("dir/food"));
    }

    #[test]
    fn test_gitignore_literal_with_prefix() {
        let file = GitIgnoreFile::empty().chain("dir/", b"foo\n");
        // I consider it undefined whether a file in a parent directory matches, but
        // let's test it anyway
        assert!(!file.matches_file("foo"));
        assert!(file.matches_file("dir/foo"));
        assert!(file.matches_file("dir/subdir/foo"));
    }

    #[test]
    fn test_gitignore_pattern_same_as_prefix() {
        let file = GitIgnoreFile::empty().chain("dir/", b"dir\n");
        assert!(file.matches_file("dir/dir"));
        // We don't want the "dir" pattern to apply to the parent directory
        assert!(!file.matches_file("dir/foo"));
    }

    #[test]
    fn test_gitignore_rooted_literal() {
        let file = GitIgnoreFile::empty().chain("", b"/foo\n");
        assert!(file.matches_file("foo"));
        assert!(!file.matches_file("dir/foo"));
    }

    #[test]
    fn test_gitignore_rooted_literal_with_prefix() {
        let file = GitIgnoreFile::empty().chain("dir/", b"/foo\n");
        // I consider it undefined whether a file in a parent directory matches, but
        // let's test it anyway
        assert!(!file.matches_file("foo"));
        assert!(file.matches_file("dir/foo"));
        assert!(!file.matches_file("dir/subdir/foo"));
    }

    #[test]
    fn test_gitignore_deep_dir() {
        let file = GitIgnoreFile::empty().chain("", b"/dir1/dir2/dir3\n");
        assert!(!file.matches_file("foo"));
        assert!(!file.matches_file("dir1/foo"));
        assert!(!file.matches_file("dir1/dir2/foo"));
        assert!(file.matches_file("dir1/dir2/dir3/foo"));
        assert!(file.matches_file("dir1/dir2/dir3/dir4/foo"));
    }

    #[test]
    fn test_gitignore_match_only_dir() {
        let file = GitIgnoreFile::empty().chain("", b"/dir/\n");
        assert!(!file.matches_file("dir"));
        assert!(file.matches_file("dir/foo"));
        assert!(file.matches_file("dir/subdir/foo"));
    }

    #[test]
    fn test_gitignore_unusual_symbols() {
        assert!(matches_file(b"\\*\n", "*"));
        assert!(!matches_file(b"\\*\n", "foo"));
        assert!(matches_file(b"\\\n", "\\"));
        assert!(matches_file(b"\\!\n", "!"));
        assert!(matches_file(b"\\?\n", "?"));
        assert!(!matches_file(b"\\?\n", "x"));
        // Invalid escapes are treated like literal backslashes
        assert!(matches_file(b"\\w\n", "\\w"));
        assert!(!matches_file(b"\\w\n", "w"));
    }

    #[test]
    fn test_gitignore_whitespace() {
        assert!(!matches_file(b" \n", " "));
        assert!(matches_file(b"\\ \n", " "));
        assert!(matches_file(b"\\\\ \n", "\\"));
        assert!(!matches_file(b"\\\\ \n", " "));
        assert!(matches_file(b"\\\\\\ \n", "\\ "));
        assert!(matches_file(b" a\n", " a"));
        assert!(matches_file(b"a b\n", "a b"));
        assert!(matches_file(b"a b \n", "a b"));
        assert!(!matches_file(b"a b \n", "a b "));
        assert!(matches_file(b"a b\\ \\ \n", "a b  "));
        // It's unclear how this should be interpreted, but we count spaces before
        // escaped spaces
        assert!(matches_file(b"a b \\  \n", "a b  "));
        // A single CR at EOL is ignored
        assert!(matches_file(b"a\r\n", "a"));
        assert!(!matches_file(b"a\r\n", "a\r"));
        assert!(matches_file(b"a\r\r\n", "a\r"));
        assert!(!matches_file(b"a\r\r\n", "a\r\r"));
        assert!(matches_file(b"\ra\n", "\ra"));
        assert!(!matches_file(b"\ra\n", "a"));
    }

    #[test]
    fn test_gitignore_glob() {
        assert!(!matches_file(b"*.o\n", "foo"));
        assert!(matches_file(b"*.o\n", "foo.o"));
        assert!(!matches_file(b"foo.?\n", "foo"));
        assert!(!matches_file(b"foo.?\n", "foo."));
        assert!(matches_file(b"foo.?\n", "foo.o"));
    }

    #[test]
    fn test_gitignore_range() {
        assert!(!matches_file(b"foo.[az]\n", "foo"));
        assert!(matches_file(b"foo.[az]\n", "foo.a"));
        assert!(!matches_file(b"foo.[az]\n", "foo.g"));
        assert!(matches_file(b"foo.[az]\n", "foo.z"));
        assert!(!matches_file(b"foo.[a-z]\n", "foo"));
        assert!(matches_file(b"foo.[a-z]\n", "foo.a"));
        assert!(matches_file(b"foo.[a-z]\n", "foo.g"));
        assert!(matches_file(b"foo.[a-z]\n", "foo.z"));
        assert!(matches_file(b"foo.[0-9a-fA-F]\n", "foo.5"));
        assert!(matches_file(b"foo.[0-9a-fA-F]\n", "foo.c"));
        assert!(matches_file(b"foo.[0-9a-fA-F]\n", "foo.E"));
        assert!(!matches_file(b"foo.[0-9a-fA-F]\n", "foo._"));
    }

    #[test]
    fn test_gitignore_leading_dir_glob() {
        assert!(matches_file(b"**/foo\n", "foo"));
        assert!(matches_file(b"**/foo\n", "dir1/dir2/foo"));
        assert!(matches_file(b"**/foo\n", "foo/file"));
        assert!(matches_file(b"**/dir/foo\n", "dir/foo"));
        assert!(matches_file(b"**/dir/foo\n", "dir1/dir2/dir/foo"));
    }

    #[test]
    fn test_gitignore_leading_dir_glob_with_prefix() {
        let file = GitIgnoreFile::empty().chain("dir1/dir2/", b"**/foo\n");
        // I consider it undefined whether a file in a parent directory matches, but
        // let's test it anyway
        assert!(!file.matches_file("foo"));
        assert!(file.matches_file("dir1/dir2/foo"));
        assert!(!file.matches_file("dir1/dir2/bar"));
        assert!(file.matches_file("dir1/dir2/sub1/sub2/foo"));
        assert!(!file.matches_file("dir1/dir2/sub1/sub2/bar"));
    }

    #[test]
    fn test_gitignore_trailing_dir_glob() {
        assert!(!matches_file(b"abc/**\n", "abc"));
        assert!(matches_file(b"abc/**\n", "abc/file"));
        assert!(matches_file(b"abc/**\n", "abc/dir/file"));
    }

    #[test]
    fn test_gitignore_internal_dir_glob() {
        assert!(matches_file(b"a/**/b\n", "a/b"));
        assert!(matches_file(b"a/**/b\n", "a/x/b"));
        assert!(matches_file(b"a/**/b\n", "a/x/y/b"));
        assert!(!matches_file(b"a/**/b\n", "ax/y/b"));
        assert!(!matches_file(b"a/**/b\n", "a/x/yb"));
        assert!(!matches_file(b"a/**/b\n", "ab"));
    }

    #[test]
    fn test_gitignore_internal_dir_glob_not_really() {
        assert!(!matches_file(b"a/x**y/b\n", "a/b"));
        assert!(matches_file(b"a/x**y/b\n", "a/xy/b"));
        assert!(matches_file(b"a/x**y/b\n", "a/xzzzy/b"));
    }

    #[test]
    fn test_gitignore_line_ordering() {
        assert!(matches_file(b"foo\n!foo/bar\n", "foo"));
        assert!(!matches_file(b"foo\n!foo/bar\n", "foo/bar"));
        assert!(matches_file(b"foo\n!foo/bar\n", "foo/baz"));
        assert!(matches_file(b"foo\n!foo/bar\nfoo/bar/baz", "foo"));
        assert!(!matches_file(b"foo\n!foo/bar\nfoo/bar/baz", "foo/bar"));
        assert!(matches_file(b"foo\n!foo/bar\nfoo/bar/baz", "foo/bar/baz"));
        assert!(!matches_file(b"foo\n!foo/bar\nfoo/bar/baz", "foo/bar/quux"));
    }

    #[test]
    fn test_gitignore_file_ordering() {
        let file1 = GitIgnoreFile::empty().chain("", b"foo\n");
        let file2 = file1.chain("foo/", b"!bar");
        let file3 = file2.chain("foo/bar/", b"baz");
        assert!(file1.matches_file("foo"));
        assert!(file1.matches_file("foo/bar"));
        assert!(!file2.matches_file("foo/bar"));
        assert!(file2.matches_file("foo/baz"));
        assert!(file3.matches_file("foo/bar/baz"));
        assert!(!file3.matches_file("foo/bar/qux"));
    }

    #[test]
    fn test_gitignore_match_dir() {
        assert!(matches_all_files_in(b"foo\n", "foo/"));
        assert!(matches_all_files_in(b"foo\nbar\n", "foo/"));
        assert!(matches_all_files_in(b"!foo\nbar\n", "bar/"));
        assert!(!matches_all_files_in(b"foo\n!bar\n", "foo/"));
        // This one could return true, but it doesn't currently
        assert!(!matches_all_files_in(b"foo\n!/bar\n", "foo/"));
    }
}
