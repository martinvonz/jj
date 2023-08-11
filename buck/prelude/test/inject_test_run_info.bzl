# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

def inject_test_run_info(ctx: AnalysisContext, test_info: ExternalRunnerTestInfo.type) -> list["provider"]:
    # Access this here so we get failures in CI if we forget to inject it
    # anywhere, regardless of whether an `env` is used.
    inject_test_env = ctx.attrs._inject_test_env[RunInfo]

    if (not test_info.env) or _exclude_test_env_from_run_info(ctx):
        return [test_info, RunInfo(args = test_info.command)]

    cell_root = ctx.label.cell_root

    env_file = ctx.actions.write_json(
        "test_env.json",
        {
            k: _maybe_relativize_path(test_info, cell_root, cmd_args(v, delimiter = " "))
            for (k, v) in test_info.env.items()
        },
        with_inputs = True,
    )

    return [test_info, RunInfo(args = [inject_test_env, env_file, "--", test_info.command])]

def _maybe_relativize_path(test_info: ExternalRunnerTestInfo.type, cell_root: "cell_root", arg: cmd_args) -> cmd_args:
    if test_info.run_from_project_root:
        return arg
    return arg.relative_to(cell_root)

def _exclude_test_env_from_run_info(ctx: AnalysisContext):
    # Antlir assumes that $(exe ...) is a single, relative path, so if we make
    # it multiple commands here, it'll break.
    return "antlir_inner_test" in ctx.attrs.labels
