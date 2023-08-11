# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolchainInfo")
load("@prelude//apple/swift:swift_toolchain_types.bzl", "SwiftToolchainInfo")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxPlatformInfo", "CxxToolchainInfo")

def apple_toolchain_impl(ctx: AnalysisContext) -> list["provider"]:
    sdk_path = ctx.attrs._internal_sdk_path or ctx.attrs.sdk_path
    platform_path = ctx.attrs._internal_platform_path or ctx.attrs.platform_path
    return [
        DefaultInfo(),
        AppleToolchainInfo(
            actool = ctx.attrs.actool[RunInfo],
            codesign = ctx.attrs.codesign[RunInfo],
            codesign_allocate = ctx.attrs.codesign_allocate[RunInfo],
            codesign_identities_command = ctx.attrs.codesign_identities_command[RunInfo] if ctx.attrs.codesign_identities_command else None,
            compile_resources_locally = ctx.attrs.compile_resources_locally,
            copy_scene_kit_assets = ctx.attrs.copy_scene_kit_assets[RunInfo],
            cxx_platform_info = ctx.attrs.cxx_toolchain[CxxPlatformInfo],
            cxx_toolchain_info = ctx.attrs.cxx_toolchain[CxxToolchainInfo],
            dsymutil = ctx.attrs.dsymutil[RunInfo],
            dwarfdump = ctx.attrs.dwarfdump[RunInfo] if ctx.attrs.dwarfdump else None,
            extra_linker_outputs = ctx.attrs.extra_linker_outputs,
            ibtool = ctx.attrs.ibtool[RunInfo],
            installer = ctx.attrs.installer,
            libtool = ctx.attrs.libtool[RunInfo],
            lipo = ctx.attrs.lipo[RunInfo],
            min_version = ctx.attrs.min_version,
            momc = ctx.attrs.momc[RunInfo],
            odrcov = ctx.attrs.odrcov[RunInfo] if ctx.attrs.odrcov else None,
            platform_path = platform_path,
            sdk_build_version = ctx.attrs.build_version,
            sdk_name = ctx.attrs.sdk_name,
            sdk_path = sdk_path,
            sdk_version = ctx.attrs.version,
            swift_toolchain_info = ctx.attrs.swift_toolchain[SwiftToolchainInfo] if ctx.attrs.swift_toolchain else None,
            watch_kit_stub_binary = ctx.attrs.watch_kit_stub_binary,
            xcode_build_version = ctx.attrs.xcode_build_version,
            xcode_version = ctx.attrs.xcode_version,
            xctest = ctx.attrs.xctest[RunInfo],
        ),
    ]
