# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple/swift:swift_toolchain.bzl", "swift_toolchain_impl")
load("@prelude//apple/swift:swift_toolchain_types.bzl", "SwiftObjectFormat")
load("@prelude//apple/user:cpu_split_transition.bzl", "cpu_split_transition")
load("@prelude//cxx:headers.bzl", "CPrecompiledHeaderInfo")
load("@prelude//cxx:omnibus.bzl", "omnibus_environment_attr")
load("@prelude//cxx/user:link_group_map.bzl", "link_group_map_attr")
load("@prelude//linking:execution_preference.bzl", "link_execution_preference_attr")
load("@prelude//linking:link_info.bzl", "LinkOrdering")
load("@prelude//decls/common.bzl", "Linkage")
load(":apple_asset_catalog.bzl", "apple_asset_catalog_impl")
load(":apple_binary.bzl", "apple_binary_impl")
load(":apple_bundle.bzl", "apple_bundle_impl")
load(":apple_bundle_types.bzl", "AppleBundleInfo")
load(":apple_core_data.bzl", "apple_core_data_impl")
load(":apple_library.bzl", "apple_library_impl")
load(":apple_package.bzl", "apple_package_impl")
load(":apple_package_config.bzl", "IpaCompressionLevel")
load(":apple_resource.bzl", "apple_resource_impl")
load(
    ":apple_rules_impl_utility.bzl",
    "APPLE_ARCHIVE_OBJECTS_LOCALLY_OVERRIDE_ATTR_NAME",
    "apple_bundle_extra_attrs",
    "apple_test_extra_attrs",
    "get_apple_toolchain_attr",
    "get_apple_xctoolchain_attr",
    "get_apple_xctoolchain_bundle_id_attr",
)
load(":apple_test.bzl", "apple_test_impl")
load(":apple_toolchain.bzl", "apple_toolchain_impl")
load(":apple_toolchain_types.bzl", "AppleToolsInfo")
load(":apple_universal_executable.bzl", "apple_universal_executable_impl")
load(":prebuilt_apple_framework.bzl", "prebuilt_apple_framework_impl")
load(":scene_kit_assets.bzl", "scene_kit_assets_impl")
load(":xcode_postbuild_script.bzl", "xcode_postbuild_script_impl")
load(":xcode_prebuild_script.bzl", "xcode_prebuild_script_impl")

implemented_rules = {
    "apple_asset_catalog": apple_asset_catalog_impl,
    "apple_binary": apple_binary_impl,
    "apple_bundle": apple_bundle_impl,
    "apple_library": apple_library_impl,
    "apple_package": apple_package_impl,
    "apple_resource": apple_resource_impl,
    "apple_test": apple_test_impl,
    "apple_toolchain": apple_toolchain_impl,
    "apple_universal_executable": apple_universal_executable_impl,
    "core_data_model": apple_core_data_impl,
    "prebuilt_apple_framework": prebuilt_apple_framework_impl,
    "scene_kit_assets": scene_kit_assets_impl,
    "swift_toolchain": swift_toolchain_impl,
    "xcode_postbuild_script": xcode_postbuild_script_impl,
    "xcode_prebuild_script": xcode_prebuild_script_impl,
}

_APPLE_TOOLCHAIN_ATTR = get_apple_toolchain_attr()

