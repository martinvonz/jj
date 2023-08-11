# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# @oss-disable: load("@prelude//meta_only:product_constraints.bzl", _PRODUCT_CONSTRAINTS = "constraints") 
# @oss-disable: load("@prelude//meta_only:third_party_version_constraints.bzl", _VERSION_CONSTRAINTS = "constraints") 

# @oss-disable: _CONSTRAINTS = _PRODUCT_CONSTRAINTS + _VERSION_CONSTRAINTS 
_CONSTRAINTS = [] # @oss-enable

# Apparently, `==` doesn't do value comparison for `ConstraintValueInfo`, so
# impl a hacky eq impl to workaround.
def _constr_eq(a, b):
    return a.label == b.label

def _constraint_overrides_transition_impl(
        platform: PlatformInfo.type,
        refs: struct.type,
        attrs: struct.type) -> PlatformInfo.type:
    # Extract actual constraint value objects.
    new_constraints = [
        getattr(refs, constraint)[ConstraintValueInfo]
        for constraint in attrs.constraint_overrides
    ]

    # Filter out new constraints which are already a part of the platform.
    new_constraints = [
        constraint
        for constraint in new_constraints
        if (
            constraint.setting.label not in platform.configuration.constraints or
            not _constr_eq(constraint, platform.configuration.constraints[constraint.setting.label])
        )
    ]

    # Nothing to do.
    if not new_constraints:
        return platform

    # Generate new constraints.
    constraints = {}
    constraints.update(platform.configuration.constraints)
    for constraint in new_constraints:
        constraints[constraint.setting.label] = constraint

    return PlatformInfo(
        label = platform.label,
        configuration = ConfigurationInfo(
            constraints = constraints,
            values = platform.configuration.values,
        ),
    )

constraint_overrides_transition = transition(
    impl = _constraint_overrides_transition_impl,
    refs = {constraint: constraint for constraint in _CONSTRAINTS},
    attrs = [
        "constraint_overrides",
    ],
)
