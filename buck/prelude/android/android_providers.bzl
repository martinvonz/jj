# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

Aapt2LinkInfo = record(
    # "APK" containing resources to be used by the Android binary
    primary_resources_apk = "artifact",
    # proguard config needed to retain used resources
    proguard_config_file = "artifact",
    # R.txt containing all the linked resources
    r_dot_txt = "artifact",
)

AndroidBinaryNativeLibsInfo = record(
    apk_under_test_prebuilt_native_library_dirs = ["PrebuiltNativeLibraryDir"],
    apk_under_test_shared_libraries = ["SharedLibrary"],
    exopackage_info = ["ExopackageNativeInfo", None],
    root_module_native_lib_assets = ["artifact"],
    non_root_module_native_lib_assets = ["artifact"],
    native_libs_for_primary_apk = ["artifact"],
    unstripped_libs = {"artifact": str},
)

AndroidBinaryResourcesInfo = record(
    # Optional information about resources that should be exopackaged
    exopackage_info = ["ExopackageResourcesInfo", None],
    # manifest to be used by the APK
    manifest = "artifact",
    # per-module manifests (packaged as assets)
    module_manifests = ["artifact"],
    # zip containing any strings packaged as assets
    packaged_string_assets = ["artifact", None],
    # "APK" containing resources to be used by the Android binary
    primary_resources_apk = "artifact",
    # proguard config needed to retain used resources
    proguard_config_file = "artifact",
    # R.java jars containing all the linked resources
    r_dot_javas = ["JavaLibraryInfo"],
    # directory containing filtered string resources files
    string_source_map = ["artifact", None],
    # directory containing filtered string resources files for Voltron language packs
    voltron_string_source_map = ["artifact", None],
    # list of jars that could contain resources that should be packaged into the APK
    jar_files_that_may_contain_resources = ["artifact"],
    # The resource infos that are used in this APK
    unfiltered_resource_infos = ["AndroidResourceInfo"],
)

# Information about an `android_build_config`
BuildConfigField = record(
    type = str,
    name = str,
    value = str,
)

AndroidBuildConfigInfo = provider(
    fields = [
        "package",  # str
        "build_config_fields",  # ["BuildConfigField"]
    ],
)

# Information about an `android_manifest`
AndroidManifestInfo = provider(
    fields = [
        "manifest",  # artifact
        "merge_report",  # artifact
    ],
)

AndroidApkInfo = provider(
    fields = [
        "apk",
        "manifest",
    ],
)

AndroidAabInfo = provider(
    fields = [
        "aab",
        "manifest",
    ],
)

AndroidApkUnderTestInfo = provider(
    fields = [
        "java_packaging_deps",  # set_type("JavaPackagingDep")
        "keystore",  # "KeystoreInfo"
        "manifest_entries",  # dict
        "prebuilt_native_library_dirs",  # set_type("PrebuiltNativeLibraryDir")
        "platforms",  # [str]
        "primary_platform",  # str
        "resource_infos",  # set_type("ResourceInfos")
        "shared_libraries",  # set_type("SharedLibrary")
    ],
)

AndroidInstrumentationApkInfo = provider(
    fields = [
        "apk_under_test",  # "artifact"
    ],
)

PrebuiltNativeLibraryDir = record(
    raw_target = "target_label",
    dir = "artifact",  # contains subdirectories for different ABIs.
    for_primary_apk = bool,
    is_asset = bool,
)

def _artifacts(value: "ManifestInfo"):
    return value.manifest

AndroidBuildConfigInfoTSet = transitive_set()
AndroidDepsTSet = transitive_set()
ManifestTSet = transitive_set(args_projections = {"artifacts": _artifacts})
PrebuiltNativeLibraryDirTSet = transitive_set()
ResourceInfoTSet = transitive_set()

DepsInfo = record(
    name = "target_label",
    deps = ["target_label"],
)

ManifestInfo = record(
    target_label = "target_label",
    manifest = "artifact",
)

AndroidPackageableInfo = provider(
    fields = [
        "target_label",  # "target_label"
        "build_config_infos",  # ["AndroidBuildConfigInfoTSet", None]
        "deps",  # ["AndroidDepsTSet", None]
        "manifests",  # ["ManifestTSet", None]
        "prebuilt_native_library_dirs",  # ["PrebuiltNativeLibraryDirTSet", None]
        "resource_infos",  # ["AndroidResourceInfoTSet", None]
    ],
)

RESOURCE_PRIORITY_NORMAL = "normal"
RESOURCE_PRIORITY_LOW = "low"

