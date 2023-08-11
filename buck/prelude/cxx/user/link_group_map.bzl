# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//cxx:groups.bzl",
    "parse_groups_definitions",
)
load(
    "@prelude//cxx:link_groups.bzl",
    "LinkGroupInfo",
    "build_link_group_info",
)
load(
    "@prelude//linking:link_groups.bzl",
    "LinkGroupLibInfo",
)
load(
    "@prelude//linking:link_info.bzl",
    "MergedLinkInfo",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableGraph",
    "create_linkable_graph",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
)
load("@prelude//user:rule_spec.bzl", "RuleRegistrationSpec")
load("@prelude//decls/common.bzl", "Linkage", "Traversal")

def _v1_attrs(
        optional_root: bool = False,
        # Whether we should parse `root` fields as a `dependency`, instead of a `label`.
        root_is_dep: bool = True):
    if root_is_dep:
        attrs_root = attrs.dep(providers = [
            LinkGroupLibInfo,
            LinkableGraph,
            MergedLinkInfo,
            SharedLibraryInfo,
        ])
    else:
        attrs_root = attrs.label()

    if optional_root:
        attrs_root = attrs.option(attrs_root)

    return attrs.list(
        attrs.tuple(
            # name
            attrs.string(),
            # list of mappings
            attrs.list(
                # a single mapping
                attrs.tuple(
                    # root node
                    attrs_root,
                    # traversal
                    attrs.enum(Traversal),
                    # filters, either `None`, a single filter, or a list of filters
                    # (which must all match).
                    attrs.option(attrs.one_of(attrs.list(attrs.string()), attrs.string())),
                    # linkage
                    attrs.option(attrs.enum(Linkage)),
                ),
            ),
            # attributes
            attrs.option(
                attrs.dict(key = attrs.string(), value = attrs.any(), sorted = False),
            ),
        ),
    )

def link_group_map_attr():
    v2_attrs = attrs.dep(providers = [LinkGroupInfo])
    return attrs.option(
        attrs.one_of(
            v2_attrs,
            _v1_attrs(
                optional_root = True,
                # Inlined `link_group_map` will parse roots as `label`s, to avoid
                # bloating deps w/ unrelated mappings (e.g. it's common to use
                # a default mapping for all rules, which would otherwise add
                # unrelated deps to them).
                root_is_dep = False,
            ),
        ),
        default = None,
    )

def _impl(ctx: AnalysisContext) -> list["provider"]:
    # Extract graphs from the roots via the raw attrs, as `parse_groups_definitions`
    # parses them as labels.
    linkable_graph = create_linkable_graph(
        ctx,
        children = [
            mapping[0][LinkableGraph]
            for entry in ctx.attrs.map
            for mapping in entry[1]
        ],
    )
    link_groups = parse_groups_definitions(ctx.attrs.map, lambda root: root.label)
    return [
        DefaultInfo(),
        build_link_group_info(linkable_graph, link_groups),
    ]

registration_spec = RuleRegistrationSpec(
    name = "link_group_map",
    impl = _impl,
    attrs = {
        "map": _v1_attrs(),
    },
)
