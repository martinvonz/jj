# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//utils:platform_flavors_util.bzl", "by_platform")
load(":cxx_context.bzl", "get_cxx_platform_info")

def cxx_by_platform(ctx: AnalysisContext, xs: list[(str, "_a")]) -> list["_a"]:
    cxx_platform_info = get_cxx_platform_info(ctx)
    platform_flavors = [cxx_platform_info.name]
    if cxx_platform_info.deps_aliases:
        platform_flavors.extend(cxx_platform_info.deps_aliases)
    return by_platform(platform_flavors, xs)
