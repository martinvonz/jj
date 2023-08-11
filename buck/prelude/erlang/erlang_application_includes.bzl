# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(":erlang_build.bzl", "BuildEnvironment", "erlang_build")
load(":erlang_info.bzl", "ErlangAppIncludeInfo")
load(
    ":erlang_toolchain.bzl",
    "select_toolchains",
)
load(
    ":erlang_utils.bzl",
    "multidict_projection",
    "multidict_projection_key",
)

def erlang_application_includes_impl(ctx: AnalysisContext) -> list["provider"]:
    """ rule for application includes target
    """

    # prepare include directory for current app
    name = ctx.attrs.application_name

    # input mapping
    input_mapping = {}
    for input_artifact in ctx.attrs.includes:
        input_mapping[paths.basename(input_artifact.short_path)] = input_artifact

    toolchains = select_toolchains(ctx)
    build_environments = {}
    for toolchain in toolchains.values():
        build_environments[toolchain.name] = (
            erlang_build.build_steps.generate_include_artifacts(
                ctx,
                toolchain,
                BuildEnvironment(input_mapping = input_mapping),
                name,
                ctx.attrs.includes,
            )
        )

    # build application info
    app_include_info = ErlangAppIncludeInfo(
        name = name,
        includes = multidict_projection(build_environments, "includes"),
        include_dir = multidict_projection_key(build_environments, "include_dirs", name),
        deps_files = multidict_projection(build_environments, "deps_files"),
        input_mapping = multidict_projection(build_environments, "input_mapping"),
    )

    return [
        DefaultInfo(),
        app_include_info,
    ]
