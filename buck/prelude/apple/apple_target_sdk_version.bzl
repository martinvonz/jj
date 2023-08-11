# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load("@prelude//cxx:preprocessor.bzl", "CPreprocessor", "CPreprocessorArgs")
load(":apple_sdk.bzl", "get_apple_sdk_name")

# TODO(T112099448): In the future, the min version flag should live on the apple_toolchain()
# TODO(T113776898): Switch to -mtargetos= flag which should live on the apple_toolchain()
_APPLE_MIN_VERSION_FLAG_SDK_MAP = {
    "iphoneos": "-mios-version-min",
    "iphonesimulator": "-mios-simulator-version-min",
    "maccatalyst": "-mios-version-min",  # Catalyst uses iOS min version flags
    "macosx": "-mmacosx-version-min",
    "watchos": "-mwatchos-version-min",
    "watchsimulator": "-mwatchsimulator-version-min",
}

# Returns the target SDK version for apple_(binary|library) and uses
# apple_toolchain() min version as a fallback. This is the central place
# where the version for a particular node is defined, no other places
# should be accessing `attrs.target_sdk_version` or `attrs.min_version`.
def get_min_deployment_version_for_node(ctx: AnalysisContext) -> [None, str]:
    toolchain_min_version = ctx.attrs._apple_toolchain[AppleToolchainInfo].min_version
    if toolchain_min_version == "":
        toolchain_min_version = None
    return getattr(ctx.attrs, "target_sdk_version", None) or toolchain_min_version

# Returns the min deployment flag to pass to the compiler + linker
def _get_min_deployment_version_target_flag(ctx: AnalysisContext) -> [None, str]:
    target_sdk_version = get_min_deployment_version_for_node(ctx)
    if target_sdk_version == None:
        return None

    sdk_name = get_apple_sdk_name(ctx)
    min_version_flag = _APPLE_MIN_VERSION_FLAG_SDK_MAP.get(sdk_name)
    if min_version_flag == None:
        fail("Could not determine min version flag for SDK {}".format(sdk_name))

    return "{}={}".format(min_version_flag, target_sdk_version)

# There are two main ways in which we can pass target SDK version:
# - versioned target triple
# - unversioned target triple + version flag
#
# A versioned target triple overrides any version flags and requires
# additional flags to disable the warning/error (`-Woverriding-t-option`),
# so we prefer to use an unversioned target triple + version flag.
#
# Furthermore, we want to ensure that there's _exactly one_ version flag
# on a compiler/link line. This makes debugging easier and avoids issues
# with multiple layers each adding/overriding target SDK. It also makes
# it easier to switch to versioned target triple.
#
# There are exactly two ways in which to specify the target SDK:
# - apple_toolchain.min_version sets the default value
# - apple_(binary|library).target_sdk_version sets the per-target value
#
# apple_toolchain() rules should _never_ add any version flags because
# the rule does _not_ know whether a particular target will request a
# non-default value. Otherwise, we end up with multiple version flags,
# one added by the toolchain and then additional overrides by targets.

def get_min_deployment_version_target_linker_flags(ctx: AnalysisContext) -> list[str]:
    min_version_flag = _get_min_deployment_version_target_flag(ctx)
    return [min_version_flag] if min_version_flag != None else []

def get_min_deployment_version_target_preprocessor_flags(ctx: AnalysisContext) -> list[CPreprocessor.type]:
    min_version_flag = _get_min_deployment_version_target_flag(ctx)
    if min_version_flag == None:
        return []

    args = cmd_args(min_version_flag)
    return [CPreprocessor(
        relative_args = CPreprocessorArgs(args = [args]),
    )]
