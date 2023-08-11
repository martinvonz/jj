# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

ScriptOs = enum("unix", "windows")

# Takes a cmd_args containing an executable and zero or more arguments to that
# executable, and bundles it together into a script that is callable as a single
# argument.
#
# For example in the Rust rules we have a `linker` + `linker_flags` that we want
# to pass to rustc as a single "-Clinker={}" argument.
#
#     linker_cmd = cmd_args(linker_info.linker, ctx.attrs.linker_flags)
#     linker_wrapper = cmd_script(
#         ctx = ctx,
#         name = "linker_wrapper",
#         cmd = linker_cmd,
#         os = ScriptOs("windows" if ctx.attrs._exec_os_type[OsLookup].platform == "windows" else "unix"),
#     )
#     return cmd_args(linker_wrapper, format = "-Clinker={}")
#
def cmd_script(
        ctx: AnalysisContext,
        name: str,
        cmd: cmd_args,
        os: ScriptOs.type) -> cmd_args:
    shell_quoted = cmd_args(cmd, quote = "shell")

    if os == ScriptOs("unix"):
        wrapper, _ = ctx.actions.write(
            ctx.actions.declare_output("{}.sh".format(name)),
            [
                "#!/usr/bin/env bash",
                cmd_args(cmd_args(shell_quoted, delimiter = " \\\n"), format = "{} \"$@\"\n"),
            ],
            is_executable = True,
            allow_args = True,
        )
    elif os == ScriptOs("windows"):
        wrapper, _ = ctx.actions.write(
            ctx.actions.declare_output("{}.bat".format(name)),
            [
                "@echo off",
                cmd_args(cmd_args(shell_quoted, delimiter = "^\n "), format = "{} %*\n"),
            ],
            allow_args = True,
        )
    else:
        fail(os)

    return cmd_args(wrapper).hidden(cmd)
