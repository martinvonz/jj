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
    "expect",
    "map_val",
    "value_or",
)
load(":compile.bzl", "compile", "get_filtered_srcs")
load(":link.bzl", "link")

def go_binary_impl(ctx: AnalysisContext) -> list["provider"]:
    lib = compile(
        ctx,
        "main",
        get_filtered_srcs(ctx, ctx.attrs.srcs),
        deps = ctx.attrs.deps,
        compile_flags = ctx.attrs.compiler_flags,
    )
    (bin, runtime_files) = link(
        ctx,
        lib,
        deps = ctx.attrs.deps,
        link_style = value_or(map_val(LinkStyle, ctx.attrs.link_style), LinkStyle("static")),
        linker_flags = ctx.attrs.linker_flags,
        link_mode = ctx.attrs.link_mode,
    )

    hidden = []
    for resource in ctx.attrs.resources:
        if type(resource) == "artifact":
            hidden.append(resource)
        else:
            # Otherwise, this is a dependency, so extract the resource and other
            # resources from the `DefaultInfo` provider.
            info = resource[DefaultInfo]
            expect(
                len(info.default_outputs) == 1,
                "expected exactly one default output from {} ({})"
                    .format(resource, info.default_outputs),
            )
            [resource] = info.default_outputs
            other = info.other_outputs

            hidden.append(resource)
            hidden.extend(other)

    return [
        DefaultInfo(
            default_output = bin,
            other_outputs = hidden + runtime_files,
        ),
        RunInfo(args = cmd_args(bin).hidden(hidden + runtime_files)),
    ]
