# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":erlang_application.bzl", "erlang_application_impl")
load(":erlang_application_includes.bzl", "erlang_application_includes_impl")
load(":erlang_escript.bzl", "erlang_escript_impl")
load(":erlang_otp_application.bzl", "normalize_application")
load(":erlang_release.bzl", "erlang_release_impl")
load(":erlang_tests.bzl", "erlang_test_impl", "erlang_tests_macro")
load(":erlang_toolchain.bzl", "erlang_otp_binaries_impl")

# all attributes are now defined in prelude//decls:erlang_rules.bzl

# target rules

implemented_rules = {
    "erlang_app": erlang_application_impl,
    "erlang_app_includes": erlang_application_includes_impl,
    "erlang_escript": erlang_escript_impl,
    "erlang_otp_binaries": erlang_otp_binaries_impl,
    "erlang_release": erlang_release_impl,
    "erlang_test": erlang_test_impl,
}

# Macros

# Wrapper to generate the erlang_app and erlang_app_include target from a single
# specification. It also redirects the target from the regular application target
# to the include-only target for extra_include deps
def erlang_application(
        erlang_app_rule,
        erlang_app_includes_rule,
        name,
        applications = [],
        included_applications = [],
        extra_includes = [],
        labels = [],
        **kwargs):
    if read_root_config("erlang", "application_only_dependencies"):
        kwargs["shell_libs"] = []
        kwargs["resources"] = []

    normalized_applications = [
        normalize_application(app)
        for app in applications
    ]

    normalized_included_applications = [
        normalize_application(app)
        for app in included_applications
    ]

    return [
        erlang_app_rule(
            name = name,
            applications = normalized_applications,
            included_applications = normalized_included_applications,
            extra_includes = [
                _extra_include_name(dep)
                for dep in extra_includes
            ],
            labels = labels,
            **kwargs
        ),
        erlang_app_includes_rule(
            name = _extra_include_name(name),
            application_name = name,
            includes = kwargs.get("includes", []),
            visibility = kwargs.get("visibility", None),
            labels = ["generated", "app_includes"],
        ),
    ]

# convenience macro to specify the includes-only target based on the base-application
# target name
def _extra_include_name(name: str) -> str:
    return name + "_includes_only"

def erlang_tests(
        erlang_app_rule,
        erlang_test_rule,
        suites: list[str] = [],
        deps: list[str] = [],
        resources: list[str] = [],
        srcs: list[str] = [],
        property_tests: list[str] = [],
        config_files: list[str] = [],
        use_default_configs: bool = True,
        use_default_deps: bool = True,
        **common_attributes):
    """
    Generate multiple erlang_test targets based on the `suites` field.
    """
    erlang_tests_macro(
        erlang_app_rule = erlang_app_rule,
        erlang_test_rule = erlang_test_rule,
        suites = suites,
        deps = deps,
        resources = resources,
        srcs = srcs,
        property_tests = property_tests,
        config_files = config_files,
        use_default_configs = use_default_configs,
        use_default_deps = use_default_deps,
        **common_attributes
    )
