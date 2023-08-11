# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",  # @unused Used as a type
    "make_artifact_tset",
    "project_artifacts",
)
load("@prelude//:paths.bzl", "paths")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo", "AppleToolsInfo")
# @oss-disable: load("@prelude//apple/meta_only:linker_outputs.bzl", "subtargets_for_apple_bundle_extra_outputs") 
load("@prelude//apple/user:apple_selective_debugging.bzl", "AppleSelectiveDebuggingInfo")
load(
    "@prelude//ide_integrations:xcode.bzl",
    "XCODE_DATA_SUB_TARGET",
    "generate_xcode_data",
)
load(
    "@prelude//linking:execution_preference.bzl",
    "LinkExecutionPreference",
    "LinkExecutionPreferenceInfo",
)
load(
    "@prelude//utils:utils.bzl",
    "expect",
    "flatten",
    "is_any",
)
load(":apple_bundle_destination.bzl", "AppleBundleDestination")
load(":apple_bundle_part.bzl", "AppleBundlePart", "SwiftStdlibArguments", "assemble_bundle", "bundle_output", "get_apple_bundle_part_relative_destination_path", "get_bundle_dir_name")
load(":apple_bundle_resources.bzl", "get_apple_bundle_resource_part_list", "get_is_watch_bundle")
load(":apple_bundle_types.bzl", "AppleBinaryExtraOutputsInfo", "AppleBundleBinaryOutput", "AppleBundleExtraOutputsInfo", "AppleBundleInfo", "AppleBundleLinkerMapInfo", "AppleBundleResourceInfo")
load(":apple_bundle_utility.bzl", "get_bundle_min_target_version", "get_default_binary_dep", "get_flattened_binary_deps", "get_product_name")
load(":apple_dsym.bzl", "DSYM_INFO_SUBTARGET", "DSYM_SUBTARGET", "get_apple_dsym", "get_apple_dsym_ext", "get_apple_dsym_info")
load(":apple_sdk.bzl", "get_apple_sdk_name")
load(":apple_universal_binaries.bzl", "create_universal_binary")
load(
    ":debug.bzl",
    "AppleDebuggableInfo",
    "get_aggregated_debug_info",
)
load(":xcode.bzl", "apple_xcode_data_add_xctoolchain")

INSTALL_DATA_SUB_TARGET = "install-data"
_INSTALL_DATA_FILE_NAME = "install_apple_data.json"

_PLIST = "plist"

_XCTOOLCHAIN_SUB_TARGET = "xctoolchain"

AppleBundleDebuggableInfo = record(
    # Can be `None` for WatchKit stub
    binary_info = field([AppleDebuggableInfo.type, None]),
    # Debugable info of all bundle deps
    dep_infos = field([AppleDebuggableInfo.type]),
    # Concat of `binary_info` and `dep_infos`
    all_infos = field([AppleDebuggableInfo.type]),
)

AppleBundlePartListConstructorParams = record(
    # The binaries/executables, required to create a bundle
    binaries = field([AppleBundlePart.type]),
)

AppleBundlePartListOutput = record(
    # The parts to be copied into an Apple bundle, *including* binaries
    parts = field([AppleBundlePart.type]),
    # Part that holds the info.plist
    info_plist_part = field(AppleBundlePart.type),
)

def _get_binary(ctx: AnalysisContext) -> AppleBundleBinaryOutput.type:
    # No binary means we are building watchOS bundle. In v1 bundle binary is present, but its sources are empty.
    if ctx.attrs.binary == None:
        return AppleBundleBinaryOutput(
            binary = _get_watch_kit_stub_artifact(ctx),
            is_watchkit_stub_binary = True,
        )

    if ctx.attrs.universal:
        if ctx.attrs.selective_debugging != None:
            fail("Selective debugging is not supported for universal binaries.")
        return create_universal_binary(
            ctx = ctx,
            binary_deps = ctx.attrs.binary,
            binary_name = "{}-UniversalBinary".format(get_product_name(ctx)),
            dsym_bundle_name = _get_bundle_dsym_name(ctx),
            split_arch_dsym = ctx.attrs.split_arch_dsym,
        )
    else:
        binary_dep = get_default_binary_dep(ctx)
        if len(binary_dep[DefaultInfo].default_outputs) != 1:
            fail("Expected single output artifact. Make sure the implementation of rule from `binary` attribute is correct.")

        return _maybe_scrub_binary(ctx, binary_dep)

def _get_bundle_dsym_name(ctx: AnalysisContext) -> str:
    return paths.replace_extension(get_bundle_dir_name(ctx), ".dSYM")

