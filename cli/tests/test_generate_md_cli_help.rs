// SPDX-FileCopyrightText: © 2020-2024 The Jujutsu Authors
// SPDX-License-Identifier: Apache-2.0

use insta::assert_snapshot;

use crate::common::TestEnvironment;

const PREAMBLE: &str = r#"
<!-- BEGIN MARKDOWN-->

"#;

#[test]
fn test_generate_markdown_docs_in_docs_dir() {
    let test_env = TestEnvironment::default();
    let mut markdown_help = PREAMBLE.to_string();
    markdown_help
        .push_str(&test_env.jj_cmd_success(test_env.env_root(), &["util", "markdown-help"]));
    // Validate partial snapshot, redacting any lines nested 2+ indent levels.
    insta::with_settings!({
        snapshot_path => ".",
        snapshot_suffix => ".md",
        prepend_module_to_snapshot => false,
        omit_expression => true,
        description => "AUTO-GENERATED FILE, DO NOT EDIT. This cli reference is generated as an \
                        `insta` snapshot. MkDocs follows they symlink from docs/cli-reference.md \
                        to the snap. Unfortunately, `insta` unavoidably creates this header. Luckily, \
                        MkDocs ignores the header since it has the same format as Markdown headers. \
                        TODO: MkDocs may fail on Windows if symlinks are not enabled in the OS \
                        settings",
    },
    { assert_snapshot!("cli-reference", markdown_help) });
}
