# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:android_binary.bzl", "get_binary_info")
load("@prelude//android:android_providers.bzl", "AndroidAabInfo")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//java/utils:java_utils.bzl", "get_path_separator")

def android_bundle_impl(ctx: AnalysisContext) -> list["provider"]:
    android_binary_info = get_binary_info(ctx, use_proto_format = True)

    output_bundle = build_bundle(
        label = ctx.label,
        actions = ctx.actions,
        android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo],
        dex_files_info = android_binary_info.dex_files_info,
        native_library_info = android_binary_info.native_library_info,
        resources_info = android_binary_info.resources_info,
    )

    java_packaging_deps = android_binary_info.java_packaging_deps
    return [
        DefaultInfo(default_output = output_bundle, sub_targets = android_binary_info.sub_targets),
        AndroidAabInfo(aab = output_bundle, manifest = android_binary_info.resources_info.manifest),
        TemplatePlaceholderInfo(
            keyed_variables = {
                "classpath": cmd_args([dep.jar for dep in java_packaging_deps if dep.jar], delimiter = get_path_separator()),
                "classpath_including_targets_with_no_output": cmd_args([dep.output_for_classpath_macro for dep in java_packaging_deps], delimiter = get_path_separator()),
            },
        ),
    ]

def build_bundle(
        label: Label,
        actions: "actions",
        android_toolchain: AndroidToolchainInfo.type,
        dex_files_info: "DexFilesInfo",
        native_library_info: "AndroidBinaryNativeLibsInfo",
        resources_info: "AndroidBinaryResourcesInfo") -> "artifact":
    output_bundle = actions.declare_output("{}.aab".format(label.name))

    bundle_builder_args = cmd_args([
        android_toolchain.bundle_builder[RunInfo],
        "--output-bundle",
        output_bundle.as_output(),
        "--resource-apk",
        resources_info.primary_resources_apk,
        "--dex-file",
        dex_files_info.primary_dex,
    ])

    root_module_asset_directories = native_library_info.root_module_native_lib_assets + dex_files_info.root_module_secondary_dex_dirs
    root_module_asset_directories_file = actions.write("root_module_asset_directories.txt", root_module_asset_directories)
    bundle_builder_args.hidden(root_module_asset_directories)
    non_root_module_asset_directories = resources_info.module_manifests + native_library_info.non_root_module_native_lib_assets + dex_files_info.non_root_module_secondary_dex_dirs
    non_root_module_asset_directories_file = actions.write("non_root_module_asset_directories.txt", non_root_module_asset_directories)
    bundle_builder_args.hidden(non_root_module_asset_directories)
    native_library_directories = actions.write("native_library_directories", native_library_info.native_libs_for_primary_apk)
    bundle_builder_args.hidden(native_library_info.native_libs_for_primary_apk)
    all_zip_files = [resources_info.packaged_string_assets] if resources_info.packaged_string_assets else []
    zip_files = actions.write("zip_files", all_zip_files)
    bundle_builder_args.hidden(all_zip_files)
    jar_files_that_may_contain_resources = actions.write("jar_files_that_may_contain_resources", resources_info.jar_files_that_may_contain_resources)
    bundle_builder_args.hidden(resources_info.jar_files_that_may_contain_resources)

    bundle_builder_args.add([
        "--root-module-asset-directories-list",
        root_module_asset_directories_file,
        "--non-root-module-asset-directories-list",
        non_root_module_asset_directories_file,
        "--native-libraries-directories-list",
        native_library_directories,
        "--zip-files-list",
        zip_files,
        "--jar-files-that-may-contain-resources-list",
        jar_files_that_may_contain_resources,
        "--zipalign_tool",
        android_toolchain.zipalign[RunInfo],
    ])

    actions.run(bundle_builder_args, category = "bundle_build")

    return output_bundle