# Information about an `android_resource`
AndroidResourceInfo = provider(
    fields = [
        # Target that produced this provider
        "raw_target",  # "target_label",
        # output of running `aapt2_compile` on the resources, if resources are present
        "aapt2_compile_output",  # ["artifact", None]
        #  if False, then the "res" are not affected by the strings-as-assets resource filter
        "allow_strings_as_assets_resource_filtering",  # bool
        # assets defined by this rule. May be empty
        "assets",  # ["artifact", None]
        # manifest file used by the resources, if resources are present
        "manifest_file",  # ["artifact", None]
        # package used for R.java, if resources are present
        "r_dot_java_package",  # ["artifact", None]
        # resources defined by this rule. May be empty
        "res",  # ["artifact", None]
        # priority of the resources, may be 'low' or 'normal'
        "res_priority",  # str.type
        # symbols defined by the resources, if resources are present
        "text_symbols",  # ["artifact", None]
    ],
)

# `AndroidResourceInfos` that are exposed via `exported_deps`
ExportedAndroidResourceInfo = provider(
    fields = [
        "resource_infos",  # ["AndroidResourceInfo"]
    ],
)

ExopackageDexInfo = record(
    metadata = "artifact",
    directory = "artifact",
)

ExopackageNativeInfo = record(
    metadata = "artifact",
    directory = "artifact",
)

ExopackageResourcesInfo = record(
    assets = ["artifact", None],
    assets_hash = ["artifact", None],
    res = "artifact",
    res_hash = "artifact",
)

DexFilesInfo = record(
    primary_dex = "artifact",
    primary_dex_class_names = ["artifact", None],
    root_module_secondary_dex_dirs = ["artifact"],
    non_root_module_secondary_dex_dirs = ["artifact"],
    secondary_dex_exopackage_info = [ExopackageDexInfo.type, None],
    proguard_text_files_path = ["artifact", None],
)

ExopackageInfo = record(
    secondary_dex_info = [ExopackageDexInfo.type, None],
    native_library_info = [ExopackageNativeInfo.type, None],
    resources_info = [ExopackageResourcesInfo.type, None],
)

AndroidLibraryIntellijInfo = provider(
    doc = "Information about android library that is required for Intellij project generation",
    fields = [
        "dummy_r_dot_java",  # ["artifact", None]
        "android_resource_deps",  # ["AndroidResourceInfo"]
    ],
)

def merge_android_packageable_info(
        label: Label,
        actions: "actions",
        deps: list[Dependency],
        build_config_info: ["AndroidBuildConfigInfo", None] = None,
        manifest: ["artifact", None] = None,
        prebuilt_native_library_dir: [PrebuiltNativeLibraryDir.type, None] = None,
        resource_info: ["AndroidResourceInfo", None] = None) -> "AndroidPackageableInfo":
    android_packageable_deps = filter(None, [x.get(AndroidPackageableInfo) for x in deps])

    build_config_infos = _get_transitive_set(
        actions,
        filter(None, [dep.build_config_infos for dep in android_packageable_deps]),
        build_config_info,
        AndroidBuildConfigInfoTSet,
    )

    deps = _get_transitive_set(
        actions,
        filter(None, [dep.deps for dep in android_packageable_deps]),
        DepsInfo(
            name = label.raw_target(),
            deps = [dep.target_label for dep in android_packageable_deps],
        ),
        AndroidDepsTSet,
    )

    manifests = _get_transitive_set(
        actions,
        filter(None, [dep.manifests for dep in android_packageable_deps]),
        ManifestInfo(
            target_label = label.raw_target(),
            manifest = manifest,
        ) if manifest else None,
        ManifestTSet,
    )

    prebuilt_native_library_dirs = _get_transitive_set(
        actions,
        filter(None, [dep.prebuilt_native_library_dirs for dep in android_packageable_deps]),
        prebuilt_native_library_dir,
        PrebuiltNativeLibraryDirTSet,
    )

    resource_infos = _get_transitive_set(
        actions,
        filter(None, [dep.resource_infos for dep in android_packageable_deps]),
        resource_info,
        ResourceInfoTSet,
    )

    return AndroidPackageableInfo(
        target_label = label.raw_target(),
        build_config_infos = build_config_infos,
        deps = deps,
        manifests = manifests,
        prebuilt_native_library_dirs = prebuilt_native_library_dirs,
        resource_infos = resource_infos,
    )

def _get_transitive_set(
        actions: "actions",
        children: list["transitive_set"],
        node: "_a",
        transitive_set_definition: "transitive_set_definition") -> ["transitive_set", None]:
    kwargs = {}
    if children:
        kwargs["children"] = children
    if node:
        kwargs["value"] = node

    return actions.tset(transitive_set_definition, **kwargs) if kwargs else None

def merge_exported_android_resource_info(
        exported_deps: list[Dependency]) -> "ExportedAndroidResourceInfo":
    exported_android_resource_infos = []
    for exported_dep in exported_deps:
        exported_resource_info = exported_dep.get(ExportedAndroidResourceInfo)
        if exported_resource_info:
            exported_android_resource_infos += exported_resource_info.resource_infos

        android_resource = exported_dep.get(AndroidResourceInfo)
        if android_resource:
            exported_android_resource_infos.append(android_resource)

    return ExportedAndroidResourceInfo(resource_infos = dedupe(exported_android_resource_infos))