def _maybe_scrub_binary(ctx, binary_dep: Dependency) -> AppleBundleBinaryOutput.type:
    binary = binary_dep[DefaultInfo].default_outputs[0]
    debuggable_info = binary_dep.get(AppleDebuggableInfo)
    if ctx.attrs.selective_debugging == None:
        return AppleBundleBinaryOutput(binary = binary, debuggable_info = debuggable_info)

    selective_debugging_info = ctx.attrs.selective_debugging[AppleSelectiveDebuggingInfo]

    # If fast adhoc code signing is enabled, we need to resign the binary as it won't be signed later.
    if ctx.attrs._fast_adhoc_signing_enabled:
        apple_tools = ctx.attrs._apple_tools[AppleToolsInfo]
        adhoc_codesign_tool = apple_tools.adhoc_codesign_tool
    else:
        adhoc_codesign_tool = None

    binary_execution_preference_info = binary_dep.get(LinkExecutionPreferenceInfo)
    preference = binary_execution_preference_info.preference if binary_execution_preference_info else LinkExecutionPreference("any")

    binary = selective_debugging_info.scrub_binary(ctx, binary, preference, adhoc_codesign_tool)

    if not debuggable_info:
        return AppleBundleBinaryOutput(binary = binary)

    # If we have debuggable info for this binary, create the scrubed dsym for the binary and filter debug info.
    debug_info_tset = debuggable_info.debug_info_tset
    dsym_artifact = _get_scrubbed_binary_dsym(ctx, binary, debug_info_tset)

    all_debug_info = debug_info_tset._tset.traverse()
    filtered_debug_info = selective_debugging_info.filter(all_debug_info)

    filtered_external_debug_info = make_artifact_tset(
        actions = ctx.actions,
        label = ctx.label,
        artifacts = flatten(filtered_debug_info.map.values()),
    )
    debuggable_info = AppleDebuggableInfo(dsyms = [dsym_artifact], debug_info_tset = filtered_external_debug_info, filtered_map = filtered_debug_info.map)

    return AppleBundleBinaryOutput(binary = binary, debuggable_info = debuggable_info)

def _get_scrubbed_binary_dsym(ctx, binary: "artifact", debug_info_tset: ArtifactTSet.type) -> "artifact":
    debug_info = project_artifacts(
        actions = ctx.actions,
        tsets = [debug_info_tset],
    )
    dsym_artifact = get_apple_dsym(
        ctx = ctx,
        executable = binary,
        debug_info = debug_info,
        action_identifier = binary.short_path,
    )
    return dsym_artifact

def _get_binary_bundle_parts(ctx: AnalysisContext, binary_output: AppleBundleBinaryOutput.type) -> (list[AppleBundlePart.type], AppleBundlePart.type):
    """Returns a tuple of all binary bundle parts and the primary bundle binary."""
    result = []

    if binary_output.is_watchkit_stub_binary:
        # If we're using a stub binary from watchkit, we also need to add extra part for stub.
        result.append(AppleBundlePart(source = binary_output.binary, destination = AppleBundleDestination("watchkitstub"), new_name = "WK"))
    primary_binary_part = AppleBundlePart(source = binary_output.binary, destination = AppleBundleDestination("executables"), new_name = get_product_name(ctx))
    result.append(primary_binary_part)
    return result, primary_binary_part

def _get_watch_kit_stub_artifact(ctx: AnalysisContext) -> "artifact":
    expect(ctx.attrs.binary == None, "Stub is useful only when binary is not set which means watchOS bundle is built.")
    stub_binary = ctx.attrs._apple_toolchain[AppleToolchainInfo].watch_kit_stub_binary
    if stub_binary == None:
        fail("Expected Watch Kit stub binary to be provided when bundle binary is not set.")
    return stub_binary

def _apple_bundle_run_validity_checks(ctx: AnalysisContext):
    if ctx.attrs.extension == None:
        fail("`extension` attribute is required")

