# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//utils:utils.bzl",
    "flatten",
)
load(
    ":compile.bzl",
    "CxxSrcWithFlags",
)
load(":platform.bzl", "cxx_by_platform")

# The source files
def get_srcs_with_flags(ctx: AnalysisContext) -> list[CxxSrcWithFlags.type]:
    all_srcs = ctx.attrs.srcs + flatten(cxx_by_platform(ctx, ctx.attrs.platform_srcs))

    # src -> flags_hash -> flags
    flags_sets_by_src = {}
    for x in all_srcs:
        if type(x) == type(()):
            artifact = x[0]
            flags = x[1]
        else:
            artifact = x
            flags = []

        flags_hash = hash(str(flags))
        flag_sets = flags_sets_by_src.setdefault(artifact, {})
        flag_sets[flags_hash] = flags

    # Go through collected (source, flags) pair and set the index field if there are duplicate source files
    cxx_src_with_flags_records = []
    for (artifact, flag_sets) in flags_sets_by_src.items():
        needs_indices = len(flag_sets) > 1
        for i, flags in enumerate(flag_sets.values()):
            index = i if needs_indices else None
            cxx_src_with_flags_records.append(CxxSrcWithFlags(file = artifact, flags = flags, index = index))

    return cxx_src_with_flags_records
