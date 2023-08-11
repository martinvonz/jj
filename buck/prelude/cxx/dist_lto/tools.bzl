# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//cxx:cxx_toolchain_types.bzl", "DistLtoToolsInfo")

def _impl(ctx):
    return [
        DefaultInfo(),
        DistLtoToolsInfo(
            planner = ctx.attrs.planner[RunInfo],
            prepare = ctx.attrs.prepare[RunInfo],
            opt = ctx.attrs.opt[RunInfo],
            copy = ctx.attrs.copy[RunInfo],
        ),
    ]

dist_lto_tools = rule(
    impl = _impl,
    attrs = {
        "copy": attrs.dep(),
        "opt": attrs.dep(),
        "planner": attrs.dep(),
        "prepare": attrs.dep(),
    },
)
