# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

IpaCompressionLevel = enum(
    "min",
    "max",
    "default",
    "none",
)

def apple_package_config() -> dict[str, ""]:
    return {
        "_ipa_compression_level": read_root_config("apple", "ipa_compression_level", IpaCompressionLevel("default").value),
    }
