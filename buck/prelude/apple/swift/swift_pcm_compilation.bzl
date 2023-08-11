# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load("@prelude//apple:apple_utility.bzl", "get_explicit_modules_env_var", "get_module_name", "get_versioned_target_triple")
load("@prelude//cxx:preprocessor.bzl", "cxx_inherited_preprocessor_infos", "cxx_merge_cpreprocessors")
load(
    ":apple_sdk_modules_utility.bzl",
    "SDKDepTSet",  # @unused Used as a type
    "get_compiled_sdk_deps_tset",
    "get_uncompiled_sdk_deps",
)
load(":swift_pcm_compilation_types.bzl", "SwiftPCMCompiledInfo", "SwiftPCMUncompiledInfo", "WrappedSwiftPCMCompiledInfo")
load(":swift_sdk_pcm_compilation.bzl", "get_shared_pcm_compilation_args", "get_swift_sdk_pcm_anon_targets")
load(":swift_sdk_swiftinterface_compilation.bzl", "get_swift_interface_anon_targets")
load(":swift_toolchain_types.bzl", "WrappedSdkCompiledModuleInfo")

_REQUIRED_SDK_MODULES = ["Foundation"]

def _project_as_clang_deps(value: SwiftPCMCompiledInfo.type):
    return cmd_args([
        "-Xcc",
        cmd_args(["-fmodule-file=", value.name, "=", value.pcm_output], delimiter = ""),
        "-Xcc",
        cmd_args(["-fmodule-map-file=", value.exported_preprocessor.modulemap_path], delimiter = ""),
        "-Xcc",
    ] + value.exported_preprocessor.relative_args.args).hidden(value.exported_preprocessor.modular_args)

PcmDepTSet = transitive_set(args_projections = {
    "clang_deps": _project_as_clang_deps,
})

def get_compiled_pcm_deps_tset(ctx: AnalysisContext, pcm_deps_providers: list) -> PcmDepTSet.type:
    pcm_deps = [
        pcm_deps_provider[WrappedSwiftPCMCompiledInfo].tset
        for pcm_deps_provider in pcm_deps_providers
        if WrappedSwiftPCMCompiledInfo in pcm_deps_provider
    ]
    return ctx.actions.tset(PcmDepTSet, children = pcm_deps)

def get_swift_pcm_anon_targets(
        ctx: AnalysisContext,
        uncompiled_deps: list[Dependency],
        swift_cxx_args: list[str]):
    deps = [
        {
            "dep": uncompiled_dep,
            "pcm_name": uncompiled_dep[SwiftPCMUncompiledInfo].name,
            "swift_cxx_args": swift_cxx_args,
            "target_sdk_version": ctx.attrs.target_sdk_version,
            "_apple_toolchain": ctx.attrs._apple_toolchain,
        }
        for uncompiled_dep in uncompiled_deps
        if SwiftPCMUncompiledInfo in uncompiled_dep
    ]
    return [(_swift_pcm_compilation, d) for d in deps]

def _compile_with_argsfile(
        ctx: AnalysisContext,
        category: str,
        module_name: str,
        args: cmd_args,
        additional_cmd: cmd_args):
    shell_quoted_cmd = cmd_args(args, quote = "shell")
    argfile, _ = ctx.actions.write(module_name + ".pcm.argsfile", shell_quoted_cmd, allow_args = True)

    swift_toolchain = ctx.attrs._apple_toolchain[AppleToolchainInfo].swift_toolchain_info
    cmd = cmd_args(swift_toolchain.compiler)
    cmd.add(cmd_args(["@", argfile], delimiter = ""))

    # Action should also depend on all artifacts from the argsfile, otherwise they won't be materialised.
    cmd.hidden([args])

    cmd.add(additional_cmd)

    # T142915880 There is an issue with hard links,
    # when we compile pcms remotely on linux machines.
    local_only = True

    ctx.actions.run(
        cmd,
        env = get_explicit_modules_env_var(True),
        category = category,
        identifier = module_name,
        local_only = local_only,
        allow_cache_upload = local_only,
    )

