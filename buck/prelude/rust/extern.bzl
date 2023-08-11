# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":context.bzl", "CompileContext", "CrateMapArg", "ExternArg")
load(":link_info.bzl", "CrateName")

# Create `--extern` flag. For crates with a name computed during analysis:
#
#     --extern=NAME=path/to/libNAME.rlib
#
# For crates with a name computed during build:
#
#     --extern @extern/libPROVISIONAL
#
# where extern/libPROVISIONAL holds a flag containing the real crate name:
#
#     REALNAME=path/to/libPROVISIONAL.rlib
#
def extern_arg(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        flags: list[str],
        crate: CrateName.type,
        lib: "artifact") -> cmd_args:
    if flags == []:
        flags = ""
    else:
        flags = ",".join(flags) + ":"

    if crate.dynamic:
        args = ExternArg(flags = flags, lib = lib)
        flagfile = compile_ctx.flagfiles_for_extern.get(args, None)
        if not flagfile:
            flagfile = ctx.actions.declare_output("extern/{}".format(lib.short_path))
            concat_cmd = [
                compile_ctx.toolchain_info.concat_tool,
                "--output",
                flagfile.as_output(),
                "--",
                flags,
                cmd_args("@", crate.dynamic, delimiter = ""),
                "=",
                cmd_args(lib).ignore_artifacts(),
            ]
            ctx.actions.run(
                concat_cmd,
                category = "concat",
                identifier = str(len(compile_ctx.flagfiles_for_extern)),
            )
            compile_ctx.flagfiles_for_extern[args] = flagfile
        return cmd_args("--extern", cmd_args("@", flagfile, delimiter = "")).hidden(lib)
    else:
        return cmd_args("--extern=", flags, crate.simple, "=", lib, delimiter = "")

# Create `--crate-map` flag. For crates with a name computed during analysis:
#
#     --crate-map=NAME=//path/to:target
#
# For crates with a name computed during build:
#
#     --crate-map @cratemap/path/to/target
#
# where cratemap/path/to/target holds a flag containing the real crate name:
#
#     REALNAME=//path/to:target
#
def crate_map_arg(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        crate: CrateName.type,
        label: Label) -> cmd_args:
    if crate.dynamic:
        args = CrateMapArg(label = label)
        flagfile = compile_ctx.flagfiles_for_crate_map.get(args, None)
        if not flagfile:
            flagfile = ctx.actions.declare_output("cratemap/{}/{}/{}".format(label.cell, label.package, label.name))
            concat_cmd = [
                compile_ctx.toolchain_info.concat_tool,
                "--output",
                flagfile.as_output(),
                "--",
                cmd_args("@", crate.dynamic, delimiter = ""),
                "=",
                str(label.raw_target()),
            ]
            ctx.actions.run(
                concat_cmd,
                category = "cratemap",
                identifier = str(len(compile_ctx.flagfiles_for_crate_map)),
            )
            compile_ctx.flagfiles_for_crate_map[args] = flagfile
        return cmd_args("--crate-map", cmd_args("@", flagfile, delimiter = ""))
    else:
        return cmd_args("--crate-map=", crate.simple, "=", str(label.raw_target()), delimiter = "")
