# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple/user:apple_resource_bundle.bzl", _apple_resource_bundle_spec = "registration_spec")
load("@prelude//apple/user:apple_selective_debugging.bzl", _apple_selective_debugging_spec = "registration_spec")
load("@prelude//apple/user:apple_simulators.bzl", _apple_simulators_spec = "registration_spec")
load("@prelude//apple/user:apple_toolchain_override.bzl", _apple_toolchain_override_spec = "registration_spec")
load("@prelude//apple/user:apple_tools.bzl", _apple_tools_spec = "registration_spec")
load("@prelude//apple/user:apple_watchos_bundle.bzl", _apple_watchos_bundle_spec = "registration_spec")
load("@prelude//apple/user:resource_group_map.bzl", _resource_group_map_spec = "registration_spec")
load("@prelude//cxx/user:cxx_toolchain_override.bzl", _cxx_toolchain_override_inheriting_target_platform_spec = "cxx_toolchain_override_inheriting_target_platform_registration_spec", _cxx_toolchain_override_spec = "cxx_toolchain_override_registration_spec")
load("@prelude//cxx/user:link_group_map.bzl", _link_group_map_spec = "registration_spec")
load(":cxx_headers_bundle.bzl", _cxx_headers_bundle_spec = "registration_spec")
load(":extract_archive.bzl", _extract_archive_spec = "registration_spec")
load(":write_file.bzl", _write_file_spec = "registration_spec")

_all_specs = [
    _extract_archive_spec,
    _apple_tools_spec,
    _apple_selective_debugging_spec,
    _apple_resource_bundle_spec,
    _link_group_map_spec,
    _resource_group_map_spec,
    _apple_watchos_bundle_spec,
    _apple_toolchain_override_spec,
    _cxx_headers_bundle_spec,
    _cxx_toolchain_override_spec,
    _cxx_toolchain_override_inheriting_target_platform_spec,
    _apple_simulators_spec,
    _write_file_spec,
]

rules = {
    s.name: rule(
        impl = s.impl,
        attrs = s.attrs,
        doc = s.doc,
        is_toolchain_rule = s.is_toolchain_rule,
        **{k: v for k, v in {"cfg": s.cfg}.items() if v != None}
    )
    for s in _all_specs
}

# The rules are accessed by doing module.name, so we have to put them on the correct module.
load_symbols(rules)
