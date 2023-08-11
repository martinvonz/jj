# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:artifact_tset.bzl", "make_artifact_tset")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load(":apple_bundle_types.bzl", "AppleBundleBinaryOutput")
load(":apple_toolchain_types.bzl", "AppleToolsInfo")
load(":debug.bzl", "AppleDebuggableInfo")

def create_universal_binary(
        ctx: AnalysisContext,
        binary_deps: dict[str, Dependency],
        binary_name: [str, None],
        dsym_bundle_name: [str, None],
        split_arch_dsym: bool) -> AppleBundleBinaryOutput.type:
    binary_output = ctx.actions.declare_output("UniversalBinary" if binary_name == None else binary_name, dir = False)
    lipo_cmd = cmd_args([ctx.attrs._apple_toolchain[AppleToolchainInfo].lipo])

    for (_, binary) in binary_deps.items():
        lipo_cmd.add(cmd_args(binary[DefaultInfo].default_outputs[0]))

    lipo_cmd.add(["-create", "-output", binary_output.as_output()])
    ctx.actions.run(lipo_cmd, category = "lipo")

    dsym_output = None
    if split_arch_dsym:
        dsym_output = ctx.actions.declare_output("UniversalBinary.dSYM" if dsym_bundle_name == None else dsym_bundle_name, dir = True)
        dsym_combine_cmd = cmd_args([ctx.attrs._apple_tools[AppleToolsInfo].split_arch_combine_dsym_bundles_tool])

        for (arch, binary) in binary_deps.items():
            dsym_combine_cmd.add(["--dsym-bundle", cmd_args(binary.get(AppleDebuggableInfo).dsyms[0]), "--arch", arch])
        dsym_combine_cmd.add(["--output", dsym_output.as_output()])
        ctx.actions.run(dsym_combine_cmd, category = "universal_binaries_dsym")

    all_debug_info_tsets = [binary.get(AppleDebuggableInfo).debug_info_tset for binary in binary_deps.values()]

    return AppleBundleBinaryOutput(
        binary = binary_output,
        debuggable_info =
            AppleDebuggableInfo(
                dsyms = [dsym_output] if dsym_output != None else [],
                debug_info_tset = make_artifact_tset(
                    actions = ctx.actions,
                    label = ctx.label,
                    children = filter(None, all_debug_info_tsets),
                ),
            ),
    )
