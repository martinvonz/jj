# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":julia_info.bzl", "JuliaToolchainInfo")

def _toolchain(lang: str, providers: list[""]) -> "attribute":
    return attrs.default_only(attrs.toolchain_dep(default = "toolchains//:" + lang, providers = providers))

def julia_toolchain():
    return _toolchain("julia", [JuliaToolchainInfo])
