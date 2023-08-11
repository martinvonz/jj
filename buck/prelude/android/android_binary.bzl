# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:android_binary_native_library_rules.bzl", "get_android_binary_native_library_info")
load("@prelude//android:android_binary_resources_rules.bzl", "get_android_binary_resources_info")
load("@prelude//android:android_build_config.bzl", "generate_android_build_config", "get_build_config_fields")
load("@prelude//android:android_providers.bzl", "BuildConfigField", "merge_android_packageable_info")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:configuration.bzl", "get_deps_by_platform")
load("@prelude//android:cpu_filters.bzl", "CPU_FILTER_FOR_DEFAULT_PLATFORM", "CPU_FILTER_FOR_PRIMARY_PLATFORM")
load("@prelude//android:dex_rules.bzl", "get_multi_dex", "get_single_primary_dex", "get_split_dex_merge_config", "merge_to_single_dex", "merge_to_split_dex")
load("@prelude//android:exopackage.bzl", "get_exopackage_flags")
load("@prelude//android:preprocess_java_classes.bzl", "get_preprocessed_java_classes")
load("@prelude//android:proguard.bzl", "get_proguard_output")
load("@prelude//android:voltron.bzl", "get_target_to_module_mapping")
load("@prelude//java:java_providers.bzl", "create_java_packaging_dep", "get_all_java_packaging_deps", "get_all_java_packaging_deps_from_packaging_infos")
load("@prelude//utils:utils.bzl", "expect")

AndroidBinaryInfo = record(
    sub_targets = dict,
    java_packaging_deps = ["JavaPackagingDep"],
    deps_by_platform = dict,
    primary_platform = str,
    dex_files_info = "DexFilesInfo",
    native_library_info = "AndroidBinaryNativeLibsInfo",
    resources_info = "AndroidBinaryResourcesInfo",
)

def get_binary_info(ctx: AnalysisContext, use_proto_format: bool) -> AndroidBinaryInfo.type:
    sub_targets = {}

    _verify_params(ctx)

    deps_by_platform = get_deps_by_platform(ctx)
    primary_platform = CPU_FILTER_FOR_PRIMARY_PLATFORM if CPU_FILTER_FOR_PRIMARY_PLATFORM in deps_by_platform else CPU_FILTER_FOR_DEFAULT_PLATFORM
    deps = deps_by_platform[primary_platform]

    target_to_module_mapping_file = get_target_to_module_mapping(ctx, deps_by_platform)

    no_dx_target_labels = [no_dx_target.label.raw_target() for no_dx_target in ctx.attrs.no_dx]
    java_packaging_deps = [packaging_dep for packaging_dep in get_all_java_packaging_deps(ctx, deps)]

    android_packageable_info = merge_android_packageable_info(ctx.label, ctx.actions, deps)
    build_config_infos = list(android_packageable_info.build_config_infos.traverse()) if android_packageable_info.build_config_infos else []

    build_config_libs = _get_build_config_java_libraries(ctx, build_config_infos)
    java_packaging_deps += get_all_java_packaging_deps_from_packaging_infos(ctx, build_config_libs)

    has_proguard_config = ctx.attrs.proguard_config != None or ctx.attrs.android_sdk_proguard_config == "default" or ctx.attrs.android_sdk_proguard_config == "optimized"
    should_pre_dex = not ctx.attrs.disable_pre_dex and not has_proguard_config and not ctx.attrs.preprocess_java_classes_bash

    referenced_resources_lists = [java_packaging_dep.dex.referenced_resources for java_packaging_dep in java_packaging_deps if java_packaging_dep.dex] if ctx.attrs.trim_resource_ids and should_pre_dex else []
    resources_info = get_android_binary_resources_info(
        ctx,
        deps,
        android_packageable_info,
        java_packaging_deps,
        apk_module_graph_file = target_to_module_mapping_file,
        use_proto_format = use_proto_format,
        referenced_resources_lists = referenced_resources_lists,
        manifest_entries = ctx.attrs.manifest_entries,
    )
    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]
    java_packaging_deps += [
        create_java_packaging_dep(
            ctx,
            r_dot_java.library_output.full_library,
            dex_weight_factor = android_toolchain.r_dot_java_weight_factor,
        )
        for r_dot_java in resources_info.r_dot_javas
    ]

    dex_java_packaging_deps = [packaging_dep for packaging_dep in java_packaging_deps if packaging_dep.dex and packaging_dep.dex.dex.owner.raw_target() not in no_dx_target_labels]
    if should_pre_dex:
        pre_dexed_libs = [packaging_dep.dex for packaging_dep in dex_java_packaging_deps]
        if ctx.attrs.use_split_dex:
            dex_files_info = merge_to_split_dex(
                ctx,
                android_toolchain,
                pre_dexed_libs,
                get_split_dex_merge_config(ctx, android_toolchain),
                target_to_module_mapping_file,
            )
        else:
            dex_files_info = merge_to_single_dex(ctx, android_toolchain, pre_dexed_libs)
    else:
        jars_to_owners = {packaging_dep.jar: packaging_dep.jar.owner.raw_target() for packaging_dep in dex_java_packaging_deps}
        if ctx.attrs.preprocess_java_classes_bash:
            jars_to_owners = get_preprocessed_java_classes(ctx, jars_to_owners)
        if has_proguard_config:
            proguard_output = get_proguard_output(
                ctx,
                jars_to_owners,
                java_packaging_deps,
                resources_info.proguard_config_file,
                [no_dx[DefaultInfo].default_outputs[0] for no_dx in ctx.attrs.no_dx if len(no_dx[DefaultInfo].default_outputs) == 1],
            )
            jars_to_owners = proguard_output.jars_to_owners
            dir_srcs = {artifact.basename: artifact for artifact in proguard_output.proguard_artifacts}
            for i, hidden_artifact in enumerate(proguard_output.proguard_hidden_artifacts):
                dir_srcs["hidden/{}_{}".format(i, hidden_artifact.basename)] = hidden_artifact
            sub_targets["proguard_text_output"] = [
                DefaultInfo(
                    default_output = ctx.actions.symlinked_dir(
                        "proguard_text_output",
                        dir_srcs,
                    ),
                ),
            ]
        else:
            proguard_output = None

        if ctx.attrs.use_split_dex:
            dex_files_info = get_multi_dex(
                ctx,
                ctx.attrs._android_toolchain[AndroidToolchainInfo],
                jars_to_owners,
                ctx.attrs.primary_dex_patterns,
                proguard_output.proguard_configuration_output_file if proguard_output else None,
                proguard_output.proguard_mapping_output_file if proguard_output else None,
                is_optimized = has_proguard_config,
                apk_module_graph_file = target_to_module_mapping_file,
            )
        else:
            dex_files_info = get_single_primary_dex(
                ctx,
                ctx.attrs._android_toolchain[AndroidToolchainInfo],
                jars_to_owners.keys(),
                is_optimized = has_proguard_config,
            )

    native_library_info = get_android_binary_native_library_info(ctx, android_packageable_info, deps_by_platform, apk_module_graph_file = target_to_module_mapping_file)
    unstripped_native_libs = native_library_info.unstripped_libs
    sub_targets["unstripped_native_libraries"] = [
        DefaultInfo(
            default_output = ctx.actions.write("unstripped_native_libraries", unstripped_native_libs.keys()),
            other_outputs = unstripped_native_libs.keys(),
        ),
    ]
    sub_targets["unstripped_native_libraries_json"] = [
        DefaultInfo(
            default_output = ctx.actions.write_json("unstripped_native_libraries_json", unstripped_native_libs),
            other_outputs = unstripped_native_libs.keys(),
        ),
    ]
    if resources_info.string_source_map:
        sub_targets["generate_string_resources"] = [DefaultInfo(default_output = resources_info.string_source_map)]

    if resources_info.voltron_string_source_map:
        sub_targets["generate_voltron_string_resources"] = [DefaultInfo(default_output = resources_info.voltron_string_source_map)]

    if dex_files_info.primary_dex_class_names:
        sub_targets["primary_dex_class_names"] = [DefaultInfo(default_output = dex_files_info.primary_dex_class_names)]

    return AndroidBinaryInfo(
        sub_targets = sub_targets,
        java_packaging_deps = java_packaging_deps,
        deps_by_platform = deps_by_platform,
        primary_platform = primary_platform,
        dex_files_info = dex_files_info,
        native_library_info = native_library_info,
        resources_info = resources_info,
    )

