# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":util.bzl", "util")

# config_setting() accepts a list of constraint_values and a list of values
# (buckconfig keys + expected values) and matches if all of those match.
#
# This is implemented as forming a single ConfigurationInfo from the union of the
# referenced values and the config keys.
#
# Attributes:
#   "constraint_values": attrs.list(attrs.configuration_label(), default = []),
#   "values": attrs.dict(key = attrs.string(), value = attrs.string(), sorted = False, default = {}),
def config_setting_impl(ctx):
    subinfos = [util.constraint_values_to_configuration(ctx.attrs.constraint_values)]
    subinfos.append(ConfigurationInfo(constraints = {}, values = ctx.attrs.values))
    return [DefaultInfo(), util.configuration_info_union(subinfos)]

# constraint_setting() targets just declare the existence of a constraint.
def constraint_setting_impl(ctx):
    return [DefaultInfo(), ConstraintSettingInfo(label = ctx.label.raw_target())]

# constraint_value() declares a specific value of a constraint_setting.
#
# Attributes:
#  constraint_setting: the target constraint that this is a value of
def constraint_value_impl(ctx):
    constraint_value = ConstraintValueInfo(
        setting = ctx.attrs.constraint_setting[ConstraintSettingInfo],
        label = ctx.label.raw_target(),
    )
    return [
        DefaultInfo(),
        constraint_value,
        # Provide `ConfigurationInfo` from `constraint_value` so it could be used as select key.
        ConfigurationInfo(constraints = {
            constraint_value.setting.label: constraint_value,
        }, values = {}),
    ]

# platform() declares a platform, it is a list of constraint values.
#
# Attributes:
#  constraint_values: list of constraint values that are set for this platform
#  deps: a list of platform target dependencies, the constraints from these platforms will be part of this platform (unless overridden)
def platform_impl(ctx):
    subinfos = (
        [dep[PlatformInfo].configuration for dep in ctx.attrs.deps] +
        [util.constraint_values_to_configuration(ctx.attrs.constraint_values)]
    )
    return [
        DefaultInfo(),
        PlatformInfo(
            label = str(ctx.label.raw_target()),
            # TODO(nga): current behavior is the last constraint value for constraint setting wins.
            #   This allows overriding constraint values from dependencies, and moreover,
            #   it allows overriding constraint values from constraint values listed
            #   in the same `constraint_values` attribute earlier.
            #   If this is intentional, state it explicitly.
            #   Otherwise, fix it.
            configuration = util.configuration_info_union(subinfos),
        ),
    ]

# TODO(cjhopman): Update the attributes for these ruletypes to declare the types of providers that they expect in their references.
extra_attributes = {
    "platform": {
        "constraint_values": attrs.list(attrs.dep(providers = [ConstraintValueInfo]), default = []),
    },
}

implemented_rules = {
    "config_setting": config_setting_impl,
    "constraint_setting": constraint_setting_impl,
    "constraint_value": constraint_value_impl,
    "platform": platform_impl,
}
