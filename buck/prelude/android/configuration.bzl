# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:cpu_filters.bzl", "ALL_CPU_FILTERS", "CPU_FILTER_FOR_DEFAULT_PLATFORM", "CPU_FILTER_FOR_PRIMARY_PLATFORM")
load("@prelude//android:min_sdk_version.bzl", "get_min_sdk_version_constraint_value_name", "get_min_sdk_version_range")
load("@prelude//utils:utils.bzl", "expect")

# Android binaries (APKs or AABs) can be built for one or more different platforms. buck2 supports
# building Android binaries for arm32, arm64, x86, and x86_64. The platform(s) that we are building
# for are specified by the `cpu_filters` attribute on the binary rule.

# In order to build our native libraries for the correct platform(s), we do a (split) transition
# (https://www.internalfb.com/intern/staticdocs/buck2/docs/rule_authors/configuration_transitions)
# on the `deps` of the binary, and have each of the resulting configured sub-graphs be responsible
# for building the native libraries for one of the specified platforms.

# We always create an "arm64" configured sub-graph and use it to build the non-native libraries (so
# that we get cache hits for the non-native libraries even if we're building for different
# platforms). We only use the "arm64" native libraries if it is one of the specified platforms. We
# "throw away" the non-native libraries for all other configured sub-graphs.

_REFS = {
    "arm64": "config//cpu/constraints:arm64",
    "armv7": "config//cpu/constraints:arm32",
    "build_only_native_code": "prelude//android/constraints:build_only_native_code",
    "building_android_binary": "prelude//os:building_android_binary",
    "cpu": "config//cpu/constraints:cpu",
    "default_platform": "config//platform/android:x86_32-fbsource",
    "maybe_build_only_native_code": "prelude//android/constraints:maybe_build_only_native_code",
    "maybe_building_android_binary": "prelude//os:maybe_building_android_binary",
    "min_sdk_version": "fbsource//xplat/buck2/platform/android:min_sdk_version",
    "x86": "config//cpu/constraints:x86_32",
    "x86_64": "config//cpu/constraints:x86_64",
}
for min_sdk in get_min_sdk_version_range():
    constraint_value_name = get_min_sdk_version_constraint_value_name(min_sdk)
    _REFS[constraint_value_name] = "fbsource//xplat/buck2/platform/android:{}".format(constraint_value_name)

def _cpu_split_transition_impl(
        platform: PlatformInfo.type,
        refs: struct.type,
        attrs: struct.type) -> dict[str, PlatformInfo.type]:
    cpu_filters = attrs.cpu_filters or ALL_CPU_FILTERS
    if attrs._is_force_single_cpu:
        cpu_filters = [CPU_FILTER_FOR_PRIMARY_PLATFORM]
    elif attrs._is_force_single_default_cpu:
        cpu_filters = ["default"]

    return _cpu_split_transition(
        platform,
        refs,
        cpu_filters,
        attrs.min_sdk_version,
    )

