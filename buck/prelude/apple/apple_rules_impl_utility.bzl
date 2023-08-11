# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_bundle_types.bzl", "AppleBundleResourceInfo")
load("@prelude//apple:apple_code_signing_types.bzl", "CodeSignType")
load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo", "AppleToolsInfo")
load("@prelude//apple/user:apple_selective_debugging.bzl", "AppleSelectiveDebuggingInfo")
load("@prelude//apple/user:cpu_split_transition.bzl", "cpu_split_transition")
load("@prelude//apple/user:resource_group_map.bzl", "resource_group_map_attr")
load("@prelude//cxx:headers.bzl", "CPrecompiledHeaderInfo")
load("@prelude//cxx:omnibus.bzl", "omnibus_environment_attr")
load("@prelude//linking:execution_preference.bzl", "link_execution_preference_attr")
load("@prelude//linking:link_info.bzl", "LinkOrdering")
load("@prelude//decls/common.bzl", "LinkableDepType", "Linkage")

def get_apple_toolchain_attr():
    # FIXME: prelude// should be standalone (not refer to fbcode//)
    return attrs.toolchain_dep(default = "fbcode//buck2/platform/toolchain:apple-default", providers = [AppleToolchainInfo])

def _get_apple_bundle_toolchain_attr():
    # FIXME: prelude// should be standalone (not refer to fbcode//)
    return attrs.toolchain_dep(default = "fbcode//buck2/platform/toolchain:apple-bundle", providers = [AppleToolchainInfo])

def get_apple_xctoolchain_attr():
    # FIXME: prelude// should be standalone (not refer to fbcode//)
    return attrs.toolchain_dep(default = "fbcode//buck2/platform/toolchain:apple-xctoolchain")

def get_apple_xctoolchain_bundle_id_attr():
    # FIXME: prelude// should be standalone (not refer to fbcode//)
    return attrs.toolchain_dep(default = "fbcode//buck2/platform/toolchain:apple-xctoolchain-bundle-id")

APPLE_ARCHIVE_OBJECTS_LOCALLY_OVERRIDE_ATTR_NAME = "_archive_objects_locally_override"
APPLE_USE_ENTITLEMENTS_WHEN_ADHOC_CODE_SIGNING_CONFIG_OVERRIDE_ATTR_NAME = "_use_entitlements_when_adhoc_code_signing"
APPLE_USE_ENTITLEMENTS_WHEN_ADHOC_CODE_SIGNING_ATTR_NAME = "use_entitlements_when_adhoc_code_signing"

def _apple_bundle_like_common_attrs():
    # `apple_bundle()` and `apple_test()` share a common set of extra attrs
    return {
        # FIXME: prelude// should be standalone (not refer to fbsource//)
        "_apple_tools": attrs.exec_dep(default = "fbsource//xplat/buck2/platform/apple:apple-tools", providers = [AppleToolsInfo]),
        "_apple_xctoolchain": get_apple_xctoolchain_attr(),
        "_apple_xctoolchain_bundle_id": get_apple_xctoolchain_bundle_id_attr(),
        "_bundling_cache_buster": attrs.option(attrs.string(), default = None),
        "_bundling_log_file_enabled": attrs.bool(default = False),
        "_codesign_type": attrs.option(attrs.enum(CodeSignType.values()), default = None),
        "_compile_resources_locally_override": attrs.option(attrs.bool(), default = None),
        "_dry_run_code_signing": attrs.bool(default = False),
        "_fast_adhoc_signing_enabled": attrs.bool(default = False),
        "_incremental_bundling_enabled": attrs.bool(default = False),
        "_profile_bundling_enabled": attrs.bool(default = False),
        "_resource_bundle": attrs.option(attrs.dep(providers = [AppleBundleResourceInfo]), default = None),
        APPLE_USE_ENTITLEMENTS_WHEN_ADHOC_CODE_SIGNING_CONFIG_OVERRIDE_ATTR_NAME: attrs.option(attrs.bool(), default = None),
        APPLE_USE_ENTITLEMENTS_WHEN_ADHOC_CODE_SIGNING_ATTR_NAME: attrs.bool(default = False),
    }

def apple_test_extra_attrs():
    # To build an `apple_test`, one needs to first build a shared `apple_library` then
    # wrap this test library into an `apple_bundle`. Because of this, `apple_test` has attributes
    # from both `apple_library` and `apple_bundle`.
    attribs = {
        # Expected by `apple_bundle`, for `apple_test` this field is always None.
        "binary": attrs.option(attrs.dep(), default = None),
        # The resulting test bundle should have .xctest extension.
        "extension": attrs.string(),
        "extra_xcode_sources": attrs.list(attrs.source(allow_directory = True), default = []),
        "link_execution_preference": link_execution_preference_attr(),
        "link_ordering": attrs.option(attrs.enum(LinkOrdering.values()), default = None),
        # Used to create the shared test library. Any library deps whose `preferred_linkage` isn't "shared" will
        # be treated as "static" deps and linked into the shared test library.
        "link_style": attrs.enum(LinkableDepType, default = "static"),
        "precompiled_header": attrs.option(attrs.dep(providers = [CPrecompiledHeaderInfo]), default = None),
        # The test source code and lib dependencies should be built into a shared library.
        "preferred_linkage": attrs.enum(Linkage, default = "shared"),
        # Expected by `apple_bundle`, for `apple_test` this field is always None.
        "resource_group": attrs.option(attrs.string(), default = None),
        # Expected by `apple_bundle`, for `apple_test` this field is always None.
        "resource_group_map": attrs.option(attrs.string(), default = None),
        "stripped": attrs.bool(default = False),
        "_apple_toolchain": get_apple_toolchain_attr(),
        "_ios_booted_simulator": attrs.default_only(attrs.dep(default = "fbsource//xplat/buck2/platform/apple:ios_booted_simulator", providers = [LocalResourceInfo])),
        "_macos_idb_companion": attrs.default_only(attrs.dep(default = "fbsource//xplat/buck2/platform/apple:macos_idb_companion", providers = [LocalResourceInfo])),
        "_omnibus_environment": omnibus_environment_attr(),
    }
    attribs.update(_apple_bundle_like_common_attrs())
    return attribs

def apple_bundle_extra_attrs():
    attribs = {
        "binary": attrs.option(attrs.split_transition_dep(cfg = cpu_split_transition), default = None),
        "resource_group_map": resource_group_map_attr(),
        "selective_debugging": attrs.option(attrs.dep(providers = [AppleSelectiveDebuggingInfo]), default = None),
        "split_arch_dsym": attrs.bool(default = False),
        "universal": attrs.option(attrs.bool(), default = None),
        "_apple_toolchain": _get_apple_bundle_toolchain_attr(),
        "_codesign_entitlements": attrs.option(attrs.source(), default = None),
        # FIXME: prelude// should be standalone (not refer to fbsource//)
        "_provisioning_profiles": attrs.dep(default = "fbsource//xplat/buck2/platform/apple:provisioning_profiles"),
        "_universal_default": attrs.bool(default = False),
    }
    attribs.update(_apple_bundle_like_common_attrs())
    return attribs
