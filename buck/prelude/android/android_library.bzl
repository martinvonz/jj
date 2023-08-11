# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//android:android_providers.bzl",
    "AndroidLibraryIntellijInfo",
    "AndroidResourceInfo",
    "merge_android_packageable_info",
    "merge_exported_android_resource_info",
)
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:r_dot_java.bzl", "get_dummy_r_dot_java")
load("@prelude//java:java_library.bzl", "build_java_library")
load("@prelude//java:java_providers.bzl", "create_native_providers", "to_list")
load("@prelude//kotlin:kotlin_library.bzl", "build_kotlin_library")

def android_library_impl(ctx: AnalysisContext) -> list["provider"]:
    packaging_deps = ctx.attrs.deps + (ctx.attrs.deps_query or []) + ctx.attrs.exported_deps + ctx.attrs.runtime_deps
    if ctx.attrs._build_only_native_code:
        shared_library_info, cxx_resource_info = create_native_providers(ctx.actions, ctx.label, packaging_deps)
        return [
            merge_android_packageable_info(ctx.label, ctx.actions, packaging_deps),
            shared_library_info,
            cxx_resource_info,
            # Add an unused default output in case this target is used as an attr.source() anywhere.
            DefaultInfo(default_output = ctx.actions.write("{}/unused.jar".format(ctx.label.name), [])),
        ]

    java_providers, android_library_intellij_info = build_android_library(ctx)
    android_providers = [android_library_intellij_info] if android_library_intellij_info else []

    return to_list(java_providers) + [
        merge_android_packageable_info(
            ctx.label,
            ctx.actions,
            packaging_deps,
            manifest = ctx.attrs.manifest,
        ),
        merge_exported_android_resource_info(ctx.attrs.exported_deps),
    ] + android_providers

def build_android_library(
        ctx: AnalysisContext,
        r_dot_java: ["artifact", None] = None) -> ("JavaProviders", [AndroidLibraryIntellijInfo.type, None]):
    bootclasspath_entries = [] + ctx.attrs._android_toolchain[AndroidToolchainInfo].android_bootclasspath
    additional_classpath_entries = []

    dummy_r_dot_java, android_library_intellij_info = _get_dummy_r_dot_java(ctx)
    extra_sub_targets = {}

    if r_dot_java:
        additional_classpath_entries.append(r_dot_java)
    elif dummy_r_dot_java:
        additional_classpath_entries.append(dummy_r_dot_java)
        extra_sub_targets["dummy_r_dot_java"] = [DefaultInfo(default_output = dummy_r_dot_java)]

    if ctx.attrs.language != None and ctx.attrs.language.lower() == "kotlin":
        return build_kotlin_library(
            ctx,
            additional_classpath_entries = additional_classpath_entries,
            bootclasspath_entries = bootclasspath_entries,
            extra_sub_targets = extra_sub_targets,
        ), android_library_intellij_info
    else:
        return build_java_library(
            ctx,
            ctx.attrs.srcs,
            additional_classpath_entries = additional_classpath_entries,
            bootclasspath_entries = bootclasspath_entries,
            extra_sub_targets = extra_sub_targets,
        ), android_library_intellij_info

def _get_dummy_r_dot_java(
        ctx: AnalysisContext) -> (["artifact", None], [AndroidLibraryIntellijInfo.type, None]):
    android_resources = dedupe([resource for resource in filter(None, [
        x.get(AndroidResourceInfo)
        for x in ctx.attrs.deps + (ctx.attrs.deps_query or []) + ctx.attrs.provided_deps + (getattr(ctx.attrs, "provided_deps_query", []) or [])
    ]) if resource.res != None])
    if len(android_resources) == 0:
        return (None, None)

    dummy_r_dot_java_library_info = get_dummy_r_dot_java(
        ctx,
        ctx.attrs._android_toolchain[AndroidToolchainInfo].merge_android_resources[RunInfo],
        android_resources,
        ctx.attrs.resource_union_package,
    )

    dummy_r_dot_java = dummy_r_dot_java_library_info.library_output.abi
    return (dummy_r_dot_java, AndroidLibraryIntellijInfo(
        dummy_r_dot_java = dummy_r_dot_java,
        android_resource_deps = android_resources,
    ))
