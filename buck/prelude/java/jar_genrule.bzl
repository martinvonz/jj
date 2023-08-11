# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:genrule.bzl", "process_genrule")
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load("@prelude//utils:utils.bzl", "expect")

def jar_genrule_impl(ctx: AnalysisContext) -> list["provider"]:
    output_name = "{}.jar".format(ctx.label.name)
    providers = process_genrule(ctx, output_name, None)
    expect(
        len(providers) == 1,
        "expected exactly one provider of type DefaultInfo from {} ({})"
            .format(ctx.label.name, providers),
    )

    default_info = providers[0]  # DefaultInfo type
    outputs = default_info.default_outputs
    expect(
        len(outputs) == 1,
        "expected exactly one output from {} ({})"
            .format(ctx.label.name, outputs),
    )
    output_jar = outputs[0]

    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    java_cmd = cmd_args(java_toolchain.java[RunInfo])
    java_cmd.add("-jar", output_jar)

    providers.append(RunInfo(args = java_cmd))
    return providers
