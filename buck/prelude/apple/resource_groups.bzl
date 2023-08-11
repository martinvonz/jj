# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//cxx:groups.bzl",
    "MATCH_ALL_LABEL",
)
load(
    "@prelude//utils:graph_utils.bzl",
    "breadth_first_traversal_by",
)
load(":apple_asset_catalog_types.bzl", "AppleAssetCatalogSpec")
load(":apple_core_data_types.bzl", "AppleCoreDataSpec")
load(":apple_resource_types.bzl", "AppleResourceSpec")
load(":scene_kit_assets_types.bzl", "SceneKitAssetsSpec")

ResourceGroupInfo = provider(fields = [
    "groups",  # [Group.type]
    "groups_hash",  # str
    "mappings",  # {"label": str}
    # Additional deps needed to cover labels referenced by the groups above.
    # This is useful in cases where the consumer of this provider won't already
    # have deps covering these.
    # NOTE(agallagher): We do this to maintain existing behavior w/ the
    # standalone `resource_group_map()` rule, but it's not clear if it's
    # actually desirable behavior.
    "implicit_deps",  # [Dependency]
])

ResourceGraphNode = record(
    label = field(Label),
    # Attribute labels on the target.
    labels = field([str], []),
    # Deps of this target which might have resources transitively.
    deps = field([Label], []),
    # Exported deps of this target which might have resources transitively.
    exported_deps = field([Label], []),
    # Actual resource data, present when node corresponds to `apple_resource` target.
    resource_spec = field([AppleResourceSpec.type, None], None),
    # Actual asset catalog data, present when node corresponds to `apple_asset_catalog` target.
    asset_catalog_spec = field([AppleAssetCatalogSpec.type, None], None),
    # Actual core data, present when node corresponds to `core_data_model` target
    core_data_spec = field([AppleCoreDataSpec.type, None], None),
    # Actual scene kit assets, present when node corresponds to `scene_kit_assets` target
    scene_kit_assets_spec = field([SceneKitAssetsSpec.type, None], None),
)

ResourceGraphTSet = transitive_set()

ResourceGraphInfo = provider(fields = [
    "label",  # "label"
    "nodes",  # "ResourceGraphTSet"
    "should_propagate",  # bool
])

def create_resource_graph(
        ctx: AnalysisContext,
        labels: list[str],
        deps: list[Dependency],
        exported_deps: list[Dependency],
        bundle_binary: [Dependency, None] = None,
        resource_spec: [AppleResourceSpec.type, None] = None,
        asset_catalog_spec: [AppleAssetCatalogSpec.type, None] = None,
        core_data_spec: [AppleCoreDataSpec.type, None] = None,
        scene_kit_assets_spec: [SceneKitAssetsSpec.type, None] = None,
        should_propagate: bool = True) -> ResourceGraphInfo.type:
    # Collect deps and exported_deps with resources that should propagate.
    dep_labels, dep_graphs = _filtered_labels_and_graphs(deps)
    exported_dep_labels, exported_dep_graphs = _filtered_labels_and_graphs(exported_deps)

    # Bundle binary targets always propagate resources to their bundle.
    # The bundle target will not pass up a ResourceGraphInfo provider itself
    # so the resources do not propagate outside the bundle folder.
    if bundle_binary and ResourceGraphInfo in bundle_binary:
        dep_graphs.append(bundle_binary[ResourceGraphInfo])

        # We use ResourceGraphInfo.label here to ensure the graph lookup works
        # when we have binary targets specified with the [shared] subtarget.
        dep_labels.append(bundle_binary[ResourceGraphInfo].label)

    node = ResourceGraphNode(
        label = ctx.label,
        labels = labels,
        deps = dep_labels,
        exported_deps = exported_dep_labels,
        resource_spec = resource_spec,
        asset_catalog_spec = asset_catalog_spec,
        core_data_spec = core_data_spec,
        scene_kit_assets_spec = scene_kit_assets_spec,
    )
    children = [child_node.nodes for child_node in dep_graphs + exported_dep_graphs]
    return ResourceGraphInfo(
        label = ctx.label,
        nodes = ctx.actions.tset(ResourceGraphTSet, value = node, children = children),
        should_propagate = should_propagate,
    )

def get_resource_graph_node_map_func(graph: ResourceGraphInfo.type):
    def get_resource_graph_node_map() -> dict[Label, ResourceGraphNode.type]:
        nodes = graph.nodes.traverse()
        return {node.label: node for node in filter(None, nodes)}

    return get_resource_graph_node_map

def _filtered_labels_and_graphs(deps: list[Dependency]) -> (list[Label], list[ResourceGraphInfo.type]):
    """
    Filters dependencies and returns only those which are relevant
    to working with resources i.e. those which contains resource graph provider
    and that should propagate.
    """
    resource_labels = []
    resource_deps = []
    for d in deps:
        graph = d.get(ResourceGraphInfo)
        if graph and graph.should_propagate:
            resource_deps.append(graph)
            resource_labels.append(graph.label)

    return resource_labels, resource_deps

def get_resource_group_info(ctx: AnalysisContext) -> [ResourceGroupInfo.type, None]:
    """
    Parses the currently analyzed context for any resource group definitions
    and returns a list of all resource groups with their mappings.
    """
    resource_group_map = ctx.attrs.resource_group_map

    if not resource_group_map:
        return None

    if type(resource_group_map) == "dependency":
        return resource_group_map[ResourceGroupInfo]

    fail("Resource group maps must be provided as a resource_group_map rule dependency.")

def get_filtered_resources(
        root: Label,
        resource_graph_node_map_func,
        resource_group: [str, None],
        resource_group_mappings: [dict[Label, str], None]) -> (list[AppleResourceSpec.type], list[AppleAssetCatalogSpec.type], list[AppleCoreDataSpec.type], list[SceneKitAssetsSpec.type]):
    """
    Walks the provided DAG and collects resources matching resource groups definition.
    """

    resource_graph_node_map = resource_graph_node_map_func()

    def get_traversed_deps(target: Label) -> list[Label]:
        node = resource_graph_node_map[target]  # buildifier: disable=uninitialized
        return node.exported_deps + node.deps

    targets = breadth_first_traversal_by(
        resource_graph_node_map,
        get_traversed_deps(root),
        get_traversed_deps,
    )

    resource_specs = []
    asset_catalog_specs = []
    core_data_specs = []
    scene_kit_assets_specs = []

    for target in targets:
        target_resource_group = resource_group_mappings.get(target)

        # Ungrouped targets belong to the unlabeled bundle
        if ((not target_resource_group and not resource_group) or
            # Does it match special "MATCH_ALL" mapping?
            target_resource_group == MATCH_ALL_LABEL or
            # Does it match currently evaluated group?
            target_resource_group == resource_group):
            node = resource_graph_node_map[target]
            resource_spec = node.resource_spec
            if resource_spec:
                resource_specs.append(resource_spec)
            asset_catalog_spec = node.asset_catalog_spec
            if asset_catalog_spec:
                asset_catalog_specs.append(asset_catalog_spec)
            core_data_spec = node.core_data_spec
            if core_data_spec:
                core_data_specs.append(core_data_spec)
            scene_kit_assets_spec = node.scene_kit_assets_spec
            if scene_kit_assets_spec:
                scene_kit_assets_specs.append(scene_kit_assets_spec)

    return resource_specs, asset_catalog_specs, core_data_specs, scene_kit_assets_specs
