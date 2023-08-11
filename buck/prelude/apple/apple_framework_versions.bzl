# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":apple_sdk.bzl", "get_apple_sdk_name")
load(":apple_target_sdk_version.bzl", "get_min_deployment_version_for_node")
load(":apple_utility.bzl", "has_apple_toolchain")

_FRAMEWORK_INTRODUCED_VERSIONS = {
    "AGL": {"macosx": (10, 0, 0)},
    "ARKit": {"iphoneos": (11, 0, 0), "maccatalyst": (14, 0, 0)},
    "AVFAudio": {
        "appletvos": (14, 5, 0),
        "iphoneos": (14, 5, 0),
        "maccatalyst": (14, 5, 0),
        "macosx": (11, 3, 0),
        "watchos": (9, 0, 0),
    },
    "AVFoundation": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 2, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 7, 0),
        "watchos": (3, 0, 0),
    },
    "AVKit": {
        "appletvos": (9, 0, 0),
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 9, 0),
        "watchos": (9, 0, 0),
    },
    "AVRouting": {
        "iphoneos": (16, 0, 0),
        "maccatalyst": (16, 0, 0),
        "macosx": (13, 0, 0),
    },
    "Accelerate": {
        "appletvos": (9, 0, 0),
        "iphoneos": (4, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 3, 0),
        "watchos": (2, 0, 0),
    },
    "Accessibility": {
        "appletvos": (14, 0, 0),
        "iphoneos": (14, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
        "watchos": (7, 0, 0),
    },
    "Accounts": {
        "iphoneos": (5, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 8, 0),
    },
    "ActivityKit": {"iphoneos": (16, 1, 0), "maccatalyst": (16, 1, 0)},
    "AdServices": {
        "iphoneos": (14, 3, 0),
        "maccatalyst": (14, 3, 0),
        "macosx": (11, 1, 0),
    },
    "AdSupport": {
        "appletvos": (9, 0, 0),
        "iphoneos": (6, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 14, 0),
    },
    "AddressBook": {
        "iphoneos": (2, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (10, 2, 0),
    },
    "AddressBookUI": {"iphoneos": (2, 0, 0), "maccatalyst": (14, 0, 0)},
    "AppClip": {"iphoneos": (14, 0, 0), "maccatalyst": (14, 0, 0)},
    "AppIntents": {
        "appletvos": (16, 0, 0),
        "iphoneos": (16, 0, 0),
        "maccatalyst": (16, 0, 0),
        "macosx": (13, 0, 0),
        "watchos": (9, 0, 0),
    },
    "AppKit": {"maccatalyst": (13, 0, 0), "macosx": (10, 0, 0)},
    "AppTrackingTransparency": {
        "appletvos": (14, 0, 0),
        "iphoneos": (14, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
    },
    "AppleScriptKit": {"macosx": (10, 0, 0)},
    "AppleScriptObjC": {"macosx": (10, 6, 0)},
    "ApplicationServices": {"maccatalyst": (13, 0, 0), "macosx": (10, 0, 0)},
    "AssetsLibrary": {"iphoneos": (4, 0, 0), "maccatalyst": (14, 0, 0)},
    "AudioToolbox": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 0, 0),
    },
    "AudioUnit": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 0, 0),
    },
    "AudioVideoBridging": {"macosx": (10, 8, 0)},
    "AuthenticationServices": {
        "appletvos": (13, 0, 0),
        "iphoneos": (12, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
        "watchos": (6, 0, 0),
    },
    "AutomatedDeviceEnrollment": {"iphoneos": (16, 1, 0)},
    "AutomaticAssessmentConfiguration": {
        "iphoneos": (13, 4, 0),
        "maccatalyst": (13, 4, 0),
        "macosx": (10, 15, 4),
    },
    "Automator": {"maccatalyst": (14, 0, 0), "macosx": (10, 4, 0)},
    "BackgroundAssets": {
        "iphoneos": (16, 0, 0),
        "maccatalyst": (16, 0, 0),
        "macosx": (13, 0, 0),
    },
    "BackgroundTasks": {
        "appletvos": (13, 0, 0),
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
    },
    "BusinessChat": {"iphoneos": (11, 3, 0), "macosx": (10, 13, 4)},
    "CFNetwork": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 8, 0),
    },
    "CalendarStore": {"macosx": (10, 5, 0)},
    "CallKit": {
        "iphoneos": (10, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (13, 0, 0),
        "watchos": (9, 0, 0),
    },
    "CarKey": {"iphoneos": (16, 0, 0), "maccatalyst": (16, 0, 0)},
    "CarPlay": {"iphoneos": (12, 0, 0), "maccatalyst": (14, 0, 0)},
    "Carbon": {"macosx": (10, 0, 0)},
    "Charts": {
        "appletvos": (16, 0, 0),
        "iphoneos": (16, 0, 0),
        "maccatalyst": (16, 0, 0),
        "macosx": (13, 0, 0),
        "watchos": (9, 0, 0),
    },
    "ClassKit": {
        "iphoneos": (11, 4, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
    },
    "ClockKit": {
        "iphoneos": (14, 0, 0),
        "maccatalyst": (14, 0, 0),
        "watchos": (2, 0, 0),
    },
    "CloudKit": {
        "appletvos": (9, 0, 0),
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 10, 0),
        "watchos": (3, 0, 0),
    },
    "Cocoa": {"macosx": (10, 0, 0)},
    "Collaboration": {"macosx": (10, 5, 0)},
    "ColorSync": {
        "appletvos": (16, 0, 0),
        "iphoneos": (16, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 13, 0),
        "watchos": (9, 0, 0),
    },
    "Combine": {
        "appletvos": (13, 0, 0),
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
        "watchos": (6, 0, 0),
    },
    "Contacts": {
        "iphoneos": (9, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 11, 0),
        "watchos": (2, 0, 0),
    },
    "ContactsUI": {
        "iphoneos": (9, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 11, 0),
    },
    "CoreAudio": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 0, 0),
        "watchos": (3, 0, 0),
    },
    "CoreAudioKit": {
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 4, 0),
    },
    "CoreAudioTypes": {
        "appletvos": (13, 0, 0),
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
        "watchos": (6, 0, 0),
    },
    "CoreBluetooth": {
        "appletvos": (9, 0, 0),
        "iphoneos": (5, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 10, 0),
        "watchos": (4, 0, 0),
    },
    "CoreData": {
        "appletvos": (9, 0, 0),
        "iphoneos": (3, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 4, 0),
        "watchos": (2, 0, 0),
    },
    "CoreFoundation": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 0, 0),
        "watchos": (2, 0, 0),
    },
    "CoreGraphics": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 8, 0),
        "watchos": (2, 0, 0),
    },
    "CoreHaptics": {
        "appletvos": (14, 0, 0),
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
    },
    "CoreImage": {
        "appletvos": (9, 0, 0),
        "iphoneos": (5, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 11, 0),
    },
    "CoreLocation": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 6, 0),
        "watchos": (2, 0, 0),
    },
    "CoreLocationUI": {
        "iphoneos": (15, 0, 0),
        "maccatalyst": (15, 0, 0),
        "watchos": (8, 0, 0),
    },
    "CoreMIDI": {
        "appletvos": (15, 0, 0),
        "iphoneos": (4, 2, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 0, 0),
        "watchos": (8, 0, 0),
    },
    "CoreML": {
        "appletvos": (11, 0, 0),
        "iphoneos": (11, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 13, 0),
        "watchos": (4, 0, 0),
    },
    "CoreMedia": {
        "appletvos": (9, 0, 0),
        "iphoneos": (4, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 7, 0),
        "watchos": (6, 0, 0),
    },
    "CoreMediaIO": {"maccatalyst": (13, 0, 0), "macosx": (10, 7, 0)},
    "CoreMotion": {
        "iphoneos": (4, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
        "watchos": (2, 0, 0),
    },
    "CoreNFC": {"iphoneos": (11, 0, 0), "maccatalyst": (13, 0, 0)},
    "CoreServices": {
        "appletvos": (12, 0, 0),
        "iphoneos": (12, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 0, 0),
        "watchos": (5, 0, 0),
    },
    "CoreSpotlight": {
        "iphoneos": (9, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 13, 0),
    },
    "CoreTelephony": {
        "iphoneos": (4, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (10, 10, 0),
    },
    "CoreText": {
        "appletvos": (9, 0, 0),
        "iphoneos": (3, 2, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 8, 0),
        "watchos": (2, 0, 0),
    },
    "CoreTransferable": {
        "appletvos": (16, 0, 0),
        "iphoneos": (16, 0, 0),
        "maccatalyst": (16, 0, 0),
        "macosx": (13, 0, 0),
        "watchos": (9, 0, 0),
    },
    "CoreVideo": {
        "appletvos": (9, 0, 0),
        "iphoneos": (4, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 4, 0),
        "watchos": (4, 0, 0),
    },
    "CoreWLAN": {"maccatalyst": (13, 0, 0), "macosx": (10, 6, 0)},
    "CreateML": {
        "appletvos": (16, 0, 0),
        "iphoneos": (15, 0, 0),
        "maccatalyst": (15, 0, 0),
        "macosx": (10, 14, 0),
    },
    "CreateMLComponents": {
        "appletvos": (16, 0, 0),
        "iphoneos": (16, 0, 0),
        "macosx": (13, 0, 0),
    },
    "CryptoKit": {
        "appletvos": (15, 0, 0),
        "iphoneos": (13, 0, 0),
        "maccatalyst": (15, 0, 0),
        "macosx": (10, 15, 0),
        "watchos": (8, 0, 0),
    },
    "CryptoTokenKit": {
        "appletvos": (13, 0, 0),
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 10, 0),
        "watchos": (8, 0, 0),
    },
    "DVDPlayback": {"macosx": (10, 3, 0)},
    "DataDetection": {
        "appletvos": (15, 0, 0),
        "iphoneos": (15, 0, 0),
        "maccatalyst": (15, 0, 0),
        "macosx": (12, 0, 0),
        "watchos": (8, 0, 0),
    },
    "DeveloperToolsSupport": {
        "appletvos": (14, 0, 0),
        "iphoneos": (14, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
        "watchos": (7, 0, 0),
    },
    "DeviceActivity": {"iphoneos": (15, 0, 0), "maccatalyst": (15, 0, 0)},
    "DeviceCheck": {
        "appletvos": (11, 0, 0),
        "iphoneos": (11, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
        "watchos": (9, 0, 0),
    },
    "DeviceDiscoveryExtension": {"iphoneos": (16, 0, 0)},
    "DeviceDiscoveryUI": {"appletvos": (16, 0, 0)},
    "DirectoryService": {"macosx": (10, 0, 0)},
    "DiscRecording": {"macosx": (10, 2, 0)},
    "DiscRecordingUI": {"macosx": (10, 2, 0)},
    "DiskArbitration": {"maccatalyst": (13, 0, 0), "macosx": (10, 4, 0)},
    "DriverKit": {"iphoneos": (16, 0, 0), "macosx": (10, 15, 0)},
    "EventKit": {
        "iphoneos": (4, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 8, 0),
        "watchos": (2, 0, 0),
    },
    "EventKitUI": {"iphoneos": (4, 0, 0), "maccatalyst": (13, 0, 0)},
    "ExceptionHandling": {"maccatalyst": (13, 0, 0), "macosx": (10, 0, 0)},
    "ExecutionPolicy": {"maccatalyst": (13, 0, 0), "macosx": (10, 15, 0)},
    "ExposureNotification": {
        "iphoneos": (13, 5, 0),
        "maccatalyst": (15, 0, 0),
        "macosx": (12, 0, 0),
    },
    "ExtensionFoundation": {
        "appletvos": (16, 0, 0),
        "iphoneos": (16, 0, 0),
        "maccatalyst": (16, 0, 0),
        "macosx": (13, 0, 0),
        "watchos": (9, 0, 0),
    },
    "ExtensionKit": {
        "appletvos": (16, 0, 0),
        "iphoneos": (16, 1, 0),
        "maccatalyst": (16, 0, 0),
        "macosx": (13, 0, 0),
        "watchos": (9, 0, 0),
    },
    "ExternalAccessory": {
        "appletvos": (10, 0, 0),
        "iphoneos": (3, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 13, 0),
    },
    "FamilyControls": {"iphoneos": (15, 0, 0), "maccatalyst": (15, 0, 0)},
    "FileProvider": {"iphoneos": (11, 0, 0), "macosx": (10, 15, 0)},
    "FileProviderUI": {
        "iphoneos": (11, 0, 0),
        "maccatalyst": (15, 0, 0),
        "macosx": (10, 15, 0),
    },
    "FinderSync": {"macosx": (10, 10, 0)},
    "ForceFeedback": {"maccatalyst": (13, 0, 0), "macosx": (10, 2, 0)},
    "Foundation": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 0, 0),
        "watchos": (2, 0, 0),
    },
    "GLKit": {
        "appletvos": (9, 0, 0),
        "iphoneos": (5, 0, 0),
        "macosx": (10, 8, 0),
    },
    "GLUT": {"macosx": (10, 0, 0)},
    "GSS": {
        "iphoneos": (5, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 14, 0),
    },
    "GameController": {
        "appletvos": (9, 0, 0),
        "iphoneos": (7, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 9, 0),
    },
    "GameKit": {
        "appletvos": (9, 0, 0),
        "iphoneos": (3, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 8, 0),
        "watchos": (3, 0, 0),
    },
    "GameplayKit": {
        "appletvos": (9, 0, 0),
        "iphoneos": (9, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 11, 0),
    },
    "GroupActivities": {
        "appletvos": (15, 0, 0),
        "iphoneos": (15, 0, 0),
        "maccatalyst": (15, 0, 0),
        "macosx": (12, 0, 0),
    },
    "HealthKit": {
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (13, 0, 0),
        "watchos": (2, 0, 0),
    },
    "HealthKitUI": {
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (13, 0, 0),
        "watchos": (2, 0, 0),
    },
    "HomeKit": {
        "appletvos": (10, 0, 0),
        "iphoneos": (8, 0, 0),
        "maccatalyst": (14, 0, 0),
        "watchos": (2, 0, 0),
    },
    "Hypervisor": {"macosx": (10, 10, 0)},
    "ICADevices": {"macosx": (10, 3, 0)},
    "IMServicePlugIn": {"macosx": (10, 7, 0)},
    "IOBluetooth": {"maccatalyst": (13, 0, 0), "macosx": (10, 2, 0)},
    "IOBluetoothUI": {"maccatalyst": (14, 0, 0), "macosx": (10, 2, 0)},
    "IOKit": {
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 0, 0),
    },
    "IOSurface": {
        "appletvos": (11, 0, 0),
        "iphoneos": (11, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 6, 0),
    },
    "IOUSBHost": {"maccatalyst": (14, 0, 0), "macosx": (10, 15, 0)},
    "IdentityLookup": {
        "iphoneos": (11, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
    },
    "IdentityLookupUI": {
        "iphoneos": (11, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
    },
    "ImageCaptureCore": {
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 6, 0),
    },
    "ImageIO": {
        "appletvos": (9, 0, 0),
        "iphoneos": (4, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 8, 0),
        "watchos": (2, 0, 0),
    },
    "InputMethodKit": {"macosx": (10, 5, 0)},
    "InstallerPlugins": {"macosx": (10, 4, 0)},
    "InstantMessage": {"macosx": (10, 4, 0)},
    "Intents": {
        "appletvos": (14, 0, 0),
        "iphoneos": (10, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (12, 0, 0),
        "watchos": (3, 2, 0),
    },
    "IntentsUI": {
        "appletvos": (14, 0, 0),
        "iphoneos": (10, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (12, 0, 0),
        "watchos": (3, 2, 0),
    },
    "JavaNativeFoundation": {"macosx": (11, 0, 0)},
    "JavaRuntimeSupport": {"macosx": (11, 0, 0)},
    "JavaScriptCore": {
        "appletvos": (9, 0, 0),
        "iphoneos": (7, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 5, 0),
    },
    "Kerberos": {"macosx": (10, 0, 0)},
    "Kernel": {"macosx": (10, 0, 0)},
    "KernelManagement": {"maccatalyst": (14, 2, 0), "macosx": (11, 0, 0)},
    "LDAP": {"macosx": (10, 0, 0)},
    "LatentSemanticMapping": {"maccatalyst": (13, 0, 0), "macosx": (10, 5, 0)},
    "LinkPresentation": {
        "appletvos": (14, 0, 0),
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
    },
    "LocalAuthentication": {
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 10, 0),
        "watchos": (9, 0, 0),
    },
    "LocalAuthenticationEmbeddedUI": {
        "iphoneos": (16, 0, 0),
        "maccatalyst": (16, 0, 0),
        "macosx": (12, 0, 0),
    },
    "MLCompute": {
        "appletvos": (14, 0, 0),
        "iphoneos": (14, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
    },
    "MailKit": {"macosx": (12, 0, 0)},
    "ManagedSettings": {"iphoneos": (15, 0, 0)},
    "ManagedSettingsUI": {"iphoneos": (15, 0, 0)},
    "MapKit": {
        "appletvos": (9, 2, 0),
        "iphoneos": (3, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 9, 0),
        "watchos": (2, 0, 0),
    },
    "Matter": {
        "appletvos": (16, 0, 0),
        "iphoneos": (16, 0, 0),
        "maccatalyst": (16, 1, 0),
        "macosx": (13, 0, 0),
        "watchos": (9, 0, 0),
    },
    "MatterSupport": {"iphoneos": (16, 1, 0)},
    "MediaAccessibility": {
        "appletvos": (9, 0, 0),
        "iphoneos": (7, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 9, 0),
    },
    "MediaLibrary": {"maccatalyst": (13, 0, 0), "macosx": (10, 9, 0)},
    "MediaPlayer": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (13, 0, 0),
        "watchos": (5, 0, 0),
    },
    "MediaSetup": {"iphoneos": (14, 0, 0), "maccatalyst": (15, 4, 0)},
    "MessageUI": {"iphoneos": (3, 0, 0), "maccatalyst": (13, 0, 0)},
    "Messages": {"iphoneos": (10, 0, 0), "maccatalyst": (14, 0, 0)},
    "Metal": {
        "appletvos": (9, 0, 0),
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 11, 0),
    },
    "MetalFX": {
        "iphoneos": (16, 0, 0),
        "maccatalyst": (16, 0, 0),
        "macosx": (13, 0, 0),
    },
    "MetalKit": {
        "appletvos": (9, 0, 0),
        "iphoneos": (9, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 11, 0),
    },
    "MetalPerformanceShaders": {
        "appletvos": (9, 0, 0),
        "iphoneos": (9, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 13, 0),
    },
    "MetalPerformanceShadersGraph": {
        "appletvos": (14, 0, 0),
        "iphoneos": (14, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
    },
    "MetricKit": {
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (12, 0, 0),
    },
    "MobileCoreServices": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "watchos": (1, 0, 0),
    },
    "ModelIO": {
        "appletvos": (9, 0, 0),
        "iphoneos": (9, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 11, 0),
    },
    "MultipeerConnectivity": {
        "appletvos": (10, 0, 0),
        "iphoneos": (7, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 10, 0),
    },
    "MusicKit": {
        "appletvos": (15, 0, 0),
        "iphoneos": (15, 0, 0),
        "maccatalyst": (15, 0, 0),
        "macosx": (12, 0, 0),
        "watchos": (8, 0, 0),
    },
    "NaturalLanguage": {
        "appletvos": (12, 0, 0),
        "iphoneos": (12, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 14, 0),
        "watchos": (5, 0, 0),
    },
    "NearbyInteraction": {
        "iphoneos": (14, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
        "watchos": (8, 0, 0),
    },
    "NetFS": {"macosx": (10, 6, 0)},
    "Network": {
        "appletvos": (12, 0, 0),
        "iphoneos": (12, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 14, 0),
        "watchos": (6, 0, 0),
    },
    "NetworkExtension": {
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 10, 0),
        "watchos": (7, 0, 0),
    },
    "NewsstandKit": {"iphoneos": (5, 0, 0)},
    "NotificationCenter": {"iphoneos": (8, 0, 0), "macosx": (10, 10, 0)},
    "OSAKit": {"macosx": (10, 4, 0)},
    "OSLog": {
        "appletvos": (15, 0, 0),
        "iphoneos": (15, 0, 0),
        "maccatalyst": (15, 0, 0),
        "macosx": (10, 15, 0),
        "watchos": (8, 0, 0),
    },
    "OpenCL": {"macosx": (10, 6, 0)},
    "OpenDirectory": {"maccatalyst": (13, 0, 0), "macosx": (10, 6, 0)},
    "OpenGL": {"macosx": (10, 0, 0)},
    "OpenGLES": {"appletvos": (9, 0, 0), "iphoneos": (2, 0, 0)},
    "PCSC": {"macosx": (10, 0, 0)},
    "PDFKit": {"iphoneos": (11, 0, 0), "macosx": (10, 4, 0)},
    "PHASE": {
        "appletvos": (15, 0, 0),
        "iphoneos": (15, 0, 0),
        "maccatalyst": (15, 0, 0),
        "macosx": (12, 0, 0),
    },
    "ParavirtualizedGraphics": {
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
    },
    "PassKit": {
        "iphoneos": (6, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (11, 0, 0),
        "watchos": (2, 0, 0),
    },
    "PencilKit": {
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
    },
    "Photos": {
        "appletvos": (10, 0, 0),
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 11, 0),
        "watchos": (9, 0, 0),
    },
    "PhotosUI": {
        "appletvos": (10, 0, 0),
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 11, 0),
        "watchos": (9, 0, 0),
    },
    "PreferencePanes": {"maccatalyst": (14, 0, 0), "macosx": (10, 1, 0)},
    "ProximityReader": {"iphoneos": (15, 4, 0), "maccatalyst": (15, 4, 0)},
    "PushKit": {
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
        "watchos": (6, 0, 0),
    },
    "PushToTalk": {"iphoneos": (16, 0, 0)},
    "Quartz": {"macosx": (10, 4, 0)},
    "QuartzCore": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 3, 0),
    },
    "QuickLook": {
        "iphoneos": (4, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 5, 0),
    },
    "QuickLookThumbnailing": {
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
    },
    "QuickLookUI": {"macosx": (12, 0, 0)},
    "RealityKit": {
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
    },
    "ReplayKit": {
        "appletvos": (10, 0, 0),
        "iphoneos": (9, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (11, 0, 0),
    },
    "RoomPlan": {"iphoneos": (16, 0, 0), "maccatalyst": (16, 0, 0)},
    "Ruby": {"macosx": (10, 5, 0)},
    "SafariServices": {
        "iphoneos": (7, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 12, 0),
    },
    "SafetyKit": {
        "iphoneos": (16, 0, 0),
        "maccatalyst": (16, 1, 0),
        "macosx": (13, 0, 0),
        "watchos": (9, 1, 0),
    },
    "SceneKit": {
        "appletvos": (9, 0, 0),
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 8, 0),
        "watchos": (3, 0, 0),
    },
    "ScreenCaptureKit": {"maccatalyst": (15, 4, 0), "macosx": (12, 3, 0)},
    "ScreenSaver": {"macosx": (10, 0, 0)},
    "ScreenTime": {
        "iphoneos": (14, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
    },
    "ScriptingBridge": {"maccatalyst": (13, 0, 0), "macosx": (10, 5, 0)},
    "Security": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 0, 0),
        "watchos": (2, 0, 0),
    },
    "SecurityFoundation": {"maccatalyst": (13, 0, 0), "macosx": (10, 3, 0)},
    "SecurityInterface": {"macosx": (10, 3, 0)},
    "SensorKit": {
        "iphoneos": (14, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (12, 3, 0),
    },
    "ServiceManagement": {
        "appletvos": (12, 1, 0),
        "iphoneos": (12, 1, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 6, 0),
        "watchos": (5, 1, 0),
    },
    "SharedWithYou": {
        "appletvos": (16, 0, 0),
        "iphoneos": (16, 0, 0),
        "maccatalyst": (16, 0, 0),
        "macosx": (13, 0, 0),
    },
    "ShazamKit": {
        "appletvos": (15, 0, 0),
        "iphoneos": (15, 0, 0),
        "maccatalyst": (15, 0, 0),
        "macosx": (12, 0, 0),
        "watchos": (8, 0, 0),
    },
    "Social": {
        "iphoneos": (6, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 8, 0),
    },
    "SoundAnalysis": {
        "appletvos": (13, 0, 0),
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
        "watchos": (9, 0, 0),
    },
    "Speech": {
        "iphoneos": (10, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
    },
    "SpriteKit": {
        "appletvos": (9, 0, 0),
        "iphoneos": (7, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 9, 0),
        "watchos": (3, 0, 0),
    },
    "StoreKit": {
        "appletvos": (9, 0, 0),
        "iphoneos": (3, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 7, 0),
        "watchos": (6, 2, 0),
    },
    "SwiftUI": {
        "appletvos": (13, 0, 0),
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 15, 0),
        "watchos": (6, 0, 0),
    },
    "SyncServices": {"macosx": (10, 4, 0)},
    "System": {
        "appletvos": (14, 0, 0),
        "iphoneos": (14, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
        "watchos": (7, 0, 0),
    },
    "SystemConfiguration": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 1, 0),
    },
    "SystemExtensions": {"macosx": (10, 15, 0)},
    "TVMLKit": {"appletvos": (9, 0, 0)},
    "TVServices": {"appletvos": (9, 0, 0)},
    "TVUIKit": {"appletvos": (12, 0, 0)},
    "TWAIN": {"macosx": (10, 2, 0)},
    "TabularData": {
        "appletvos": (15, 0, 0),
        "iphoneos": (15, 0, 0),
        "maccatalyst": (15, 0, 0),
        "macosx": (12, 0, 0),
        "watchos": (8, 0, 0),
    },
    "Tcl": {"macosx": (10, 3, 0)},
    "ThreadNetwork": {
        "iphoneos": (15, 0, 0),
        "maccatalyst": (16, 1, 0),
        "macosx": (13, 0, 0),
    },
    "Tk": {"macosx": (10, 4, 0)},
    "UIKit": {
        "appletvos": (9, 0, 0),
        "iphoneos": (2, 0, 0),
        "maccatalyst": (13, 0, 0),
        "watchos": (2, 0, 0),
    },
    "UniformTypeIdentifiers": {
        "appletvos": (14, 0, 0),
        "iphoneos": (14, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
        "watchos": (7, 0, 0),
    },
    "UserNotifications": {
        "appletvos": (10, 0, 0),
        "iphoneos": (10, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 14, 0),
        "watchos": (3, 0, 0),
    },
    "UserNotificationsUI": {
        "iphoneos": (10, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
    },
    "VideoDecodeAcceleration": {"macosx": (10, 7, 0)},
    "VideoSubscriberAccount": {
        "appletvos": (10, 0, 0),
        "iphoneos": (10, 0, 0),
        "macosx": (10, 14, 0),
    },
    "VideoToolbox": {
        "appletvos": (10, 2, 0),
        "iphoneos": (6, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 8, 0),
    },
    "Virtualization": {"macosx": (11, 0, 0)},
    "Vision": {
        "appletvos": (11, 0, 0),
        "iphoneos": (11, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 13, 0),
    },
    "VisionKit": {
        "iphoneos": (13, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (13, 0, 0),
    },
    "WatchConnectivity": {
        "iphoneos": (9, 0, 0),
        "maccatalyst": (13, 0, 0),
        "watchos": (2, 0, 0),
    },
    "WatchKit": {"watchos": (2, 0, 0)},
    "WeatherKit": {
        "appletvos": (16, 0, 0),
        "iphoneos": (16, 0, 0),
        "maccatalyst": (16, 0, 0),
        "macosx": (13, 0, 0),
        "watchos": (9, 0, 0),
    },
    "WebKit": {
        "iphoneos": (8, 0, 0),
        "maccatalyst": (13, 0, 0),
        "macosx": (10, 2, 0),
    },
    "WidgetKit": {
        "iphoneos": (14, 0, 0),
        "maccatalyst": (14, 0, 0),
        "macosx": (11, 0, 0),
    },
    "iAd": {"iphoneos": (4, 0, 0), "maccatalyst": (13, 0, 0)},
    "iTunesLibrary": {"maccatalyst": (14, 0, 0), "macosx": (10, 13, 0)},
    "vecLib": {"macosx": (10, 0, 0)},
    "vmnet": {"maccatalyst": (13, 0, 0), "macosx": (10, 10, 0)},
}

def _parse_version(version: str) -> (int, int, int):
    result = [0, 0, 0]
    components = [int(x) for x in version.split(".")]
    for i in range(0, len(components)):
        result[i] = components[i]
    return (result[0], result[1], result[2])

def get_framework_linker_args(ctx: AnalysisContext, framework_names: list[str]) -> list[str]:
    if not has_apple_toolchain(ctx):
        return _get_unchecked_framework_linker_args(framework_names)

    # Convert deployment target string into tuple of ints for easier comparison
    deployment_target_str = get_min_deployment_version_for_node(ctx)
    if not deployment_target_str:
        return _get_unchecked_framework_linker_args(framework_names)

    deployment_target = _parse_version(deployment_target_str)

    # Simulator and device platforms have the same framework versions
    sdk_name = get_apple_sdk_name(ctx)
    if sdk_name.endswith("simulator"):
        sdk_name = sdk_name[:-len("simulator")] + "os"

    args = []
    for name in framework_names:
        versions = _FRAMEWORK_INTRODUCED_VERSIONS.get(name, None)
        if versions:
            introduced = versions.get(sdk_name, None)
            if not introduced:
                fail("SDK framework {} is not compatible with platform {}".format(name, sdk_name))

            if _version_is_greater_than(introduced, deployment_target):
                args.append("-weak_framework")
            else:
                args.append("-framework")
        else:
            # Assume this is a non-SDK framework
            args.append("-framework")

        args.append(name)

    return args

def _get_unchecked_framework_linker_args(framework_names: list[str]) -> list[str]:
    args = []
    for f in framework_names:
        args.append("-framework")
        args.append(f)

    return args

def _version_is_greater_than(x: (int, int, int), y: (int, int, int)) -> bool:
    return x[0] > y[0] or (x[0] == y[0] and x[1] > y[1]) or (x[0] == y[0] and x[1] == y[1] and x[2] > y[2])
