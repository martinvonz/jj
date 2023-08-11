# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//tests:re_utils.bzl", "get_re_executor_from_props")
load("@prelude//test/inject_test_run_info.bzl", "inject_test_run_info")

def sh_test_impl(ctx: AnalysisContext) -> list["provider"]:
    # This does not exist in v1 either, but v1 has those attributes presumably
    # to be compatible with this. We just fail if they're passed.
    if ctx.attrs.list_args or ctx.attrs.list_env or ctx.attrs.run_args or ctx.attrs.run_env:
        fail("An unsupported attribute was passed")

    args = cmd_args()

    if ctx.attrs.test != None:
        if type(ctx.attrs.test) == "artifact":
            args.add(ctx.attrs.test)
        elif type(ctx.attrs.test) == "dependency":
            run_info = ctx.attrs.test.get(RunInfo)
            if run_info != None:
                args.add(run_info.args)
            else:
                info = ctx.attrs.test[DefaultInfo]
                args.add(info.default_outputs).hidden(info.other_outputs)
        else:
            fail("Unexpected type for test attribute")

    args.hidden(ctx.attrs.resources)

    deps = []
    for dep in ctx.attrs.deps:
        info = dep[DefaultInfo]
        deps.extend(info.default_outputs)
        deps.extend(info.other_outputs)

    args.hidden(deps)

    command = [args] + ctx.attrs.args

    # Setup a RE executor based on the `remote_execution` param.
    re_executor = get_re_executor_from_props(ctx.attrs.remote_execution)

    # We implicitly make the target run from the project root if remote
    # excution options were specified
    run_from_project_root = "buck2_run_from_project_root" in (ctx.attrs.labels or []) or re_executor != None

    # TODO support default info and runinfo properly by writing a sh script that invokes the command properly

    return inject_test_run_info(
        ctx,
        ExternalRunnerTestInfo(
            type = ctx.attrs.type or "custom",
            command = command,
            env = ctx.attrs.env,
            labels = ctx.attrs.labels,
            contacts = ctx.attrs.contacts,
            default_executor = re_executor,
            run_from_project_root = run_from_project_root,
            use_project_relative_paths = run_from_project_root,
        ),
    ) + [
        DefaultInfo(),
    ]
