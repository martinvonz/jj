# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":common.bzl", "prelude_rule")

git_fetch = prelude_rule(
    name = "git_fetch",
    docs = """
        Checkout a commit from a git repository.
    """,
    examples = """
        ```
        git_fetch(
            name = "serde.git",
            repo = "https://github.com/serde-rs/serde",
            rev = "fccb9499bccbaca0b7eef91a3a82dfcb31e0b149",
        )
        ```
    """,
    further = None,
    attrs = (
        # @unsorted-dict-items
        {
            "repo": attrs.string(doc = """
                Url suitable as a git remote.
            """),
            "rev": attrs.string(doc = """
                40-digit hex SHA-1 of the git commit.
            """),
            "contacts": attrs.list(attrs.string(), default = []),
            "default_host_platform": attrs.option(attrs.configuration_label(), default = None),
            "labels": attrs.list(attrs.string(), default = []),
            "licenses": attrs.list(attrs.source(), default = []),
            "_git_fetch_tool": attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = "prelude//git/tools:git_fetch")),
        }
    ),
)

git_rules = struct(
    git_fetch = git_fetch,
)