def _get_build_config_java_libraries(ctx: AnalysisContext, build_config_infos: list["AndroidBuildConfigInfo"]) -> list["JavaPackagingInfo"]:
    # BuildConfig deps should not be added for instrumented APKs because BuildConfig.class has
    # already been added to the APK under test.
    if ctx.attrs.package_type == "instrumented":
        return []

    build_config_constants = [
        BuildConfigField(type = "boolean", name = "DEBUG", value = str(ctx.attrs.package_type != "release").lower()),
        BuildConfigField(type = "boolean", name = "IS_EXOPACKAGE", value = str(len(ctx.attrs.exopackage_modes) > 0).lower()),
        BuildConfigField(type = "int", name = "EXOPACKAGE_FLAGS", value = str(get_exopackage_flags(ctx.attrs.exopackage_modes))),
    ]

    default_build_config_fields = get_build_config_fields(ctx.attrs.build_config_values)

    java_libraries = []
    java_packages_seen = []
    for build_config_info in build_config_infos:
        java_package = build_config_info.package
        expect(java_package not in java_packages_seen, "Got the same java_package {} for different AndroidBuildConfigs".format(java_package))
        java_packages_seen.append(java_package)

        all_build_config_values = {}
        for build_config_field in build_config_info.build_config_fields + default_build_config_fields + build_config_constants:
            all_build_config_values[build_config_field.name] = build_config_field

        java_libraries.append(generate_android_build_config(
            ctx,
            java_package,
            java_package,
            True,  # use_constant_expressions
            all_build_config_values.values(),
            ctx.attrs.build_config_values_file[DefaultInfo].default_outputs[0] if type(ctx.attrs.build_config_values_file) == "dependency" else ctx.attrs.build_config_values_file,
        )[1])

    return java_libraries

def _verify_params(ctx: AnalysisContext):
    expect(ctx.attrs.aapt_mode == "aapt2", "aapt1 is deprecated!")
    expect(ctx.attrs.dex_tool == "d8", "dx is deprecated!")
    expect(not ctx.attrs.use_split_dex or ctx.attrs.allow_r_dot_java_in_secondary_dex)