extra_attributes = {
    "apple_asset_catalog": {
        "dirs": attrs.list(attrs.source(allow_directory = True), default = []),
    },
    "apple_binary": {
        "binary_linker_flags": attrs.list(attrs.arg(), default = []),
        "enable_distributed_thinlto": attrs.bool(default = False),
        "extra_xcode_sources": attrs.list(attrs.source(allow_directory = True), default = []),
        "link_execution_preference": link_execution_preference_attr(),
        "link_group_map": link_group_map_attr(),
        "link_ordering": attrs.option(attrs.enum(LinkOrdering.values()), default = None),
        "precompiled_header": attrs.option(attrs.dep(providers = [CPrecompiledHeaderInfo]), default = None),
        "prefer_stripped_objects": attrs.bool(default = False),
        "preferred_linkage": attrs.enum(Linkage, default = "any"),
        "stripped": attrs.bool(default = False),
        "_apple_toolchain": _APPLE_TOOLCHAIN_ATTR,
        # FIXME: prelude// should be standalone (not refer to fbsource//)
        "_apple_tools": attrs.exec_dep(default = "fbsource//xplat/buck2/platform/apple:apple-tools", providers = [AppleToolsInfo]),
        "_apple_xctoolchain": get_apple_xctoolchain_attr(),
        "_apple_xctoolchain_bundle_id": get_apple_xctoolchain_bundle_id_attr(),
        "_omnibus_environment": omnibus_environment_attr(),
    },
    "apple_bundle": apple_bundle_extra_attrs(),
    "apple_library": {
        "extra_xcode_sources": attrs.list(attrs.source(allow_directory = True), default = []),
        "link_execution_preference": link_execution_preference_attr(),
        "link_group_map": link_group_map_attr(),
        "link_ordering": attrs.option(attrs.enum(LinkOrdering.values()), default = None),
        "precompiled_header": attrs.option(attrs.dep(providers = [CPrecompiledHeaderInfo]), default = None),
        "preferred_linkage": attrs.enum(Linkage, default = "any"),
        "serialize_debugging_options": attrs.bool(default = True),
        "stripped": attrs.bool(default = False),
        "supports_header_symlink_subtarget": attrs.bool(default = False),
        "supports_shlib_interfaces": attrs.bool(default = True),
        "use_archive": attrs.option(attrs.bool(), default = None),
        "_apple_toolchain": _APPLE_TOOLCHAIN_ATTR,
        # FIXME: prelude// should be standalone (not refer to fbsource//)
        "_apple_tools": attrs.exec_dep(default = "fbsource//xplat/buck2/platform/apple:apple-tools", providers = [AppleToolsInfo]),
        "_apple_xctoolchain": get_apple_xctoolchain_attr(),
        "_apple_xctoolchain_bundle_id": get_apple_xctoolchain_bundle_id_attr(),
        "_omnibus_environment": omnibus_environment_attr(),
        APPLE_ARCHIVE_OBJECTS_LOCALLY_OVERRIDE_ATTR_NAME: attrs.option(attrs.bool(), default = None),
    },
    "apple_package": {
        "bundle": attrs.dep(providers = [AppleBundleInfo]),
        "_apple_toolchain": _APPLE_TOOLCHAIN_ATTR,
        "_ipa_compression_level": attrs.enum(IpaCompressionLevel.values()),
    },
    "apple_resource": {
        "codesign_on_copy": attrs.bool(default = False),
        "content_dirs": attrs.list(attrs.source(allow_directory = True), default = []),
        "dirs": attrs.list(attrs.source(allow_directory = True), default = []),
        "files": attrs.list(attrs.one_of(attrs.dep(), attrs.source()), default = []),
    },
    "apple_test": apple_test_extra_attrs(),
    "apple_toolchain": {
        # The Buck v1 attribute specs defines those as `attrs.source()` but
        # we want to properly handle any runnable tools that might have
        # addition runtime requirements.
        "actool": attrs.exec_dep(providers = [RunInfo]),
        "codesign": attrs.exec_dep(providers = [RunInfo]),
        "codesign_allocate": attrs.exec_dep(providers = [RunInfo]),
        "codesign_identities_command": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        # Controls invocations of `ibtool`, `actool` and `momc`
        "compile_resources_locally": attrs.bool(default = False),
        "copy_scene_kit_assets": attrs.exec_dep(providers = [RunInfo]),
        "cxx_toolchain": attrs.toolchain_dep(),
        "dsymutil": attrs.exec_dep(providers = [RunInfo]),
        "dwarfdump": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "extra_linker_outputs": attrs.set(attrs.string(), default = []),
        "ibtool": attrs.exec_dep(providers = [RunInfo]),
        "installer": attrs.default_only(attrs.label(default = "buck//src/com/facebook/buck/installer/apple:apple_installer")),
        "libtool": attrs.exec_dep(providers = [RunInfo]),
        "lipo": attrs.exec_dep(providers = [RunInfo]),
        "min_version": attrs.option(attrs.string(), default = None),
        "momc": attrs.exec_dep(providers = [RunInfo]),
        "odrcov": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        # A placeholder tool that can be used to set up toolchain constraints.
        # Useful when fat and thin toolchahins share the same underlying tools via `command_alias()`,
        # which requires setting up separate platform-specific aliases with the correct constraints.
        "placeholder_tool": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "platform_path": attrs.option(attrs.source(), default = None),  # Mark as optional until we remove `_internal_platform_path`
        # Defines whether the Xcode project generator needs to check
        # that the selected Xcode version matches the one defined
        # by the `xcode_build_version` fields.
        "requires_xcode_version_match": attrs.bool(default = False),
        "sdk_path": attrs.option(attrs.source(), default = None),  # Mark as optional until we remove `_internal_sdk_path`
        "swift_toolchain": attrs.option(attrs.toolchain_dep(), default = None),
        "version": attrs.option(attrs.string(), default = None),
        "xcode_build_version": attrs.option(attrs.string(), default = None),
        "xcode_version": attrs.option(attrs.string(), default = None),
        "xctest": attrs.exec_dep(providers = [RunInfo]),
        # TODO(T111858757): Mirror of `platform_path` but treated as a string. It allows us to
        #                   pass abs paths during development and using the currently selected Xcode.
        "_internal_platform_path": attrs.option(attrs.string(), default = None),
        # TODO(T111858757): Mirror of `sdk_path` but treated as a string. It allows us to
        #                   pass abs paths during development and using the currently selected Xcode.
        "_internal_sdk_path": attrs.option(attrs.string(), default = None),
    },
    "apple_universal_executable": {
        "executable": attrs.split_transition_dep(cfg = cpu_split_transition),
        "labels": attrs.list(attrs.string()),
        "split_arch_dsym": attrs.bool(default = False),
        "universal": attrs.option(attrs.bool(), default = None),
        "_apple_toolchain": _APPLE_TOOLCHAIN_ATTR,
        "_apple_tools": attrs.exec_dep(default = "fbsource//xplat/buck2/platform/apple:apple-tools", providers = [AppleToolsInfo]),
        "_universal_default": attrs.bool(default = False),
    },
    "core_data_model": {
        "path": attrs.source(allow_directory = True),
    },
    "prebuilt_apple_framework": {
        "framework": attrs.option(attrs.source(allow_directory = True), default = None),
        "preferred_linkage": attrs.enum(Linkage, default = "any"),
        "_apple_toolchain": _APPLE_TOOLCHAIN_ATTR,
        "_omnibus_environment": omnibus_environment_attr(),
    },
    "scene_kit_assets": {
        "path": attrs.source(allow_directory = True),
    },
    "swift_library": {
        "preferred_linkage": attrs.enum(Linkage, default = "any"),
    },
    "swift_toolchain": {
        "architecture": attrs.option(attrs.string(), default = None),  # TODO(T115173356): Make field non-optional
        "object_format": attrs.enum(SwiftObjectFormat.values(), default = "object"),
        # A placeholder tool that can be used to set up toolchain constraints.
        # Useful when fat and thin toolchahins share the same underlying tools via `command_alias()`,
        # which requires setting up separate platform-specific aliases with the correct constraints.
        "placeholder_tool": attrs.option(attrs.exec_dep(providers = [RunInfo]), default = None),
        "platform_path": attrs.option(attrs.source(), default = None),  # Mark as optional until we remove `_internal_platform_path`
        "sdk_modules": attrs.list(attrs.exec_dep(), default = []),  # A list or a root target that represent a graph of sdk modules (e.g Frameworks)
        "sdk_path": attrs.option(attrs.source(), default = None),  # Mark as optional until we remove `_internal_sdk_path`
        "swift_stdlib_tool": attrs.exec_dep(providers = [RunInfo]),
        "swiftc": attrs.exec_dep(providers = [RunInfo]),
        # TODO(T111858757): Mirror of `platform_path` but treated as a string. It allows us to
        #                   pass abs paths during development and using the currently selected Xcode.
        "_internal_platform_path": attrs.option(attrs.string(), default = None),
        # TODO(T111858757): Mirror of `sdk_path` but treated as a string. It allows us to
        #                   pass abs paths during development and using the currently selected Xcode.
        "_internal_sdk_path": attrs.option(attrs.string(), default = None),
        "_swiftc_wrapper": attrs.exec_dep(providers = [RunInfo], default = "prelude//apple/tools:swift_exec"),
    },
}
