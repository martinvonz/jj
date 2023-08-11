# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//decls/android_rules.bzl", "TargetCpuType")
load("@prelude//decls/core_rules.bzl", "Platform")

OsLookup = provider(fields = ["cpu", "platform"])

def _os_lookup_impl(ctx: AnalysisContext):
    return [
        DefaultInfo(),
        OsLookup(
            cpu = ctx.attrs.cpu,
            platform = ctx.attrs.platform,
        ),
    ]

os_lookup = rule(impl = _os_lookup_impl, attrs = {
    "cpu": attrs.option(attrs.enum(TargetCpuType), default = None),
    "platform": attrs.enum(Platform),
})
