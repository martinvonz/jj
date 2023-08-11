# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":apple_bundle_destination.bzl", "AppleBundleDestination")
load(
    ":apple_resource_types.bzl",
    "AppleResourceDestination",  # @unused Used as a type
)

def apple_bundle_destination_from_resource_destination(res_destination: AppleResourceDestination.type) -> AppleBundleDestination.type:
    return AppleBundleDestination(res_destination.value)
