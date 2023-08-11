# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":apple_bundle_destination.bzl", "AppleBundleDestination")
load(":apple_bundle_part.bzl", "AppleBundlePart")
load(":apple_bundle_utility.bzl", "get_bundle_min_target_version", "get_product_name")
load(":apple_sdk.bzl", "get_apple_sdk_name")
load(
    ":apple_sdk_metadata.bzl",
    "AppleSdkMetadata",  # @unused Used as a type
    "MacOSXCatalystSdkMetadata",
    "MacOSXSdkMetadata",
    "WatchOSSdkMetadata",
    "WatchSimulatorSdkMetadata",
    "get_apple_sdk_metadata_for_sdk_name",
)
load(":apple_toolchain_types.bzl", "AppleToolchainInfo", "AppleToolsInfo")

def process_info_plist(ctx: AnalysisContext, override_input: ["artifact", None]) -> AppleBundlePart.type:
    input = _preprocess_info_plist(ctx)
    output = ctx.actions.declare_output("Info.plist")
    additional_keys = _additional_keys_as_json_file(ctx)
    override_keys = _override_keys_as_json_file(ctx)
    process_plist(
        ctx = ctx,
        input = input,
        output = output.as_output(),
        override_input = override_input,
        additional_keys = additional_keys,
        override_keys = override_keys,
    )
    return AppleBundlePart(source = output, destination = AppleBundleDestination("metadata"))

def _get_plist_run_options() -> dict[str, bool]:
    return {
        # Output is deterministic, so can be cached
        "allow_cache_upload": True,
        # plist generation is cheap and fast, RE network overhead not worth it
        "prefer_local": True,
    }

def _preprocess_info_plist(ctx: AnalysisContext) -> "artifact":
    input = ctx.attrs.info_plist
    output = ctx.actions.declare_output("PreprocessedInfo.plist")
    substitutions_json = _plist_substitutions_as_json_file(ctx)
    apple_tools = ctx.attrs._apple_tools[AppleToolsInfo]
    processor = apple_tools.info_plist_processor
    command = cmd_args([
        processor,
        "preprocess",
        "--input",
        input,
        "--output",
        output.as_output(),
        "--product-name",
        get_product_name(ctx),
    ])
    if substitutions_json != None:
        command.add(["--substitutions-json", substitutions_json])
    ctx.actions.run(command, category = "apple_preprocess_info_plist", **_get_plist_run_options())
    return output

def _plist_substitutions_as_json_file(ctx: AnalysisContext) -> ["artifact", None]:
    info_plist_substitutions = ctx.attrs.info_plist_substitutions
    if not info_plist_substitutions:
        return None

    substitutions_json = ctx.actions.write_json("plist_substitutions.json", info_plist_substitutions)
    return substitutions_json

def process_plist(ctx: AnalysisContext, input: "artifact", output: "output_artifact", override_input: ["artifact", None] = None, additional_keys: ["artifact", None] = None, override_keys: ["artifact", None] = None, action_id: [str, None] = None):
    apple_tools = ctx.attrs._apple_tools[AppleToolsInfo]
    processor = apple_tools.info_plist_processor
    override_input_arguments = ["--override-input", override_input] if override_input != None else []
    additional_keys_arguments = ["--additional-keys", additional_keys] if additional_keys != None else []
    override_keys_arguments = ["--override-keys", override_keys] if override_keys != None else []
    command = cmd_args([
        processor,
        "process",
        "--input",
        input,
        "--output",
        output,
    ] + override_input_arguments + additional_keys_arguments + override_keys_arguments)
    ctx.actions.run(command, category = "apple_process_info_plist", identifier = action_id or input.basename, **_get_plist_run_options())

def _additional_keys_as_json_file(ctx: AnalysisContext) -> "artifact":
    additional_keys = _info_plist_additional_keys(ctx)
    return ctx.actions.write_json("plist_additional.json", additional_keys)

def _info_plist_additional_keys(ctx: AnalysisContext) -> dict[str, ""]:
    sdk_name = get_apple_sdk_name(ctx)
    sdk_metadata = get_apple_sdk_metadata_for_sdk_name(sdk_name)
    result = _extra_mac_info_plist_keys(sdk_metadata, ctx.attrs.extension)
    result["CFBundleSupportedPlatforms"] = sdk_metadata.info_plist_supported_platforms_values
    result["DTPlatformName"] = sdk_name
    sdk_version = ctx.attrs._apple_toolchain[AppleToolchainInfo].sdk_version
    if sdk_version:
        result["DTPlatformVersion"] = sdk_version
        result["DTSDKName"] = sdk_name + sdk_version
    sdk_build_version = ctx.attrs._apple_toolchain[AppleToolchainInfo].sdk_build_version
    if sdk_build_version:
        result["DTPlatformBuild"] = sdk_build_version
        result["DTSDKBuild"] = sdk_build_version
    xcode_build_version = ctx.attrs._apple_toolchain[AppleToolchainInfo].xcode_build_version
    if xcode_build_version:
        result["DTXcodeBuild"] = xcode_build_version
    xcode_version = ctx.attrs._apple_toolchain[AppleToolchainInfo].xcode_version
    if xcode_version:
        result["DTXcode"] = xcode_version
    result[sdk_metadata.min_version_plist_info_key] = get_bundle_min_target_version(ctx, ctx.attrs.binary)
    return result

def _extra_mac_info_plist_keys(sdk_metadata: AppleSdkMetadata.type, extension: str) -> dict[str, ""]:
    if sdk_metadata.name == MacOSXSdkMetadata.name and extension == "xpc":
        return {
            "NSHighResolutionCapable": True,
            "NSSupportsAutomaticGraphicsSwitching": True,
        }
    else:
        return {}

def _override_keys_as_json_file(ctx: AnalysisContext) -> "artifact":
    override_keys = _info_plist_override_keys(ctx)
    return ctx.actions.write_json("plist_override.json", override_keys)

def _info_plist_override_keys(ctx: AnalysisContext) -> dict[str, ""]:
    sdk_name = get_apple_sdk_name(ctx)
    result = {}
    if sdk_name == MacOSXSdkMetadata.name:
        if ctx.attrs.extension != "xpc":
            result["LSRequiresIPhoneOS"] = False
    elif sdk_name not in [WatchOSSdkMetadata.name, WatchSimulatorSdkMetadata.name, MacOSXCatalystSdkMetadata.name]:
        result["LSRequiresIPhoneOS"] = True
    return result
