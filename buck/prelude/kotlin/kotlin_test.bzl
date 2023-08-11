# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//java:java_test.bzl", "build_junit_test")
load("@prelude//kotlin:kotlin_library.bzl", "build_kotlin_library")
load("@prelude//test/inject_test_run_info.bzl", "inject_test_run_info")

def kotlin_test_impl(ctx: AnalysisContext) -> list["provider"]:
    if ctx.attrs._build_only_native_code:
        return [DefaultInfo()]

    java_providers = build_kotlin_library(ctx, ctx.attrs.srcs)
    external_runner_test_info = build_junit_test(ctx, java_providers.java_library_info, java_providers.java_packaging_info, java_providers.class_to_src_map)

    return inject_test_run_info(ctx, external_runner_test_info) + [
        java_providers.java_library_intellij_info,
        java_providers.java_library_info,
        java_providers.java_packaging_info,
        java_providers.template_placeholder_info,
        java_providers.default_info,
    ]
