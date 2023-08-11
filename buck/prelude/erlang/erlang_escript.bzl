# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(":erlang_build.bzl", "erlang_build")
load(":erlang_dependencies.bzl", "check_dependencies", "flatten_dependencies")
load(":erlang_info.bzl", "ErlangAppInfo")
load(":erlang_toolchain.bzl", "get_primary", "select_toolchains")
load(":erlang_utils.bzl", "action_identifier", "to_term_args")

def create_escript(
        ctx: AnalysisContext,
        spec_file: "artifact",
        toolchain: "Toolchain",
        files: list["artifact"],
        output: "artifact",
        escript_name: str) -> None:
    """ build the escript with the escript builder tool
    """
    script = toolchain.escript_builder

    escript_build_cmd = cmd_args(
        [
            toolchain.otp_binaries.escript,
            script,
            spec_file,
        ],
    )
    escript_build_cmd.hidden(output.as_output())
    escript_build_cmd.hidden(files)
    erlang_build.utils.run_with_env(
        ctx,
        toolchain,
        escript_build_cmd,
        category = "escript",
        identifier = action_identifier(toolchain, escript_name),
    )
    return None

def erlang_escript_impl(ctx: AnalysisContext) -> list["provider"]:
    # select the correct tools from the toolchain
    toolchain_name = get_primary(ctx)
    toolchain = select_toolchains(ctx)[get_primary(ctx)]

    # collect all dependencies
    dependencies = flatten_dependencies(ctx, check_dependencies(ctx.attrs.deps, [ErlangAppInfo]))

    artifacts = {}

    for dep in dependencies.values():
        if ErlangAppInfo not in dep:
            # skip extra includes
            continue
        dep_info = dep[ErlangAppInfo]
        if dep_info.virtual:
            # skip virtual apps
            continue

        # add ebin
        ebin_files = dep_info.beams[toolchain_name].values() + [dep_info.app_file[toolchain_name]]
        for ebin_file in ebin_files:
            artifacts[_ebin_path(ebin_file, dep_info.name)] = ebin_file

        # priv dir
        if ctx.attrs.include_priv:
            artifacts[_priv_path(dep_info.name)] = dep_info.priv_dir[toolchain_name]

    # additional resources
    for res in ctx.attrs.resources:
        for artifact in res[DefaultInfo].default_outputs + res[DefaultInfo].other_outputs:
            if artifact.short_path in artifacts:
                fail("multiple artifacts defined for path %s", (artifact.short_path))
            artifacts[artifact.short_path] = artifact

    if ctx.attrs.script_name:
        escript_name = ctx.attrs.script_name
    else:
        escript_name = ctx.attrs.name + ".escript"
    output = ctx.actions.declare_output(escript_name)

    args = ctx.attrs.emu_args
    if ctx.attrs.main_module:
        args += ["-escript", "main", ctx.attrs.main_module]

    escript_build_spec = {
        "artifacts": artifacts,
        "emu_args": args,
        "output": output.as_output(),
    }

    spec_file = ctx.actions.write(
        "escript_build_spec.term",
        to_term_args(escript_build_spec),
    )

    create_escript(ctx, spec_file, toolchain, artifacts.values(), output, escript_name)

    escript_cmd = cmd_args(
        [
            toolchain.otp_binaries.escript,
            output,
        ],
    )

    return [
        DefaultInfo(default_output = output),
        RunInfo(escript_cmd),
    ]

def _ebin_path(file: "artifact", app_name: str) -> str:
    return paths.join(app_name, "ebin", file.basename)

def _priv_path(app_name: str) -> str:
    return paths.join(app_name, "priv")
