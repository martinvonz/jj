# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//utils:utils.bzl", "value_or")
load(":rule_spec.bzl", "RuleRegistrationSpec")

# Buck v2 doesn't support directories as source inputs, while v1 allows that.
# This rule fills that gap and allows to produce a directory from archive,
# which then can be used as an input for other rules.

def _impl(ctx: AnalysisContext) -> list["provider"]:
    output = ctx.actions.declare_output(value_or(ctx.attrs.directory_name, ctx.label.name))
    archive = ctx.attrs.contents_archive
    script, _ = ctx.actions.write(
        "unpack.sh",
        [
            cmd_args(output, format = "mkdir -p {}"),
            cmd_args(output, format = "cd {}"),
            cmd_args(archive, format = "tar -xzf {}").relative_to(output),
        ],
        is_executable = True,
        allow_args = True,
    )
    ctx.actions.run(cmd_args(["/bin/sh", script])
        .hidden([archive, output.as_output()]), category = "extract_archive")

    return [DefaultInfo(default_output = output)]

registration_spec = RuleRegistrationSpec(
    name = "extract_archive",
    impl = _impl,
    attrs = {
        # .tar.gz archive with the contents of the result directory
        "contents_archive": attrs.source(),
        # name of the result directory, if omitted, `name` attribute will be used instead
        "directory_name": attrs.option(attrs.string(), default = None),
    },
)
