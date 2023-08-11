# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//java:java_providers.bzl", "get_java_packaging_info")
load("@prelude//utils:utils.bzl", "expect")
load(":android_providers.bzl", "AndroidResourceInfo", "ExportedAndroidResourceInfo", "RESOURCE_PRIORITY_NORMAL", "merge_android_packageable_info")
load(":android_toolchain.bzl", "AndroidToolchainInfo")

JAVA_PACKAGE_FILENAME = "java_package.txt"

def _convert_to_artifact_dir(ctx: AnalysisContext, attr: [Dependency, dict, "artifact", None], attr_name: str) -> ["artifact", None]:
    if type(attr) == "dependency":
        expect(len(attr[DefaultInfo].default_outputs) == 1, "Expect one default output from build dep of attr {}!".format(attr_name))
        return attr[DefaultInfo].default_outputs[0]
    elif type(attr) == "dict":
        return None if len(attr) == 0 else ctx.actions.symlinked_dir(attr_name, attr)
    else:
        return attr

def android_resource_impl(ctx: AnalysisContext) -> list["provider"]:
    if ctx.attrs._build_only_native_code:
        return [DefaultInfo()]

    # TODO(T100007184) filter res/assets by ignored filenames
    sub_targets = {}
    providers = []
    default_output = None

    res = _convert_to_artifact_dir(ctx, ctx.attrs.res, "res")
    assets = _convert_to_artifact_dir(ctx, ctx.attrs.assets, "assets")

    if res:
        aapt2_compile_output = aapt2_compile(ctx, res, ctx.attrs._android_toolchain[AndroidToolchainInfo])

        sub_targets["aapt2_compile"] = [DefaultInfo(default_output = aapt2_compile_output)]

        r_dot_txt_output = get_text_symbols(ctx, res, ctx.attrs.deps)
        default_output = r_dot_txt_output

        r_dot_java_package = _get_package(ctx, ctx.attrs.package, ctx.attrs.manifest)
        resource_info = AndroidResourceInfo(
            raw_target = ctx.label.raw_target(),
            aapt2_compile_output = aapt2_compile_output,
            allow_strings_as_assets_resource_filtering = not ctx.attrs.has_whitelisted_strings,
            assets = assets,
            manifest_file = ctx.attrs.manifest,
            r_dot_java_package = r_dot_java_package,
            res = res,
            res_priority = RESOURCE_PRIORITY_NORMAL,
            text_symbols = r_dot_txt_output,
        )
    else:
        resource_info = AndroidResourceInfo(
            raw_target = ctx.label.raw_target(),
            aapt2_compile_output = None,
            allow_strings_as_assets_resource_filtering = not ctx.attrs.has_whitelisted_strings,
            assets = assets,
            manifest_file = ctx.attrs.manifest,
            r_dot_java_package = None,
            res = None,
            res_priority = RESOURCE_PRIORITY_NORMAL,
            text_symbols = None,
        )
    providers.append(resource_info)
    providers.append(merge_android_packageable_info(ctx.label, ctx.actions, ctx.attrs.deps, manifest = ctx.attrs.manifest, resource_info = resource_info))
    providers.append(get_java_packaging_info(ctx, ctx.attrs.deps))
    providers.append(DefaultInfo(default_output = default_output, sub_targets = sub_targets))

    return providers

def aapt2_compile(
        ctx: AnalysisContext,
        resources_dir: "artifact",
        android_toolchain: "AndroidToolchainInfo",
        skip_crunch_pngs: bool = False,
        identifier: [str, None] = None) -> "artifact":
    aapt2_command = cmd_args(android_toolchain.aapt2)
    aapt2_command.add("compile")
    aapt2_command.add("--legacy")
    if skip_crunch_pngs:
        aapt2_command.add("--no-crunch")
    aapt2_command.add(["--dir", resources_dir])
    aapt2_output = ctx.actions.declare_output("{}_resources.flata".format(identifier) if identifier else "resources.flata")
    aapt2_command.add("-o", aapt2_output.as_output())

    ctx.actions.run(aapt2_command, category = "aapt2_compile", identifier = identifier)

    return aapt2_output

def _get_package(ctx: AnalysisContext, package: [str, None], manifest: ["artifact", None]) -> "artifact":
    if package:
        return ctx.actions.write(JAVA_PACKAGE_FILENAME, package)
    else:
        expect(manifest != None, "if package is not declared then a manifest must be")
        return extract_package_from_manifest(ctx, manifest)

def extract_package_from_manifest(ctx: AnalysisContext, manifest: "artifact") -> "artifact":
    r_dot_java_package = ctx.actions.declare_output(JAVA_PACKAGE_FILENAME)
    extract_package_cmd = cmd_args(ctx.attrs._android_toolchain[AndroidToolchainInfo].manifest_utils[RunInfo])
    extract_package_cmd.add(["--manifest-path", manifest])
    extract_package_cmd.add(["--package-output", r_dot_java_package.as_output()])

    ctx.actions.run(extract_package_cmd, category = "android_extract_package")

    return r_dot_java_package

def get_text_symbols(
        ctx: AnalysisContext,
        res: "artifact",
        deps: list[Dependency],
        identifier: [str, None] = None):
    mini_aapt_cmd = cmd_args(ctx.attrs._android_toolchain[AndroidToolchainInfo].mini_aapt[RunInfo])

    mini_aapt_cmd.add(["--resource-paths", res])

    dep_symbol_paths = cmd_args()
    dep_symbols = _get_dep_symbols(deps)
    dep_symbol_paths.add(dep_symbols)

    dep_symbol_paths_file, _ = ctx.actions.write("{}_dep_symbol_paths_file".format(identifier) if identifier else "dep_symbol_paths_file", dep_symbol_paths, allow_args = True)

    mini_aapt_cmd.add(["--dep-symbol-paths", dep_symbol_paths_file])
    mini_aapt_cmd.hidden(dep_symbols)

    text_symbols = ctx.actions.declare_output("{}_R.txt".format(identifier) if identifier else "R.txt")
    mini_aapt_cmd.add(["--output-path", text_symbols.as_output()])

    ctx.actions.run(mini_aapt_cmd, category = "mini_aapt", identifier = identifier)

    return text_symbols

def _get_dep_symbols(deps: list[Dependency]) -> list["artifact"]:
    dep_symbols = []
    for dep in deps:
        android_resource_info = dep.get(AndroidResourceInfo)
        exported_android_resource_info = dep.get(ExportedAndroidResourceInfo)
        expect(android_resource_info != None or exported_android_resource_info != None, "Dependencies of `android_resource` rules should be `android_resource`s or `android_library`s")
        if android_resource_info and android_resource_info.text_symbols:
            dep_symbols.append(android_resource_info.text_symbols)
        if exported_android_resource_info:
            dep_symbols += [resource_info.text_symbols for resource_info in exported_android_resource_info.resource_infos if resource_info.text_symbols]

    return dedupe(dep_symbols)
