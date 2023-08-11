# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load("@prelude//apple:apple_utility.bzl", "expand_relative_prefixed_sdk_path", "get_disable_pch_validation_flags")
load(":apple_sdk_modules_utility.bzl", "SDKDepTSet", "get_compiled_sdk_deps_tset")
load(":swift_toolchain_types.bzl", "SdkCompiledModuleInfo", "SdkUncompiledModuleInfo", "WrappedSdkCompiledModuleInfo")

def get_shared_pcm_compilation_args(target: str, module_name: str) -> cmd_args:
    cmd = cmd_args()
    cmd.add([
        "-emit-pcm",
        "-target",
        target,
        "-module-name",
        module_name,
        "-Xfrontend",
        "-disable-implicit-swift-modules",
        "-Xcc",
        "-fno-implicit-modules",
        "-Xcc",
        "-fno-implicit-module-maps",
        # Disable debug info in pcm files. This is required to avoid embedding absolute paths
        # and ending up with mismatched pcm file sizes.
        "-Xcc",
        "-Xclang",
        "-Xcc",
        "-fmodule-format=raw",
        # Embed all input files into the PCM so we don't need to include module map files when
        # building remotely.
        # https://github.com/apple/llvm-project/commit/fb1e7f7d1aca7bcfc341e9214bda8b554f5ae9b6
        "-Xcc",
        "-Xclang",
        "-Xcc",
        "-fmodules-embed-all-files",
        # Embed all files that were read during compilation into the generated PCM.
        "-Xcc",
        "-Xclang",
        "-Xcc",
        "-fmodule-file-home-is-cwd",
        # Once we have an empty working directory the compiler provided headers such as float.h
        # cannot be found, so add . to the header search paths.
        "-Xcc",
        "-I.",
    ])

    cmd.add(get_disable_pch_validation_flags())

    return cmd

def _remove_path_components_from_right(path: str, count: int):
    path_components = path.split("/")
    removed_path = "/".join(path_components[0:-count])
    return removed_path

def _add_sdk_module_search_path(cmd, uncompiled_sdk_module_info, apple_toolchain):
    modulemap_path = uncompiled_sdk_module_info.input_relative_path

    # If this input is a framework we need to search above the
    # current framework location, otherwise we include the
    # modulemap root.
    if uncompiled_sdk_module_info.is_framework:
        frameworks_dir_path = _remove_path_components_from_right(modulemap_path, 3)
        expanded_path = expand_relative_prefixed_sdk_path(
            cmd_args(apple_toolchain.swift_toolchain_info.sdk_path),
            cmd_args(apple_toolchain.swift_toolchain_info.resource_dir),
            cmd_args(apple_toolchain.platform_path),
            frameworks_dir_path,
        )
    else:
        module_root_path = _remove_path_components_from_right(modulemap_path, 1)
        expanded_path = expand_relative_prefixed_sdk_path(
            cmd_args(apple_toolchain.swift_toolchain_info.sdk_path),
            cmd_args(apple_toolchain.swift_toolchain_info.resource_dir),
            cmd_args(apple_toolchain.platform_path),
            module_root_path,
        )
    cmd.add([
        "-Xcc",
        ("-F" if uncompiled_sdk_module_info.is_framework else "-I"),
        "-Xcc",
        cmd_args(expanded_path),
    ])

def get_swift_sdk_pcm_anon_targets(
        ctx: AnalysisContext,
        uncompiled_sdk_deps: list[Dependency],
        swift_cxx_args: list[str]):
    deps = [
        {
            "dep": uncompiled_sdk_dep,
            "sdk_pcm_name": uncompiled_sdk_dep[SdkUncompiledModuleInfo].name,
            "swift_cxx_args": swift_cxx_args,
            "_apple_toolchain": ctx.attrs._apple_toolchain,
        }
        for uncompiled_sdk_dep in uncompiled_sdk_deps
        if SdkUncompiledModuleInfo in uncompiled_sdk_dep and not uncompiled_sdk_dep[SdkUncompiledModuleInfo].is_swiftmodule
    ]
    return [(_swift_sdk_pcm_compilation, d) for d in deps]

def _swift_sdk_pcm_compilation_impl(ctx: AnalysisContext) -> ["promise", list["provider"]]:
    def k(sdk_pcm_deps_providers) -> list["provider"]:
        uncompiled_sdk_module_info = ctx.attrs.dep[SdkUncompiledModuleInfo]
        module_name = uncompiled_sdk_module_info.module_name
        apple_toolchain = ctx.attrs._apple_toolchain[AppleToolchainInfo]
        swift_toolchain = apple_toolchain.swift_toolchain_info
        cmd = cmd_args(swift_toolchain.compiler)
        cmd.add(uncompiled_sdk_module_info.partial_cmd)
        cmd.add(["-sdk", swift_toolchain.sdk_path])
        cmd.add(swift_toolchain.compiler_flags)

        if swift_toolchain.resource_dir:
            cmd.add([
                "-resource-dir",
                swift_toolchain.resource_dir,
            ])

        sdk_deps_tset = get_compiled_sdk_deps_tset(ctx, sdk_pcm_deps_providers)
        cmd.add(sdk_deps_tset.project_as_args("clang_deps"))

        expanded_modulemap_path_cmd = expand_relative_prefixed_sdk_path(
            cmd_args(swift_toolchain.sdk_path),
            cmd_args(swift_toolchain.resource_dir),
            cmd_args(apple_toolchain.platform_path),
            uncompiled_sdk_module_info.input_relative_path,
        )
        pcm_output = ctx.actions.declare_output(module_name + ".pcm")
        cmd.add([
            "-o",
            pcm_output.as_output(),
            expanded_modulemap_path_cmd,
        ])

        # For SDK modules we need to set a few more args
        cmd.add([
            "-Xcc",
            "-Xclang",
            "-Xcc",
            "-emit-module",
            "-Xcc",
            "-Xclang",
            "-Xcc",
            "-fsystem-module",
        ])

        cmd.add(ctx.attrs.swift_cxx_args)

        _add_sdk_module_search_path(cmd, uncompiled_sdk_module_info, apple_toolchain)

        # T142915880 There is an issue with hard links,
        # when we compile pcms remotely on linux machines.
        local_only = True

        ctx.actions.run(
            cmd,
            category = "sdk_swift_pcm_compile",
            identifier = module_name,
            local_only = local_only,
            allow_cache_upload = local_only,
        )

        compiled_sdk = SdkCompiledModuleInfo(
            name = uncompiled_sdk_module_info.name,
            module_name = module_name,
            is_framework = uncompiled_sdk_module_info.is_framework,
            output_artifact = pcm_output,
            is_swiftmodule = False,
            input_relative_path = expanded_modulemap_path_cmd,
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

    # Recursively compile PCMs of any other exported_deps
    sdk_pcm_anon_targets = get_swift_sdk_pcm_anon_targets(
        ctx,
        ctx.attrs.dep[SdkUncompiledModuleInfo].deps,
        ctx.attrs.swift_cxx_args,
    )

    return ctx.actions.anon_targets(sdk_pcm_anon_targets).map(k)

_swift_sdk_pcm_compilation = rule(
    impl = _swift_sdk_pcm_compilation_impl,
    attrs = {
        "dep": attrs.dep(),
        "sdk_pcm_name": attrs.string(),
        "swift_cxx_args": attrs.list(attrs.string(), default = []),
        "_apple_toolchain": attrs.dep(),
    },
)
