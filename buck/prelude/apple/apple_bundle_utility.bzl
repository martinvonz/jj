# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//utils:utils.bzl", "flatten", "value_or")
load(":apple_bundle_types.bzl", "AppleBundleLinkerMapInfo", "AppleMinDeploymentVersionInfo")
load(":apple_resource_types.bzl", "AppleResourceProcessingOptions")
load(":apple_target_sdk_version.bzl", "get_min_deployment_version_for_node")
load(":apple_toolchain_types.bzl", "AppleToolchainInfo")
load(":resource_groups.bzl", "ResourceGraphInfo")

# `ctx` in all functions below is expected to be of `apple_bundle` or `apple_test` rule

def _get_bundle_target_name(ctx: AnalysisContext):
    if hasattr(ctx.attrs, "_bundle_target_name"):
        # `apple_resource_bundle` rules are proxies for the real rules,
        # so make sure we return the real target name rather the proxy one
        return ctx.attrs._bundle_target_name
    return ctx.attrs.name

def get_product_name(ctx: AnalysisContext) -> str:
    return ctx.attrs.product_name if hasattr(ctx.attrs, "product_name") and ctx.attrs.product_name != None else _get_bundle_target_name(ctx)

def get_extension_attr(ctx: AnalysisContext) -> "":
    return ctx.attrs.extension

def get_default_binary_dep(ctx: AnalysisContext) -> ["dependency", None]:
    if ctx.attrs.binary == None:
        return None

    if len(ctx.attrs.binary.items()) == 1:
        return ctx.attrs.binary.values()[0]

    return ctx.attrs.binary["arm64"] if "arm64" in ctx.attrs.binary else ctx.attrs.binary["x86_64"]

def get_flattened_binary_deps(ctx: AnalysisContext) -> list["dependency"]:
    return [] if ctx.attrs.binary == None else ctx.attrs.binary.values()

# Derives the effective deployment target for the bundle. It's
# usually the deployment target of the binary if present,
# otherwise it falls back to other values (see implementation).
def get_bundle_min_target_version(ctx: AnalysisContext, binary: [Dependency, None]) -> str:
    binary_min_version = None

    # Could be not set for e.g. watchOS bundles which have a stub
    # binary that comes from the apple_toolchain(), not from the
    # apple_bundle() itself (i.e., binary field will be None).
    #
    # TODO(T114147746): The top-level stub bundle for a watchOS app
    # does not have the ability to set its deployment target via
    # a binary (as that field is empty). If it contains asset
    # catalogs (can it?), we need to use correct target version.
    #
    # The solution might to be support SDK version from
    # Info.plist (T110378109).
    if binary != None:
        min_version_info = binary[AppleMinDeploymentVersionInfo]
        if min_version_info != None:
            binary_min_version = min_version_info.version

    fallback_min_version = get_min_deployment_version_for_node(ctx)
    min_version = binary_min_version or fallback_min_version

    if min_version != None:
        return min_version

    # TODO(T110378109): support default value from SDK `Info.plist`
    fail("Could not determine min target sdk version for bundle: {}".format(ctx.label))

def get_bundle_resource_processing_options(ctx: AnalysisContext) -> AppleResourceProcessingOptions.type:
    compile_resources_locally = value_or(ctx.attrs._compile_resources_locally_override, ctx.attrs._apple_toolchain[AppleToolchainInfo].compile_resources_locally)
    return AppleResourceProcessingOptions(prefer_local = compile_resources_locally, allow_cache_upload = compile_resources_locally)

def get_bundle_infos_from_graph(graph: ResourceGraphInfo.type) -> list[AppleBundleLinkerMapInfo.type]:
    bundle_infos = []
    for node in graph.nodes.traverse():
        if not node.resource_spec:
            continue

        resource_spec = node.resource_spec
        for artifact in resource_spec.files:
            if type(artifact) != "dependency":
                continue

            bundle_info = artifact.get(AppleBundleLinkerMapInfo)
            if bundle_info:
                bundle_infos.append(bundle_info)

    return bundle_infos

def merge_bundle_linker_maps_info(infos: list[AppleBundleLinkerMapInfo.type]) -> AppleBundleLinkerMapInfo.type:
    return AppleBundleLinkerMapInfo(
        linker_maps = flatten([info.linker_maps for info in infos]),
    )
