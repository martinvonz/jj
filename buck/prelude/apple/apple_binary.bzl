# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:artifact_tset.bzl", "project_artifacts")
load("@prelude//:paths.bzl", "paths")
load("@prelude//apple:apple_stripping.bzl", "apple_strip_args")
# @oss-disable: load("@prelude//apple/meta_only:linker_outputs.bzl", "add_extra_linker_outputs") 
load(
    "@prelude//apple/swift:swift_compilation.bzl",
    "compile_swift",
    "get_swift_anonymous_targets",
    "uses_explicit_modules",
)
load("@prelude//apple/swift:swift_types.bzl", "SWIFT_EXTENSION")
load("@prelude//cxx:cxx_executable.bzl", "cxx_executable")
load("@prelude//cxx:cxx_library_utility.bzl", "cxx_attr_deps", "cxx_attr_exported_deps")
load("@prelude//cxx:cxx_sources.bzl", "get_srcs_with_flags")
load("@prelude//cxx:cxx_types.bzl", "CxxRuleConstructorParams")
load(
    "@prelude//cxx:headers.bzl",
    "cxx_attr_headers",
    "cxx_get_regular_cxx_headers_layout",
    "prepare_headers",
)
load(
    "@prelude//cxx:link_groups.bzl",
    "get_link_group_info",
)
load(
    "@prelude//cxx:preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorArgs",
)
load(":apple_bundle_types.bzl", "AppleBundleLinkerMapInfo", "AppleMinDeploymentVersionInfo")
load(":apple_bundle_utility.bzl", "get_bundle_infos_from_graph", "merge_bundle_linker_maps_info")
load(":apple_code_signing_types.bzl", "AppleEntitlementsInfo")
load(":apple_dsym.bzl", "DSYM_SUBTARGET", "get_apple_dsym")
load(":apple_frameworks.bzl", "get_framework_search_path_flags")
load(":apple_sdk_metadata.bzl", "IPhoneSimulatorSdkMetadata", "MacOSXCatalystSdkMetadata")
load(":apple_target_sdk_version.bzl", "get_min_deployment_version_for_node", "get_min_deployment_version_target_linker_flags", "get_min_deployment_version_target_preprocessor_flags")
load(":apple_toolchain_types.bzl", "AppleToolchainInfo")
load(":apple_utility.bzl", "get_apple_cxx_headers_layout")
load(":debug.bzl", "AppleDebuggableInfo", "DEBUGINFO_SUBTARGET")
load(":resource_groups.bzl", "create_resource_graph")
load(":xcode.bzl", "apple_populate_xcode_attributes")

