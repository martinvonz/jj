# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# BOLT (Binary Optimization Layout Tool) is a post link profile guided optimizer used for
# performance-critical services in fbcode: https://www.internalfb.com/intern/wiki/HHVM-BOLT/

load("@prelude//:local_only.bzl", "link_cxx_binary_locally")
load(":cxx_context.bzl", "get_cxx_toolchain_info")

def cxx_use_bolt(ctx: AnalysisContext) -> bool:
    cxx_toolchain_info = get_cxx_toolchain_info(ctx)
    return cxx_toolchain_info.bolt_enabled and ctx.attrs.bolt_profile != None

def bolt_gdb_index(ctx: AnalysisContext, bolt_output: "artifact", identifier: [str, None]) -> "artifact":
    # Run gdb-indexer
    # gdb-indexer <input_binary> -o <output_binary>
    gdb_index_output_name = bolt_output.short_path.removesuffix("-pre_gdb_index") + "-gdb_index"
    gdb_index_output = ctx.actions.declare_output(gdb_index_output_name)
    gdb_index_args = cmd_args(
        ctx.attrs.bolt_gdb_index,
        bolt_output,
        "-o",
        gdb_index_output.as_output(),
    )
    ctx.actions.run(
        gdb_index_args,
        category = "gdb_index",
        identifier = identifier,
        local_only = link_cxx_binary_locally(ctx),
    )

    # Run objcopy
    # objcopy -R .gdb_index --add-section=.gdb_index=<gdb_index_binary> <input_binary> <output_binary>
    objcopy_output_name = gdb_index_output_name.removesuffix("-gdb_index")
    objcopy_output = ctx.actions.declare_output(objcopy_output_name)
    objcopy_args = cmd_args(
        get_cxx_toolchain_info(ctx).binary_utilities_info.objcopy,
        "-R",
        ".gdb_index",
        cmd_args(gdb_index_output, format = "--add-section=.gdb_index={}"),
        bolt_output,
        objcopy_output.as_output(),
    )
    ctx.actions.run(
        objcopy_args,
        category = "objcopy",
        identifier = identifier,
        local_only = link_cxx_binary_locally(ctx),
    )

    return objcopy_output

def bolt(ctx: AnalysisContext, prebolt_output: "artifact", identifier: [str, None]) -> "artifact":
    output_name = prebolt_output.short_path.removesuffix("-wrapper") + ("-pre_gdb_index" if (ctx.attrs.bolt_gdb_index != None) else "")
    postbolt_output = ctx.actions.declare_output(output_name)
    bolt_msdk = get_cxx_toolchain_info(ctx).binary_utilities_info.bolt_msdk

    if not bolt_msdk or not cxx_use_bolt(ctx):
        fail("Cannot use bolt if bolt_msdk is not available or bolt profile is not available")
    args = cmd_args()

    # bolt command format:
    # {llvm_bolt} {input_bin} -o $OUT -data={fdata} {args}
    args.add(
        cmd_args(bolt_msdk, format = "{}/bin/llvm-bolt"),
        prebolt_output,
        "-o",
        postbolt_output.as_output(),
        cmd_args(ctx.attrs.bolt_profile, format = "-data={}"),
        ctx.attrs.bolt_flags,
    )

    ctx.actions.run(
        args,
        category = "bolt",
        identifier = identifier,
        local_only = get_cxx_toolchain_info(ctx).linker_info.link_binaries_locally,
    )

    if ctx.attrs.bolt_gdb_index != None:
        return bolt_gdb_index(ctx, postbolt_output, identifier)

    return postbolt_output
