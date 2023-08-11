# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

def _os_constraint_value(platform: PlatformInfo.type, refs: struct.type) -> [None, ConstraintValueInfo.type]:
    return platform.configuration.constraints.get(refs.os[ConstraintSettingInfo].label)

def _sdk_constraint_value(platform: PlatformInfo.type, refs: struct.type) -> [None, ConstraintValueInfo.type]:
    return platform.configuration.constraints.get(refs.sdk[ConstraintSettingInfo].label)

def _cpu_constraint_readable_value(platform: PlatformInfo.type, refs: struct.type) -> [None, str]:
    cpu = platform.configuration.constraints.get(refs.cpu[ConstraintSettingInfo].label)
    if cpu == None:
        return platform.label

    if cpu.label == refs.arm64[ConstraintValueInfo].label:
        return "arm64"
    elif cpu.label == refs.x86_64[ConstraintValueInfo].label:
        return "x86_64"
    else:
        return platform.label

def _cpu_split_transition_impl(
        platform: PlatformInfo.type,
        refs: struct.type,
        attrs: struct.type) -> dict[str, PlatformInfo.type]:
    universal = attrs.universal if attrs.universal != None else attrs._universal_default
    os = _os_constraint_value(platform, refs)
    if not universal or os == None:
        # Don't do the splitting, since we don't know what OS type this is.
        return {_cpu_constraint_readable_value(platform, refs): platform}

    os_label = os.label
    sdk = _sdk_constraint_value(platform, refs)
    sdk_label = sdk.label if sdk != None else None

    cpu_name_to_cpu_constraint = {}
    if os_label == refs.ios[ConstraintValueInfo].label:
        if sdk == None or sdk_label == refs.ios_simulator_sdk[ConstraintValueInfo].label:
            # default to simulator if SDK is not specified
            cpu_name_to_cpu_constraint["arm64"] = refs.arm64[ConstraintValueInfo]
            cpu_name_to_cpu_constraint["x86_64"] = refs.x86_64[ConstraintValueInfo]
        elif sdk_label == refs.ios_device_sdk[ConstraintValueInfo].label:
            return {"arm64": platform}
        else:
            fail("Unsupported SDK {} for IPhoneOS".format(sdk_label))
    elif os_label == refs.watchos[ConstraintValueInfo].label:
        if sdk == None or sdk_label == refs.watchos_simulator_sdk[ConstraintValueInfo].label:
            cpu_name_to_cpu_constraint["arm64"] = refs.arm64[ConstraintValueInfo]
            cpu_name_to_cpu_constraint["x86_64"] = refs.x86_64[ConstraintValueInfo]
        elif sdk_label == refs.watchos_device_sdk[ConstraintValueInfo].label:
            cpu_name_to_cpu_constraint["arm64"] = refs.arm64[ConstraintValueInfo]
            cpu_name_to_cpu_constraint["arm32"] = refs.arm32[ConstraintValueInfo]
        else:
            fail("Unsupported SDK {} for WatchOS".format(sdk_label))
    elif os_label == refs.macos[ConstraintValueInfo].label:
        cpu_name_to_cpu_constraint["arm64"] = refs.arm64[ConstraintValueInfo]
        cpu_name_to_cpu_constraint["x86_64"] = refs.x86_64[ConstraintValueInfo]
    else:
        fail("Unsupported OS: {}".format(os_label))

    cpu_constraint_name = refs.cpu[ConstraintSettingInfo].label
    base_constraints = {
        constraint_setting_label: constraint_setting_value
        for (constraint_setting_label, constraint_setting_value) in platform.configuration.constraints.items()
        if constraint_setting_label != cpu_constraint_name
    }

    new_configs = {}
    for platform_name, cpu_constraint in cpu_name_to_cpu_constraint.items():
        updated_constraints = dict(base_constraints)
        updated_constraints[cpu_constraint_name] = cpu_constraint
        new_configs[platform_name] = PlatformInfo(
            label = platform_name,
            configuration = ConfigurationInfo(
                constraints = updated_constraints,
                values = platform.configuration.values,
            ),
        )

    return new_configs

cpu_split_transition = transition(
    impl = _cpu_split_transition_impl,
    refs = {
        "arm32": "config//cpu/constraints:arm32",
        "arm64": "config//cpu/constraints:arm64",
        "cpu": "config//cpu/constraints:cpu",
        "ios": "config//os/constraints:iphoneos",
        "ios_device_sdk": "config//os/sdk/apple/constraints:iphoneos",
        "ios_simulator_sdk": "config//os/sdk/apple/constraints:iphonesimulator",
        "macos": "config//os/constraints:macos",
        "os": "config//os/constraints:os",
        "sdk": "config//os/sdk/apple/constraints:_",
        "watchos": "config//os/constraints:watchos",
        "watchos_device_sdk": "config//os/sdk/apple/constraints:watchos",
        "watchos_simulator_sdk": "config//os/sdk/apple/constraints:watchsimulator",
        "x86_64": "config//cpu/constraints:x86_64",
    },
    attrs = [
        "universal",
        "_universal_default",
    ],
    split = True,
)
