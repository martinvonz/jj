# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":apple_bundle_config.bzl", "apple_bundle_config")
load(":apple_info_plist_substitutions_parsing.bzl", "parse_codesign_entitlements")
load(":apple_resource_bundle.bzl", "make_resource_bundle_rule")

def apple_bundle_macro_impl(apple_bundle_rule, apple_resource_bundle_rule, **kwargs):
    info_plist_substitutions = kwargs.get("info_plist_substitutions")
    kwargs.update(apple_bundle_config())
    apple_bundle_rule(
        _codesign_entitlements = parse_codesign_entitlements(info_plist_substitutions),
        _resource_bundle = make_resource_bundle_rule(apple_resource_bundle_rule, **kwargs),
        **kwargs
    )