def _get_debuggable_deps(ctx: AnalysisContext, binary_output: AppleBundleBinaryOutput.type, run_cmd: "_arglike") -> AppleBundleDebuggableInfo.type:
    # `label` captures configuration as well, so it's safe to use for comparison purposes
    binary_labels = filter(None, [getattr(binary_dep, "label", None) for binary_dep in get_flattened_binary_deps(ctx)])
    deps_debuggable_infos = filter(
        None,
        # It's allowed for `ctx.attrs.binary` to appear in `ctx.attrs.deps` as well,
        # in this case, do not duplicate the debugging info for the binary coming from two paths.
        [dep.get(AppleDebuggableInfo) for dep in ctx.attrs.deps if dep.label not in binary_labels],
    )

    # We don't care to process the watchkit stub binary.
    if binary_output.is_watchkit_stub_binary:
        return AppleBundleDebuggableInfo(
            binary_info = None,
            dep_infos = deps_debuggable_infos,
            all_infos = deps_debuggable_infos,
        )

    if not ctx.attrs.split_arch_dsym:
        # Calling `dsymutil` on the correctly named binary in the _final bundle_ to yield dsym files
        # with naming convention compatible with Meta infra.
        binary_debuggable_info = binary_output.debuggable_info
        bundle_binary_dsym_artifact = get_apple_dsym_ext(
            ctx = ctx,
            executable = run_cmd,
            debug_info = project_artifacts(
                actions = ctx.actions,
                tsets = [binary_debuggable_info.debug_info_tset] if binary_debuggable_info else [],
            ),
            action_identifier = get_bundle_dir_name(ctx),
            output_path = _get_bundle_dsym_name(ctx),
        )
        bundle_debuggable_info = AppleDebuggableInfo(
            dsyms = [bundle_binary_dsym_artifact],
            debug_info_tset = binary_debuggable_info.debug_info_tset,
            filtered_map = binary_debuggable_info.filtered_map,
        )
    else:
        bundle_debuggable_info = binary_output.debuggable_info

    return AppleBundleDebuggableInfo(
        binary_info = bundle_debuggable_info,
        dep_infos = deps_debuggable_infos,
        all_infos = deps_debuggable_infos + ([bundle_debuggable_info] if bundle_debuggable_info else []),
    )

def get_apple_bundle_part_list(ctx: AnalysisContext, params: AppleBundlePartListConstructorParams.type) -> AppleBundlePartListOutput.type:
    resource_part_list = None
    if hasattr(ctx.attrs, "_resource_bundle") and ctx.attrs._resource_bundle != None:
        resource_info = ctx.attrs._resource_bundle[AppleBundleResourceInfo]
        if resource_info != None:
            resource_part_list = resource_info.resource_output

    if resource_part_list == None:
        resource_part_list = get_apple_bundle_resource_part_list(ctx)

    return AppleBundlePartListOutput(
        parts = resource_part_list.resource_parts + params.binaries,
        info_plist_part = resource_part_list.info_plist_part,
    )

def apple_bundle_impl(ctx: AnalysisContext) -> list["provider"]:
    _apple_bundle_run_validity_checks(ctx)

    binary_outputs = _get_binary(ctx)
    all_binary_parts, primary_binary_part = _get_binary_bundle_parts(ctx, binary_outputs)
    apple_bundle_part_list_output = get_apple_bundle_part_list(ctx, AppleBundlePartListConstructorParams(binaries = all_binary_parts))

    bundle = bundle_output(ctx)

    primary_binary_rel_path = get_apple_bundle_part_relative_destination_path(ctx, primary_binary_part)

    sub_targets = assemble_bundle(ctx, bundle, apple_bundle_part_list_output.parts, apple_bundle_part_list_output.info_plist_part, SwiftStdlibArguments(primary_binary_rel_path = primary_binary_rel_path))

    primary_binary_path = cmd_args([bundle, primary_binary_rel_path], delimiter = "/")
    run_cmd = cmd_args(primary_binary_path).hidden(bundle)

    linker_maps_directory, linker_map_info = _linker_maps_data(ctx)
    sub_targets["linker-maps"] = [DefaultInfo(default_output = linker_maps_directory)]

    bundle_debuggable_info = _get_debuggable_deps(ctx, binary_outputs, run_cmd)

    binary_dsym_artifacts = getattr(bundle_debuggable_info.binary_info, "dsyms", [])
    dep_dsym_artifacts = flatten([info.dsyms for info in bundle_debuggable_info.dep_infos])
    dsym_artifacts = binary_dsym_artifacts + dep_dsym_artifacts
    if dsym_artifacts:
        sub_targets[DSYM_SUBTARGET] = [DefaultInfo(default_outputs = dsym_artifacts)]

    aggregated_debug_info = get_aggregated_debug_info(ctx, bundle_debuggable_info.all_infos, dsym_artifacts)
    sub_targets.update(aggregated_debug_info.sub_targets)

    dsym_info = get_apple_dsym_info(ctx, binary_dsyms = binary_dsym_artifacts, dep_dsyms = dep_dsym_artifacts)
    sub_targets[DSYM_INFO_SUBTARGET] = [
        DefaultInfo(default_output = dsym_info, other_outputs = dsym_artifacts),
    ]

    sub_targets[_PLIST] = [DefaultInfo(default_output = apple_bundle_part_list_output.info_plist_part.source)]

    sub_targets[_XCTOOLCHAIN_SUB_TARGET] = ctx.attrs._apple_xctoolchain.providers

    # Define the xcode data sub target
    xcode_data_default_info, xcode_data_info = generate_xcode_data(ctx, "apple_bundle", bundle, _xcode_populate_attributes, processed_info_plist = apple_bundle_part_list_output.info_plist_part.source)
    sub_targets[XCODE_DATA_SUB_TARGET] = xcode_data_default_info

    plist_bundle_relative_path = get_apple_bundle_part_relative_destination_path(ctx, apple_bundle_part_list_output.info_plist_part)
    install_data = generate_install_data(ctx, plist_bundle_relative_path)

    # Collect extra bundle outputs
    extra_output_provider = _extra_output_provider(ctx)
    # @oss-disable: extra_output_subtargets = subtargets_for_apple_bundle_extra_outputs(ctx, extra_output_provider) 
    # @oss-disable: sub_targets.update(extra_output_subtargets) 

    return [
        DefaultInfo(default_output = bundle, sub_targets = sub_targets),
        AppleBundleInfo(
            bundle = bundle,
            binary_name = get_product_name(ctx),
            is_watchos = get_is_watch_bundle(ctx),
            contains_watchapp = is_any(lambda part: part.destination == AppleBundleDestination("watchapp"), apple_bundle_part_list_output.parts),
            skip_copying_swift_stdlib = ctx.attrs.skip_copying_swift_stdlib,
        ),
        aggregated_debug_info.debug_info,
        InstallInfo(
            installer = ctx.attrs._apple_toolchain[AppleToolchainInfo].installer,
            files = {
                "app_bundle": bundle,
                "options": install_data,
            },
        ),
        RunInfo(args = run_cmd),
        linker_map_info,
        xcode_data_info,
        extra_output_provider,
    ]

