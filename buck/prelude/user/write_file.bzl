# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":rule_spec.bzl", "RuleRegistrationSpec")

_NL = {
    "unix": "\n",
    "windows": "\r\n",
}

def _impl(ctx: AnalysisContext):
    # Figure out the type of newlines we're using.
    newline_type = ctx.attrs.newline
    if newline_type == "auto":
        newline_type = ctx.attrs._auto_newline

    # Join lines with newline.
    newline = _NL[newline_type]
    content = "".join([line + newline for line in ctx.attrs.content])

    output = ctx.actions.write(
        ctx.attrs.out,
        content,
        is_executable = ctx.attrs.is_executable,
    )

    providers = [DefaultInfo(default_output = output)]

    if ctx.attrs.is_executable:
        providers.append(RunInfo(cmd_args(output)))

    return providers

registration_spec = RuleRegistrationSpec(
    name = "write_file",
    impl = _impl,
    attrs = {
        # API based on https://github.com/bazelbuild/bazel-skylib/blob/main/docs/write_file_doc.md.
        "content": attrs.list(attrs.string(), default = []),
        "is_executable": attrs.bool(default = False),
        "labels": attrs.list(attrs.string(), default = []),
        "newline": attrs.enum(["auto", "unix", "windows"], default = "auto"),
        "out": attrs.string(),
        "_auto_newline": attrs.default_only(
            attrs.string(
                default = select({
                    "DEFAULT": "unix",
                    "config//os:windows": "windows",
                }),
            ),
        ),
    },
)
