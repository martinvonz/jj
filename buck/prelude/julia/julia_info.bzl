# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",  # @unused Used as a type
    "merge_shared_libraries",
)

JuliaToolchainInfo = provider(fields = [
    "julia",
    "env",
    "cmd_processor",
])

JllInfo = record(
    name = field(str),
    libs = field(dict),  # Julia name to label
)

JuliaLibrary = record(
    uuid = str,
    src_labels = "",
    srcs = "",
    project_toml = "",
    label = field(Label),
    jll = field([JllInfo.type, None]),
)

def project_load_src_label(lib):
    return lib.src_labels

def project_load_srcs(lib):
    return lib.srcs

JuliaLibraryTSet = transitive_set(
    args_projections = {
        "load_src_label": project_load_src_label,
        "load_srcs": project_load_srcs,
    },
)

# Information about a julia library and its dependencies.
JuliaLibraryInfo = provider(fields = [
    "julia_tsets",  # JuliaLibraryTSet
    "shared_library_info",  # SharedLibraryInfo
])

def create_julia_library_info(
        actions: "actions",
        label: Label,
        uuid: str = "",
        src_labels: "" = [],
        project_toml: "" = None,
        srcs: "" = [],
        deps: list[JuliaLibraryInfo.type] = [],
        jll: [JllInfo.type, None] = None,
        shlibs: list[SharedLibraryInfo.type] = []) -> "JuliaLibraryInfo":
    julia_tsets = JuliaLibrary(
        uuid = uuid,
        label = label,
        src_labels = src_labels,
        srcs = srcs,
        project_toml = project_toml,
        jll = jll,
    )

    return JuliaLibraryInfo(
        julia_tsets = actions.tset(JuliaLibraryTSet, value = julia_tsets, children = [dep.julia_tsets for dep in deps]),
        shared_library_info = merge_shared_libraries(
            actions,
            None,
            [dep.shared_library_info for dep in deps] + shlibs,
        ),
    )
