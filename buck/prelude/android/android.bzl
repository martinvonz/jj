# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//java:dex_toolchain.bzl", "DexToolchainInfo")
load("@prelude//java:java.bzl", "AbiGenerationMode", "dex_min_sdk_version", "select_java_test_toolchain")
load("@prelude//java:java_toolchain.bzl", "JavaPlatformInfo", "JavaTestToolchainInfo", "JavaToolchainInfo")
load("@prelude//kotlin:kotlin_toolchain.bzl", "KotlinToolchainInfo")
load("@prelude//decls/android_rules.bzl", "AaptMode", "DuplicateResourceBehaviour", "TargetCpuType")
load("@prelude//genrule.bzl", "genrule_attributes")
load(":android_aar.bzl", "android_aar_impl")
load(":android_apk.bzl", "android_apk_impl")
load(":android_build_config.bzl", "android_build_config_impl")
load(":android_bundle.bzl", "android_bundle_impl")
load(":android_instrumentation_apk.bzl", "android_instrumentation_apk_impl")
load(":android_instrumentation_test.bzl", "android_instrumentation_test_impl")
load(":android_library.bzl", "android_library_impl")
load(":android_manifest.bzl", "android_manifest_impl")
load(":android_prebuilt_aar.bzl", "android_prebuilt_aar_impl")
load(":android_resource.bzl", "android_resource_impl")
load(":android_toolchain.bzl", "AndroidPlatformInfo", "AndroidToolchainInfo")
load(":apk_genrule.bzl", "apk_genrule_impl")
load(":build_only_native_code.bzl", "is_build_only_native_code")
load(":configuration.bzl", "cpu_split_transition", "cpu_transition", "is_building_android_binary_attr")
load(":gen_aidl.bzl", "gen_aidl_impl")
load(":prebuilt_native_library.bzl", "prebuilt_native_library_impl")
load(":robolectric_test.bzl", "robolectric_test_impl")
load(":voltron.bzl", "android_app_modularity_impl")

def android_toolchain():
    return attrs.toolchain_dep(
        # FIXME: prelude// should be standalone (not refer to fbcode//)
        default = "fbcode//buck2/platform/toolchain:android",
        providers = [
            AndroidPlatformInfo,
            AndroidToolchainInfo,
        ],
    )

def _dex_toolchain():
    return attrs.toolchain_dep(
        # FIXME: prelude// should be standalone (not refer to fbcode//)
        default = "fbcode//buck2/platform/toolchain:dex_for_android",
        providers = [
            DexToolchainInfo,
        ],
    )

def java_toolchain_for_android():
    return attrs.toolchain_dep(
        # FIXME: prelude// should be standalone (not refer to fbcode//)
        default = "fbcode//buck2/platform/toolchain:java_for_android",
        providers = [
            JavaPlatformInfo,
            JavaToolchainInfo,
        ],
    )

def java_toolchain_for_android_test():
    return attrs.toolchain_dep(
        # FIXME: prelude// should be standalone (not refer to fbcode//)
        default = "fbcode//buck2/platform/toolchain:java_for_host_test",
        providers = [
            JavaPlatformInfo,
            JavaToolchainInfo,
        ],
    )

def _kotlin_toolchain():
    return attrs.toolchain_dep(
        # FIXME: prelude// should be standalone (not refer to fbcode//)
        default = "fbcode//buck2/platform/toolchain:kotlin",
        providers = [
            KotlinToolchainInfo,
        ],
    )

implemented_rules = {
    "android_aar": android_aar_impl,
    "android_app_modularity": android_app_modularity_impl,
    "android_binary": android_apk_impl,
    "android_build_config": android_build_config_impl,
    "android_bundle": android_bundle_impl,
    "android_instrumentation_apk": android_instrumentation_apk_impl,
    "android_instrumentation_test": android_instrumentation_test_impl,
    "android_library": android_library_impl,
    "android_manifest": android_manifest_impl,
    "android_prebuilt_aar": android_prebuilt_aar_impl,
    "android_resource": android_resource_impl,
    "apk_genrule": apk_genrule_impl,
    "gen_aidl": gen_aidl_impl,
    "prebuilt_native_library": prebuilt_native_library_impl,
    "robolectric_test": robolectric_test_impl,
}