def _swift_pcm_compilation_impl(ctx: AnalysisContext) -> ["promise", list["provider"]]:
    def k(compiled_pcm_deps_providers) -> list["provider"]:
        uncompiled_pcm_info = ctx.attrs.dep[SwiftPCMUncompiledInfo]

        # `compiled_pcm_deps_providers` will contain `WrappedSdkCompiledModuleInfo` providers
        # from direct SDK deps and transitive deps that export sdk deps.
        sdk_deps_tset = get_compiled_sdk_deps_tset(ctx, compiled_pcm_deps_providers)

        # To compile a pcm we only use the exported_deps as those are the only
        # ones that should be transitively exported through public headers
        pcm_deps_tset = get_compiled_pcm_deps_tset(ctx, compiled_pcm_deps_providers)

        # We don't need to compile non-modular or targets that do not export any headers,
        # but for the sake of BUCK1 compatibility, we need to pass them up,
        # in case they re-export some dependencies.
        if uncompiled_pcm_info.is_transient:
            return [
                DefaultInfo(),
                WrappedSwiftPCMCompiledInfo(
                    tset = ctx.actions.tset(PcmDepTSet, children = [pcm_deps_tset]),
                ),
                WrappedSdkCompiledModuleInfo(
                    tset = sdk_deps_tset,
                ),
            ]

        module_name = ctx.attrs.pcm_name
        cmd, additional_cmd, pcm_output = _get_base_pcm_flags(
            ctx,
            module_name,
            uncompiled_pcm_info,
            sdk_deps_tset,
            pcm_deps_tset,
            ctx.attrs.swift_cxx_args,
        )

        # It's possible that modular targets can re-export headers of non-modular targets,
        # (e.g `raw_headers`) because of that we need to provide search paths of such targets to
        # pcm compilation actions in order for them to be successful.
        inherited_preprocessor_infos = cxx_inherited_preprocessor_infos(uncompiled_pcm_info.exported_deps)
        preprocessors = cxx_merge_cpreprocessors(ctx, [], inherited_preprocessor_infos)
        cmd.add(cmd_args(preprocessors.set.project_as_args("include_dirs"), prepend = "-Xcc"))

        # When compiling pcm files, module's exported pps and inherited pps
        # must be provided to an action like hmaps which are used for headers resolution.
        if uncompiled_pcm_info.propagated_preprocessor_args_cmd:
            cmd.add(uncompiled_pcm_info.propagated_preprocessor_args_cmd)

        _compile_with_argsfile(
            ctx,
            "swift_pcm_compile",
            module_name,
            cmd,
            additional_cmd,
        )

        compiled_pcm = SwiftPCMCompiledInfo(
            name = module_name,
            pcm_output = pcm_output,
            exported_preprocessor = uncompiled_pcm_info.exported_preprocessor,
        )

        return [
            DefaultInfo(default_outputs = [pcm_output]),
            WrappedSwiftPCMCompiledInfo(
                tset = ctx.actions.tset(PcmDepTSet, value = compiled_pcm, children = [pcm_deps_tset]),
            ),
            WrappedSdkCompiledModuleInfo(
                tset = sdk_deps_tset,
            ),
        ]

    # Skip deps compilations if run not on SdkUncompiledModuleInfo
    if SwiftPCMUncompiledInfo not in ctx.attrs.dep:
        return []

    direct_uncompiled_sdk_deps = get_uncompiled_sdk_deps(
        ctx.attrs.dep[SwiftPCMUncompiledInfo].uncompiled_sdk_modules,
        _REQUIRED_SDK_MODULES,
        ctx.attrs._apple_toolchain[AppleToolchainInfo].swift_toolchain_info,
    )

    # Recursively compiling SDK's Clang dependencies
    sdk_pcm_deps_anon_targets = get_swift_sdk_pcm_anon_targets(
        ctx,
        direct_uncompiled_sdk_deps,
        ctx.attrs.swift_cxx_args,
    )

    # Recursively compiling SDK's Swift dependencies
    # We need to match BUCK1 behavior, which can't distinguish between Swift and Clang SDK modules,
    # so we pass more SDK deps than is strictly necessary. When BUCK1 is deprecated, we can try to avoid doing that,
    # by passing Clang and Swift deps up separately.
    swift_interface_anon_targets = get_swift_interface_anon_targets(
        ctx,
        direct_uncompiled_sdk_deps,
        ctx.attrs.swift_cxx_args,
    )

    # Recursively compile PCMs of transitevely visible exported_deps
    swift_pcm_anon_targets = get_swift_pcm_anon_targets(
        ctx,
        ctx.attrs.dep[SwiftPCMUncompiledInfo].exported_deps,
        ctx.attrs.swift_cxx_args,
    )
    return ctx.actions.anon_targets(sdk_pcm_deps_anon_targets + swift_pcm_anon_targets + swift_interface_anon_targets).map(k)

