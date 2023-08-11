# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//linking:link_info.bzl",
    "Linkage",
)
load(
    "@prelude//utils:build_target_pattern.bzl",
    "BuildTargetPattern",
    "parse_build_target_pattern",
)
load(
    "@prelude//utils:graph_utils.bzl",
    "breadth_first_traversal_by",
)
load(
    "@prelude//utils:strings.bzl",
    "strip_prefix",
)
load(
    "@prelude//utils:utils.bzl",
    "map_val",
    "value_or",
)

# Types of group traversal
Traversal = enum(
    # Includes the target and all of it's transitive dependencies in the group.
    "tree",
    # Includes only the target in the group.
    "node",
)

# Optional type of filtering
FilterType = enum(
    # Filters for targets with labels matching the regex pattern defined after `label:`.
    "label",
    # Filters for targets for the build target pattern defined after "pattern:".
    "pattern",
)

BuildTargetFilter = record(
    pattern = field(BuildTargetPattern.type),
    _type = field(FilterType.type, FilterType("pattern")),
)

LabelFilter = record(
    regex = field("regex"),
    _type = field(FilterType.type, FilterType("label")),
)

# Label for special group mapping which makes every target associated with it to be included in all groups
MATCH_ALL_LABEL = "MATCH_ALL"

# Label for special group mapping which makes every target associated with it to be linked directly
# against the final binary
NO_MATCH_LABEL = "NO_MATCH"

# Representation of a parsed group mapping
GroupMapping = record(
    # The root to apply this mapping to.
    root = field([Label, None], None),
    # The type of traversal to use.
    traversal = field(Traversal.type, Traversal("tree")),
    # Optional filter type to apply to the traversal.
    filters = field([[BuildTargetFilter.type, LabelFilter.type]], []),
    # Preferred linkage for this target when added to a link group.
    preferred_linkage = field([Linkage.type, None], None),
)

_VALID_ATTRS = [
    "enable_distributed_thinlto",
    "enable_if_node_count_exceeds",
    "exported_linker_flags",
    "discard_group",
    "linker_flags",
]

# Representation of group attributes
GroupAttrs = record(
    # Use distributed thinlto to build the link group shared library.
    enable_distributed_thinlto = field(bool, False),
    # Enable this link group if the binary's node count exceeds the given threshold
    enable_if_node_count_exceeds = field([int, None], None),
    # Discard all dependencies in the link group, useful for dropping unused dependencies
    # from the build graph.
    discard_group = field(bool, False),
    # Adds additional linker flags used to link the link group shared object.
    linker_flags = field(list, []),
    # Adds additional linker flags to apply to dependents that link against the
    # link group's shared object.
    exported_linker_flags = field(list, []),
)

# Representation of a parsed group
Group = record(
    # The name for this group.
    name = str,
    # The mappings that are part of this group.
    mappings = [GroupMapping.type],
    attrs = GroupAttrs.type,
)

# Creates a group from an existing group, overwriting any properties provided
def create_group(
        group: Group.type,
        name: [None, str] = None,
        mappings: [None, list[GroupMapping.type]] = None,
        attrs: [None, GroupAttrs.type] = None):
    return Group(
        name = value_or(name, group.name),
        mappings = value_or(mappings, group.mappings),
        attrs = value_or(attrs, group.attrs),
    )

def parse_groups_definitions(
        map: list,
        # Function to parse a root label from the input type, allowing different
        # callers to have different top-level types for the `root`s.
        parse_root: "function" = lambda d: d) -> list[Group.type]:
    groups = []
    for map_entry in map:
        name = map_entry[0]
        mappings = map_entry[1]
        attrs = (map_entry[2] or {}) if len(map_entry) > 2 else {}

        for attr in attrs:
            if attr not in _VALID_ATTRS:
                fail("invalid attr '{}' for link group '{}' found. Valid attributes are {}.".format(attr, name, _VALID_ATTRS))
        group_attrs = GroupAttrs(
            enable_distributed_thinlto = attrs.get("enable_distributed_thinlto", False),
            enable_if_node_count_exceeds = attrs.get("enable_if_node_count_exceeds", None),
            exported_linker_flags = attrs.get("exported_linker_flags", []),
            discard_group = attrs.get("discard_group", False),
            linker_flags = attrs.get("linker_flags", []),
        )

        parsed_mappings = []
        for entry in mappings:
            traversal = _parse_traversal_from_mapping(entry[1])
            mapping = GroupMapping(
                root = map_val(parse_root, entry[0]),
                traversal = traversal,
                filters = _parse_filter_from_mapping(entry[2]),
                preferred_linkage = Linkage(entry[3]) if len(entry) > 3 and entry[3] else None,
            )
            parsed_mappings.append(mapping)

        group = Group(name = name, mappings = parsed_mappings, attrs = group_attrs)
        groups.append(group)

    return groups

