# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:android_binary.bzl", "get_binary_info")
load("@prelude//android:android_providers.bzl", "AndroidApkInfo", "AndroidApkUnderTestInfo", "ExopackageInfo")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//java:java_providers.bzl", "KeystoreInfo")
load("@prelude//java/utils:java_utils.bzl", "get_path_separator")
load("@prelude//utils:set.bzl", "set")

def android_apk_impl(ctx: AnalysisContext) -> list["provider"]:
    android_binary_info = get_binary_info(ctx, use_proto_format = False)
    java_packaging_deps = android_binary_info.java_packaging_deps
    sub_targets = android_binary_info.sub_targets
    dex_files_info = android_binary_info.dex_files_info
    native_library_info = android_binary_info.native_library_info
    resources_info = android_binary_info.resources_info

    keystore = ctx.attrs.keystore[KeystoreInfo]
    output_apk = build_apk(
        label = ctx.label,
        actions = ctx.actions,
        android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo],
        keystore = keystore,
        dex_files_info = dex_files_info,
        native_library_info = native_library_info,
        resources_info = resources_info,
        compress_resources_dot_arsc = ctx.attrs.resource_compression == "enabled" or ctx.attrs.resource_compression == "enabled_with_strings_as_assets",
    )

    exopackage_info = ExopackageInfo(
        secondary_dex_info = dex_files_info.secondary_dex_exopackage_info,
        native_library_info = native_library_info.exopackage_info,
        resources_info = resources_info.exopackage_info,
    )

    return [
        AndroidApkInfo(apk = output_apk, manifest = resources_info.manifest),
        AndroidApkUnderTestInfo(
            java_packaging_deps = set([dep.label.raw_target() for dep in java_packaging_deps]),
            keystore = keystore,
            manifest_entries = ctx.attrs.manifest_entries,
            prebuilt_native_library_dirs = set([native_lib.raw_target for native_lib in native_library_info.apk_under_test_prebuilt_native_library_dirs]),
            platforms = android_binary_info.deps_by_platform.keys(),
            primary_platform = android_binary_info.primary_platform,
            resource_infos = set([info.raw_target for info in resources_info.unfiltered_resource_infos]),
            shared_libraries = set([shared_lib.label.raw_target() for shared_lib in native_library_info.apk_under_test_shared_libraries]),
        ),
        DefaultInfo(default_output = output_apk, other_outputs = _get_exopackage_outputs(exopackage_info), sub_targets = sub_targets),
        get_install_info(ctx, output_apk = output_apk, manifest = resources_info.manifest, exopackage_info = exopackage_info),
        TemplatePlaceholderInfo(
            keyed_variables = {
                "classpath": cmd_args([dep.jar for dep in java_packaging_deps if dep.jar], delimiter = get_path_separator()),
                "classpath_including_targets_with_no_output": cmd_args([dep.output_for_classpath_macro for dep in java_packaging_deps], delimiter = get_path_separator()),
            },
        ),
    ]

def build_apk(
        label: Label,
        actions: "actions",
        keystore: KeystoreInfo.type,
        android_toolchain: AndroidToolchainInfo.type,
        dex_files_info: "DexFilesInfo",
        native_library_info: "AndroidBinaryNativeLibsInfo",
        resources_info: "AndroidBinaryResourcesInfo",
        compress_resources_dot_arsc: bool = False) -> "artifact":
    output_apk = actions.declare_output("{}.apk".format(label.name))

    apk_builder_args = cmd_args([
        android_toolchain.apk_builder[RunInfo],
        "--output-apk",
        output_apk.as_output(),
        "--resource-apk",
        resources_info.primary_resources_apk,
        "--dex-file",
        dex_files_info.primary_dex,
        "--keystore-path",
        keystore.store,
        "--keystore-properties-path",
        keystore.properties,
        "--zipalign_tool",
        android_toolchain.zipalign[RunInfo],
    ])

    if compress_resources_dot_arsc:
        apk_builder_args.add("--compress-resources-dot-arsc")

    asset_directories = (
        native_library_info.root_module_native_lib_assets +
        native_library_info.non_root_module_native_lib_assets +
        dex_files_info.root_module_secondary_dex_dirs +
        dex_files_info.non_root_module_secondary_dex_dirs +
        resources_info.module_manifests
    )
    asset_directories_file = actions.write("asset_directories.txt", asset_directories)
    apk_builder_args.hidden(asset_directories)
    native_library_directories = actions.write("native_library_directories", native_library_info.native_libs_for_primary_apk)
    apk_builder_args.hidden(native_library_info.native_libs_for_primary_apk)
    all_zip_files = [resources_info.packaged_string_assets] if resources_info.packaged_string_assets else []
    zip_files = actions.write("zip_files", all_zip_files)
    apk_builder_args.hidden(all_zip_files)
    jar_files_that_may_contain_resources = actions.write("jar_files_that_may_contain_resources", resources_info.jar_files_that_may_contain_resources)
    apk_builder_args.hidden(resources_info.jar_files_that_may_contain_resources)

    apk_builder_args.add([
        "--asset-directories-list",
        asset_directories_file,
        "--native-libraries-directories-list",
        native_library_directories,
        "--zip-files-list",
        zip_files,
        "--jar-files-that-may-contain-resources-list",
        jar_files_that_may_contain_resources,
    ])

    actions.run(apk_builder_args, category = "apk_build")

    return output_apk

