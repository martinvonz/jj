# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":context.bzl", "CompileContext")

# Inputs to the fail filter
RustFailureFilter = provider(fields = [
    # Build status json
    "buildstatus",
    # Required files
    "required",
    # stderr
    "stderr",
])

# This creates an action which takes a buildstatus json artifact as an input, and a list of other
# artifacts. If all those artifacts are present in the buildstatus as successfully generated, then
# the action will succeed with those artifacts as outputs. Otherwise it fails.
# Either way it streams whatever stderr content there is to stream.
def failure_filter(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        prefix: str,
        predecl_out: ["artifact", None],
        failprov: "RustFailureFilter",
        short_cmd: str) -> "artifact":
    toolchain_info = compile_ctx.toolchain_info
    failure_filter_action = toolchain_info.failure_filter_action

    buildstatus = failprov.buildstatus
    required = failprov.required
    stderr = failprov.stderr

    if predecl_out:
        output = predecl_out
    else:
        output = ctx.actions.declare_output("out/" + required.short_path)

    cmd = cmd_args(
        failure_filter_action,
        "--stderr",
        stderr,
        "--required-file",
        required.short_path,
        required,
        output.as_output(),
        "--build-status",
        buildstatus,
    )

    ctx.actions.run(cmd, category = "failure_filter", identifier = "{} {}".format(prefix, short_cmd))

    return output
