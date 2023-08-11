# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":apple_package_config.bzl", "apple_package_config")
load(
    ":apple_rules_impl_utility.bzl",
    "APPLE_ARCHIVE_OBJECTS_LOCALLY_OVERRIDE_ATTR_NAME",
)

AppleBuckConfigAttributeOverride = record(
    name = field(str),
    section = field(str, default = "apple"),
    key = field(str),
    positive_values = field([[str], [bool]], default = ["True", "true"]),
    value_if_true = field([str, bool, None], default = True),
    value_if_false = field([str, bool, None], default = False),
    skip_if_false = field(bool, default = False),
)

APPLE_LINK_LIBRARIES_LOCALLY_OVERRIDE = AppleBuckConfigAttributeOverride(
    name = "link_execution_preference",
    key = "link_libraries_locally_override",
    value_if_true = "local",
    skip_if_false = True,
)

APPLE_STRIPPED_OVERRIDE = AppleBuckConfigAttributeOverride(
    name = "stripped",
    key = "stripped_override",
    skip_if_false = True,
)

_APPLE_LIBRARY_LOCAL_EXECUTION_OVERRIDES = [
    APPLE_LINK_LIBRARIES_LOCALLY_OVERRIDE,
    AppleBuckConfigAttributeOverride(name = APPLE_ARCHIVE_OBJECTS_LOCALLY_OVERRIDE_ATTR_NAME, key = "archive_objects_locally_override"),
]

_APPLE_BINARY_LOCAL_EXECUTION_OVERRIDES = [
    AppleBuckConfigAttributeOverride(
        name = "link_execution_preference",
        key = "link_binaries_locally_override",
        value_if_true = "local",
        skip_if_false = True,
    ),
]

def apple_macro_layer_set_bool_override_attrs_from_config(overrides: list[AppleBuckConfigAttributeOverride.type]) -> dict[str, "selector"]:
    attribs = {}
    for override in overrides:
        config_value = read_root_config(override.section, override.key, None)
        if config_value != None:
            config_is_true = config_value in override.positive_values
            if not config_is_true and override.skip_if_false:
                continue
            attribs[override.name] = select({
                "DEFAULT": override.value_if_true if config_is_true else override.value_if_false,
                # Do not set attribute value for host tools
                "ovr_config//platform/macos/constraints:execution-platform-transitioned": None,
            })
    return attribs

def apple_library_macro_impl(apple_library_rule = None, **kwargs):
    kwargs.update(apple_macro_layer_set_bool_override_attrs_from_config(_APPLE_LIBRARY_LOCAL_EXECUTION_OVERRIDES))
    kwargs.update(apple_macro_layer_set_bool_override_attrs_from_config([APPLE_STRIPPED_OVERRIDE]))
    apple_library_rule(**kwargs)

def apple_binary_macro_impl(apple_binary_rule = None, **kwargs):
    kwargs.update(apple_macro_layer_set_bool_override_attrs_from_config(_APPLE_BINARY_LOCAL_EXECUTION_OVERRIDES))
    kwargs.update(apple_macro_layer_set_bool_override_attrs_from_config([APPLE_STRIPPED_OVERRIDE]))
    apple_binary_rule(**kwargs)

def apple_package_macro_impl(apple_package_rule = None, **kwargs):
    kwargs.update(apple_package_config())
    apple_package_rule(
        **kwargs
    )