def get_install_info(ctx: AnalysisContext, output_apk: "artifact", manifest: "artifact", exopackage_info: [ExopackageInfo.type, None]) -> InstallInfo.type:
    files = {
        ctx.attrs.name: output_apk,
        "manifest": manifest,
        "options": generate_install_config(ctx),
    }

    if exopackage_info:
        secondary_dex_exopackage_info = exopackage_info.secondary_dex_info
        native_library_exopackage_info = exopackage_info.native_library_info
        resources_info = exopackage_info.resources_info
    else:
        secondary_dex_exopackage_info = None
        native_library_exopackage_info = None
        resources_info = None

    if secondary_dex_exopackage_info:
        files["secondary_dex_exopackage_info_directory"] = secondary_dex_exopackage_info.directory
        files["secondary_dex_exopackage_info_metadata"] = secondary_dex_exopackage_info.metadata

    if native_library_exopackage_info:
        files["native_library_exopackage_info_directory"] = native_library_exopackage_info.directory
        files["native_library_exopackage_info_metadata"] = native_library_exopackage_info.metadata

    if resources_info:
        if resources_info.assets:
            files["resources_exopackage_assets"] = resources_info.assets
            files["resources_exopackage_assets_hash"] = resources_info.assets_hash

        files["resources_exopackage_res"] = resources_info.res
        files["resources_exopackage_res_hash"] = resources_info.res_hash

    if secondary_dex_exopackage_info or native_library_exopackage_info or resources_info:
        files["exopackage_agent_apk"] = ctx.attrs._android_toolchain[AndroidToolchainInfo].exopackage_agent_apk

    return InstallInfo(
        installer = ctx.attrs._android_toolchain[AndroidToolchainInfo].installer,
        files = files,
    )

def _get_exopackage_outputs(exopackage_info: ExopackageInfo.type) -> list["artifact"]:
    outputs = []
    secondary_dex_exopackage_info = exopackage_info.secondary_dex_info
    if secondary_dex_exopackage_info:
        outputs.append(secondary_dex_exopackage_info.metadata)
        outputs.append(secondary_dex_exopackage_info.directory)

    native_library_exopackage_info = exopackage_info.native_library_info
    if native_library_exopackage_info:
        outputs.append(native_library_exopackage_info.metadata)
        outputs.append(native_library_exopackage_info.directory)

    resources_info = exopackage_info.resources_info
    if resources_info:
        outputs.append(resources_info.res)
        outputs.append(resources_info.res_hash)

        if resources_info.assets:
            outputs.append(resources_info.assets)
            outputs.append(resources_info.assets_hash)

    return outputs

def generate_install_config(ctx: AnalysisContext) -> "artifact":
    data = get_install_config()
    return ctx.actions.write_json("install_android_options.json", data)

def get_install_config() -> dict[str, ""]:
    # TODO: read from toolchains
    install_config = {
        "adb_restart_on_failure": read_root_config("adb", "adb_restart_on_failure", "true"),
        "agent_port_base": read_root_config("adb", "agent_port_base", "2828"),
        "always_use_java_agent": read_root_config("adb", "always_use_java_agent", "false"),
        "is_zstd_compression_enabled": read_root_config("adb", "is_zstd_compression_enabled", "false"),
        "multi_install_mode": read_root_config("adb", "multi_install_mode", "false"),
        "skip_install_metadata": read_root_config("adb", "skip_install_metadata", "false"),
    }

    adb_executable = read_root_config("android", "adb", None)
    if adb_executable:
        install_config["adb_executable"] = adb_executable

    return install_config
