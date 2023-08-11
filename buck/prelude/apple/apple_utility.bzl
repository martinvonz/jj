# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load("@prelude//cxx:headers.bzl", "CxxHeadersLayout", "CxxHeadersNaming")
load("@prelude//utils:utils.bzl", "value_or")
load(":apple_target_sdk_version.bzl", "get_min_deployment_version_for_node")

_VERSION_PLACEHOLDER = "(VERSION)"

# TODO(T115177501): Make target triples part of the toolchains
# Map from SDK name -> target triple _without_ leading architecture
_TARGET_TRIPLE_MAP = {
    "iphoneos": "apple-ios{}".format(_VERSION_PLACEHOLDER),
    "iphonesimulator": "apple-ios{}-simulator".format(_VERSION_PLACEHOLDER),
    "maccatalyst": "apple-ios{}-macabi".format(_VERSION_PLACEHOLDER),
    "macosx": "apple-macosx{}".format(_VERSION_PLACEHOLDER),
    "watchos": "apple-watchos{}".format(_VERSION_PLACEHOLDER),
    "watchsimulator": "apple-watchos{}-simulator".format(_VERSION_PLACEHOLDER),
}

def get_explicit_modules_env_var(uses_explicit_modules: bool) -> dict:
    return ({"EXPLICIT_MODULES_ENABLED": "TRUE"} if uses_explicit_modules else {})

def get_apple_cxx_headers_layout(ctx: AnalysisContext) -> CxxHeadersLayout.type:
    namespace = value_or(ctx.attrs.header_path_prefix, ctx.attrs.name)
    return CxxHeadersLayout(namespace = namespace, naming = CxxHeadersNaming("apple"))

def get_module_name(ctx: AnalysisContext) -> str:
    return ctx.attrs.module_name or ctx.attrs.header_path_prefix or ctx.attrs.name

def has_apple_toolchain(ctx: AnalysisContext) -> bool:
    return hasattr(ctx.attrs, "_apple_toolchain")

def get_versioned_target_triple(ctx: AnalysisContext) -> str:
    apple_toolchain_info = ctx.attrs._apple_toolchain[AppleToolchainInfo]
    swift_toolchain_info = apple_toolchain_info.swift_toolchain_info

    architecture = swift_toolchain_info.architecture
    if architecture == None:
        fail("Need to set `architecture` field of swift_toolchain(), target: {}".format(ctx.label))

    target_sdk_version = get_min_deployment_version_for_node(ctx) or ""

    sdk_name = apple_toolchain_info.sdk_name
    target_triple_with_version_placeholder = _TARGET_TRIPLE_MAP.get(sdk_name)
    if target_triple_with_version_placeholder == None:
        fail("Could not find target triple for sdk = {}".format(sdk_name))

    versioned_target_triple = target_triple_with_version_placeholder.replace(_VERSION_PLACEHOLDER, target_sdk_version)
    return "{}-{}".format(architecture, versioned_target_triple)

def expand_relative_prefixed_sdk_path(
        sdk_path: cmd_args,
        swift_resource_dir: cmd_args,
        platform_path: cmd_args,
        path_to_expand: str) -> cmd_args:
    path_expansion_map = {
        "$PLATFORM_DIR": platform_path,
        "$RESOURCEDIR": swift_resource_dir,
        "$SDKROOT": sdk_path,
    }
    expanded_cmd = cmd_args()
    for (path_variable, path_value) in path_expansion_map.items():
        if path_to_expand.startswith(path_variable):
            path = path_to_expand[len(path_variable):]
            if path.find("$") == 0:
                fail("Failed to expand framework path: {}".format(path))
            expanded_cmd.add(cmd_args([path_value, path], delimiter = ""))

    return expanded_cmd

def get_disable_pch_validation_flags() -> list[str]:
    """
    We need to disable PCH validation for some actions like Swift compilation and Swift PCM generation.
    Currently, we don't have a mechanism to compile with enabled pch validation and Swift explicit modules,
    which we need to be able to do while we are waiting for Anonymous targets which will allow us to solve this problem properly.
    """
    return [
        "-Xcc",
        "-Xclang",
        "-Xcc",
        "-fno-validate-pch",
    ]
