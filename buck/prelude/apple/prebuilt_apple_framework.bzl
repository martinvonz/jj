# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load(
    "@prelude//cxx:cxx_library_utility.bzl",
    "cxx_attr_exported_linker_flags",
    "cxx_platform_supported",
)
load(
    "@prelude//cxx:preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorArgs",
    "cxx_inherited_preprocessor_infos",
    "cxx_merge_cpreprocessors",
)
load(
    "@prelude//linking:link_groups.bzl",
    "merge_link_group_lib_info",
)
load(
    "@prelude//linking:link_info.bzl",
    "LinkInfo",
    "LinkInfos",
    "LinkStyle",
    "Linkage",
    "create_merged_link_info",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "merge_shared_libraries",
)
load("@prelude//utils:utils.bzl", "filter_and_map_idx")
load(":apple_bundle_types.bzl", "AppleBundleInfo")
load(":apple_frameworks.bzl", "to_framework_name")

def prebuilt_apple_framework_impl(ctx: AnalysisContext) -> list["provider"]:
    providers = []

    framework_directory_artifact = ctx.attrs.framework

    # Check this rule's `supported_platforms_regex` with the current platform.
    if cxx_platform_supported(ctx):
        # Sandbox the framework, to avoid leaking other frameworks via search paths.
        framework_name = to_framework_name(framework_directory_artifact.basename)
        framework_dir = ctx.actions.symlinked_dir(
            "Frameworks",
            {framework_name + ".framework": framework_directory_artifact},
        )

        # Add framework & pp info from deps.
        inherited_pp_info = cxx_inherited_preprocessor_infos(ctx.attrs.deps)
        providers.append(cxx_merge_cpreprocessors(
            ctx,
            [CPreprocessor(relative_args = CPreprocessorArgs(args = ["-F", framework_dir]))],
            inherited_pp_info,
        ))

        # Add framework to link args.
        # TODO(T110378120): Support shared linking for mac targets:
        # https://fburl.com/code/pqrtt1qr.
        args = []
        args.extend(cxx_attr_exported_linker_flags(ctx))
        args.extend(["-F", framework_dir])
        args.extend(["-framework", framework_name])
        link = LinkInfo(
            name = framework_name,
            pre_flags = args,
        )
        providers.append(create_merged_link_info(
            ctx,
            get_cxx_toolchain_info(ctx).pic_behavior,
            {link_style: LinkInfos(default = link) for link_style in LinkStyle},
        ))

        # Create, augment and provide the linkable graph.
        linkable_graph = create_linkable_graph(
            ctx,
            node = create_linkable_graph_node(
                ctx,
                linkable_node = create_linkable_node(
                    ctx,
                    preferred_linkage = Linkage("shared"),
                    link_infos = {LinkStyle("shared"): LinkInfos(default = link)},
                ),
                excluded = {ctx.label: None},
            ),
        )
        providers.append(linkable_graph)

    # The default output is the provided framework.
    providers.append(DefaultInfo(default_output = framework_directory_artifact))
    providers.append(AppleBundleInfo(
        bundle = framework_directory_artifact,
        is_watchos = None,
        skip_copying_swift_stdlib = True,
        contains_watchapp = None,
    ))
    providers.append(merge_link_group_lib_info(deps = ctx.attrs.deps))
    providers.append(merge_shared_libraries(ctx.actions, deps = filter_and_map_idx(SharedLibraryInfo, ctx.attrs.deps)))

    return providers
