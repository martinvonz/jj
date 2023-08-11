#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import argparse
import json
import os
import re
import sys
from typing import Dict, Iterable, TextIO

# Out-of-date? Update with this command:
#
# xcode-select --print-path | xargs printf '%s/Platforms/iPhoneSimulator.platform/Developer/SDKs/iPhoneSimulator.sdk/System/Library/Frameworks/' | xargs ls | rg '^([A-Z].+)\.framework$' -r '${1}' | xargs printf '    "%s",\n' && xcode-select --print-path | xargs printf '%s/Platforms/iPhoneSimulator.platform/Developer/SDKs/iPhoneSimulator.sdk/usr/include/module.modulemap' | xargs cat | rg '^module ([a-zA-Z0-9_]*) .*$' -r '${1}'| xargs printf '    "%s",\n'
APPLE_SYSTEM_MODULES = {
    "ARKit",
    "AVFAudio",
    "AVFoundation",
    "AVKit",
    "Accelerate",
    "Accessibility",
    "Accounts",
    "AdServices",
    "AdSupport",
    "AddressBook",
    "AddressBookUI",
    "AppClip",
    "AppTrackingTransparency",
    "AssetsLibrary",
    "AudioToolbox",
    "AudioUnit",
    "AuthenticationServices",
    "AutomaticAssessmentConfiguration",
    "BackgroundTasks",
    "BusinessChat",
    "CFNetwork",
    "CallKit",
    "CarPlay",
    "ClassKit",
    "ClockKit",
    "CloudKit",
    "Combine",
    "Contacts",
    "ContactsUI",
    "CoreAudio",
    "CoreAudioKit",
    "CoreAudioTypes",
    "CoreBluetooth",
    "CoreData",
    "CoreFoundation",
    "CoreGraphics",
    "CoreHaptics",
    "CoreImage",
    "CoreLocation",
    "CoreLocationUI",
    "CoreMIDI",
    "CoreML",
    "CoreMedia",
    "CoreMotion",
    "CoreNFC",
    "CoreServices",
    "CoreSpotlight",
    "CoreTelephony",
    "CoreText",
    "CoreVideo",
    "CryptoKit",
    "CryptoTokenKit",
    "DataDetection",
    "DeveloperToolsSupport",
    "DeviceActivity",
    "DeviceCheck",
    "EventKit",
    "EventKitUI",
    "ExposureNotification",
    "ExternalAccessory",
    "FamilyControls",
    "FileProvider",
    "FileProviderUI",
    "Foundation",
    "GLKit",
    "GSS",
    "GameController",
    "GameKit",
    "GameplayKit",
    "GroupActivities",
    "HealthKit",
    "HealthKitUI",
    "HomeKit",
    "IOKit",
    "IOSurface",
    "IdentityLookup",
    "IdentityLookupUI",
    "ImageCaptureCore",
    "ImageIO",
    "Intents",
    "IntentsUI",
    "JavaScriptCore",
    "LinkPresentation",
    "LocalAuthentication",
    "ManagedSettings",
    "ManagedSettingsUI",
    "MapKit",
    "MediaAccessibility",
    "MediaPlayer",
    "MediaToolbox",
    "MessageUI",
    "Messages",
    "Metal",
    "MetalKit",
    "MetalPerformanceShaders",
    "MetalPerformanceShadersGraph",
    "MetricKit",
    "MobileCoreServices",
    "ModelIO",
    "MultipeerConnectivity",
    "MusicKit",
    "NaturalLanguage",
    "NearbyInteraction",
    "Network",
    "NetworkExtension",
    "NewsstandKit",
    "NotificationCenter",
    "OSLog",
    "OpenAL",
    "OpenGLES",
    "PDFKit",
    "PHASE",
    "PassKit",
    "PencilKit",
    "Photos",
    "PhotosUI",
    "PushKit",
    "QuartzCore",
    "QuickLook",
    "QuickLookThumbnailing",
    "RealityFoundation",
    "RealityKit",
    "ReplayKit",
    "SafariServices",
    "SceneKit",
    "ScreenTime",
    "Security",
    "SensorKit",
    "ShazamKit",
    "Social",
    "SoundAnalysis",
    "Speech",
    "SpriteKit",
    "StoreKit",
    "SwiftUI",
    "SystemConfiguration",
    "TabularData",
    "Twitter",
    "UIKit",
    "UniformTypeIdentifiers",
    "UserNotifications",
    "UserNotificationsUI",
    "VideoSubscriberAccount",
    "VideoToolbox",
    "Vision",
    "VisionKit",
    "WatchConnectivity",
    "WebKit",
    "WidgetKit",
    "AppleTextureEncoder",
    "Compression",
    "Darwin",
    "asl",
    "dnssd",
    "os",
    "os_object",
    "os_workgroup",
    "libkern",
    "notify",
    "zlib",
    "SQLite3",
}

