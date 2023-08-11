# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":debug.bzl", "AppleDebuggableInfo")

# Provider flagging that result of the rule contains Apple bundle.
# It might be copied into main bundle to appropriate place if rule
# with this provider is a dependency of `apple_bundle`.
AppleBundleInfo = provider(fields = [
    # Result bundle; `artifact`
    "bundle",
    # The name of the executable within the bundle.
    # `str`
    "binary_name",
    # If the bundle was built for watchOS Apple platform, this affects packaging.
    # Might be omitted for certain types of bundle (e.g. frameworks) when packaging doesn't depend on it.
    # [None, `bool`]
    "is_watchos",
    # If the bundle contains a Watch Extension executable, we have to update the packaging.
    # Similar to `is_watchos`, this might be omitted for certain types of bundles which don't depend on it.
    # [None, `bool`]
    "contains_watchapp",
    # By default, non-framework, non-appex binaries copy Swift libraries into the final
    # binary. This is the opt-out for that.
    # [None, `bool`]
    "skip_copying_swift_stdlib",
])

# Provider which helps to propagate minimum deployment version up the target graph.
AppleMinDeploymentVersionInfo = provider(fields = [
    # `str`
    "version",
])

AppleBundleResourceInfo = provider(fields = [
    "resource_output",  # AppleBundleResourcePartListOutput.type
])

AppleBundleLinkerMapInfo = provider(fields = [
    "linker_maps",  # ["artifact"]
])

# Providers used to merge extra linker outputs as a top level output
# of an application bundle.
AppleBinaryExtraOutputsInfo = provider(fields = [
    "default_output",  # "artifact"
    "extra_outputs",  # {`str`: ["artifact"]}
    "name",  # `str`
])

AppleBundleExtraOutputsInfo = provider(fields = [
    "extra_outputs",  # [AppleBinaryExtraOutputsInfo]
])

AppleBundleBinaryOutput = record(
    binary = field("artifact"),
    debuggable_info = field([AppleDebuggableInfo.type, None], None),
    # In the case of watchkit, the `ctx.attrs.binary`'s not set, and we need to create a stub binary.
    is_watchkit_stub_binary = field(bool, False),
)
