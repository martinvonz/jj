# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//utils:utils.bzl",
    "expect",
)
load(
    ":link_groups.bzl",
    "LinkGroupLibInfo",
)
load(
    ":link_info.bzl",
    "MergedLinkInfo",
)
load(
    ":linkable_graph.bzl",
    "LinkableGraph",
    "LinkableRootInfo",
)
load(
    ":shared_libraries.bzl",
    "SharedLibraryInfo",
)

# A record containing all provider types used for linking in the prelude.  This
# is essentially the minimal subset of a "linkable" `dependency` that user rules
# need to implement linking, and avoids needing functions to take the heavier-
# weight `dependency` type.
LinkableProviders = record(
    link_group_lib_info = field(LinkGroupLibInfo.type),
    linkable_graph = field([LinkableGraph.type, None], None),
    merged_link_info = field(MergedLinkInfo.type),
    shared_library_info = field(SharedLibraryInfo.type),
    linkable_root_info = field([LinkableRootInfo.type, None], None),
)

def linkable(dep: Dependency) -> LinkableProviders.type:
    expect(LinkGroupLibInfo in dep, str(dep.label.raw_target()))
    return LinkableProviders(
        shared_library_info = dep[SharedLibraryInfo],
        linkable_graph = dep.get(LinkableGraph),
        merged_link_info = dep[MergedLinkInfo],
        link_group_lib_info = dep[LinkGroupLibInfo],
        linkable_root_info = dep.get(LinkableRootInfo),
    )

def linkables(deps: list[Dependency]) -> list[LinkableProviders.type]:
    return [linkable(dep) for dep in deps if MergedLinkInfo in dep]
