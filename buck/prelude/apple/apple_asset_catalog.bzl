# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load("@prelude//utils:utils.bzl", "flatten")
load(":apple_asset_catalog_compilation_options.bzl", "AppleAssetCatalogsCompilationOptions", "get_apple_asset_catalogs_compilation_options")  # @unused Used as a type
load(":apple_asset_catalog_types.bzl", "AppleAssetCatalogResult", "AppleAssetCatalogSpec", "StringWithSourceTarget")
load(":apple_bundle_utility.bzl", "get_bundle_min_target_version", "get_bundle_resource_processing_options")
load(":apple_sdk.bzl", "get_apple_sdk_name")
load(":apple_sdk_metadata.bzl", "get_apple_sdk_metadata_for_sdk_name")
load(":resource_groups.bzl", "create_resource_graph")

def apple_asset_catalog_impl(ctx: AnalysisContext) -> list["provider"]:
    spec = AppleAssetCatalogSpec(
        app_icon = StringWithSourceTarget(source = ctx.label, value = ctx.attrs.app_icon) if ctx.attrs.app_icon != None else None,
        dirs = ctx.attrs.dirs,
        launch_image = StringWithSourceTarget(source = ctx.label, value = ctx.attrs.launch_image) if ctx.attrs.launch_image != None else None,
    )
    graph = create_resource_graph(
        ctx = ctx,
        labels = ctx.attrs.labels,
        deps = [],
        exported_deps = [],
        asset_catalog_spec = spec,
    )
    return [DefaultInfo(default_output = None), graph]

def compile_apple_asset_catalog(ctx: AnalysisContext, specs: list[AppleAssetCatalogSpec.type]) -> [AppleAssetCatalogResult.type, None]:
    single_spec = _merge_asset_catalog_specs(ctx, specs)
    if len(single_spec.dirs) == 0:
        return None
    plist = ctx.actions.declare_output("AssetCatalog.plist")
    catalog = ctx.actions.declare_output("AssetCatalogCompiled", dir = True)
    processing_options = get_bundle_resource_processing_options(ctx)
    compilation_options = get_apple_asset_catalogs_compilation_options(ctx)
    command = _get_actool_command(ctx, single_spec, catalog.as_output(), plist.as_output(), compilation_options)
    ctx.actions.run(command, prefer_local = processing_options.prefer_local, allow_cache_upload = processing_options.allow_cache_upload, category = "apple_asset_catalog")
    return AppleAssetCatalogResult(compiled_catalog = catalog, catalog_plist = plist)

def _merge_asset_catalog_specs(ctx: AnalysisContext, xs: list[AppleAssetCatalogSpec.type]) -> AppleAssetCatalogSpec.type:
    app_icon = _get_at_most_one_attribute(ctx, xs, "app_icon")
    launch_image = _get_at_most_one_attribute(ctx, xs, "launch_image")
    dirs = dedupe(flatten([x.dirs for x in xs]))
    return AppleAssetCatalogSpec(app_icon = app_icon, dirs = dirs, launch_image = launch_image)

def _get_at_most_one_attribute(ctx: AnalysisContext, xs: list["_record"], attr_name: str) -> ["StringWithSourceTarget", None]:
    all_values = dedupe(filter(None, [getattr(x, attr_name) for x in xs]))
    if len(all_values) > 1:
        fail("At most one asset catalog in the dependencies of `{}` can have an `{}` attribute. At least 2 catalogs are providing it: `{}` and `{}`.".format(_get_target(ctx), attr_name, all_values[0].source, all_values[1].source))
    elif len(all_values) == 1:
        return all_values[0]
    else:
        return None

def _get_target(ctx: AnalysisContext) -> str:
    return ctx.label.package + ":" + ctx.label.name

def _get_actool_command(ctx: AnalysisContext, info: AppleAssetCatalogSpec.type, catalog_output: "output_artifact", plist_output: "output_artifact", compilation_options: AppleAssetCatalogsCompilationOptions.type) -> cmd_args:
    external_name = get_apple_sdk_name(ctx)
    sdk_metadata = get_apple_sdk_metadata_for_sdk_name(external_name)
    target_device = sdk_metadata.target_device_flags

    actool_platform = sdk_metadata.actool_platform_override
    if not actool_platform:
        actool_platform = external_name

    actool = ctx.attrs._apple_toolchain[AppleToolchainInfo].actool
    actool_command = cmd_args([
                                  actool,
                                  "--platform",
                                  actool_platform,
                                  "--minimum-deployment-target",
                                  get_bundle_min_target_version(ctx, ctx.attrs.binary),
                                  "--compile",
                                  '"$TMPDIR"',
                                  "--output-partial-info-plist",
                                  plist_output,
                              ] +
                              target_device +
                              (
                                  ["--app-icon", info.app_icon.value] if info.app_icon else []
                              ) + (
                                  ["--launch-image", info.launch_image.value] if info.launch_image else []
                              ) + (
                                  ["--notices"] if compilation_options.enable_notices else []
                              ) + (
                                  ["--warnings"] if compilation_options.enable_warnings else []
                              ) + (
                                  ["--errors"] if compilation_options.enable_errors else []
                              ) + (
                                  ["--compress-pngs"] if compilation_options.compress_pngs else []
                              ) +
                              ["--optimization", compilation_options.optimization] +
                              ["--output-format", compilation_options.output_format] +
                              compilation_options.extra_flags +
                              info.dirs)

    # `actool` expects the output directory to be present.
    # Use the wrapper script to create the directory first and then actually call `actool`.
    wrapper_script, _ = ctx.actions.write(
        "actool_wrapper.sh",
        [
            cmd_args("set -euo pipefail"),
            cmd_args('export TMPDIR="$(mktemp -d)"'),
            cmd_args(actool_command, delimiter = " "),
            cmd_args(catalog_output, format = 'mkdir -p {} && cp -r "$TMPDIR"/ {}'),
        ],
        allow_args = True,
    )
    command = cmd_args(["/bin/sh", wrapper_script]).hidden([actool_command, catalog_output])
    return command
