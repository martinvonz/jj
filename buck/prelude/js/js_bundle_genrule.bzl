# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:genrule.bzl", "process_genrule")
load("@prelude//android:android_providers.bzl", "AndroidResourceInfo", "merge_android_packageable_info")
load("@prelude//js:js_providers.bzl", "JsBundleInfo")
load("@prelude//js:js_utils.bzl", "RAM_BUNDLE_TYPES", "TRANSFORM_PROFILES", "get_apple_resource_providers_for_js_bundle", "get_bundle_name")
load("@prelude//utils:utils.bzl", "expect")

def _build_js_bundle(
        ctx: AnalysisContext,
        bundle_name_out: str,
        js_bundle_info: JsBundleInfo.type,
        named_output: str) -> JsBundleInfo.type:
    env_vars = {
        "DEPENDENCIES": cmd_args(js_bundle_info.dependencies_file),
        "JS_BUNDLE_NAME": cmd_args(js_bundle_info.bundle_name),
        "JS_BUNDLE_NAME_OUT": cmd_args(bundle_name_out),
        "JS_DIR": cmd_args(js_bundle_info.built_js),
        "MISC_DIR": cmd_args(js_bundle_info.misc),
        "PLATFORM": cmd_args(ctx.attrs._platform),
        "RELEASE": cmd_args("1" if ctx.attrs._is_release else ""),
        "RES_DIR": cmd_args(js_bundle_info.res),
        "SOURCEMAP": cmd_args(js_bundle_info.source_map),
    }

    if ctx.attrs.rewrite_sourcemap:
        source_map_out = ctx.actions.declare_output("{}/source_map".format(named_output))
        env_vars["SOURCEMAP_OUT"] = cmd_args(source_map_out.as_output())
    else:
        source_map_out = js_bundle_info.source_map

    if ctx.attrs.rewrite_misc:
        misc_out = ctx.actions.declare_output("{}/misc".format(named_output))
        env_vars["MISC_OUT"] = cmd_args(misc_out.as_output())
    else:
        misc_out = js_bundle_info.misc

    if ctx.attrs.rewrite_deps_file:
        dependencies_out = ctx.actions.declare_output("{}/dependencies".format(named_output))
        env_vars["DEPENDENCIES_OUT"] = cmd_args(dependencies_out.as_output())
    else:
        dependencies_out = js_bundle_info.dependencies_file

    providers = process_genrule(ctx, "{}-js".format(named_output), None, env_vars, named_output)

    expect(
        len(providers) == 1,
        "expected exactly one provider of type DefaultInfo from {} ({})"
            .format(ctx.label.name, providers),
    )

    default_info = providers[0]  # DefaultInfo type
    outputs = default_info.default_outputs
    expect(
        len(outputs) == 1,
        "expected exactly one output from {} ({})"
            .format(ctx.label.name, outputs),
    )
    built_js = outputs[0]

    return JsBundleInfo(
        bundle_name = bundle_name_out,
        built_js = built_js,
        source_map = source_map_out,
        res = js_bundle_info.res,
        misc = misc_out,
        dependencies_file = dependencies_out,
    )

def _get_extra_providers(
        ctx: AnalysisContext,
        skip_resources: bool,
        initial_target: ["provider_collection", Dependency],
        js_bundle_out: JsBundleInfo.type) -> list["provider"]:
    providers = []
    android_resource_info = initial_target.get(AndroidResourceInfo)
    if android_resource_info:
        new_android_resource_info = AndroidResourceInfo(
            raw_target = ctx.label.raw_target(),
            aapt2_compile_output = None if skip_resources else android_resource_info.aapt2_compile_output,
            allow_strings_as_assets_resource_filtering = True,
            assets = js_bundle_out.built_js,
            manifest_file = None,
            r_dot_java_package = None if skip_resources else android_resource_info.r_dot_java_package,
            res = None if skip_resources else android_resource_info.res,
            res_priority = android_resource_info.res_priority,
            text_symbols = None if skip_resources else android_resource_info.text_symbols,
        )
        providers.append(new_android_resource_info)
        providers.append(merge_android_packageable_info(ctx.label, ctx.actions, deps = [], resource_info = new_android_resource_info))

    providers += get_apple_resource_providers_for_js_bundle(ctx, js_bundle_out, ctx.attrs._platform, skip_resources)

    return providers

def js_bundle_genrule_impl(ctx: AnalysisContext) -> list["provider"]:
    sub_targets = {}
    for transform_profile in TRANSFORM_PROFILES:
        for ram_bundle_name in RAM_BUNDLE_TYPES.keys():
            simple_named_output = transform_profile if not ram_bundle_name else "{}-{}".format(ram_bundle_name, transform_profile)

            js_bundle = ctx.attrs.js_bundle[DefaultInfo].sub_targets[simple_named_output]
            js_bundle_info = js_bundle[JsBundleInfo]
            bundle_name_out = get_bundle_name(ctx, js_bundle_info.bundle_name)

            js_bundle_out = _build_js_bundle(ctx, bundle_name_out, js_bundle_info, simple_named_output)

            sub_targets[simple_named_output] = [
                DefaultInfo(default_output = js_bundle_out.built_js),
                js_bundle_out,
            ] + _get_extra_providers(ctx, ctx.attrs.skip_resources, js_bundle, js_bundle_out)
            sub_targets["{}-misc".format(simple_named_output)] = [DefaultInfo(default_output = js_bundle_out.misc)]
            sub_targets["{}-source_map".format(simple_named_output)] = [DefaultInfo(default_output = js_bundle_out.source_map)]
            sub_targets["{}-dependencies".format(simple_named_output)] = [DefaultInfo(default_output = js_bundle_out.dependencies_file)]
            sub_targets["{}-res".format(simple_named_output)] = [DefaultInfo(default_output = js_bundle_out.res)]

    js_bundle_info = ctx.attrs.js_bundle[JsBundleInfo]
    bundle_name_out = get_bundle_name(ctx, js_bundle_info.bundle_name)
    js_bundle_out = _build_js_bundle(ctx, bundle_name_out, js_bundle_info, "default")

    sub_targets["dependencies"] = [DefaultInfo(default_output = js_bundle_out.dependencies_file)]
    sub_targets["misc"] = [DefaultInfo(default_output = js_bundle_out.misc)]
    sub_targets["source_map"] = [DefaultInfo(default_output = js_bundle_out.source_map)]
    sub_targets["res"] = [DefaultInfo(default_output = js_bundle_out.res)]
    default_info_out = DefaultInfo(
        default_output = js_bundle_out.built_js,
        sub_targets = sub_targets,
    )

    return [default_info_out, js_bundle_out] + _get_extra_providers(ctx, ctx.attrs.skip_resources, ctx.attrs.js_bundle, js_bundle_out)
