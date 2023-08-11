# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")

def strip_debug_info(ctx: AnalysisContext, name: str, obj: "artifact") -> "artifact":
    """
    Strip debug information from an object.
    """
    strip = get_cxx_toolchain_info(ctx).binary_utilities_info.strip
    output = ctx.actions.declare_output("__stripped_objects__", name)
    cmd = cmd_args([strip, "-S", "-o", output.as_output(), obj])
    ctx.actions.run(cmd, category = "strip_debug", identifier = name)
    return output

def strip_shared_library(ctx: AnalysisContext, cxx_toolchain: "CxxToolchainInfo", shared_lib: "artifact", strip_flags: cmd_args, category_suffix: [str, None] = None) -> "artifact":
    """
    Strip unneeded information from a shared library.
    """
    strip = cxx_toolchain.binary_utilities_info.strip
    stripped_lib = ctx.actions.declare_output("stripped/{}".format(shared_lib.short_path))

    # TODO(T109996375) support configuring the flags used for stripping
    cmd = cmd_args()
    cmd.add(strip)
    cmd.add(strip_flags)
    cmd.add([shared_lib, "-o", stripped_lib.as_output()])

    effective_category_suffix = category_suffix if category_suffix else "shared_lib"
    category = "strip_{}".format(effective_category_suffix)

    ctx.actions.run(cmd, category = category, identifier = shared_lib.short_path)

    return stripped_lib
