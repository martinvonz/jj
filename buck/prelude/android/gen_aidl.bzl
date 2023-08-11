# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")
load(":android_toolchain.bzl", "AndroidToolchainInfo")

_AidlSourceInfo = provider(fields = [
    "srcs",
])

def gen_aidl_impl(ctx: AnalysisContext) -> list["provider"]:
    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]
    aidl_cmd = cmd_args(android_toolchain.aidl)
    aidl_cmd.add("-p", android_toolchain.framework_aidl_file)
    aidl_cmd.add("-I", ctx.attrs.import_path)
    for path in ctx.attrs.import_paths:
        aidl_cmd.add("-I", path)

    # We need the `aidl_srcs` files - otherwise the search on the `import_path` won't find anything.
    aidl_cmd.hidden(ctx.attrs.aidl_srcs)

    # Allow gen_aidl rules to depend on other gen_aidl rules, and make the source files from the
    # deps accessible in this context. This is an alternative to adding dependent files in
    # aidl_srcs.
    dep_srcs = []
    for dep in ctx.attrs.deps:
        source_info = dep.get(_AidlSourceInfo)
        if source_info != None:
            dep_srcs += source_info.srcs
        else:
            warning("`{}` dependency `{}` is not a `gen_aidl` rule and will be ignored".format(ctx.label, dep.label))

    aidl_cmd.hidden(dep_srcs)

    aidl_out = ctx.actions.declare_output("aidl_output", dir = True)
    aidl_cmd.add("-o", aidl_out.as_output())
    aidl_cmd.add(ctx.attrs.aidl)
    ctx.actions.run(aidl_cmd, category = "aidl")

    # Put the generated Java files into a zip file to be used as srcs to other rules.
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    jar_cmd = cmd_args(java_toolchain.jar)
    jar_cmd.add("-cfM")
    out = ctx.actions.declare_output("{}_aidl_java_output.src.zip".format(ctx.attrs.name))
    jar_cmd.add(out.as_output())
    jar_cmd.add(aidl_out)

    ctx.actions.run(jar_cmd, category = "aidl_jar")

    return [
        DefaultInfo(default_output = out),
        _AidlSourceInfo(srcs = [ctx.attrs.aidl] + ctx.attrs.aidl_srcs + dep_srcs),
    ]
