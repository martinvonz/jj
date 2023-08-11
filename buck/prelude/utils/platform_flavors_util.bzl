# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

def by_platform(
        platform_flavors: list[str],
        xs: list[(str, "_a")]) -> list["_a"]:
    """
    Resolve platform-flavor-specific parameters, given the list of platform
    flavors to match against.  Meant to mirror the usage of
    `PatternMatchedCollection`s in v1 for `platform_*` parameters.
    """

    res = []

    for (dtype, deps) in xs:
        for platform in platform_flavors:
            if regex_match(dtype, platform):
                res.append(deps)

    return res
