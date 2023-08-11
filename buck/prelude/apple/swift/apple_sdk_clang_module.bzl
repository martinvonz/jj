# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":swift_sdk_pcm_compilation.bzl", "get_shared_pcm_compilation_args")
load(":swift_toolchain_types.bzl", "SdkSwiftOverlayInfo", "SdkUncompiledModuleInfo")

def apple_sdk_clang_module_impl(ctx: AnalysisContext) -> list["provider"]:
    cmd = get_shared_pcm_compilation_args(ctx.attrs.target, ctx.attrs.module_name)
    overlays = []
    if ctx.attrs.overlays:
        overlays = [SdkSwiftOverlayInfo(overlays = ctx.attrs.overlays)]
    return [
        DefaultInfo(),
        SdkUncompiledModuleInfo(
            name = ctx.attrs.name,
            module_name = ctx.attrs.module_name,
            is_framework = ctx.attrs.is_framework,
            is_swiftmodule = False,
            partial_cmd = cmd,
            input_relative_path = ctx.attrs.modulemap_relative_path,
            deps = ctx.attrs.deps,
        ),
    ] + overlays

# This rule represent a Clang module from SDK and forms a graph of dependencies between such modules.
apple_sdk_clang_module = rule(
    impl = apple_sdk_clang_module_impl,
    attrs = {
        "deps": attrs.list(attrs.dep(), default = []),
        "is_framework": attrs.bool(default = False),
        # This is a real module name, contrary to `name`
        # which has a special suffix to distinguish Swift and Clang modules with the same name
        "module_name": attrs.string(),
        "modulemap_relative_path": attrs.string(),
        "overlays": attrs.dict(key = attrs.string(), value = attrs.list(attrs.string(), default = []), sorted = False, default = {}),
        "target": attrs.string(),
    },
)