# Can't load `read_bool` here because it will cause circular load.
FORCE_SINGLE_CPU = read_root_config("buck2", "android_force_single_cpu") in ("True", "true")
FORCE_SINGLE_DEFAULT_CPU = read_root_config("buck2", "android_force_single_default_cpu") in ("True", "true")

extra_attributes = {
    "android_aar": {
        "abi_generation_mode": attrs.option(attrs.enum(AbiGenerationMode), default = None),
        "resources_root": attrs.option(attrs.string(), default = None),
    },
    "android_app_modularity": {
        "_android_toolchain": android_toolchain(),
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
    },
    "android_binary": {
        "aapt_mode": attrs.enum(AaptMode, default = "aapt1"),  # Match default in V1
        "application_module_configs": attrs.dict(key = attrs.string(), value = attrs.list(attrs.transition_dep(cfg = cpu_transition)), sorted = False, default = {}),
        "build_config_values_file": attrs.option(attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()), default = None),
        "constraint_overrides": attrs.list(attrs.string(), default = []),
        "deps": attrs.list(attrs.split_transition_dep(cfg = cpu_split_transition), default = []),
        "dex_tool": attrs.string(default = "d8"),  # Match default in V1
        "duplicate_resource_behavior": attrs.enum(DuplicateResourceBehaviour, default = "allow_by_default"),  # Match default in V1
        "manifest": attrs.option(attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()), default = None),
        "manifest_skeleton": attrs.option(attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()), default = None),
        "min_sdk_version": attrs.option(attrs.int(), default = None),
        "module_manifest_skeleton": attrs.option(attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()), default = None),
        "_android_toolchain": android_toolchain(),
        "_dex_toolchain": _dex_toolchain(),
        "_is_building_android_binary": attrs.default_only(attrs.bool(default = True)),
        "_is_force_single_cpu": attrs.default_only(attrs.bool(default = FORCE_SINGLE_CPU)),
        "_is_force_single_default_cpu": attrs.default_only(attrs.bool(default = FORCE_SINGLE_DEFAULT_CPU)),
        "_java_toolchain": java_toolchain_for_android(),
    },
    "android_build_config": {
        "_android_toolchain": android_toolchain(),
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
        "_is_building_android_binary": is_building_android_binary_attr(),
        "_java_toolchain": java_toolchain_for_android(),
    },
    "android_bundle": {
        "aapt_mode": attrs.enum(AaptMode, default = "aapt1"),  # Match default in V1
        "application_module_configs": attrs.dict(key = attrs.string(), value = attrs.list(attrs.transition_dep(cfg = cpu_transition)), sorted = False, default = {}),
        "build_config_values_file": attrs.option(attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()), default = None),
        "deps": attrs.list(attrs.split_transition_dep(cfg = cpu_split_transition), default = []),
        "dex_tool": attrs.string(default = "d8"),  # Match default in V1
        "duplicate_resource_behavior": attrs.enum(DuplicateResourceBehaviour, default = "allow_by_default"),  # Match default in V1
        "manifest": attrs.option(attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()), default = None),
        "manifest_skeleton": attrs.option(attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()), default = None),
        "min_sdk_version": attrs.option(attrs.int(), default = None),
        "module_manifest_skeleton": attrs.option(attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()), default = None),
        "_android_toolchain": android_toolchain(),
        "_dex_toolchain": _dex_toolchain(),
        "_is_building_android_binary": attrs.default_only(attrs.bool(default = True)),
        "_is_force_single_cpu": attrs.default_only(attrs.bool(default = FORCE_SINGLE_CPU)),
        "_is_force_single_default_cpu": attrs.default_only(attrs.bool(default = FORCE_SINGLE_DEFAULT_CPU)),
        "_java_toolchain": java_toolchain_for_android(),
    },
    "android_instrumentation_apk": {
        "aapt_mode": attrs.enum(AaptMode, default = "aapt1"),  # Match default in V1
        "apk": attrs.dep(),
        "cpu_filters": attrs.list(attrs.enum(TargetCpuType), default = []),
        "deps": attrs.list(attrs.split_transition_dep(cfg = cpu_split_transition), default = []),
        "dex_tool": attrs.string(default = "d8"),  # Match default in V1
        "manifest": attrs.option(attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()), default = None),
        "manifest_skeleton": attrs.option(attrs.one_of(attrs.transition_dep(cfg = cpu_transition), attrs.source()), default = None),
        "min_sdk_version": attrs.option(attrs.int(), default = None),
        "_android_toolchain": android_toolchain(),
        "_dex_toolchain": _dex_toolchain(),
        "_is_building_android_binary": attrs.default_only(attrs.bool(default = True)),
        "_is_force_single_cpu": attrs.default_only(attrs.bool(default = FORCE_SINGLE_CPU)),
        "_is_force_single_default_cpu": attrs.default_only(attrs.bool(default = FORCE_SINGLE_DEFAULT_CPU)),
        "_java_toolchain": java_toolchain_for_android(),
    },
    "android_instrumentation_test": {
        "_android_toolchain": android_toolchain(),
        "_java_toolchain": java_toolchain_for_android(),
    },
    "android_library": {
        "abi_generation_mode": attrs.option(attrs.enum(AbiGenerationMode), default = None),
        "resources_root": attrs.option(attrs.string(), default = None),
        "_android_toolchain": android_toolchain(),
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
        "_dex_min_sdk_version": attrs.default_only(attrs.option(attrs.int(), default = dex_min_sdk_version())),
        "_dex_toolchain": _dex_toolchain(),
        "_is_building_android_binary": is_building_android_binary_attr(),
        "_java_toolchain": java_toolchain_for_android(),
        "_kotlin_toolchain": _kotlin_toolchain(),
    },
    "android_manifest": {
        "_android_toolchain": android_toolchain(),
    },
    "android_prebuilt_aar": {
        # Prebuilt jars are quick to build, and often contain third-party code, which in turn is
        # often a source of annotations and constants. To ease migration to ABI generation from
        # source without deps, we have them present during ABI gen by default.
        "required_for_source_only_abi": attrs.bool(default = True),
        "_android_toolchain": android_toolchain(),
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
        "_dex_min_sdk_version": attrs.default_only(attrs.option(attrs.int(), default = dex_min_sdk_version())),
        "_dex_toolchain": _dex_toolchain(),
        "_java_toolchain": java_toolchain_for_android(),
    },
    "android_resource": {
        "assets": attrs.option(attrs.one_of(attrs.source(allow_directory = True), attrs.dict(key = attrs.string(), value = attrs.source(), sorted = True)), default = None),
        "project_assets": attrs.option(attrs.source(allow_directory = True), default = None),
        "project_res": attrs.option(attrs.source(allow_directory = True), default = None),
        "res": attrs.option(attrs.one_of(attrs.source(allow_directory = True), attrs.dict(key = attrs.string(), value = attrs.source(), sorted = True)), default = None),
        "_android_toolchain": android_toolchain(),
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
    },
    "apk_genrule": genrule_attributes() | {
        "type": attrs.string(default = "apk"),
        "_android_toolchain": android_toolchain(),
    },
    "gen_aidl": {
        "import_paths": attrs.list(attrs.arg(), default = []),
        "_android_toolchain": android_toolchain(),
        "_java_toolchain": java_toolchain_for_android(),
    },
    "prebuilt_native_library": {
        "native_libs": attrs.source(allow_directory = True),
    },
    "robolectric_test": {
        "abi_generation_mode": attrs.option(attrs.enum(AbiGenerationMode), default = None),
        "resources_root": attrs.option(attrs.string(), default = None),
        "robolectric_runtime_dependencies": attrs.list(attrs.source(), default = []),
        "unbundled_resources_root": attrs.option(attrs.source(allow_directory = True), default = None),
        "_android_toolchain": android_toolchain(),
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
        "_is_building_android_binary": attrs.default_only(attrs.bool(default = False)),
        "_java_test_toolchain": attrs.default_only(attrs.exec_dep(
            default = select_java_test_toolchain(),
            providers = [
                JavaTestToolchainInfo,
            ],
        )),
        "_java_toolchain": java_toolchain_for_android_test(),
        "_kotlin_toolchain": _kotlin_toolchain(),
    },
}
