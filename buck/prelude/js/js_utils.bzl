# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load("@prelude//:worker_tool.bzl", "WorkerToolInfo")
load("@prelude//apple:apple_resource_types.bzl", "AppleResourceDestination", "AppleResourceSpec")
load("@prelude//apple:resource_groups.bzl", "ResourceGraphInfo", "create_resource_graph")  # @unused `ResourceGraphInfo` used as a type
load("@prelude//js:js_providers.bzl", "JsBundleInfo")
load("@prelude//utils:utils.bzl", "expect")

RAM_BUNDLE_TYPES = {
    "": "",
    "rambundle-indexed": "indexed",
}

TRANSFORM_PROFILES = ["transform-profile-default", "hermes-stable", "hermes-canary"]

# Matches the default value for resolver.assetExts in metro-config
ASSET_EXTENSIONS = [
    # Image formats
    "bmp",
    "gif",
    "jpg",
    "jpeg",
    "png",
    "psd",
    "svg",
    "webp",
    # Video formats
    "m4v",
    "mov",
    "mp4",
    "mpeg",
    "mpg",
    "webm",
    # Audio formats
    "aac",
    "aiff",
    "caf",
    "m4a",
    "mp3",
    "wav",
    # Document formats
    "html",
    "pdf",
    "yaml",
    "yml",
    # Font formats
    "otf",
    "ttf",
    # Archives (virtual files)
    "zip",
]

# Matches the default value for resolver.platforms in metro-config
ASSET_PLATFORMS = ["ios", "android", "windows", "web"]

def get_apple_resource_providers_for_js_bundle(ctx: AnalysisContext, js_bundle_info: JsBundleInfo.type, platform: str, skip_resources: bool) -> list[ResourceGraphInfo.type]:
    if platform != "ios":
        return []

    # `skip_resources` controls whether the JS resources should be skipped, not whether
    # the whole bundle should be skipped as a resource
    resources_content_dirs = [] if skip_resources else [js_bundle_info.res]
    resource_spec = AppleResourceSpec(
        content_dirs = [
            js_bundle_info.built_js,
        ] + resources_content_dirs,
        destination = AppleResourceDestination("resources"),
    )
    return [create_resource_graph(
        ctx = ctx,
        labels = ctx.attrs.labels,
        deps = [],
        exported_deps = [],
        resource_spec = resource_spec,
    )]

def _strip_platform_from_asset_name(name: str) -> str:
    name_without_extension, extension = paths.split_extension(name)
    return name_without_extension if extension[1:] in ASSET_PLATFORMS else name

def _strip_scale_from_asset_name(name: str) -> str:
    scale_start = -1
    for i in range(len(name)):
        char = name[i]
        if scale_start != -1:
            if char == "x":
                return name[:scale_start] + name[i + 1:]
            if char.isdigit() or char == ".":
                continue
            fail("Invalid format for scale of asset {}!".format(name))
        if name[i] == "@":
            scale_start = i

    expect(scale_start == -1, "Found scale_start but not its end {}!".format(name))

    return name

def get_canonical_src_name(src: str) -> str:
    dirname = paths.dirname(src)
    basename = paths.basename(src)
    filename, extension = paths.split_extension(basename)
    if extension[1:] not in ASSET_EXTENSIONS:
        return src

    filename = _strip_platform_from_asset_name(filename)
    filename = _strip_scale_from_asset_name(filename)

    return paths.join(dirname, filename + extension)

def get_flavors(ctx: AnalysisContext) -> list[str]:
    flavors = [ctx.attrs._platform]
    if ctx.attrs._is_release:
        flavors.append("release")

    return flavors

def get_bundle_name(ctx: AnalysisContext, default_bundle_name: str) -> str:
    bundle_name_for_flavor_map = {key: value for key, value in ctx.attrs.bundle_name_for_flavor}
    flavors = bundle_name_for_flavor_map.keys()
    for flavor in flavors:
        expect(
            flavor == "android" or flavor == "ios" or flavor == "macos" or flavor == "windows",
            "Currently only support picking bundle name by platform!",
        )

    platform = ctx.attrs._platform
    if platform in flavors:
        return bundle_name_for_flavor_map[platform]
    else:
        return ctx.attrs.bundle_name if ctx.attrs.bundle_name else default_bundle_name

def run_worker_commands(
        ctx: AnalysisContext,
        worker_tool: Dependency,
        command_args_files: list["artifact"],
        identifier: str,
        category: str,
        hidden_artifacts = [cmd_args]):
    worker_tool_info = worker_tool[WorkerToolInfo]
    worker_command = worker_tool_info.command.copy()
    worker_command.add("--command-args-file", command_args_files)
    worker_command.hidden(hidden_artifacts)
    worker_command.add("--command-args-file-extra-data-fixup-hack=true")

    ctx.actions.run(
        worker_command,
        category = category.replace("-", "_"),
        identifier = identifier,
    )
