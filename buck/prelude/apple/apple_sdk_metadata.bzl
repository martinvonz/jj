# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

AppleSdkMetadata = record(
    name = field(str),
    target_device_flags = field([str], []),
    is_ad_hoc_code_sign_sufficient = field(bool),
    info_plist_supported_platforms_values = field([str]),
    min_version_plist_info_key = field(str),
    actool_platform_override = field([str, None], None),
)

IPhoneOSSdkMetadata = AppleSdkMetadata(
    name = "iphoneos",
    target_device_flags = ["--target-device", "iphone", "--target-device", "ipad"],
    is_ad_hoc_code_sign_sufficient = False,
    info_plist_supported_platforms_values = ["iPhoneOS"],
    min_version_plist_info_key = "MinimumOSVersion",
)

IPhoneSimulatorSdkMetadata = AppleSdkMetadata(
    name = "iphonesimulator",
    target_device_flags = ["--target-device", "iphone", "--target-device", "ipad"],
    is_ad_hoc_code_sign_sufficient = True,
    info_plist_supported_platforms_values = ["iPhoneSimulator"],
    min_version_plist_info_key = "MinimumOSVersion",
)

TVOSSdkMetadata = AppleSdkMetadata(
    name = "appletvos",
    target_device_flags = ["--target-device", "tv"],
    is_ad_hoc_code_sign_sufficient = False,
    info_plist_supported_platforms_values = ["AppleTVOS"],
    min_version_plist_info_key = "MinimumOSVersion",
)

TVSimulatorSdkMetadata = AppleSdkMetadata(
    name = "appletvsimulator",
    target_device_flags = ["--target-device", "tv"],
    is_ad_hoc_code_sign_sufficient = True,
    info_plist_supported_platforms_values = ["AppleTVSimulator"],
    min_version_plist_info_key = "MinimumOSVersion",
)

WatchOSSdkMetadata = AppleSdkMetadata(
    name = "watchos",
    target_device_flags = ["--target-device", "watch"],
    is_ad_hoc_code_sign_sufficient = False,
    info_plist_supported_platforms_values = ["WatchOS"],
    min_version_plist_info_key = "MinimumOSVersion",
)

WatchSimulatorSdkMetadata = AppleSdkMetadata(
    name = "watchsimulator",
    target_device_flags = ["--target-device", "watch"],
    is_ad_hoc_code_sign_sufficient = True,
    info_plist_supported_platforms_values = ["WatchSimulator"],
    min_version_plist_info_key = "MinimumOSVersion",
)

MacOSXSdkMetadata = AppleSdkMetadata(
    name = "macosx",
    target_device_flags = ["--target-device", "mac"],
    is_ad_hoc_code_sign_sufficient = True,
    info_plist_supported_platforms_values = ["MacOSX"],
    min_version_plist_info_key = "LSMinimumSystemVersion",
)

MacOSXCatalystSdkMetadata = AppleSdkMetadata(
    name = "maccatalyst",
    # TODO(T112097815): Support for macOS idiom
    target_device_flags = ["--target-device", "ipad"],
    is_ad_hoc_code_sign_sufficient = True,
    info_plist_supported_platforms_values = ["MacOSX"],
    min_version_plist_info_key = "LSMinimumSystemVersion",
    actool_platform_override = "macosx",
)

_SDK_MAP = {
    IPhoneOSSdkMetadata.name: IPhoneOSSdkMetadata,
    IPhoneSimulatorSdkMetadata.name: IPhoneSimulatorSdkMetadata,
    TVOSSdkMetadata.name: TVOSSdkMetadata,
    TVSimulatorSdkMetadata.name: TVSimulatorSdkMetadata,
    WatchOSSdkMetadata.name: WatchOSSdkMetadata,
    WatchSimulatorSdkMetadata.name: WatchSimulatorSdkMetadata,
    MacOSXSdkMetadata.name: MacOSXSdkMetadata,
    MacOSXCatalystSdkMetadata.name: MacOSXCatalystSdkMetadata,
}

def get_apple_sdk_metadata_for_sdk_name(name: str) -> AppleSdkMetadata.type:
    sdk = _SDK_MAP.get(name)
    if sdk == None:
        fail("unrecognized sdk name: `{}`".format(name))
    return sdk
