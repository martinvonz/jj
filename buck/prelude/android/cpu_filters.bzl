# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

CPU_FILTER_TO_ABI_DIRECTORY = {
    "arm64": "arm64-v8a",
    "armv7": "armeabi-v7a",
    "x86": "x86",
    "x86_64": "x86_64",
}

ALL_CPU_FILTERS = CPU_FILTER_TO_ABI_DIRECTORY.keys()

CPU_FILTER_FOR_DEFAULT_PLATFORM = "x86"

# The "primary platform" is the one that we use for all
# the non-native targets. We keep this consistent regardless
# of which cpus the native libraries are built for so that
# we get cache hits for the non-native targets across all
# possible cpu filters.
CPU_FILTER_FOR_PRIMARY_PLATFORM = "arm64"
