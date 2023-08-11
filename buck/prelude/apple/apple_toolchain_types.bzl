# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

AppleToolchainInfo = provider(fields = [
    "actool",  # "RunInfo"
    "codesign_allocate",  # "RunInfo"
    "codesign_identities_command",  # ["RunInfo", None]
    "codesign",  # "RunInfo"
    "compile_resources_locally",  # bool
    "copy_scene_kit_assets",  # "RunInfo"
    "cxx_platform_info",  # "CxxPlatformInfo"
    "cxx_toolchain_info",  # "CxxToolchainInfo"
    "dsymutil",  # "RunInfo"
    "dwarfdump",  # ["RunInfo", None]
    "extra_linker_outputs",  # [str]
    "ibtool",  # "RunInfo"
    "installer",  # label
    "libtool",  # "RunInfo"
    "lipo",  # "RunInfo"
    "min_version",  # [None, str]
    "momc",  # "RunInfo"
    "odrcov",  # ["RunInfo", None]
    "platform_path",  # [str, artifact]
    "sdk_build_version",  # "[None, str]"
    # SDK name to be passed to tools (e.g. actool), equivalent to ApplePlatform::getExternalName() in v1.
    "sdk_name",  # str
    "sdk_path",  # [str, artifact]
    # TODO(T124581557) Make it non-optional once there is no "selected xcode" toolchain
    "sdk_version",  # [None, str]
    "swift_toolchain_info",  # "SwiftToolchainInfo"
    "watch_kit_stub_binary",  # "artifact"
    "xcode_build_version",  # "[None, str]"
    "xcode_version",  # "[None, str]"
    "xctest",  # "RunInfo"
])

AppleToolsInfo = provider(fields = [
    "assemble_bundle",  # RunInfo
    "split_arch_combine_dsym_bundles_tool",  # RunInfo
    "dry_codesign_tool",  # "RunInfo"
    "adhoc_codesign_tool",  # "RunInfo"
    "selective_debugging_scrubber",  # "RunInfo"
    "info_plist_processor",  # RunInfo
    "make_modulemap",  # "RunInfo"
    "make_vfsoverlay",  # "RunInfo"
    "swift_objc_header_postprocess",  # "RunInfo"
])
