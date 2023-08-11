# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxPlatformInfo")
load("@prelude//linking:shared_libraries.bzl", "traverse_shared_library_info")
load("@prelude//utils:utils.bzl", "flatten")
load(":compile.bzl", "PycInvalidationMode")
load(":interface.bzl", "PythonLibraryInterface", "PythonLibraryManifestsInterface")
load(":manifest.bzl", "ManifestInfo")
load(":toolchain.bzl", "PythonPlatformInfo", "get_platform_attr")

PythonLibraryManifests = record(
    label = field(Label),
    srcs = field([ManifestInfo.type, None]),
    src_types = field([ManifestInfo.type, None], None),
    resources = field([(ManifestInfo.type, ["_arglike"]), None]),
    bytecode = field([{PycInvalidationMode.type: ManifestInfo.type}, None]),
    dep_manifest = field([ManifestInfo.type, None]),
    extensions = field([{str: "_a"}, None]),
)

def _bytecode_artifacts(invalidation_mode: PycInvalidationMode.type):
    return lambda value: [] if value.bytecode == None else (
        [a for a, _ in value.bytecode[invalidation_mode].artifacts]
    )

def _bytecode_manifests(invalidation_mode: PycInvalidationMode.type):
    return lambda value: [] if value.bytecode == None else (
        value.bytecode[invalidation_mode].manifest
    )

def _dep_manifests(value: PythonLibraryManifests.type):
    if value.dep_manifest == None:
        return []
    return cmd_args(value.dep_manifest.manifest, format = "--manifest={}")

def _dep_artifacts(value: PythonLibraryManifests.type):
    if value.dep_manifest == None:
        return []
    return value.dep_manifest.artifacts

def _hidden_resources(value: PythonLibraryManifests.type):
    if value.resources == None:
        return []
    return value.resources[1]

def _has_hidden_resources(children: list[bool], value: [PythonLibraryManifests.type, None]):
    if value:
        if value.resources and len(value.resources[1]) > 0:
            return True
    return any(children)

def _resource_manifests(value: PythonLibraryManifests.type):
    if value.resources == None:
        return []
    return value.resources[0].manifest

def _resource_artifacts(value: PythonLibraryManifests.type):
    if value.resources == None:
        return []
    return [a for a, _ in value.resources[0].artifacts]

def _source_manifests(value: PythonLibraryManifests.type):
    if value.srcs == None:
        return []
    return value.srcs.manifest

def _source_artifacts(value: PythonLibraryManifests.type):
    if value.srcs == None:
        return []
    return [a for a, _ in value.srcs.artifacts]

def _source_type_manifests(value: PythonLibraryManifests.type):
    if value.src_types == None:
        return []
    return value.src_types.manifest

def _source_type_manifest_jsons(value: PythonLibraryManifests.type):
    if value.src_types == None:
        return None
    return (value.label.raw_target(), value.src_types.manifest)

def _source_type_artifacts(value: PythonLibraryManifests.type):
    if value.src_types == None:
        return []
    return [a for a, _ in value.src_types.artifacts]

_BYTECODE_PROJ_PREFIX = {
    PycInvalidationMode("CHECKED_HASH"): "checked_bytecode",
    PycInvalidationMode("UNCHECKED_HASH"): "bytecode",
}

PythonLibraryManifestsTSet = transitive_set(
    args_projections = dict({
        "dep_artifacts": _dep_artifacts,
        "dep_manifests": _dep_manifests,
        "hidden_resources": _hidden_resources,
        "resource_artifacts": _resource_artifacts,
        "resource_manifests": _resource_manifests,
        "source_artifacts": _source_artifacts,
        "source_manifests": _source_manifests,
        "source_type_artifacts": _source_type_artifacts,
        "source_type_manifests": _source_type_manifests,
    }.items() + {
        "{}_artifacts".format(prefix): _bytecode_artifacts(mode)
        for mode, prefix in _BYTECODE_PROJ_PREFIX.items()
    }.items() + {
        "{}_manifests".format(prefix): _bytecode_manifests(mode)
        for mode, prefix in _BYTECODE_PROJ_PREFIX.items()
    }.items()),
    json_projections = {
        "source_type_manifests_json": _source_type_manifest_jsons,
    },
    reductions = {
        "has_hidden_resources": _has_hidden_resources,
    },
)

# Information about a python library and its dependencies.
# TODO(nmj): Resources in general, and mapping of resources to new paths too.
PythonLibraryInfo = provider(fields = [
    "manifests",  # PythonLibraryManifestsTSet
    "shared_libraries",  # "SharedLibraryInfo"
])

def info_to_interface(info: PythonLibraryInfo.type) -> PythonLibraryInterface.type:
    return PythonLibraryInterface(
        shared_libraries = lambda: traverse_shared_library_info(info.shared_libraries),
        iter_manifests = lambda: info.manifests.traverse(),
        manifests = lambda: manifests_to_interface(info.manifests),
        has_hidden_resources = lambda: info.manifests.reduce("has_hidden_resources"),
        hidden_resources = lambda: [info.manifests.project_as_args("hidden_resources")],
    )

def manifests_to_interface(manifests: PythonLibraryManifestsTSet.type) -> PythonLibraryManifestsInterface.type:
    return PythonLibraryManifestsInterface(
        src_manifests = lambda: [manifests.project_as_args("source_manifests")],
        src_artifacts = lambda: [manifests.project_as_args("source_artifacts")],
        src_artifacts_with_paths = lambda: [(a, p) for m in manifests.traverse() if m != None and m.srcs != None for a, p in m.srcs.artifacts],
        src_type_manifests = lambda: [manifests.project_as_args("source_manifests")],
        src_type_artifacts = lambda: [manifests.project_as_args("source_artifacts")],
        src_type_artifacts_with_path = lambda: [(a, p) for m in manifests.traverse() if m != None and m.src_types != None for a, p in m.src_types.artifacts],
        bytecode_manifests = lambda mode: [manifests.project_as_args("{}_manifests".format(_BYTECODE_PROJ_PREFIX[mode]))],
        bytecode_artifacts = lambda mode: [manifests.project_as_args("{}_artifacts".format(_BYTECODE_PROJ_PREFIX[mode]))],
        bytecode_artifacts_with_paths = lambda mode: [(a, p) for m in manifests.traverse() if m != None and m.bytecode != None for a, p in m.bytecode[mode].artifacts],
        resource_manifests = lambda: [manifests.project_as_args("resource_manifests")],
        resource_artifacts = lambda: [manifests.project_as_args("resource_artifacts")],
        resource_artifacts_with_paths = lambda: [(a, p) for m in manifests.traverse() if m != None and m.resources != None for a, p in m.resources[0].artifacts],
    )

def get_python_deps(ctx: AnalysisContext):
    python_platform = ctx.attrs._python_toolchain[PythonPlatformInfo]
    cxx_platform = ctx.attrs._cxx_toolchain[CxxPlatformInfo]
    return flatten(
        [ctx.attrs.deps] +
        get_platform_attr(python_platform, cxx_platform, ctx.attrs.platform_deps),
    )
