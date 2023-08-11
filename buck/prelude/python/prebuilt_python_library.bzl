# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//:resources.bzl",
    "ResourceInfo",
    "gather_resources",
)
load(
    "@prelude//cxx:omnibus.bzl",
    "get_excluded",
    "get_roots",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "create_linkable_graph",
    "create_linkable_graph_node",
)
load(":compile.bzl", "compile_manifests")
load(":manifest.bzl", "create_manifest_for_source_dir")
load(
    ":python_library.bzl",
    "create_python_library_info",
    "gather_dep_libraries",
)
load(":source_db.bzl", "create_python_source_db_info", "create_source_db_no_deps_from_manifest")

def prebuilt_python_library_impl(ctx: AnalysisContext) -> list["provider"]:
    providers = []

    # Extract prebuilt wheel and wrap in python library provider.
    # TODO(nmj): Make sure all attrs are used if necessary, esp compile
    extracted_src = ctx.actions.declare_output("{}_extracted".format(ctx.label.name), dir = True)
    ctx.actions.run([ctx.attrs._extract[RunInfo], ctx.attrs.binary_src, "--output", extracted_src.as_output()], category = "py_extract_prebuilt_library")
    deps, shared_deps = gather_dep_libraries([ctx.attrs.deps])
    src_manifest = create_manifest_for_source_dir(ctx, "binary_src", extracted_src, exclude = "\\.pyc$")
    bytecode = compile_manifests(ctx, [src_manifest])
    library_info = create_python_library_info(
        ctx.actions,
        ctx.label,
        srcs = src_manifest,
        src_types = src_manifest,
        bytecode = bytecode,
        deps = deps,
        shared_libraries = shared_deps,
    )
    providers.append(library_info)

    # Create, augment and provide the linkable graph.
    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            roots = get_roots(ctx.label, ctx.attrs.deps),
            excluded = get_excluded(deps = ctx.attrs.deps if ctx.attrs.exclude_deps_from_merged_linking else []),
        ),
        deps = ctx.attrs.deps,
    )
    providers.append(linkable_graph)

    sub_targets = {"source-db-no-deps": [create_source_db_no_deps_from_manifest(ctx, src_manifest), create_python_source_db_info(library_info.manifests)]}
    providers.append(DefaultInfo(default_output = ctx.attrs.binary_src, sub_targets = sub_targets))

    # C++ resources.
    providers.append(ResourceInfo(resources = gather_resources(
        label = ctx.label,
        deps = ctx.attrs.deps,
    )))

    return providers
