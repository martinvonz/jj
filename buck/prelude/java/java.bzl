# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//android:configuration.bzl", "is_building_android_binary_attr")
load("@prelude//android:min_sdk_version.bzl", "get_min_sdk_version_constraint_value_name", "get_min_sdk_version_range")
load("@prelude//java:dex_toolchain.bzl", "DexToolchainInfo")
load(
    "@prelude//java:java_toolchain.bzl",
    "JavaPlatformInfo",
    "JavaTestToolchainInfo",
    "JavaToolchainInfo",
    "PrebuiltJarToolchainInfo",
)
load("@prelude//java/plugins:java_annotation_processor.bzl", "java_annotation_processor_impl")
load("@prelude//java/plugins:java_plugin.bzl", "java_plugin_impl")
load("@prelude//genrule.bzl", "genrule_attributes")
load(":jar_genrule.bzl", "jar_genrule_impl")
load(":java_binary.bzl", "java_binary_impl")
load(":java_library.bzl", "java_library_impl")
load(":java_test.bzl", "java_test_impl")
load(":keystore.bzl", "keystore_impl")
load(":prebuilt_jar.bzl", "prebuilt_jar_impl")

AbiGenerationMode = ["class", "source", "source_only", "none"]

def select_java_toolchain():
    # FIXME: prelude// should be standalone (not refer to fbcode//, fbsource//)
    return select(
        {
            # By default use the fbsource toolchain
            "DEFAULT": "fbsource//xplat/buck2/platform/java:java",
            # if target is meant to run on host but with an android environment then use .buckconfig from fbsource cell
            "config//runtime/constraints:android-host-test": "fbsource//xplat/buck2/platform/java:java-for-host-tests",
            # if target is with fbcode constraint then use .buckconfig from fbcode cell
            "config//runtime:fbcode": "fbcode//buck2/platform:java_fbcode",
            # if target is for android (fbsource repo) then use .buckconfig from fbsource cell
            "config//toolchain/fb:android-ndk": "fbsource//xplat/buck2/platform/java:java",
        },
    )

def select_dex_toolchain():
    # FIXME: prelude// should be standalone (not refer to fbsource//)
    return select(
        {
            # Only need a Dex toolchain for Android builds.
            "DEFAULT": None,
            "config//os/constraints:android": "fbsource//xplat/buck2/platform/java:dex",
        },
    )

def dex_min_sdk_version():
    min_sdk_version_dict = {"DEFAULT": None}
    for min_sdk in get_min_sdk_version_range():
        constraint = "fbsource//xplat/buck2/platform/android:{}".format(get_min_sdk_version_constraint_value_name(min_sdk))
        min_sdk_version_dict[constraint] = min_sdk

    return select(min_sdk_version_dict)

def select_java_test_toolchain():
    # FIXME: prelude// should be standalone (not refer to fbsource//)
    return "fbsource//xplat/buck2/platform/java:java_test"

def select_prebuilt_jar_toolchain():
    # FIXME: prelude// should be standalone (not refer to fbcode//)
    return "fbcode//buck2/platform:prebuilt_jar"

implemented_rules = {
    "jar_genrule": jar_genrule_impl,
    "java_annotation_processor": java_annotation_processor_impl,
    "java_binary": java_binary_impl,
    "java_library": java_library_impl,
    "java_plugin": java_plugin_impl,
    "java_test": java_test_impl,
    "keystore": keystore_impl,
    "prebuilt_jar": prebuilt_jar_impl,
}

extra_attributes = {
    "jar_genrule": genrule_attributes() | {
        # FIXME: prelude// should be standalone (not refer to fbsource//)
        "_java_toolchain": attrs.exec_dep(
            default = select_java_toolchain(),
            providers = [
                JavaToolchainInfo,
            ],
        ),
    },
    "java_annotation_processor": {
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
    },
    "java_binary": {
        "java_args_for_run_info": attrs.list(attrs.string(), default = []),
        "meta_inf_directory": attrs.option(attrs.source(allow_directory = True), default = None),
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
        "_java_toolchain": attrs.exec_dep(
            default = select_java_toolchain(),
            providers = [
                JavaPlatformInfo,
                JavaToolchainInfo,
            ],
        ),
    },
    "java_library": {
        "abi_generation_mode": attrs.option(attrs.enum(AbiGenerationMode), default = None),
        "javac": attrs.option(attrs.one_of(attrs.dep(), attrs.source()), default = None),
        "resources_root": attrs.option(attrs.string(), default = None),
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
        "_dex_min_sdk_version": attrs.option(attrs.int(), default = dex_min_sdk_version()),
        "_dex_toolchain": attrs.option(attrs.exec_dep(
            providers = [
                DexToolchainInfo,
            ],
        ), default = select_dex_toolchain()),
        "_is_building_android_binary": is_building_android_binary_attr(),
        "_java_toolchain": attrs.exec_dep(
            default = select_java_toolchain(),
            providers = [
                JavaPlatformInfo,
                JavaToolchainInfo,
            ],
        ),
    },
    "java_plugin": {
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
    },
    "java_test": {
        "abi_generation_mode": attrs.option(attrs.enum(AbiGenerationMode), default = None),
        "javac": attrs.option(attrs.one_of(attrs.dep(), attrs.source()), default = None),
        "resources_root": attrs.option(attrs.string(), default = None),
        "unbundled_resources_root": attrs.option(attrs.source(allow_directory = True), default = None),
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
        "_is_building_android_binary": attrs.default_only(attrs.bool(default = False)),
        "_java_test_toolchain": attrs.exec_dep(
            default = select_java_test_toolchain(),
            providers = [
                JavaTestToolchainInfo,
            ],
        ),
        "_java_toolchain": attrs.exec_dep(
            default = select_java_toolchain(),
            providers = [
                JavaPlatformInfo,
                JavaToolchainInfo,
            ],
        ),
    },
    "java_test_runner": {
        "abi_generation_mode": attrs.option(attrs.enum(AbiGenerationMode), default = None),
        "resources_root": attrs.option(attrs.string(), default = None),
    },
    "prebuilt_jar": {
        "generate_abi": attrs.bool(default = True),
        # Prebuilt jars are quick to build, and often contain third-party code, which in turn is
        # often a source of annotations and constants. To ease migration to ABI generation from
        # source without deps, we have them present during ABI gen by default.
        "required_for_source_only_abi": attrs.bool(default = True),
        "_build_only_native_code": attrs.default_only(attrs.bool(default = is_build_only_native_code())),
        "_dex_min_sdk_version": attrs.option(attrs.int(), default = dex_min_sdk_version()),
        "_dex_toolchain": attrs.option(attrs.exec_dep(
            providers = [
                DexToolchainInfo,
            ],
        ), default = select_dex_toolchain()),
        "_prebuilt_jar_toolchain": attrs.exec_dep(
            default = select_prebuilt_jar_toolchain(),
            providers = [
                PrebuiltJarToolchainInfo,
            ],
        ),
    },
}
