# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//utils:cmd_script.bzl", "ScriptOs", "cmd_script")

VisualStudio = provider(fields = [
    # cl.exe
    "cl_exe",
    # lib.exe
    "lib_exe",
    # ml64.exe
    "ml64_exe",
    # link.exe
    "link_exe",
])

def _find_msvc_tools_impl(ctx: AnalysisContext) -> list["provider"]:
    cl_exe_json = ctx.actions.declare_output("cl.exe.json")
    lib_exe_json = ctx.actions.declare_output("lib.exe.json")
    ml64_exe_json = ctx.actions.declare_output("ml64.exe.json")
    link_exe_json = ctx.actions.declare_output("link.exe.json")

    cmd = [
        ctx.attrs.vswhere[RunInfo],
        cmd_args("--cl=", cl_exe_json.as_output(), delimiter = ""),
        cmd_args("--lib=", lib_exe_json.as_output(), delimiter = ""),
        cmd_args("--ml64=", ml64_exe_json.as_output(), delimiter = ""),
        cmd_args("--link=", link_exe_json.as_output(), delimiter = ""),
    ]

    ctx.actions.run(
        cmd,
        category = "vswhere",
        local_only = True,
    )

    run_msvc_tool = ctx.attrs.run_msvc_tool[RunInfo]
    cl_exe_script = cmd_script(
        ctx = ctx,
        name = "cl",
        cmd = cmd_args(run_msvc_tool, cl_exe_json),
        os = ScriptOs("windows"),
    )
    lib_exe_script = cmd_script(
        ctx = ctx,
        name = "lib",
        cmd = cmd_args(run_msvc_tool, lib_exe_json),
        os = ScriptOs("windows"),
    )
    ml64_exe_script = cmd_script(
        ctx = ctx,
        name = "ml64",
        cmd = cmd_args(run_msvc_tool, ml64_exe_json),
        os = ScriptOs("windows"),
    )
    link_exe_script = cmd_script(
        ctx = ctx,
        name = "link",
        cmd = cmd_args(run_msvc_tool, link_exe_json),
        os = ScriptOs("windows"),
    )

    return [
        # Supports `buck2 run prelude//toolchains/msvc:msvc_tools[cl.exe]`
        # and `buck2 build prelude//toolchains/msvc:msvc_tools[cl.exe][json]`
        DefaultInfo(sub_targets = {
            "cl.exe": [
                RunInfo(args = [cl_exe_script]),
                DefaultInfo(sub_targets = {
                    "json": [DefaultInfo(default_output = cl_exe_json)],
                }),
            ],
            "lib.exe": [
                RunInfo(args = [lib_exe_script]),
                DefaultInfo(sub_targets = {
                    "json": [DefaultInfo(default_output = lib_exe_json)],
                }),
            ],
            "link.exe": [
                RunInfo(args = [link_exe_script]),
                DefaultInfo(sub_targets = {
                    "json": [DefaultInfo(default_output = link_exe_json)],
                }),
            ],
            "ml64.exe": [
                RunInfo(args = [ml64_exe_script]),
                DefaultInfo(sub_targets = {
                    "json": [DefaultInfo(default_output = ml64_exe_json)],
                }),
            ],
        }),
        VisualStudio(
            cl_exe = cl_exe_script,
            lib_exe = lib_exe_script,
            ml64_exe = ml64_exe_script,
            link_exe = link_exe_script,
        ),
    ]

find_msvc_tools = rule(
    impl = _find_msvc_tools_impl,
    attrs = {
        "run_msvc_tool": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//toolchains/msvc:run_msvc_tool")),
        "vswhere": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//toolchains/msvc:vswhere")),
    },
)
