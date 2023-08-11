# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":manifest.bzl", "ManifestInfo")
load(":toolchain.bzl", "PythonToolchainInfo")

PycInvalidationMode = enum(
    "UNCHECKED_HASH",
    "CHECKED_HASH",
    # timestamp isn't supported at the moment
    # "TIMESTAMP",
)

def compile_manifests(
        ctx: AnalysisContext,
        manifests: list[ManifestInfo.type]) -> dict[PycInvalidationMode.type, ManifestInfo.type]:
    return {
        mode: compile_manifests_for_mode(ctx, manifests, mode)
        for mode in [PycInvalidationMode("UNCHECKED_HASH"), PycInvalidationMode("CHECKED_HASH")]
    }

def compile_manifests_for_mode(
        ctx: AnalysisContext,
        manifests: list[ManifestInfo.type],
        invalidation_mode: PycInvalidationMode.type = PycInvalidationMode("UNCHECKED_HASH")) -> ManifestInfo.type:
    output = ctx.actions.declare_output("bytecode_{}".format(invalidation_mode.value), dir = True)
    bytecode_manifest = ctx.actions.declare_output("bytecode_{}.manifest".format(invalidation_mode.value))
    cmd = cmd_args(ctx.attrs._python_toolchain[PythonToolchainInfo].host_interpreter)
    cmd.add(ctx.attrs._python_toolchain[PythonToolchainInfo].compile)
    cmd.add(cmd_args(output.as_output(), format = "--output={}"))
    cmd.add(cmd_args(bytecode_manifest.as_output(), format = "--bytecode-manifest={}"))
    cmd.add("--invalidation-mode={}".format(invalidation_mode.value))

    for manifest in manifests:
        cmd.add(manifest.manifest)
        cmd.hidden([a for a, _ in manifest.artifacts])
    ctx.actions.run(
        cmd,
        # On some platforms (e.g. linux), python hash code randomness can cause
        # the bytecode to be non-deterministic, so pin via the `PYTHONHASHSEED`
        # env var.
        env = {"PYTHONHASHSEED": "7"},
        category = "py_compile",
        identifier = invalidation_mode.value,
    )
    return ManifestInfo(manifest = bytecode_manifest, artifacts = [(output, "bytecode")])
