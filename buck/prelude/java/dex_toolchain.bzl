# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# TODO(T107163344) These should be part of the Android toolchain!
# Move out once we have overlays.
DexToolchainInfo = provider(
    doc = "Dex toolchain info",
    fields = [
        "android_jar",
        "d8_command",
    ],
)
