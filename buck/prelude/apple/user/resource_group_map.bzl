# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//apple:resource_groups.bzl",
    "ResourceGroupInfo",
    "create_resource_graph",
    "get_resource_graph_node_map_func",
)
load(
    "@prelude//cxx:groups.bzl",
    "compute_mappings",
    "create_group",
    "parse_groups_definitions",
)
load("@prelude//user:rule_spec.bzl", "RuleRegistrationSpec")
load("@prelude//decls/common.bzl", "Traversal")

def resource_group_map_attr():
    return attrs.option(attrs.dep(providers = [ResourceGroupInfo]), default = None)

def _impl(ctx: AnalysisContext) -> list["provider"]:
    resource_groups = parse_groups_definitions(ctx.attrs.map, lambda root: root.label)

    # Extract deps from the roots via the raw attrs, as `parse_groups_definitions`
    # parses them as labels.
    resource_groups_deps = [
        mapping[0]
        for entry in ctx.attrs.map
        for mapping in entry[1]
    ]
    resource_graph = create_resource_graph(
        ctx = ctx,
        labels = [],
        deps = resource_groups_deps,
        exported_deps = [],
    )
    resource_graph_node_map = get_resource_graph_node_map_func(resource_graph)()
    mappings = compute_mappings(
        groups = [
            create_group(
                group = group,
                # User provided mappings may contain entries that don't support
                # ResourceGraphInfo, which `create_resource_graph` removes above.
                # So make sure we remove them from the mappings too, otherwise
                # `compute_mappings` crashes on the inconsistency.
                mappings = [
                    mapping
                    for mapping in group.mappings
                    if mapping.root == None or mapping.root in resource_graph_node_map
                ],
            )
            for group in resource_groups
        ],
        graph_map = resource_graph_node_map,
    )
    return [
        DefaultInfo(),
        ResourceGroupInfo(
            groups = resource_groups,
            groups_hash = hash(str(resource_groups)),
            mappings = mappings,
            # The consumer of this info may not have deps that cover that labels
            # referenced in our roots, so propagate them here.
            # NOTE(agallagher): We do this to maintain existing behavior here
            # but it's not clear if it's actually desirable behavior.
            implicit_deps = resource_groups_deps,
        ),
    ]

registration_spec = RuleRegistrationSpec(
    name = "resource_group_map",
    impl = _impl,
    attrs = {
        "map": attrs.list(attrs.tuple(attrs.string(), attrs.list(attrs.tuple(attrs.dep(), attrs.enum(Traversal), attrs.option(attrs.string()))))),
    },
)
