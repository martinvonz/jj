# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//linking:link_info.bzl",
    "LinkStyle",
)
load(
    "@prelude//utils:utils.bzl",
    "map_val",
    "value_or",
)
load(":compile.bzl", "compile", "get_filtered_srcs")
load(":link.bzl", "GoBuildMode", "link")

def go_exported_library_impl(ctx: AnalysisContext) -> list["provider"]:
    lib = compile(
        ctx,
        "main",
        get_filtered_srcs(ctx, ctx.attrs.srcs),
        deps = ctx.attrs.deps,
        compile_flags = ctx.attrs.compiler_flags,
        shared = True,
    )
    (bin, runtime_files) = link(
        ctx,
        lib,
        deps = ctx.attrs.deps,
        build_mode = GoBuildMode(ctx.attrs.build_mode),
        link_style = value_or(map_val(LinkStyle, ctx.attrs.link_style), LinkStyle("static_pic")),
        linker_flags = ctx.attrs.linker_flags,
        shared = True,
    )
    return [
        DefaultInfo(
            default_output = bin,
            other_outputs = runtime_files,
        ),
    ]
