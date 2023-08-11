# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Provider which exposes a field from `apple_binary` to `apple_bundle` as it might be used during code signing.
AppleEntitlementsInfo = provider(fields = [
    # Optional "artifact"
    "entitlements_file",
])

CodeSignType = enum(
    "skip",
    "adhoc",
    "distribution",
)
