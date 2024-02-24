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

use std::path::PathBuf;
use std::sync::Arc;
use std::{fs, io, iter};

use ignore::gitignore;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitIgnoreError {
    #[error("Failed to read ignore patterns from file {path}")]
    ReadFile { path: PathBuf, source: io::Error },
    #[error("invalid UTF-8 for ignore pattern in {path} on line #{line_num_for_display}: {line}")]
    InvalidUtf8 {
        path: PathBuf,
        line_num_for_display: usize,
        line: String,
        source: std::str::Utf8Error,
    },
    #[error(transparent)]
    Underlying(#[from] ignore::Error),
}

/// Models the effective contents of multiple .gitignore files.
#[derive(Debug)]
pub struct GitIgnoreFile {
    parent: Option<Arc<GitIgnoreFile>>,
    matcher: gitignore::Gitignore,
}

impl GitIgnoreFile {
    pub fn empty() -> Arc<GitIgnoreFile> {
        Arc::new(GitIgnoreFile {
            parent: None,
            matcher: gitignore::Gitignore::empty(),
        })
    }

    /// Concatenates new `.gitignore` content at the `prefix` directory.
    ///
    /// The `prefix` should be a slash-separated path relative to the workspace
    /// root.
    pub fn chain(
        self: &Arc<GitIgnoreFile>,
        prefix: &str,
        input: &[u8],
    ) -> Result<Arc<GitIgnoreFile>, GitIgnoreError> {
        let mut builder = gitignore::GitignoreBuilder::new(prefix);
        for (i, input_line) in input.split(|b| *b == b'\n').enumerate() {
            let line =
                std::str::from_utf8(input_line).map_err(|err| GitIgnoreError::InvalidUtf8 {
                    path: PathBuf::from(prefix),
                    line_num_for_display: i + 1,
                    line: String::from_utf8_lossy(input_line).to_string(),
                    source: err,
                })?;
            // FIXME: do we need to provide the `from` argument? Is it for providing
            // diagnostics or correctness?
            builder.add_line(None, line)?;
        }
        let matcher = builder.build()?;
        let parent = if self.matcher.is_empty() {
            self.parent.clone() // omit the empty root
        } else {
            Some(self.clone())
        };
        Ok(Arc::new(GitIgnoreFile { parent, matcher }))
    }

    /// Concatenates new `.gitignore` file at the `prefix` directory.
    ///
    /// The `prefix` should be a slash-separated path relative to the workspace
    /// root.
    pub fn chain_with_file(
        self: &Arc<GitIgnoreFile>,
        prefix: &str,
        file: PathBuf,
    ) -> Result<Arc<GitIgnoreFile>, GitIgnoreError> {
        if file.is_file() {
            let buf = fs::read(&file).map_err(|err| GitIgnoreError::ReadFile {
                path: file.clone(),
                source: err,
            })?;
            self.chain(prefix, &buf)
        } else {
            Ok(self.clone())
        }
    }

    fn matches_helper(&self, path: &str, is_dir: bool) -> bool {
        iter::successors(Some(self), |file| file.parent.as_deref())
            .find_map(|file| {
                // TODO: the documentation warns that
                // `matched_path_or_any_parents` is slower than `matched`;
                // ideally, we would switch to that.
                match file.matcher.matched_path_or_any_parents(path, is_dir) {
                    ignore::Match::None => None,
                    ignore::Match::Ignore(_) => Some(true),
                    ignore::Match::Whitelist(_) => Some(false),
                }
            })
            .unwrap_or_default()
    }

    /// Returns whether specified path (not just file!) should be ignored. This
    /// method does not directly define which files should not be tracked in
    /// the repository. Instead, it performs a simple matching against the
    /// last applicable .gitignore line. The effective set of paths
    /// ignored in the repository should take into account that all (untracked)
    /// files within a ignored directory should be ignored unconditionally.
    /// The code in this file does not take that into account.
    pub fn matches(&self, path: &str) -> bool {
        //If path ends with slash, consider it as a directory.
        let (path, is_dir) = match path.strip_suffix('/') {
            Some(path) => (path, true),
            None => (path, false),
        };
        self.matches_helper(path, is_dir)
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn matches(input: &[u8], path: &str) -> bool {
        let file = GitIgnoreFile::empty().chain("", input).unwrap();
        file.matches(path)
    }

    #[test]
    fn test_gitignore_empty_file() {
        let file = GitIgnoreFile::empty();
        assert!(!file.matches("foo"));
    }

    #[test]
    fn test_gitignore_empty_file_with_prefix() {
        let file = GitIgnoreFile::empty().chain("dir/", b"").unwrap();
        assert!(!file.matches("dir/foo"));
    }

    #[test]
    fn test_gitignore_literal() {
        let file = GitIgnoreFile::empty().chain("", b"foo\n").unwrap();
        assert!(file.matches("foo"));
        assert!(file.matches("dir/foo"));
        assert!(file.matches("dir/subdir/foo"));
        assert!(!file.matches("food"));
        assert!(!file.matches("dir/food"));
    }

    #[test]
    fn test_gitignore_literal_with_prefix() {
        let file = GitIgnoreFile::empty().chain("./dir/", b"foo\n").unwrap();
        assert!(file.matches("dir/foo"));
        assert!(file.matches("dir/subdir/foo"));
    }

    #[test]
    fn test_gitignore_pattern_same_as_prefix() {
        let file = GitIgnoreFile::empty().chain("dir/", b"dir\n").unwrap();
        assert!(file.matches("dir/dir"));
        // We don't want the "dir" pattern to apply to the parent directory
        assert!(!file.matches("dir/foo"));
    }

    #[test]
    fn test_gitignore_rooted_literal() {
        let file = GitIgnoreFile::empty().chain("", b"/foo\n").unwrap();
        assert!(file.matches("foo"));
        assert!(!file.matches("dir/foo"));
    }

    #[test]
    fn test_gitignore_rooted_literal_with_prefix() {
        let file = GitIgnoreFile::empty().chain("dir/", b"/foo\n").unwrap();
        assert!(file.matches("dir/foo"));
        assert!(!file.matches("dir/subdir/foo"));
    }

    #[test]
    fn test_gitignore_deep_dir() {
        let file = GitIgnoreFile::empty()
            .chain("", b"/dir1/dir2/dir3\n")
            .unwrap();
        assert!(!file.matches("foo"));
        assert!(!file.matches("dir1/foo"));
        assert!(!file.matches("dir1/dir2/foo"));
        assert!(file.matches("dir1/dir2/dir3/foo"));
        assert!(file.matches("dir1/dir2/dir3/dir4/foo"));
    }

    #[test]
    fn test_gitignore_deep_dir_chained() {
        // Prefix is relative to root, not to parent file
        let file = GitIgnoreFile::empty()
            .chain("", b"/dummy\n")
            .unwrap()
            .chain("dir1/", b"/dummy\n")
            .unwrap()
            .chain("dir1/dir2/", b"/dir3\n")
            .unwrap();
        assert!(!file.matches("foo"));
        assert!(!file.matches("dir1/foo"));
        assert!(!file.matches("dir1/dir2/foo"));
        assert!(file.matches("dir1/dir2/dir3/foo"));
        assert!(file.matches("dir1/dir2/dir3/dir4/foo"));
    }

    #[test]
    fn test_gitignore_match_only_dir() {
        let file = GitIgnoreFile::empty().chain("", b"/dir/\n").unwrap();
        assert!(!file.matches("dir"));
        assert!(file.matches("dir/foo"));
        assert!(file.matches("dir/subdir/foo"));
    }

    #[test]
    fn test_gitignore_unusual_symbols() {
        assert!(matches(b"\\*\n", "*"));
        assert!(!matches(b"\\*\n", "foo"));
        assert!(matches(b"\\!\n", "!"));
        assert!(matches(b"\\?\n", "?"));
        assert!(!matches(b"\\?\n", "x"));
        assert!(matches(b"\\w\n", "w"));
        assert!(GitIgnoreFile::empty().chain("", b"\\\n").is_err());
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn teest_gitignore_backslash_path() {
        assert!(!matches(b"/foo/bar", "/foo\\bar"));
        assert!(!matches(b"/foo/bar", "/foo/bar\\"));

        assert!(!matches(b"/foo/bar/", "/foo\\bar/"));
        assert!(!matches(b"/foo/bar/", "/foo\\bar\\/"));

        // Invalid escapes are treated like literal backslashes
        assert!(!matches(b"\\w\n", "\\w"));
        assert!(matches(b"\\\\ \n", "\\ "));
        assert!(matches(b"\\\\\\ \n", "\\ "));
    }

    #[test]
    #[cfg(target_os = "windows")]
    /// ignore crate consider backslashes as a directory divider only on
    /// Windows.
    fn teest_gitignore_backslash_path() {
        assert!(matches(b"/foo/bar", "/foo\\bar"));
        assert!(matches(b"/foo/bar", "/foo/bar\\"));

        assert!(matches(b"/foo/bar/", "/foo\\bar/"));
        assert!(matches(b"/foo/bar/", "/foo\\bar\\/"));

        assert!(matches(b"\\w\n", "\\w"));
        assert!(!matches(b"\\\\ \n", "\\ "));
        assert!(!matches(b"\\\\\\ \n", "\\ "));
    }

    #[test]
    fn test_gitignore_whitespace() {
        assert!(!matches(b" \n", " "));
        assert!(matches(b"\\ \n", " "));
        assert!(!matches(b"\\\\ \n", " "));
        assert!(matches(b" a\n", " a"));
        assert!(matches(b"a b\n", "a b"));
        assert!(matches(b"a b \n", "a b"));
        assert!(!matches(b"a b \n", "a b "));
        assert!(matches(b"a b\\ \\ \n", "a b  "));
        // Trail CRs at EOL is ignored
        assert!(matches(b"a\r\n", "a"));
        assert!(!matches(b"a\r\n", "a\r"));
        assert!(!matches(b"a\r\r\n", "a\r"));
        assert!(matches(b"a\r\r\n", "a"));
        assert!(!matches(b"a\r\r\n", "a\r\r"));
        assert!(matches(b"a\r\r\n", "a"));
        assert!(matches(b"\ra\n", "\ra"));
        assert!(!matches(b"\ra\n", "a"));
        assert!(GitIgnoreFile::empty().chain("", b"a b \\  \n").is_err());
    }

    #[test]
    fn test_gitignore_glob() {
        assert!(!matches(b"*.o\n", "foo"));
        assert!(matches(b"*.o\n", "foo.o"));
        assert!(!matches(b"foo.?\n", "foo"));
        assert!(!matches(b"foo.?\n", "foo."));
        assert!(matches(b"foo.?\n", "foo.o"));
    }

    #[test]
    fn test_gitignore_range() {
        assert!(!matches(b"foo.[az]\n", "foo"));
        assert!(matches(b"foo.[az]\n", "foo.a"));
        assert!(!matches(b"foo.[az]\n", "foo.g"));
        assert!(matches(b"foo.[az]\n", "foo.z"));
        assert!(!matches(b"foo.[a-z]\n", "foo"));
        assert!(matches(b"foo.[a-z]\n", "foo.a"));
        assert!(matches(b"foo.[a-z]\n", "foo.g"));
        assert!(matches(b"foo.[a-z]\n", "foo.z"));
        assert!(matches(b"foo.[0-9a-fA-F]\n", "foo.5"));
        assert!(matches(b"foo.[0-9a-fA-F]\n", "foo.c"));
        assert!(matches(b"foo.[0-9a-fA-F]\n", "foo.E"));
        assert!(!matches(b"foo.[0-9a-fA-F]\n", "foo._"));
    }

    #[test]
    fn test_gitignore_leading_dir_glob() {
        assert!(matches(b"**/foo\n", "foo"));
        assert!(matches(b"**/foo\n", "dir1/dir2/foo"));
        assert!(matches(b"**/foo\n", "foo/file"));
        assert!(matches(b"**/dir/foo\n", "dir/foo"));
        assert!(matches(b"**/dir/foo\n", "dir1/dir2/dir/foo"));
    }

    #[test]
    fn test_gitignore_leading_dir_glob_with_prefix() {
        let file = GitIgnoreFile::empty()
            .chain("dir1/dir2/", b"**/foo\n")
            .unwrap();
        assert!(file.matches("dir1/dir2/foo"));
        assert!(!file.matches("dir1/dir2/bar"));
        assert!(file.matches("dir1/dir2/sub1/sub2/foo"));
        assert!(!file.matches("dir1/dir2/sub1/sub2/bar"));
    }

    #[test]
    fn test_gitignore_trailing_dir_glob() {
        assert!(!matches(b"abc/**\n", "abc"));
        assert!(matches(b"abc/**\n", "abc/file"));
        assert!(matches(b"abc/**\n", "abc/dir/file"));
    }

    #[test]
    fn test_gitignore_internal_dir_glob() {
        assert!(matches(b"a/**/b\n", "a/b"));
        assert!(matches(b"a/**/b\n", "a/x/b"));
        assert!(matches(b"a/**/b\n", "a/x/y/b"));
        assert!(!matches(b"a/**/b\n", "ax/y/b"));
        assert!(!matches(b"a/**/b\n", "a/x/yb"));
        assert!(!matches(b"a/**/b\n", "ab"));
    }

    #[test]
    fn test_gitignore_internal_dir_glob_not_really() {
        assert!(!matches(b"a/x**y/b\n", "a/b"));
        assert!(matches(b"a/x**y/b\n", "a/xy/b"));
        assert!(matches(b"a/x**y/b\n", "a/xzzzy/b"));
    }

    #[test]
    fn test_gitignore_line_ordering() {
        assert!(matches(b"foo\n!foo/bar\n", "foo"));
        assert!(!matches(b"foo\n!foo/bar\n", "foo/bar"));
        assert!(matches(b"foo\n!foo/bar\n", "foo/baz"));
        assert!(matches(b"foo\n!foo/bar\nfoo/bar/baz", "foo"));
        assert!(!matches(b"foo\n!foo/bar\nfoo/bar/baz", "foo/bar"));
        assert!(matches(b"foo\n!foo/bar\nfoo/bar/baz", "foo/bar/baz"));
        assert!(!matches(b"foo\n!foo/bar\nfoo/bar/baz", "foo/bar/quux"));
        assert!(!matches(b"foo/*\n!foo/bar", "foo/bar"));
    }

    #[test]
    fn test_gitignore_file_ordering() {
        let file1 = GitIgnoreFile::empty().chain("", b"/foo\n").unwrap();
        let file2 = file1.chain("foo/", b"!/bar").unwrap();
        let file3 = file2.chain("foo/bar/", b"/baz").unwrap();
        assert!(file1.matches("foo"));
        assert!(file1.matches("foo/bar"));
        assert!(!file2.matches("foo/bar"));
        assert!(!file2.matches("foo/bar/baz"));
        assert!(file2.matches("foo/baz"));
        assert!(file3.matches("foo/bar/baz"));
        assert!(!file3.matches("foo/bar/qux"));
    }

    #[test]
    fn test_gitignore_negative_parent_directory() {
        // The following script shows that Git ignores the file:
        //
        // ```bash
        // $ rm -rf test-repo && \
        //   git init test-repo &>/dev/null && \
        //   cd test-repo && \
        //   printf 'A/B.*\n!/A/\n' >.gitignore && \
        //   mkdir A && \
        //   touch A/B.ext && \
        //   git check-ignore A/B.ext
        // A/B.ext
        // ```
        let ignore = GitIgnoreFile::empty()
            .chain("", b"foo/bar.*\n!/foo/\n")
            .unwrap();
        assert!(ignore.matches("foo/bar.ext"));

        let ignore = GitIgnoreFile::empty()
            .chain("", b"!/foo/\nfoo/bar.*\n")
            .unwrap();
        assert!(ignore.matches("foo/bar.ext"));
    }
}
