# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:local_only.bzl", "link_cxx_binary_locally")
load(":cxx_context.bzl", "get_cxx_toolchain_info")
load(":debug.bzl", "SplitDebugMode")

def dwp_available(ctx: AnalysisContext):
    toolchain = get_cxx_toolchain_info(ctx)
    dwp = toolchain.binary_utilities_info.dwp
    split_debug_mode = toolchain.split_debug_mode
    return dwp != None and split_debug_mode != SplitDebugMode("none")

def run_dwp_action(
        ctx: AnalysisContext,
        obj: "artifact",
        identifier: [str, None],
        category_suffix: [str, None],
        referenced_objects: ["_arglike", list["artifact"]],
        dwp_output: "artifact",
        local_only: bool):
    args = cmd_args()
    dwp = get_cxx_toolchain_info(ctx).binary_utilities_info.dwp

    # llvm trunk now supports 64-bit debug cu indedx, add --continue-on-cu-index-overflow by default
    # to suppress dwp file overflow warning
    args.add("/bin/sh", "-c", '"$1" --continue-on-cu-index-overflow -o "$2" -e "$3" && touch "$2"', "")
    args.add(dwp, dwp_output.as_output(), obj)

    # All object/dwo files referenced in the library/executable are implicitly
    # processed by dwp.
    args.hidden(referenced_objects)

    category = "dwp"
    if category_suffix != None:
        category += "_" + category_suffix

    ctx.actions.run(
        args,
        category = category,
        identifier = identifier,
        local_only = local_only,
    )

def dwp(
        ctx: AnalysisContext,
        # Executable/library to extra dwo paths from.
        obj: "artifact",
        # An identifier that will uniquely name this link action in the context of a category. Useful for
        # differentiating multiple link actions in the same rule.
        identifier: [str, None],
        # A category suffix that will be added to the category of the link action that is generated.
        category_suffix: [str, None],
        # All `.o`/`.dwo` paths referenced in `obj`.
        # TODO(T110378122): Ideally, referenced objects are a list of artifacts,
        # but currently we don't track them properly.  So, we just pass in the full
        # link line and extract all inputs from that, which is a bit of an
        # overspecification.
        referenced_objects: ["_arglike", list["artifact"]]) -> "artifact":
    # gdb/lldb expect to find a file named $file.dwp next to $file.
    output = ctx.actions.declare_output(obj.short_path + ".dwp")
    run_dwp_action(
        ctx,
        obj,
        identifier,
        category_suffix,
        referenced_objects,
        output,
        # dwp produces ELF files on the same size scale as the corresponding @obj.
        # The files are a concatenation of input DWARF debug info.
        # Caching dwp has the same issues as caching binaries, so use the same local_only policy.
        local_only = link_cxx_binary_locally(ctx),
    )
    return output