APPLE_TEST_FRAMEWORKS = {
    "XCTest",
}


# These modules require specific handling, as they do not have an umbrella
# header that matches the module name, as typical Apple frameworks do.
APPLE_SYSTEM_MODULE_OVERRIDES = {
    "Dispatch": ("dispatch", ("dispatch.h",)),
    "ObjectiveC": ("objc", ("runtime.h",)),
}


def write_imports_for_headers(out: TextIO, prefix: str, headers: Iterable[str]) -> None:
    for header in headers:
        print(f"#import <{prefix}/{header}>", file=out)


def write_imports_for_modules(
    out: TextIO,
    postprocessing_module_name: str,
    modules: Iterable[str],
    deps: Dict[str, Iterable[str]],
) -> None:
    # We only include the traditional textual imports when modules are disabled, so
    # that the behavior with modules enabled is identical to the behavior without
    # the postprocessing.
    print("#else", file=out)
    for module in modules:
        if headers := deps.get(module):
            write_imports_for_headers(out, module, headers)
        elif override := APPLE_SYSTEM_MODULE_OVERRIDES.get(module):
            write_imports_for_headers(out, override[0], override[1])
        elif module in APPLE_SYSTEM_MODULES or module in APPLE_TEST_FRAMEWORKS:
            # When we don't have an explicit override for the module, we use the module's
            # name as an umbrella header. This is used for typical Apple frameworks like
            # Foundation and UIKit.
            write_imports_for_headers(out, module, (f"{module}.h",))
        else:
            print(
                f"""
The module "{module}" was imported as a dependency of Swift code in "{postprocessing_module_name}", but could not be mapped to a list of header imports by Buck's Swift header postprocessing. There are two possibilities:

1. If "{module}" is an internal library, it is likely that the exported_deps of "{postprocessing_module_name}" are incorrect. Try fixing them manually or with "arc fixmydeps". This is the most likely issue.

2. If "{module}" is a system (Apple) framework, the list of Apple system modules in {os.path.basename(__file__)} is out-of-date. There is a command to fix it in that file. This issue is unlikely.
""",
                file=sys.stderr,
            )
            sys.exit(1)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("header")
    parser.add_argument("deps")
    parser.add_argument("out")
    args = parser.parse_args()

    with open(args.deps) as f:
        deps = json.load(f)

    # Strips the suffix from the header name, leaving us with just the name
    # of the module that we are postprocessing the header for. This is used
    # for error reporting.
    postprocessing_module_name = os.path.basename(args.header).split("-")[0]

    # The Swift compiler's output looks like this for Swift5.8:
    #
    # #if __has_feature(objc_modules)
    # #if __has_warning("-Watimport-in-framework-header")
    # #pragma clang diagnostic ignored "-Watimport-in-framework-header"
    # #endif
    # @import ModuleA;
    # @import ModuleB;
    # @import ModuleC;
    # #endif
    #
    # The implementation here balances being somewhat flexible to changes to the compiler's
    # output, unlikely though they may be, with avoiding adding too much complexity and getting
    # too close to implementing a full parser for Objective-C un-preprocessed header files.

    with open(args.header) as header, open(args.out, "w") as out:
        # When this is None, it means that we are still searching for the start of the conditional
        # @import block in the generated header.
        modules = None
        # The Swift compiler emits an additional #if gate inside the conditional @import block, so
        # we need to track whether we're in a further nested conditional so that we know when the
        # main conditional block has ended.
        if_level = 0

        for line in header:
            line = line.rstrip("\n")
            # When the modules has not been set, we are still searching for the start of the
            # modules @import section.
            if modules is None:
                # The line changed from __has_feature(modules) to __has_feature(objc_modules) between Swift5.7 and Swift5.8.
                # For the time being, we need to check for either to support both Xcode14.2 and Xcode14.3 onwards.
                if (
                    line == "#if __has_feature(objc_modules)"
                    or line == "#if __has_feature(modules)"
                ):
                    modules = []
                    if_level = 1
            else:
                if line.startswith("@import"):
                    # Splitting on:
                    #   "@import ": to separate from the @import.
                    #   Semicolon and period: to separate the main module name from submodules or EOL.
                    # The module name will then be the first item.
                    modules.append(re.split(r"@import |[;.]", line)[1])
                elif line.startswith("#if"):
                    # This allows us to handle the Clang diagnostic #if block that the compiler inserts
                    # within the main #if block for modules.
                    if_level += 1
                elif line.startswith("#endif"):
                    if_level -= 1
                    if if_level == 0:
                        write_imports_for_modules(
                            out,
                            postprocessing_module_name,
                            modules,
                            deps,
                        )
                        modules = None
            print(line, file=out)


if __name__ == "__main__":
    main()
