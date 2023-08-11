# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:build_only_native_code.bzl", "is_build_only_native_code")
load("@prelude//android:configuration.bzl", "is_building_android_binary_attr")
load("@prelude//java:dex_toolchain.bzl", "DexToolchainInfo")
load("@prelude//java:java.bzl", "AbiGenerationMode", "dex_min_sdk_version", "select_dex_toolchain", "select_java_test_toolchain", "select_java_toolchain")
load(
    "@prelude//java:java_toolchain.bzl",
    "JavaPlatformInfo",
    "JavaTestToolchainInfo",
    "JavaToolchainInfo",
)
load(
    "@prelude//kotlin:kotlin_toolchain.bzl",
    "KotlinToolchainInfo",
)
load(":kotlin_library.bzl", "kotlin_library_impl")
load(":kotlin_test.bzl", "kotlin_test_impl")

def _select_kotlin_toolchain():
    # FIXME: prelude// should be standalone (not refer to fbsource//)
    return "fbsource//xplat/buck2/platform/kotlin:kotlin"

implemented_rules = {
    "kotlin_library": kotlin_library_impl,
    "kotlin_test": kotlin_test_impl,
}

extra_attributes = {
    "kotlin_library": {
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
        "_kotlin_toolchain": attrs.exec_dep(
            default = _select_kotlin_toolchain(),
            providers = [
                KotlinToolchainInfo,
            ],
        ),
    },
    "kotlin_test": {
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
        "_kotlin_toolchain": attrs.exec_dep(
            default = _select_kotlin_toolchain(),
            providers = [
                KotlinToolchainInfo,
            ],
        ),
    },
}
