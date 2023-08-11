# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

AbiGenerationMode = enum("class", "none", "source", "source_only")

DepFiles = enum("none", "per_class", "per_jar")

JavacProtocol = enum("classic", "javacd")

JavaPlatformInfo = provider(
    doc = "Java platform info",
    fields = [
        "name",
    ],
)

JavaToolchainInfo = provider(
    doc = "Java toolchain info",
    fields = [
        "abi_generation_mode",
        "bootclasspath_7",
        "bootclasspath_8",
        "class_abi_generator",
        "class_loader_bootstrapper",
        "compile_and_package",
        "dep_files",
        "fat_jar",
        "fat_jar_main_class_lib",
        "jar",
        "jar_builder",
        "java",
        "javacd_debug_port",
        "javacd_debug_target",
        "javacd_jvm_args",
        "javacd_jvm_args_target",
        "javacd_main_class",
        "javacd_worker",
        "java_for_tests",
        "javac",
        "javac_protocol",
        "nullsafe",
        "nullsafe_extra_args",
        "nullsafe_signatures",
        "source_level",
        "src_root_elements",
        "src_root_prefixes",
        "target_level",
        "zip_scrubber",
        "is_bootstrap_toolchain",
        "gen_class_to_source_map",
    ],
)

JavaTestToolchainInfo = provider(
    doc = "Java test toolchain info",
    fields = [
        "java_custom_class_loader_class",
        "java_custom_class_loader_library_jar",
        "java_custom_class_loader_vm_args",
        "test_runner_library_jar",
        "junit_test_runner_main_class_args",
        "junit5_test_runner_main_class_args",
        "testng_test_runner_main_class_args",
        "list_class_names",
        "use_java_custom_class_loader",
        "merge_class_to_source_maps",
    ],
)

# prebuilt_jar needs so little of the Java toolchain that it's worth
# giving it its own to reduce the occurrence of cycles as we add
# more Java- and Kotlin-built tools to the Java and Kotlin toolchains
PrebuiltJarToolchainInfo = provider(
    doc = "prebuilt_jar toolchain info",
    fields = [
        "class_abi_generator",
        "is_bootstrap_toolchain",
    ],
)