_swift_pcm_compilation = rule(
    impl = _swift_pcm_compilation_impl,
    attrs = {
        "dep": attrs.dep(),
        "pcm_name": attrs.string(),
        "swift_cxx_args": attrs.list(attrs.string(), default = []),
        "target_sdk_version": attrs.option(attrs.string(), default = None),
        "_apple_toolchain": attrs.dep(),
    },
)

def compile_underlying_pcm(
        ctx: AnalysisContext,
        uncompiled_pcm_info: "SwiftPCMUncompiledInfo",
        compiled_pcm_deps_providers,
        swift_cxx_args: list[str],
        framework_search_path_flags: cmd_args) -> "SwiftPCMCompiledInfo":
    module_name = get_module_name(ctx)

    # `compiled_pcm_deps_providers` will contain `WrappedSdkCompiledModuleInfo` providers
    # from direct SDK deps and transitive deps that export sdk deps.
    sdk_deps_tset = get_compiled_sdk_deps_tset(ctx, compiled_pcm_deps_providers)

    # To compile a pcm we only use the exported_deps as those are the only
    # ones that should be transitively exported through public headers
    pcm_deps_tset = get_compiled_pcm_deps_tset(ctx, compiled_pcm_deps_providers)

    cmd, additional_cmd, pcm_output = _get_base_pcm_flags(
        ctx,
        module_name,
        uncompiled_pcm_info,
        sdk_deps_tset,
        pcm_deps_tset,
        swift_cxx_args,
    )
    modulemap_path = uncompiled_pcm_info.exported_preprocessor.modulemap_path
    cmd.add([
        "-Xcc",
        "-I",
        "-Xcc",
        cmd_args([cmd_args(modulemap_path).parent(), "exported_symlink_tree"], delimiter = "/"),
    ])
    cmd.add(framework_search_path_flags)

    _compile_with_argsfile(
        ctx,
        "swift_underlying_pcm_compile",
        module_name,
        cmd,
        additional_cmd,
    )

    return SwiftPCMCompiledInfo(
        name = module_name,
        pcm_output = pcm_output,
        exported_preprocessor = uncompiled_pcm_info.exported_preprocessor,
    )

def _get_base_pcm_flags(
        ctx: AnalysisContext,
        module_name: str,
        uncompiled_pcm_info: SwiftPCMUncompiledInfo.type,
        sdk_deps_tset: SDKDepTSet.type,
        pcm_deps_tset: PcmDepTSet.type,
        swift_cxx_args: list[str]) -> (cmd_args, cmd_args, "artifact"):
    swift_toolchain = ctx.attrs._apple_toolchain[AppleToolchainInfo].swift_toolchain_info

    cmd = cmd_args()
    cmd.add(get_shared_pcm_compilation_args(get_versioned_target_triple(ctx), module_name))
    cmd.add(["-sdk", swift_toolchain.sdk_path])
    cmd.add(swift_toolchain.compiler_flags)

    # This allows us to avoid usage of absolute paths in generated PCM modules.
    cmd.add([
        "-working-directory",
        ".",
    ])

    if swift_toolchain.resource_dir:
        cmd.add([
            "-resource-dir",
            swift_toolchain.resource_dir,
        ])

    cmd.add(sdk_deps_tset.project_as_args("clang_deps"))
    cmd.add(pcm_deps_tset.project_as_args("clang_deps"))

    modulemap_path = uncompiled_pcm_info.exported_preprocessor.modulemap_path
    pcm_output = ctx.actions.declare_output(module_name + ".pcm")

    additional_cmd = cmd_args(swift_cxx_args)
    additional_cmd.add([
        "-o",
        pcm_output.as_output(),
        modulemap_path,
    ])

    # To correctly resolve modulemap's headers,
    # a search path to the root of modulemap should be passed.
    cmd.add([
        "-Xcc",
        "-I",
        "-Xcc",
        cmd_args(modulemap_path).parent(),
    ])

    # Modular deps like `-Swift.h` have to be materialized.
    cmd.hidden(uncompiled_pcm_info.exported_preprocessor.modular_args)

    return (cmd, additional_cmd, pcm_output)
