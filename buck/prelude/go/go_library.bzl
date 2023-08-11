# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//linking:link_groups.bzl",
    "LinkGroupLibInfo",
)
load(
    "@prelude//linking:link_info.bzl",
    "MergedLinkInfo",
    "merge_link_infos",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "merge_shared_libraries",
)
load(
    "@prelude//utils:utils.bzl",
    "map_idx",
)
load(":compile.bzl", "GoPkgCompileInfo", "GoTestInfo", "compile", "get_filtered_srcs", "get_inherited_compile_pkgs")
load(":link.bzl", "GoPkgLinkInfo", "get_inherited_link_pkgs")
load(":packages.bzl", "GoPkg", "go_attr_pkg_name", "merge_pkgs")

def go_library_impl(ctx: AnalysisContext) -> list["provider"]:
    pkgs = {}
    default_output = None
    pkg_name = None
    if ctx.attrs.srcs:
        pkg_name = go_attr_pkg_name(ctx)
        srcs = get_filtered_srcs(ctx, ctx.attrs.srcs)

        static_pkg = compile(
            ctx,
            pkg_name,
            srcs = srcs,
            deps = ctx.attrs.deps + ctx.attrs.exported_deps,
            compile_flags = ctx.attrs.compiler_flags,
            assemble_flags = ctx.attrs.assembler_flags,
            shared = False,
        )

        shared_pkg = compile(
            ctx,
            pkg_name,
            srcs = srcs,
            deps = ctx.attrs.deps + ctx.attrs.exported_deps,
            compile_flags = ctx.attrs.compiler_flags,
            assemble_flags = ctx.attrs.assembler_flags,
            shared = True,
        )

        default_output = static_pkg
        pkgs[pkg_name] = GoPkg(
            shared = shared_pkg,
            static = static_pkg,
        )

    return [
        DefaultInfo(default_output = default_output),
        LinkGroupLibInfo(libs = {}),
        GoPkgCompileInfo(pkgs = merge_pkgs([
            pkgs,
            get_inherited_compile_pkgs(ctx.attrs.exported_deps),
        ])),
        GoPkgLinkInfo(pkgs = merge_pkgs([
            pkgs,
            get_inherited_link_pkgs(ctx.attrs.deps + ctx.attrs.exported_deps),
        ])),
        GoTestInfo(
            deps = ctx.attrs.deps,
            srcs = ctx.attrs.srcs,
            pkg_name = pkg_name,
        ),
        merge_link_infos(ctx, filter(None, [d.get(MergedLinkInfo) for d in ctx.attrs.deps])),
        merge_shared_libraries(
            ctx.actions,
            deps = filter(None, map_idx(SharedLibraryInfo, ctx.attrs.deps)),
        ),
    ]
