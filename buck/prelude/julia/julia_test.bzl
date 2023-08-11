# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//test/inject_test_run_info.bzl", "inject_test_run_info")
load(":julia_binary.bzl", "build_julia_command")

def julia_test_impl(ctx: AnalysisContext) -> list["provider"]:
    cmd = build_julia_command(ctx)
    external_runner_test_info = ExternalRunnerTestInfo(
        type = "julia",
        command = [cmd],
        contacts = ctx.attrs.contacts,
    )

    return inject_test_run_info(ctx, external_runner_test_info) + [DefaultInfo()]
