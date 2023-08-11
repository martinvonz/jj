# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//utils:utils.bzl", "value_or")
load(":cxx_context.bzl", "get_cxx_toolchain_info")

BitcodeBundle = record(
    artifact = field("artifact"),
    # For a thin archive, this contains all the referenced .o files
    external_objects = field(["artifact"]),
)

BitcodeTSet = transitive_set()

BitcodeBundleInfo = provider(fields = [
    "bitcode",
    "bitcode_bundle",
])

def _bundle_locally(ctx: AnalysisContext, linker_info: "LinkerInfo") -> bool:
    archive_locally = linker_info.archive_objects_locally
    if hasattr(ctx.attrs, "_archive_objects_locally_override"):
        return value_or(ctx.attrs._archive_objects_locally_override, archive_locally)
    return archive_locally

def _bundle(ctx: AnalysisContext, name: str, args: cmd_args, prefer_local: bool) -> "artifact":
    llvm_link = get_cxx_toolchain_info(ctx).llvm_link
    if llvm_link == None:
        fail("Bitcode generation not supported when no LLVM linker, the `cxx_toolchain` has no `llvm_link`.")

    bundle_output = ctx.actions.declare_output(name)

    command = cmd_args(llvm_link)
    command.add("-o")
    command.add(bundle_output.as_output())
    command.add(args)

    ctx.actions.run(command, category = "bitcode_bundle", identifier = name, prefer_local = prefer_local)
    return bundle_output

# Creates a static library given a list of object files.
def make_bitcode_bundle(
        ctx: AnalysisContext,
        name: str,
        objects: list["artifact"]) -> [BitcodeBundle.type, None]:
    if len(objects) == 0:
        fail("no objects to archive")

    llvm_link = get_cxx_toolchain_info(ctx).llvm_link
    if llvm_link == None:
        return None

    linker_info = get_cxx_toolchain_info(ctx).linker_info

    bundle = _bundle(ctx, name, cmd_args(objects), _bundle_locally(ctx, linker_info))

    return BitcodeBundle(artifact = bundle, external_objects = objects)

def llvm_link_bitcode_impl(ctx: AnalysisContext) -> list["provider"]:
    llvm_link = get_cxx_toolchain_info(ctx).llvm_link
    if llvm_link == None:
        fail("llvm-link is not provided by toolchain.")

    result = make_bitcode_bundle(ctx, ctx.attrs.name, ctx.attrs.srcs)
    if result != None:
        return [DefaultInfo(default_output = result.artifact), BitcodeBundleInfo(bitcode_bundle = result)]
    else:
        return [DefaultInfo()]
