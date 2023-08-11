# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//tests:re_utils.bzl",
    "get_re_executor_from_props",
)
load("@prelude//test/inject_test_run_info.bzl", "inject_test_run_info")
load(
    ":needed_coverage.bzl",
    "parse_python_needed_coverage_specs",
)

def python_needed_coverage_test_impl(ctx: AnalysisContext) -> list["provider"]:
    test_cmd = list(ctx.attrs.test[ExternalRunnerTestInfo].command)

    test_env = {}
    test_env.update(ctx.attrs.env)

    # Pass in needed coverage flags to the test.
    needed_coverages = parse_python_needed_coverage_specs(ctx.attrs.needed_coverage)
    test_cmd.append("--collect-coverage")
    test_cmd.append("--coverage-include")
    test_cmd.append(",".join([
        "*/{}".format(module)
        for needed_coverage in needed_coverages
        for module in needed_coverage.modules
    ]))
    for needed_coverage in needed_coverages:
        for module in needed_coverage.modules:
            test_cmd.append("--coverage-verdict={}={}".format(module, needed_coverage.ratio))

    # A needed coverage run just runs the entire test binary as bundle to
    # determine coverage.  Rather than special implementation in tpx, we
    # just use a simple test type to do this, which requires settings a few
    # additional flags/env-vars which the Python tpx test type would
    # otherwise handle.
    test_type = "simple"
    test_env["TEST_PILOT"] = "1"

    # Setup a RE executor based on the `remote_execution` param.
    re_executor = get_re_executor_from_props(ctx.attrs.remote_execution)

    return inject_test_run_info(
        ctx,
        ExternalRunnerTestInfo(
            type = test_type,
            command = test_cmd,
            env = test_env,
            labels = ctx.attrs.labels,
            contacts = ctx.attrs.contacts,
            default_executor = re_executor,
            # We implicitly make this test via the project root, instead of
            # the cell root (e.g. fbcode root).
            run_from_project_root = re_executor != None,
            use_project_relative_paths = re_executor != None,
        ),
    ) + [
        DefaultInfo(),
    ]
