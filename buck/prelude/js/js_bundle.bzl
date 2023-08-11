# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:android_providers.bzl", "AndroidResourceInfo", "RESOURCE_PRIORITY_NORMAL", "merge_android_packageable_info")
load("@prelude//android:android_resource.bzl", "JAVA_PACKAGE_FILENAME", "aapt2_compile", "get_text_symbols")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//js:js_providers.bzl", "JsBundleInfo", "JsLibraryInfo", "get_transitive_outputs")
load("@prelude//js:js_utils.bzl", "RAM_BUNDLE_TYPES", "TRANSFORM_PROFILES", "get_apple_resource_providers_for_js_bundle", "get_bundle_name", "get_flavors", "run_worker_commands")
load("@prelude//utils:utils.bzl", "expect", "map_idx")

def _build_dependencies_file(
        ctx: AnalysisContext,
        transform_profile: str,
        flavors: list[str],
        transitive_js_library_outputs: "transitive_set_args_projection") -> "artifact":
    dependencies_file = ctx.actions.declare_output("{}/dependencies_file", transform_profile)

    # ctx.attrs.extra_json can contain attrs.arg().
    #
    # As a result, we need to pass extra_data_args as hidden arguments so that the rule
    # it is referencing exists as an input.
    extra_data_args = cmd_args(
        ctx.attrs.extra_json if ctx.attrs.extra_json else "{}",
        delimiter = "",
    )
    job_args = {
        "command": "dependencies",
        "entryPoints": [ctx.attrs.entry] if type(ctx.attrs.entry) == "string" else list(ctx.attrs.entry),
        "extraData": extra_data_args,
        "flavors": flavors,
        "libraries": transitive_js_library_outputs,
        "outputFilePath": dependencies_file,
        "platform": ctx.attrs._platform,
        "release": ctx.attrs._is_release,
    }
    command_args_file = ctx.actions.write_json(
        "{}_dep_command_args".format(transform_profile),
        job_args,
    )

    run_worker_commands(
        ctx = ctx,
        worker_tool = ctx.attrs.worker,
        command_args_files = [command_args_file],
        identifier = transform_profile,
        category = "dependencies",
        hidden_artifacts = [cmd_args([
            dependencies_file.as_output(),
            extra_data_args,
        ]).add(transitive_js_library_outputs)],
    )
    return dependencies_file

def _build_js_bundle(
        ctx: AnalysisContext,
        bundle_name: str,
        ram_bundle_name: str,
        ram_bundle_command: str,
        transform_profile: str,
        flavors: list[str],
        transitive_js_library_outputs: "transitive_set_args_projection",
        dependencies_file: "artifact") -> JsBundleInfo.type:
    base_dir = "{}_{}".format(ram_bundle_name, transform_profile) if ram_bundle_name else transform_profile
    assets_dir = ctx.actions.declare_output("{}/assets_dir".format(base_dir))
    bundle_dir_output = ctx.actions.declare_output("{}/js".format(base_dir), dir = True)
    misc_dir_path = ctx.actions.declare_output("{}/misc_dir_path".format(base_dir))
    source_map = ctx.actions.declare_output("{}/source_map".format(base_dir))

    # ctx.attrs.extra_json can contain attrs.arg().
    #
    # As a result, we need to pass extra_data_args as hidden arguments so that the rule
    # it is referencing exists as an input.
    extra_data_args = cmd_args(
        ctx.attrs.extra_json if ctx.attrs.extra_json else "{}",
        delimiter = "",
    )
    job_args = {
        "assetsDirPath": assets_dir,
        "bundlePath": cmd_args(
            [bundle_dir_output, bundle_name],
            delimiter = "/",
        ),
        "command": "bundle",
        "entryPoints": [ctx.attrs.entry] if type(ctx.attrs.entry) == "string" else list(ctx.attrs.entry),
        "extraData": extra_data_args,
        "flavors": flavors,
        "libraries": transitive_js_library_outputs,
        "miscDirPath": misc_dir_path,
        "platform": ctx.attrs._platform,
        "release": ctx.attrs._is_release,
        "sourceMapPath": source_map,
    }

    if ram_bundle_command:
        job_args["ramBundle"] = ram_bundle_command

    command_args_file = ctx.actions.write_json(
        "{}_bundle_command_args".format(base_dir),
        job_args,
    )

    run_worker_commands(
        ctx = ctx,
        worker_tool = ctx.attrs.worker,
        command_args_files = [command_args_file],
        identifier = base_dir,
        category = job_args["command"],
        hidden_artifacts = [cmd_args([
            bundle_dir_output.as_output(),
            assets_dir.as_output(),
            misc_dir_path.as_output(),
            source_map.as_output(),
            extra_data_args,
        ]).add(transitive_js_library_outputs)],
    )

    return JsBundleInfo(
        bundle_name = bundle_name,
        built_js = bundle_dir_output,
        source_map = source_map,
        res = assets_dir,
        misc = misc_dir_path,
        dependencies_file = dependencies_file,
    )

def _get_fallback_transform_profile(ctx: AnalysisContext) -> str:
    if ctx.attrs.fallback_transform_profile in TRANSFORM_PROFILES:
        return ctx.attrs.fallback_transform_profile

    if ctx.attrs.fallback_transform_profile == "default" or ctx.attrs.fallback_transform_profile == None:
        return "transform-profile-default"

    fail("Invalid fallback_transform_profile attribute {}!".format(ctx.attrs.fallback_transform_profile))

