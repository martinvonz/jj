# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load("@prelude//apple:apple_utility.bzl", "expand_relative_prefixed_sdk_path", "get_explicit_modules_env_var")
load("@prelude//apple/swift:swift_types.bzl", "SWIFTMODULE_EXTENSION")
load(":apple_sdk_modules_utility.bzl", "SDKDepTSet", "get_compiled_sdk_deps_tset")
load(":swift_module_map.bzl", "write_swift_module_map")
load(":swift_sdk_pcm_compilation.bzl", "get_swift_sdk_pcm_anon_targets")
load(":swift_toolchain_types.bzl", "SdkCompiledModuleInfo", "SdkUncompiledModuleInfo", "WrappedSdkCompiledModuleInfo")

def get_swift_interface_anon_targets(
        ctx: AnalysisContext,
        uncompiled_sdk_deps: list[Dependency],
        swift_cxx_args: list[str]):
    deps = [
        {
            "dep": uncompiled_sdk_dep,
            "sdk_swiftinterface_name": uncompiled_sdk_dep[SdkUncompiledModuleInfo].module_name,
            "swift_cxx_args": swift_cxx_args,
            "_apple_toolchain": ctx.attrs._apple_toolchain,
        }
        for uncompiled_sdk_dep in uncompiled_sdk_deps
        if SdkUncompiledModuleInfo in uncompiled_sdk_dep and uncompiled_sdk_dep[SdkUncompiledModuleInfo].is_swiftmodule
    ]
    return [(_swift_interface_compilation, d) for d in deps]

def _swift_interface_compilation_impl(ctx: AnalysisContext) -> ["promise", list["provider"]]:
    def k(sdk_deps_providers) -> list["provider"]:
        uncompiled_sdk_module_info = ctx.attrs.dep[SdkUncompiledModuleInfo]
        uncompiled_module_info_name = uncompiled_sdk_module_info.module_name

        apple_toolchain = ctx.attrs._apple_toolchain[AppleToolchainInfo]
        swift_toolchain = apple_toolchain.swift_toolchain_info
        cmd = cmd_args(swift_toolchain.compiler)
        cmd.add(uncompiled_sdk_module_info.partial_cmd)
        cmd.add(["-sdk", swift_toolchain.sdk_path])

        if swift_toolchain.resource_dir:
            cmd.add([
                "-resource-dir",
                swift_toolchain.resource_dir,
            ])

        # `sdk_deps_providers` contains providers of direct SDK deps,
        # as well as a provider that aggregates SDK deps coming from all transitive pcm deps.
        sdk_deps_tset = get_compiled_sdk_deps_tset(ctx, sdk_deps_providers)

        # FIXME: - Get rid of slow traversal here, and unify with two projections below.
        swift_module_map_artifact = write_swift_module_map(ctx, uncompiled_module_info_name, list(sdk_deps_tset.traverse()))
        cmd.add([
            "-explicit-swift-module-map-file",
            swift_module_map_artifact,
        ])

        # sdk_swiftinterface_compile should explicitly depend on its deps that go to swift_modulemap
        cmd.hidden(sdk_deps_tset.project_as_args("hidden"))
        cmd.add(sdk_deps_tset.project_as_args("clang_deps"))

        swiftmodule_output = ctx.actions.declare_output(uncompiled_module_info_name + SWIFTMODULE_EXTENSION)
        expanded_swiftinterface_cmd = expand_relative_prefixed_sdk_path(
            cmd_args(swift_toolchain.sdk_path),
            cmd_args(swift_toolchain.resource_dir),
            cmd_args(apple_toolchain.platform_path),
            uncompiled_sdk_module_info.input_relative_path,
        )
        cmd.add([
            "-o",
            swiftmodule_output.as_output(),
            expanded_swiftinterface_cmd,
        ])

        ctx.actions.run(
            cmd,
            env = get_explicit_modules_env_var(True),
            category = "sdk_swiftinterface_compile",
            identifier = uncompiled_module_info_name,
        )

        compiled_sdk = SdkCompiledModuleInfo(
            name = uncompiled_sdk_module_info.name,
            module_name = uncompiled_module_info_name,
            is_framework = uncompiled_sdk_module_info.is_framework,
            is_swiftmodule = True,
            output_artifact = swiftmodule_output,
            input_relative_path = expanded_swiftinterface_cmd,
        )

        return [
            DefaultInfo(),
            WrappedSdkCompiledModuleInfo(
                tset = ctx.actions.tset(SDKDepTSet, value = compiled_sdk, children = [sdk_deps_tset]),
            ),
        ]

    # Skip deps compilations if run not on SdkUncompiledModuleInfo
    if SdkUncompiledModuleInfo not in ctx.attrs.dep:
        return []

    sdk_pcm_deps_anon_targets = get_swift_sdk_pcm_anon_targets(
        ctx,
        ctx.attrs.dep[SdkUncompiledModuleInfo].deps,
        ctx.attrs.swift_cxx_args,
    )

    # Recursively compile swiftinterface of any other exported_deps
    sdk_swift_interface_anon_targets = get_swift_interface_anon_targets(
        ctx,
        ctx.attrs.dep[SdkUncompiledModuleInfo].deps,
        ctx.attrs.swift_cxx_args,
    )

    return ctx.actions.anon_targets(sdk_pcm_deps_anon_targets + sdk_swift_interface_anon_targets).map(k)

_swift_interface_compilation = rule(
    impl = _swift_interface_compilation_impl,
    attrs = {
        "dep": attrs.dep(),
        "sdk_swiftinterface_name": attrs.string(),
        "swift_cxx_args": attrs.list(attrs.string(), default = []),
        "_apple_toolchain": attrs.dep(),
    },
)
