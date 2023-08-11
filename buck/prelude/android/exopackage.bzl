# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

SECONDARY_DEX = 1
NATIVE_LIBRARY = 2
RESOURCES = 4
MODULES = 8
ARCH64 = 16

def get_exopackage_flags(exopackage_modes: list[str]) -> int:
    flags = 0

    for (name, flag) in [
        ("secondary_dex", SECONDARY_DEX),
        ("native_library", NATIVE_LIBRARY),
        ("resources", RESOURCES),
        ("modules", MODULES),
        ("arch64", ARCH64),
    ]:
        if name in exopackage_modes:
            flags += flag

    return flags
