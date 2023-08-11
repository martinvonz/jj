# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:android_providers.bzl", "AndroidResourceInfo", "PrebuiltNativeLibraryDir", "RESOURCE_PRIORITY_LOW", "RESOURCE_PRIORITY_NORMAL", "merge_android_packageable_info")
load("@prelude//android:android_resource.bzl", "aapt2_compile", "extract_package_from_manifest")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load(
    "@prelude//java:java_providers.bzl",
    "JavaClasspathEntry",
    "create_abi",
    "create_java_library_providers",
)
load("@prelude//java:java_toolchain.bzl", "JavaToolchainInfo")

def android_prebuilt_aar_impl(ctx: AnalysisContext) -> list["provider"]:
    manifest = ctx.actions.declare_output("AndroidManifest.xml")
    all_classes_jar = ctx.actions.declare_output("classes.jar")
    r_dot_txt = ctx.actions.declare_output("R.txt")
    res = ctx.actions.declare_output("res", dir = True)
    assets = ctx.actions.declare_output("assets", dir = True)
    jni = ctx.actions.declare_output("jni", dir = True)
    annotation_jars_dir = ctx.actions.declare_output("annotation_jars", dir = True)
    proguard_config = ctx.actions.declare_output("proguard.txt")

    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]
    unpack_aar_tool = android_toolchain.unpack_aar[RunInfo]
    java_toolchain = ctx.attrs._java_toolchain[JavaToolchainInfo]
    jar_builder_tool = cmd_args(java_toolchain.jar_builder, delimiter = " ")

    unpack_aar_cmd = [
        unpack_aar_tool,
        "--aar",
        ctx.attrs.aar,
        "--manifest-path",
        manifest.as_output(),
        "--all-classes-jar-path",
        all_classes_jar.as_output(),
        "--r-dot-txt-path",
        r_dot_txt.as_output(),
        "--res-path",
        res.as_output(),
        "--assets-path",
        assets.as_output(),
        "--jni-path",
        jni.as_output(),
        "--annotation-jars-dir",
        annotation_jars_dir.as_output(),
        "--proguard-config-path",
        proguard_config.as_output(),
        "--jar-builder-tool",
        jar_builder_tool,
    ]

    ctx.actions.run(unpack_aar_cmd, category = "android_unpack_aar")

    resource_info = AndroidResourceInfo(
        raw_target = ctx.label.raw_target(),
        aapt2_compile_output = aapt2_compile(ctx, res, android_toolchain),
        allow_strings_as_assets_resource_filtering = True,
        assets = assets,
        manifest_file = manifest,
        r_dot_java_package = extract_package_from_manifest(ctx, manifest),
        res = res,
        res_priority = RESOURCE_PRIORITY_LOW if android_toolchain.prebuilt_aar_resources_have_low_priority else RESOURCE_PRIORITY_NORMAL,
        text_symbols = r_dot_txt,
    )

    abi = None if java_toolchain.is_bootstrap_toolchain else create_abi(ctx.actions, java_toolchain.class_abi_generator, all_classes_jar)

    library_output_classpath_entry = JavaClasspathEntry(
        full_library = all_classes_jar,
        abi = abi or all_classes_jar,
        abi_as_dir = None,
        required_for_source_only_abi = ctx.attrs.required_for_source_only_abi,
    )

    java_library_info, java_packaging_info, shared_library_info, cxx_resource_info, template_placeholder_info, java_library_intellij_info = create_java_library_providers(
        ctx = ctx,
        library_output = library_output_classpath_entry,
        exported_deps = ctx.attrs.deps,
        needs_desugar = True,
        is_prebuilt_jar = True,
        annotation_jars_dir = annotation_jars_dir,
        proguard_config = proguard_config,
    )

    native_library = PrebuiltNativeLibraryDir(
        raw_target = ctx.label.raw_target(),
        dir = jni,
        for_primary_apk = ctx.attrs.use_system_library_loader,
        is_asset = False,
    )

    return [
        java_library_info,
        java_packaging_info,
        shared_library_info,
        cxx_resource_info,
        template_placeholder_info,
        java_library_intellij_info,
        merge_android_packageable_info(ctx.label, ctx.actions, ctx.attrs.deps, manifest = manifest, prebuilt_native_library_dir = native_library, resource_info = resource_info),
        resource_info,
        DefaultInfo(default_output = all_classes_jar, other_outputs = [
            manifest,
            r_dot_txt,
            res,
            assets,
            jni,
            annotation_jars_dir,
            proguard_config,
        ]),
    ]