def _get_default_providers(js_bundle_info: JsBundleInfo.type) -> list["provider"]:
    return [DefaultInfo(default_output = js_bundle_info.built_js)]

def _get_android_resource_info(ctx: AnalysisContext, js_bundle_info: JsBundleInfo.type, identifier: str) -> "AndroidResourceInfo":
    aapt2_compile_output = aapt2_compile(
        ctx,
        js_bundle_info.res,
        ctx.attrs._android_toolchain[AndroidToolchainInfo],
        identifier = identifier,
    )
    expect(ctx.attrs.android_package != None, "Must provide android_package for android builds!")
    r_dot_java_package = ctx.actions.write("{}_{}".format(identifier, JAVA_PACKAGE_FILENAME), ctx.attrs.android_package)
    return AndroidResourceInfo(
        raw_target = ctx.label.raw_target(),
        aapt2_compile_output = aapt2_compile_output,
        allow_strings_as_assets_resource_filtering = True,
        assets = js_bundle_info.built_js,
        manifest_file = None,
        r_dot_java_package = r_dot_java_package,
        res = js_bundle_info.res,
        res_priority = RESOURCE_PRIORITY_NORMAL,
        text_symbols = get_text_symbols(ctx, js_bundle_info.res, [], identifier),
    )

def _get_extra_providers(ctx: AnalysisContext, js_bundle_info: JsBundleInfo.type, identifier: str) -> list["provider"]:
    providers = [js_bundle_info]
    if ctx.attrs._platform == "android":
        resource_info = _get_android_resource_info(ctx, js_bundle_info, identifier)
        providers.append(resource_info)
        providers.append(merge_android_packageable_info(ctx.label, ctx.actions, ctx.attrs.deps, resource_info = resource_info))

    providers += get_apple_resource_providers_for_js_bundle(ctx, js_bundle_info, ctx.attrs._platform, skip_resources = False)

    return providers

def js_bundle_impl(ctx: AnalysisContext) -> list["provider"]:
    sub_targets = {}
    default_outputs = []
    extra_unnamed_output_providers = None
    bundle_name = get_bundle_name(ctx, "{}.js".format(ctx.attrs.name))
    flavors = get_flavors(ctx)
    for transform_profile in TRANSFORM_PROFILES:
        dep_infos = map_idx(JsLibraryInfo, [dep[DefaultInfo].sub_targets[transform_profile] for dep in ctx.attrs.deps])

        transitive_js_library_tset = get_transitive_outputs(ctx.actions, deps = dep_infos)
        transitive_js_library_outputs = transitive_js_library_tset.project_as_args("artifacts")
        dependencies_file = _build_dependencies_file(ctx, transform_profile, flavors, transitive_js_library_outputs)

        for ram_bundle_name, ram_bundle_command in RAM_BUNDLE_TYPES.items():
            js_bundle_info = _build_js_bundle(ctx, bundle_name, ram_bundle_name, ram_bundle_command, transform_profile, flavors, transitive_js_library_outputs, dependencies_file)

            simple_name = transform_profile if not ram_bundle_name else "{}-{}".format(ram_bundle_name, transform_profile)
            built_js_providers = _get_default_providers(js_bundle_info)
            extra_providers = _get_extra_providers(ctx, js_bundle_info, simple_name)
            misc_providers = [DefaultInfo(default_output = js_bundle_info.misc)]
            source_map_providers = [DefaultInfo(default_output = js_bundle_info.source_map)]
            dependencies_providers = [DefaultInfo(default_output = js_bundle_info.dependencies_file)]
            res_providers = [DefaultInfo(default_output = js_bundle_info.res)]

            sub_targets[simple_name] = built_js_providers + extra_providers
            sub_targets["{}-misc".format(simple_name)] = misc_providers
            sub_targets["{}-source_map".format(simple_name)] = source_map_providers
            sub_targets["{}-dependencies".format(simple_name)] = dependencies_providers

            fallback_transform_profile = _get_fallback_transform_profile(ctx)
            if transform_profile == fallback_transform_profile:
                if not ram_bundle_name:
                    default_outputs.append(js_bundle_info.built_js)
                    expect(extra_unnamed_output_providers == None, "Extra unnamed output providers should only be set once!")
                    extra_unnamed_output_providers = extra_providers
                    sub_targets["misc"] = misc_providers
                    sub_targets["source_map"] = source_map_providers
                    sub_targets["dependencies"] = dependencies_providers
                    sub_targets["res"] = res_providers
                else:
                    sub_targets[ram_bundle_name] = built_js_providers + extra_providers
                    sub_targets["{}-misc".format(ram_bundle_name)] = misc_providers
                    sub_targets["{}-source_map".format(ram_bundle_name)] = source_map_providers
                    sub_targets["{}-dependencies".format(ram_bundle_name)] = dependencies_providers
                    sub_targets["{}-res".format(ram_bundle_name)] = res_providers

    expect(len(default_outputs) == 1, "Should get exactly one default output!")
    expect(extra_unnamed_output_providers != None, "Should set extra unnamed output providers once!")
    return [
        DefaultInfo(default_outputs = default_outputs, sub_targets = sub_targets),
    ] + extra_unnamed_output_providers
