# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":apple_resource_types.bzl", "AppleResourceDestination", "AppleResourceSpec")
load(":resource_groups.bzl", "create_resource_graph")

def apple_resource_impl(ctx: AnalysisContext) -> list["provider"]:
    destination = ctx.attrs.destination or "resources"
    resource_spec = AppleResourceSpec(
        files = ctx.attrs.files,
        dirs = ctx.attrs.dirs,
        content_dirs = ctx.attrs.content_dirs,
        destination = AppleResourceDestination(destination),
        variant_files = ctx.attrs.variants or [],
        named_variant_files = ctx.attrs.named_variants or {},
        codesign_files_on_copy = ctx.attrs.codesign_on_copy,
    )

    # `files` can contain `apple_library()` which in turn can have `apple_resource()` deps
    file_deps = [file_or_dep for file_or_dep in ctx.attrs.files if type(file_or_dep) == "dependency"]
    deps = file_deps + ctx.attrs.resources_from_deps
    graph = create_resource_graph(
        ctx = ctx,
        labels = ctx.attrs.labels,
        deps = deps,
        exported_deps = [],
        resource_spec = resource_spec,
    )
    return [DefaultInfo(
        sub_targets = {
            "headers": [
                DefaultInfo(default_outputs = []),
            ],
        },
    ), graph]
