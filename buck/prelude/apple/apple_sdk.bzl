# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")

def get_apple_sdk_name(ctx: AnalysisContext) -> str:
    """
    Get the SDK defined on the toolchain.
    Will throw if the `_apple_toolchain` is not present.
    """
    return ctx.attrs._apple_toolchain[AppleToolchainInfo].sdk_name
