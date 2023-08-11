# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//android:android_providers.bzl",
    "PrebuiltNativeLibraryDir",
    "merge_android_packageable_info",
)

def prebuilt_native_library_impl(ctx: AnalysisContext) -> list["provider"]:
    if ctx.attrs.is_asset and ctx.attrs.has_wrap_script:
        fail("Cannot use `is_asset` and `has_wrap_script` in the same rule")

    prebuilt_native_library_dir = PrebuiltNativeLibraryDir(
        raw_target = ctx.label.raw_target(),
        dir = ctx.attrs.native_libs,
        for_primary_apk = ctx.attrs.has_wrap_script,
        is_asset = ctx.attrs.is_asset,
    )
    android_packageable_info = merge_android_packageable_info(
        ctx.label,
        ctx.actions,
        ctx.attrs.deps,
        prebuilt_native_library_dir = prebuilt_native_library_dir,
    )
    return [
        # Buck1 copies the input directory and returns it as the output path. We don't
        # copy; we could just return the input directory itself as the output path, but
        # we're avoiding that (due to potential confusion from the output path being an
        # input directory) until we have an actual need for prebuilt_native_library
        # having an output path.
        DefaultInfo(),
        android_packageable_info,
    ]