def _parse_traversal_from_mapping(entry: str) -> Traversal.type:
    if entry == "tree":
        return Traversal("tree")
    elif entry == "node":
        return Traversal("node")
    else:
        fail("Unrecognized group traversal type: " + entry)

def _parse_filter(entry: str) -> [BuildTargetFilter.type, LabelFilter.type]:
    for prefix in ("label:", "tag:"):
        label_regex = strip_prefix(prefix, entry)
        if label_regex != None:
            # We need the anchors "^"" and "$" because experimental_regex match
            # anywhere in the text, while we want full text match for group label
            # text.
            return LabelFilter(
                regex = experimental_regex("^{}$".format(label_regex)),
            )

    pattern = strip_prefix("pattern:", entry)
    if pattern != None:
        return BuildTargetFilter(
            pattern = parse_build_target_pattern(pattern),
        )

    fail("Invalid group mapping filter: {}\nFilter must begin with `label:`, `tag:`, or `pattern:`.".format(entry))

def _parse_filter_from_mapping(entry: [list[str], str, None]) -> list[[BuildTargetFilter.type, LabelFilter.type]]:
    if type(entry) == type([]):
        return [_parse_filter(e) for e in entry]
    if type(entry) == type(""):
        return [_parse_filter(entry)]
    return []

def compute_mappings(groups: list[Group.type], graph_map: dict[Label, "_b"]) -> dict[Label, str]:
    """
    Returns the group mappings {target label -> group name} based on the provided groups and graph.
    """
    if not groups:
        return {}

    target_to_group_map = {}
    node_traversed_targets = {}

    for group in groups:
        for mapping in group.mappings:
            targets_in_mapping = _find_targets_in_mapping(graph_map, mapping)
            for target in targets_in_mapping:
                # If the target doesn't exist in our graph, skip the mapping.
                if target not in graph_map:
                    continue
                _update_target_to_group_mapping(graph_map, target_to_group_map, node_traversed_targets, group.name, mapping, target)

    return target_to_group_map

def _find_targets_in_mapping(
        graph_map: dict[Label, "_b"],
        mapping: GroupMapping.type) -> list[Label]:
    # If we have no filtering, we don't need to do any traversal to find targets to include.
    if not mapping.filters:
        if mapping.root == None:
            fail("no filter or explicit root given: {}", mapping)
        return [mapping.root]

    # Else find all dependencies that match the filter.
    matching_targets = {}

    def any_labels_match(regex, labels):
        # Use a for loop to avoid creating a temporary array in a BFS.
        for label in labels:
            if regex.match(label):
                return True
        return False

    def matches_target(
            target,  # "label"
            labels) -> bool:  # labels: [str]
        # All filters must match to select this node.
        for filter in mapping.filters:
            if filter._type == FilterType("label"):
                if not any_labels_match(filter.regex, labels):
                    return False
            elif not filter.pattern.matches(target):
                return False
        return True

    def find_matching_targets(node):  # Label -> [Label]:
        graph_node = graph_map[node]
        if matches_target(node, graph_node.labels):
            matching_targets[node] = None
            if mapping.traversal == Traversal("tree"):
                # We can stop traversing the tree at this point because we've added the
                # build target to the list of all targets that will be traversed by the
                # algorithm that applies the groups.
                return []
        return graph_node.deps + graph_node.exported_deps

    if mapping.root == None:
        for node in graph_map:
            find_matching_targets(node)
    else:
        breadth_first_traversal_by(graph_map, [mapping.root], find_matching_targets)

    return matching_targets.keys()

# Types removed to avoid unnecessary type checking which degrades performance.
def _update_target_to_group_mapping(
        graph_map,  # {"label": "_b"}
        target_to_group_map,  #: {"label": str}
        node_traversed_targets,  #: {"label": None}
        group,  #  str,
        mapping,  # GroupMapping.type,
        target):  # Label
    def assign_target_to_group(
            target: Label,
            node_traversal: bool) -> bool:
        # If the target hasn't already been assigned to a group, assign it to the
        # first group claiming the target. Return whether the target was already assigned.
        if target not in target_to_group_map:
            target_to_group_map[target] = group
            if node_traversal:
                node_traversed_targets[target] = None
            return False
        else:
            return True

    def transitively_add_targets_to_group_mapping(node: Label) -> list[Label]:
        previously_processed = assign_target_to_group(target = node, node_traversal = False)

        # If the node has been previously processed, and it was via tree (not node), all child nodes have been assigned
        if previously_processed and node not in node_traversed_targets:
            return []
        graph_node = graph_map[node]
        return graph_node.deps + graph_node.exported_deps

    if mapping.traversal == Traversal("node"):
        assign_target_to_group(target = target, node_traversal = True)
    else:  # tree
        breadth_first_traversal_by(graph_map, [target], transitively_add_targets_to_group_mapping)