def _cpu_split_transition(
        platform: PlatformInfo.type,
        refs: struct.type,
        cpu_filters: list[str],
        min_sdk_version: [int, None]) -> dict[str, PlatformInfo.type]:
    cpu = refs.cpu
    x86 = refs.x86[ConstraintValueInfo]
    x86_64 = refs.x86_64[ConstraintValueInfo]
    armv7 = refs.armv7[ConstraintValueInfo]
    arm64 = refs.arm64[ConstraintValueInfo]

    if len(cpu_filters) == 1 and cpu_filters[0] == "default":
        default = refs.default_platform[PlatformInfo]
        return {CPU_FILTER_FOR_DEFAULT_PLATFORM: default}

    expect(CPU_FILTER_FOR_PRIMARY_PLATFORM == "arm64")
    cpu_name_to_cpu_constraint = {"arm64": arm64}
    for cpu_filter in cpu_filters:
        if cpu_filter == "x86":
            cpu_name_to_cpu_constraint["x86"] = x86
        elif cpu_filter == "armv7":
            cpu_name_to_cpu_constraint["armv7"] = armv7
        elif cpu_filter == "x86_64":
            cpu_name_to_cpu_constraint["x86_64"] = x86_64
        elif cpu_filter == "arm64":
            # Always included as the primary platform
            pass
        else:
            fail("Unexpected cpu_filter: {}".format(cpu_filter))

    base_constraints = {
        constraint_setting_label: constraint_setting_value
        for (constraint_setting_label, constraint_setting_value) in platform.configuration.constraints.items()
        if constraint_setting_label != cpu[ConstraintSettingInfo].label and constraint_setting_label != refs.maybe_build_only_native_code[ConstraintSettingInfo].label
    }

    base_constraints[refs.maybe_building_android_binary[ConstraintSettingInfo].label] = refs.building_android_binary[ConstraintValueInfo]

    if min_sdk_version:
        base_constraints[refs.min_sdk_version[ConstraintSettingInfo].label] = _get_min_sdk_constraint_value(min_sdk_version, refs)

    new_configs = {}
    for platform_name, cpu_constraint in cpu_name_to_cpu_constraint.items():
        updated_constraints = dict(base_constraints)
        updated_constraints[refs.cpu[ConstraintSettingInfo].label] = cpu_constraint
        if len(new_configs) > 0:
            updated_constraints[refs.maybe_build_only_native_code[ConstraintSettingInfo].label] = refs.build_only_native_code[ConstraintValueInfo]

        new_configs[platform_name] = PlatformInfo(
            label = platform_name,
            configuration = ConfigurationInfo(
                constraints = updated_constraints,
                values = platform.configuration.values,
            ),
        )

    return new_configs

def _cpu_transition_impl(
        platform: PlatformInfo.type,
        refs: struct.type,
        attrs: struct.type) -> PlatformInfo.type:
    return _cpu_split_transition_impl(platform, refs, attrs).values()[0]

cpu_split_transition = transition(
    impl = _cpu_split_transition_impl,
    refs = _REFS,
    attrs = [
        "cpu_filters",
        "min_sdk_version",
        "_is_force_single_cpu",
        "_is_force_single_default_cpu",
    ],
    split = True,
)

# If our deps have been split-transitioned by CPU then we are already analyzing the dependency
# graph using the resulting configurations. If there are any other attributes on the same target
# that also need to analyze the dependency graph, then we want to use one of the configurations
# from the split transition so that we don't end up analyzing the graph again using a different
# configuration. This rule just picks the first configuration from the split-transition.
#
# This is used for the `manifest` attribute of `android_binary`.
cpu_transition = transition(
    impl = _cpu_transition_impl,
    refs = _REFS,
    attrs = [
        "cpu_filters",
        "min_sdk_version",
        "_is_force_single_cpu",
        "_is_force_single_default_cpu",
    ],
)

def get_deps_by_platform(ctx: AnalysisContext) -> dict[str, list[Dependency]]:
    deps_by_platform = {}
    for dep_dict in ctx.attrs.deps:
        for platform, dep in dep_dict.items():
            deps = deps_by_platform.get(platform, [])
            deps.append(dep)
            deps_by_platform[platform] = deps

    return deps_by_platform

def _get_min_sdk_constraint_value(min_sdk_version: int, refs: struct.type) -> ConstraintValueInfo.type:
    constraint_name = get_min_sdk_version_constraint_value_name(min_sdk_version)
    constraint = getattr(refs, constraint_name, None)
    if not constraint:
        fail("Unsupported min_sdk_version {}, please report!".format(min_sdk_version))

    return constraint[ConstraintValueInfo]

def _is_building_android_binary() -> "selector":
    return select(
        {
            "DEFAULT": False,
            "prelude//os:building_android_binary": True,
        },
    )

def is_building_android_binary_attr() -> "attribute":
    return attrs.default_only(attrs.bool(default = _is_building_android_binary()))