def apple_binary_impl(ctx: AnalysisContext) -> [list["provider"], "promise"]:
    def get_apple_binary_providers(deps_providers) -> list["provider"]:
        # FIXME: Ideally we'd like to remove the support of "bridging header",
        # cause it affects build time and in general considered a bad practise.
        # But we need it for now to achieve compatibility with BUCK1.
        objc_bridging_header_flags = _get_bridging_header_flags(ctx)

        cxx_srcs, swift_srcs = _filter_swift_srcs(ctx)

        framework_search_path_flags = get_framework_search_path_flags(ctx)
        swift_compile = compile_swift(
            ctx,
            swift_srcs,
            False,  # parse_as_library
            deps_providers,
            [],
            None,
            framework_search_path_flags,
            objc_bridging_header_flags,
        )
        swift_object_files = [swift_compile.object_file] if swift_compile else []

        swift_preprocessor = [swift_compile.pre] if swift_compile else []

        extra_linker_output_flags, extra_linker_output_providers = [], {} # @oss-enable
        # @oss-disable: extra_linker_output_flags, extra_linker_output_providers = add_extra_linker_outputs(ctx) 
        extra_link_flags = get_min_deployment_version_target_linker_flags(ctx) + _entitlements_link_flags(ctx) + extra_linker_output_flags

        framework_search_path_pre = CPreprocessor(
            relative_args = CPreprocessorArgs(args = [framework_search_path_flags]),
        )
        constructor_params = CxxRuleConstructorParams(
            rule_type = "apple_binary",
            headers_layout = get_apple_cxx_headers_layout(ctx),
            extra_link_flags = extra_link_flags,
            srcs = cxx_srcs,
            extra_link_input = swift_object_files,
            extra_preprocessors = get_min_deployment_version_target_preprocessor_flags(ctx) + [framework_search_path_pre] + swift_preprocessor,
            strip_executable = ctx.attrs.stripped,
            strip_args_factory = apple_strip_args,
            cxx_populate_xcode_attributes_func = apple_populate_xcode_attributes,
            link_group_info = get_link_group_info(ctx),
            prefer_stripped_objects = ctx.attrs.prefer_stripped_objects,
            # Some apple rules rely on `static` libs *not* following dependents.
            link_groups_force_static_follows_dependents = False,
        )
        cxx_output = cxx_executable(ctx, constructor_params)

        debug_info = project_artifacts(
            actions = ctx.actions,
            tsets = [cxx_output.external_debug_info],
        )
        dsym_artifact = get_apple_dsym(
            ctx = ctx,
            executable = cxx_output.binary,
            debug_info = debug_info,
            action_identifier = cxx_output.binary.short_path,
        )
        cxx_output.sub_targets[DSYM_SUBTARGET] = [DefaultInfo(default_output = dsym_artifact)]
        cxx_output.sub_targets[DEBUGINFO_SUBTARGET] = [DefaultInfo(other_outputs = debug_info)]
        cxx_output.sub_targets.update(extra_linker_output_providers)

        min_version = get_min_deployment_version_for_node(ctx)
        min_version_providers = [AppleMinDeploymentVersionInfo(version = min_version)] if min_version != None else []

        resource_graph = create_resource_graph(
            ctx = ctx,
            labels = ctx.attrs.labels,
            deps = cxx_attr_deps(ctx),
            exported_deps = cxx_attr_exported_deps(ctx),
        )
        bundle_infos = get_bundle_infos_from_graph(resource_graph)
        if cxx_output.linker_map_data:
            bundle_infos.append(AppleBundleLinkerMapInfo(linker_maps = [cxx_output.linker_map_data.map]))

        return [
            DefaultInfo(default_output = cxx_output.binary, sub_targets = cxx_output.sub_targets),
            RunInfo(args = cmd_args(cxx_output.binary).hidden(cxx_output.runtime_files)),
            AppleEntitlementsInfo(entitlements_file = ctx.attrs.entitlements_file),
            AppleDebuggableInfo(dsyms = [dsym_artifact], debug_info_tset = cxx_output.external_debug_info),
            cxx_output.xcode_data,
            cxx_output.compilation_db,
            merge_bundle_linker_maps_info(bundle_infos),
        ] + [resource_graph] + min_version_providers

    if uses_explicit_modules(ctx):
        return get_swift_anonymous_targets(ctx, get_apple_binary_providers)
    else:
        return get_apple_binary_providers([])

_SDK_NAMES_NEED_ENTITLEMENTS_IN_BINARY = [
    IPhoneSimulatorSdkMetadata.name,
    MacOSXCatalystSdkMetadata.name,
]

def _needs_entitlements_in_binary(ctx: AnalysisContext) -> bool:
    apple_toolchain_info = ctx.attrs._apple_toolchain[AppleToolchainInfo]
    return apple_toolchain_info.sdk_name in _SDK_NAMES_NEED_ENTITLEMENTS_IN_BINARY

def _entitlements_link_flags(ctx: AnalysisContext) -> list[""]:
    return [
        "-Xlinker",
        "-sectcreate",
        "-Xlinker",
        "__TEXT",
        "-Xlinker",
        "__entitlements",
        "-Xlinker",
        ctx.attrs.entitlements_file,
    ] if (ctx.attrs.entitlements_file and _needs_entitlements_in_binary(ctx)) else []

def _filter_swift_srcs(ctx: AnalysisContext) -> (list["CxxSrcWithFlags"], list["CxxSrcWithFlags"]):
    cxx_srcs = []
    swift_srcs = []
    for s in get_srcs_with_flags(ctx):
        if s.file.extension == SWIFT_EXTENSION:
            swift_srcs.append(s)
        else:
            cxx_srcs.append(s)
    return cxx_srcs, swift_srcs

def _get_bridging_header_flags(ctx: AnalysisContext) -> list["_arglike"]:
    if ctx.attrs.bridging_header:
        objc_bridging_header_flags = [
            # Disable bridging header -> PCH compilation to mitigate an issue in Xcode 13 beta.
            "-disable-bridging-pch",
            "-import-objc-header",
            cmd_args(ctx.attrs.bridging_header),
        ]

        headers_layout = cxx_get_regular_cxx_headers_layout(ctx)
        headers = cxx_attr_headers(ctx, headers_layout)
        header_map = {paths.join(h.namespace, h.name): h.artifact for h in headers}

        # We need to expose private headers to swift-compile action, in case something is imported to bridging header.
        # TODO(chatatap): Handle absolute paths here.
        header_root = prepare_headers(ctx, header_map, "apple-binary-private-headers", None)
        if header_root != None:
            private_headers_args = [cmd_args("-I"), header_root.include_path]
        else:
            private_headers_args = []

        return objc_bridging_header_flags + private_headers_args
    else:
        return []