def _xcode_populate_attributes(ctx, processed_info_plist: "artifact") -> dict[str, ""]:
    data = {
        "deployment_version": get_bundle_min_target_version(ctx, get_default_binary_dep(ctx)),
        "info_plist": ctx.attrs.info_plist,
        "processed_info_plist": processed_info_plist,
        "product_name": get_product_name(ctx),
        "sdk": get_apple_sdk_name(ctx),
    }

    apple_xcode_data_add_xctoolchain(ctx, data)
    return data

def _linker_maps_data(ctx: AnalysisContext) -> ("artifact", AppleBundleLinkerMapInfo.type):
    deps_with_binary = ctx.attrs.deps + get_flattened_binary_deps(ctx)
    deps_linker_map_infos = filter(
        None,
        [dep.get(AppleBundleLinkerMapInfo) for dep in deps_with_binary],
    )
    deps_linker_maps = flatten([info.linker_maps for info in deps_linker_map_infos])
    all_maps = {map.basename: map for map in deps_linker_maps}
    directory = ctx.actions.copied_dir(
        "LinkMap",
        all_maps,
    )
    provider = AppleBundleLinkerMapInfo(linker_maps = all_maps.values())
    return (directory, provider)

def _extra_output_provider(ctx: AnalysisContext) -> AppleBundleExtraOutputsInfo.type:
    # Collect the sub_targets for this bundle's binary that are extra_linker_outputs.
    extra_outputs = []
    for binary_dep in get_flattened_binary_deps(ctx):
        linker_outputs = ctx.attrs._apple_toolchain[AppleToolchainInfo].extra_linker_outputs
        binary_outputs = {k: v[DefaultInfo].default_outputs for k, v in binary_dep[DefaultInfo].sub_targets.items() if k in linker_outputs}
        extra_outputs.append(AppleBinaryExtraOutputsInfo(
            name = get_product_name(ctx),
            default_output = binary_dep[DefaultInfo].default_outputs[0],
            extra_outputs = binary_outputs,
        ))

    # Collect the transitive extra bundle outputs from the deps.
    for dep in ctx.attrs.deps:
        if AppleBundleExtraOutputsInfo in dep:
            extra_outputs.extend(dep[AppleBundleExtraOutputsInfo].extra_outputs)

    return AppleBundleExtraOutputsInfo(extra_outputs = extra_outputs)

def generate_install_data(
        ctx: AnalysisContext,
        plist_path: str,
        populate_rule_specific_attributes_func: ["function", None] = None,
        **kwargs) -> "artifact":
    data = {
        "fullyQualifiedName": ctx.label,
        "info_plist": plist_path,
        "use_idb": "true",
        ## TODO(T110665037): read from .buckconfig
        # We require the user to have run `xcode-select` and `/var/db/xcode_select_link` to symlink
        # to the selected Xcode. e.g: `/Applications/Xcode_14.2.app/Contents/Developer`
        "xcode_developer_path": "/var/db/xcode_select_link",
    }

    if populate_rule_specific_attributes_func:
        data.update(populate_rule_specific_attributes_func(ctx, **kwargs))

    return ctx.actions.write_json(_INSTALL_DATA_FILE_NAME, data)
