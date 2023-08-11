# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":toolchain.bzl", "GoToolchainInfo")

GoCoverageMode = enum(
    "set",
    "count",
    "atomic",
)

# The result of running `go tool cover` on the input sources.
GoCoverResult = record(
    # All sources after annotating non-`_test.go` sources.  This will be a
    # combination of the original `*_test.go` sources and the annotated non-
    # `*_test.go` sources.
    srcs = field(cmd_args),
    # Coverage variables we used when annotating non-test sources.
    variables = field(cmd_args),
)

def cover_srcs(ctx: AnalysisContext, pkg_name: str, mode: GoCoverageMode.type, srcs: cmd_args) -> GoCoverResult.type:
    out_covered_src_dir = ctx.actions.declare_output("__covered_srcs__", dir = True)
    out_srcs_argsfile = ctx.actions.declare_output("covered_srcs.txt")
    out_coverage_vars_argsfile = ctx.actions.declare_output("coverage_vars.txt")

    go_toolchain = ctx.attrs._go_toolchain[GoToolchainInfo]
    cmd = cmd_args()
    cmd.add(go_toolchain.cover_srcs[RunInfo])
    cmd.add("--cover", go_toolchain.cover)
    cmd.add("--coverage-mode", mode.value)
    cmd.add("--coverage-var-argsfile", out_coverage_vars_argsfile.as_output())
    cmd.add("--covered-srcs-dir", out_covered_src_dir.as_output())
    cmd.add("--out-srcs-argsfile", out_srcs_argsfile.as_output())
    cmd.add("--pkg-name", pkg_name)
    cmd.add(srcs)
    ctx.actions.run(cmd, category = "go_cover")

    return GoCoverResult(
        srcs = cmd_args(out_srcs_argsfile, format = "@{}").hidden(out_covered_src_dir).hidden(srcs),
        variables = cmd_args(out_coverage_vars_argsfile, format = "@{}"),
    )
