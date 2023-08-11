# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//cxx:debug.bzl", "SplitDebugMode")

# Styles of LTO.
LtoMode = enum(
    # No LTO
    "none",
    # Object files contain both LTO IR and native code to allow binaries to link
    # either via standard or LTO.
    "fat",
    # Traditional, monolithic LTO.
    "monolithic",
    # https://clang.llvm.org/docs/ThinLTO.html
    "thin",
)

def lto_compiler_flags(lto_mode: "LtoMode") -> list[str]:
    if lto_mode == LtoMode("none"):
        return []
    elif lto_mode == LtoMode("fat") or lto_mode == LtoMode("monolithic"):
        return ["-flto=full"]
    elif lto_mode == LtoMode("thin"):
        return ["-flto=thin"]
    else:
        fail("Unhandled LTO mode: " + repr(lto_mode))

SplitDebugLtoInfo = record(
    # The output that the linker will generate split debug object(s) too.  May
    # be a single object file (e.g. for darwin+monoLTO) or a directory.
    output = field("artifact"),
    # The flags to add to the link to use the above output.
    linker_flags = field(cmd_args),
)

def get_split_debug_lto_info(ctx: AnalysisContext, name: str) -> [SplitDebugLtoInfo.type, None]:
    cxx_toolchain = get_cxx_toolchain_info(ctx)
    linker_info = cxx_toolchain.linker_info

    # We only generate these flags for LTO+DWO.
    if cxx_toolchain.split_debug_mode == SplitDebugMode("none"):
        return None
    if cxx_toolchain.linker_info.lto_mode == LtoMode("none"):
        return None

    # TODO: It might be nice to generalize a but more and move the darwin v. gnu
    # differences into toolchain settings (e.g. `split_debug_lto_flags_fmt`).
    if linker_info.type == "darwin":
        # https://releases.llvm.org/14.0.0/tools/clang/docs/CommandGuide/clang.html#cmdoption-flto
        # We need to pass -object_path_lto to keep the temporary LTO object files around to use
        # for dSYM generation.
        if linker_info.lto_mode == LtoMode("thin"):
            # For thin LTO the path is a folder that will contain the various object files.
            lto_object_path_artifact = ctx.actions.declare_output("lto_object_files", dir = True)
        else:
            # For monolithic LTO the path is a single object file.
            lto_object_path_artifact = ctx.actions.declare_output("lto_object_file.o", dir = False)

        linker_args = cmd_args([
            # Use -Xlinker in case the path has a ,
            "-Xlinker",
            "-object_path_lto",
            "-Xlinker",
            lto_object_path_artifact.as_output(),
        ])

        return SplitDebugLtoInfo(
            output = lto_object_path_artifact,
            linker_flags = linker_args,
        )

    if linker_info.type == "gnu":
        dwo_dir = ctx.actions.declare_output(name + ".dwo.d", dir = True)

        linker_flags = cmd_args([
            "-Xlinker",
            "-plugin-opt",
            "-Xlinker",
            # NOTE: can't use -Wl,plugin-opt,dwo_dir=... because the $(output ...)
            # macro might be expanded to a value with commas breaking -Wl, parsing.
            cmd_args(dwo_dir.as_output(), format = "dwo_dir={}"),
        ])

        return SplitDebugLtoInfo(
            output = dwo_dir,
            linker_flags = linker_flags,
        )

    fail("can't handle split-debug+LTO for linker type: {}".format(linker_info.type))
