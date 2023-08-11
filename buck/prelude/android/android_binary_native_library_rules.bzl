# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load("@prelude//android:android_providers.bzl", "AndroidBinaryNativeLibsInfo", "ExopackageNativeInfo")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:cpu_filters.bzl", "CPU_FILTER_FOR_PRIMARY_PLATFORM", "CPU_FILTER_TO_ABI_DIRECTORY")
load("@prelude//android:voltron.bzl", "ROOT_MODULE", "all_targets_in_root_module", "get_apk_module_graph_info", "is_root_module")
load("@prelude//linking:shared_libraries.bzl", "SharedLibraryInfo", "merge_shared_libraries", "traverse_shared_library_info")
load("@prelude//utils:set.bzl", "set_type")  # @unused Used as a type
load("@prelude//utils:utils.bzl", "expect")

# Native libraries on Android are built for a particular Application Binary Interface (ABI). We
# package native libraries for one (or more, for multi-arch builds) ABIs into an Android APK.
#
# Our native libraries come from two sources:
# 1. "Prebuilt native library dirs", which are directory artifacts whose sub-directories are ABIs,
#    and those ABI subdirectories contain native libraries. These come from `android_prebuilt_aar`s
#    and `prebuilt_native_library`s, for example.
# 2. "Native linkables". These are each a single shared library - `.so`s for one particular ABI.
#
# Native libraries can be packaged into Android APKs in two ways.
# 1. As native libraries. This means that they are passed to the APK builder as native libraries,
#    and the APK builder will package `<ABI>/library.so` into the APK at `libs/<ABI>/library.so`.
# 2. As assets. These are passed to the APK build as assets, and are stored at
#    `assets/lib/<ABI>/library.so` In the root module, we only package a native library as an
#    asset if it is eligible to be an asset (e.g. `can_be_asset` on a `cxx_library`), and
#    `package_asset_libraries` is set to True for the APK. We will additionally compress all the
#    assets into a single `assets/lib/libs.xz` (or `assets/libs/libs.zstd` for `zstd` compression)
#    if `compress_asset_libraries` is set to True for the APK. Regardless of whether we compress
#    the assets or not, we create a metadata file at `assets/libs/metadata.txt` that has a single
#    line entry for each packaged asset consisting of '<ABI/library_name> <file_size> <sha256>'.
#
#    Any native library that is not part of the root module (i.e. it is part of some other Voltron
#    module) is automatically packaged as an asset, and the assets for each module are compressed
#    to a single `assets/<module_name>/libs.xz`. Similarly, the metadata for each module is stored
#    at `assets/<module_name>/libs.txt`.

def get_android_binary_native_library_info(
        ctx: AnalysisContext,
        android_packageable_info: "AndroidPackageableInfo",
        deps_by_platform: dict[str, list[Dependency]],
        apk_module_graph_file: ["artifact", None] = None,
        prebuilt_native_library_dirs_to_exclude: [set_type, None] = None,
        shared_libraries_to_exclude: [set_type, None] = None) -> AndroidBinaryNativeLibsInfo.type:
    traversed_prebuilt_native_library_dirs = android_packageable_info.prebuilt_native_library_dirs.traverse() if android_packageable_info.prebuilt_native_library_dirs else []
    all_prebuilt_native_library_dirs = [
        native_lib
        for native_lib in traversed_prebuilt_native_library_dirs
        if not (prebuilt_native_library_dirs_to_exclude and prebuilt_native_library_dirs_to_exclude.contains(native_lib.raw_target))
    ]

    unstripped_libs = {}
    all_shared_libraries = []
    platform_to_native_linkables = {}
    for platform, deps in deps_by_platform.items():
        if platform == CPU_FILTER_FOR_PRIMARY_PLATFORM and platform not in ctx.attrs.cpu_filters:
            continue
        shared_library_info = merge_shared_libraries(
            ctx.actions,
            deps = filter(None, [x.get(SharedLibraryInfo) for x in deps]),
        )
        native_linkables = {
            so_name: shared_lib
            for so_name, shared_lib in traverse_shared_library_info(shared_library_info).items()
            if not (shared_libraries_to_exclude and shared_libraries_to_exclude.contains(shared_lib.label.raw_target()))
        }
        all_shared_libraries.extend(native_linkables.values())
        for shared_lib in native_linkables.values():
            unstripped_libs[shared_lib.lib.output] = platform
        platform_to_native_linkables[platform] = native_linkables

    if apk_module_graph_file == None:
        native_libs_and_assets_info = _get_native_libs_and_assets(
            ctx,
            all_targets_in_root_module,
            all_prebuilt_native_library_dirs,
            platform_to_native_linkables,
        )
        native_libs_for_primary_apk, exopackage_info = _get_exopackage_info(
            ctx,
            native_libs_and_assets_info.native_libs_always_in_primary_apk,
            native_libs_and_assets_info.native_libs,
            native_libs_and_assets_info.native_libs_metadata,
        )
        root_module_native_lib_assets = filter(None, [
            native_libs_and_assets_info.native_lib_assets_for_primary_apk,
            native_libs_and_assets_info.stripped_native_linkable_assets_for_primary_apk,
            native_libs_and_assets_info.root_module_metadata_assets,
            native_libs_and_assets_info.root_module_compressed_lib_assets,
        ])
        return AndroidBinaryNativeLibsInfo(
            apk_under_test_prebuilt_native_library_dirs = all_prebuilt_native_library_dirs,
            apk_under_test_shared_libraries = all_shared_libraries,
            native_libs_for_primary_apk = native_libs_for_primary_apk,
            exopackage_info = exopackage_info,
            unstripped_libs = unstripped_libs,
            root_module_native_lib_assets = root_module_native_lib_assets,
            non_root_module_native_lib_assets = [],
        )
    else:
        native_libs = ctx.actions.declare_output("native_libs_symlink")
        native_libs_metadata = ctx.actions.declare_output("native_libs_metadata_symlink")
        native_libs_always_in_primary_apk = ctx.actions.declare_output("native_libs_always_in_primary_apk_symlink")
        native_lib_assets_for_primary_apk = ctx.actions.declare_output("native_lib_assets_for_primary_apk_symlink")
        stripped_native_linkable_assets_for_primary_apk = ctx.actions.declare_output("stripped_native_linkable_assets_for_primary_apk_symlink")
        root_module_metadata_assets = ctx.actions.declare_output("root_module_metadata_assets_symlink")
        root_module_compressed_lib_assets = ctx.actions.declare_output("root_module_compressed_lib_assets_symlink")
        non_root_module_metadata_assets = ctx.actions.declare_output("non_root_module_metadata_assets_symlink")
        non_root_module_compressed_lib_assets = ctx.actions.declare_output("non_root_module_compressed_lib_assets_symlink")

        outputs = [
            native_libs,
            native_libs_metadata,
            native_libs_always_in_primary_apk,
            native_lib_assets_for_primary_apk,
            stripped_native_linkable_assets_for_primary_apk,
            root_module_metadata_assets,
            root_module_compressed_lib_assets,
            non_root_module_metadata_assets,
            non_root_module_compressed_lib_assets,
        ]

        def get_native_libs_info_modular(ctx: AnalysisContext, artifacts, outputs):
            get_module_from_target = get_apk_module_graph_info(ctx, apk_module_graph_file, artifacts).target_to_module_mapping_function
            dynamic_info = _get_native_libs_and_assets(
                ctx,
                get_module_from_target,
                all_prebuilt_native_library_dirs,
                platform_to_native_linkables,
            )

            # Since we are using a dynamic action, we need to declare the outputs in advance.
            # Rather than passing the created outputs into `_get_native_libs_and_assets`, we
            # just symlink to the outputs that function produces.
            ctx.actions.symlink_file(outputs[native_libs], dynamic_info.native_libs)
            ctx.actions.symlink_file(outputs[native_libs_metadata], dynamic_info.native_libs_metadata)
            ctx.actions.symlink_file(outputs[native_libs_always_in_primary_apk], dynamic_info.native_libs_always_in_primary_apk)
            ctx.actions.symlink_file(outputs[native_lib_assets_for_primary_apk], dynamic_info.native_lib_assets_for_primary_apk if dynamic_info.native_lib_assets_for_primary_apk else ctx.actions.symlinked_dir("empty_native_lib_assets", {}))
            ctx.actions.symlink_file(outputs[stripped_native_linkable_assets_for_primary_apk], dynamic_info.stripped_native_linkable_assets_for_primary_apk if dynamic_info.stripped_native_linkable_assets_for_primary_apk else ctx.actions.symlinked_dir("empty_stripped_native_linkable_assets", {}))
            ctx.actions.symlink_file(outputs[root_module_metadata_assets], dynamic_info.root_module_metadata_assets)
            ctx.actions.symlink_file(outputs[root_module_compressed_lib_assets], dynamic_info.root_module_compressed_lib_assets)
            ctx.actions.symlink_file(outputs[non_root_module_metadata_assets], dynamic_info.non_root_module_metadata_assets)
            ctx.actions.symlink_file(outputs[non_root_module_compressed_lib_assets], dynamic_info.non_root_module_compressed_lib_assets)

        ctx.actions.dynamic_output(dynamic = [apk_module_graph_file], inputs = [], outputs = outputs, f = get_native_libs_info_modular)

        native_libs_for_primary_apk, exopackage_info = _get_exopackage_info(ctx, native_libs_always_in_primary_apk, native_libs, native_libs_metadata)
        return AndroidBinaryNativeLibsInfo(
            apk_under_test_prebuilt_native_library_dirs = all_prebuilt_native_library_dirs,
            apk_under_test_shared_libraries = all_shared_libraries,
            native_libs_for_primary_apk = native_libs_for_primary_apk,
            exopackage_info = exopackage_info,
            unstripped_libs = unstripped_libs,
            root_module_native_lib_assets = [native_lib_assets_for_primary_apk, stripped_native_linkable_assets_for_primary_apk, root_module_metadata_assets, root_module_compressed_lib_assets],
            non_root_module_native_lib_assets = [non_root_module_metadata_assets, non_root_module_compressed_lib_assets],
        )

# We could just return two artifacts of libs (one for the primary APK, one which can go
# either into the primary APK or be exopackaged), and one artifact of assets,
# but we'd need an extra action in order to combine them (we can't use `symlinked_dir` since
# the paths overlap) so it's easier to just be explicit about exactly what we produce.
_NativeLibsAndAssetsInfo = record(
    native_libs = "artifact",
    native_libs_metadata = "artifact",
    native_libs_always_in_primary_apk = "artifact",
    native_lib_assets_for_primary_apk = ["artifact", None],
    stripped_native_linkable_assets_for_primary_apk = ["artifact", None],
    root_module_metadata_assets = "artifact",
    root_module_compressed_lib_assets = "artifact",
    non_root_module_metadata_assets = "artifact",
    non_root_module_compressed_lib_assets = "artifact",
)

def _get_exopackage_info(
        ctx: AnalysisContext,
        native_libs_always_in_primary_apk: "artifact",
        native_libs: "artifact",
        native_libs_metadata: "artifact") -> (list["artifact"], [ExopackageNativeInfo.type, None]):
    is_exopackage_enabled_for_native_libs = "native_library" in getattr(ctx.attrs, "exopackage_modes", [])
    if is_exopackage_enabled_for_native_libs:
        return [native_libs_always_in_primary_apk], ExopackageNativeInfo(directory = native_libs, metadata = native_libs_metadata)
    else:
        return [native_libs, native_libs_always_in_primary_apk], None

def _get_native_libs_and_assets(
        ctx: AnalysisContext,
        get_module_from_target: "function",
        all_prebuilt_native_library_dirs: list["PrebuiltNativeLibraryDir"],
        platform_to_native_linkables: dict[str, dict[str, "SharedLibrary"]]) -> _NativeLibsAndAssetsInfo.type:
    is_packaging_native_libs_as_assets_supported = getattr(ctx.attrs, "package_asset_libraries", False)

    prebuilt_native_library_dirs = []
    prebuilt_native_library_dirs_always_in_primary_apk = []
    prebuilt_native_library_dir_assets_for_primary_apk = []
    prebuilt_native_library_dir_module_assets_map = {}
    for native_lib in all_prebuilt_native_library_dirs:
        native_lib_target = str(native_lib.raw_target)
        module = get_module_from_target(native_lib_target)
        if not is_root_module(module):
            # In buck1, we always package native libs as assets when they are not in the root module
            expect(not native_lib.for_primary_apk, "{} which is marked as needing to be in the primary APK cannot be included in non-root-module {}".format(native_lib_target, module))
            prebuilt_native_library_dir_module_assets_map.setdefault(module, []).append(native_lib)
        elif native_lib.is_asset and is_packaging_native_libs_as_assets_supported:
            expect(not native_lib.for_primary_apk, "{} which is marked as needing to be in the primary APK cannot be an asset".format(native_lib_target))
            prebuilt_native_library_dir_assets_for_primary_apk.append(native_lib)
        elif native_lib.for_primary_apk:
            prebuilt_native_library_dirs_always_in_primary_apk.append(native_lib)
        else:
            prebuilt_native_library_dirs.append(native_lib)

    native_libs = _filter_prebuilt_native_library_dir(
        ctx,
        prebuilt_native_library_dirs,
        "native_libs",
    )
    native_libs_always_in_primary_apk = _filter_prebuilt_native_library_dir(
        ctx,
        prebuilt_native_library_dirs_always_in_primary_apk,
        "native_libs_always_in_primary_apk",
    )
    native_lib_assets_for_primary_apk = _filter_prebuilt_native_library_dir(
        ctx,
        prebuilt_native_library_dir_assets_for_primary_apk,
        "native_lib_assets_for_primary_apk",
        package_as_assets = True,
        module = ROOT_MODULE,
    ) if prebuilt_native_library_dir_assets_for_primary_apk else None
    native_lib_module_assets_map = {}
    for module, native_lib_dir in prebuilt_native_library_dir_module_assets_map.items():
        native_lib_module_assets_map[module] = [_filter_prebuilt_native_library_dir(
            ctx,
            native_lib_dir,
            "native_lib_assets_for_module_{}".format(module),
            package_as_assets = True,
            module = module,
        )]

    (
        stripped_native_linkables,
        stripped_native_linkables_always_in_primary_apk,
        stripped_native_linkable_assets_for_primary_apk,
        stripped_native_linkable_module_assets_map,
    ) = _get_native_linkables(ctx, platform_to_native_linkables, get_module_from_target, is_packaging_native_libs_as_assets_supported)
    for module, native_linkable_assets in stripped_native_linkable_module_assets_map.items():
        native_lib_module_assets_map.setdefault(module, []).append(native_linkable_assets)

    root_module_metadata_srcs = {}
    root_module_compressed_lib_srcs = {}
    non_root_module_metadata_srcs = {}
    non_root_module_compressed_lib_srcs = {}
    assets_for_primary_apk = filter(None, [native_lib_assets_for_primary_apk, stripped_native_linkable_assets_for_primary_apk])
    if assets_for_primary_apk:
        metadata_file, native_library_paths = _get_native_libs_as_assets_metadata(ctx, assets_for_primary_apk, ROOT_MODULE)
        root_module_metadata_srcs[paths.join(_get_native_libs_as_assets_dir(ROOT_MODULE), "metadata.txt")] = metadata_file
        if ctx.attrs.compress_asset_libraries:
            compressed_lib_dir = _get_compressed_native_libs_as_assets(ctx, assets_for_primary_apk, native_library_paths, ROOT_MODULE)
            root_module_compressed_lib_srcs[_get_native_libs_as_assets_dir(ROOT_MODULE)] = compressed_lib_dir

            # Since we're storing these as compressed assets, we need to ignore the uncompressed libs.
            native_lib_assets_for_primary_apk = None
            stripped_native_linkable_assets_for_primary_apk = None

    for module, native_lib_assets in native_lib_module_assets_map.items():
        metadata_file, native_library_paths = _get_native_libs_as_assets_metadata(ctx, native_lib_assets, module)
        non_root_module_metadata_srcs[paths.join(_get_native_libs_as_assets_dir(module), "libs.txt")] = metadata_file
        compressed_lib_dir = _get_compressed_native_libs_as_assets(ctx, native_lib_assets, native_library_paths, module)
        non_root_module_compressed_lib_srcs[_get_native_libs_as_assets_dir(module)] = compressed_lib_dir

    combined_native_libs = ctx.actions.declare_output("combined_native_libs", dir = True)
    native_libs_metadata = ctx.actions.declare_output("native_libs_metadata.txt")
    ctx.actions.run(cmd_args([
        ctx.attrs._android_toolchain[AndroidToolchainInfo].combine_native_library_dirs[RunInfo],
        "--output-dir",
        combined_native_libs.as_output(),
        "--library-dirs",
        native_libs,
        stripped_native_linkables,
        "--metadata-file",
        native_libs_metadata.as_output(),
    ]), category = "combine_native_libs")

    combined_native_libs_always_in_primary_apk = ctx.actions.declare_output("combined_native_libs_always_in_primary_apk", dir = True)
    ctx.actions.run(cmd_args([
        ctx.attrs._android_toolchain[AndroidToolchainInfo].combine_native_library_dirs[RunInfo],
        "--output-dir",
        combined_native_libs_always_in_primary_apk.as_output(),
        "--library-dirs",
        native_libs_always_in_primary_apk,
        stripped_native_linkables_always_in_primary_apk,
    ]), category = "combine_native_libs_always_in_primary_apk")

    return _NativeLibsAndAssetsInfo(
        native_libs = combined_native_libs,
        native_libs_metadata = native_libs_metadata,
        native_libs_always_in_primary_apk = combined_native_libs_always_in_primary_apk,
        native_lib_assets_for_primary_apk = native_lib_assets_for_primary_apk,
        stripped_native_linkable_assets_for_primary_apk = stripped_native_linkable_assets_for_primary_apk,
        root_module_metadata_assets = ctx.actions.symlinked_dir("root_module_metadata_assets", root_module_metadata_srcs),
        root_module_compressed_lib_assets = ctx.actions.symlinked_dir("root_module_compressed_lib_assets", root_module_compressed_lib_srcs),
        non_root_module_metadata_assets = ctx.actions.symlinked_dir("non_root_module_metadata_assets", non_root_module_metadata_srcs),
        non_root_module_compressed_lib_assets = ctx.actions.symlinked_dir("non_root_module_compressed_lib_assets", non_root_module_compressed_lib_srcs),
    )

def _filter_prebuilt_native_library_dir(
        ctx: AnalysisContext,
        native_libs: list["PrebuiltNativeLibraryDir"],
        identifier: str,
        package_as_assets: bool = False,
        module: str = ROOT_MODULE) -> "artifact":
    cpu_filters = ctx.attrs.cpu_filters or CPU_FILTER_TO_ABI_DIRECTORY.keys()
    abis = [CPU_FILTER_TO_ABI_DIRECTORY[cpu] for cpu in cpu_filters]
    filter_tool = ctx.attrs._android_toolchain[AndroidToolchainInfo].filter_prebuilt_native_library_dir[RunInfo]
    native_libs_dirs = [native_lib.dir for native_lib in native_libs]
    native_libs_dirs_file = ctx.actions.write("{}_list.txt".format(identifier), native_libs_dirs)
    base_output_dir = ctx.actions.declare_output(identifier, dir = True)
    output_dir = base_output_dir.project(_get_native_libs_as_assets_dir(module)) if package_as_assets else base_output_dir
    ctx.actions.run(
        cmd_args([filter_tool, native_libs_dirs_file, output_dir.as_output(), "--abis"] + abis).hidden(native_libs_dirs),
        category = "filter_prebuilt_native_library_dir",
        identifier = identifier,
    )

    return base_output_dir

def _get_native_linkables(
        ctx: AnalysisContext,
        platform_to_native_linkables: dict[str, dict[str, "SharedLibrary"]],
        get_module_from_target: "function",
        package_native_libs_as_assets_enabled: bool) -> ("artifact", "artifact", ["artifact", None], dict[str, "artifact"]):
    stripped_native_linkables_srcs = {}
    stripped_native_linkables_always_in_primary_apk_srcs = {}
    stripped_native_linkable_assets_for_primary_apk_srcs = {}
    stripped_native_linkable_module_assets_srcs = {}

    cpu_filters = ctx.attrs.cpu_filters
    for platform, native_linkables in platform_to_native_linkables.items():
        if cpu_filters and platform not in cpu_filters and platform != CPU_FILTER_FOR_PRIMARY_PLATFORM:
            fail("Platform `{}` is not in the CPU filters `{}`".format(platform, cpu_filters))

        abi_directory = CPU_FILTER_TO_ABI_DIRECTORY[platform]
        for so_name, native_linkable in native_linkables.items():
            native_linkable_target = str(native_linkable.label.raw_target())
            module = get_module_from_target(native_linkable_target)

            if not is_root_module(module):
                expect(not native_linkable.for_primary_apk, "{} which is marked as needing to be in the primary APK cannot be included in non-root-module {}".format(native_linkable_target, module))
                so_name_path = paths.join(_get_native_libs_as_assets_dir(module), abi_directory, so_name)
                stripped_native_linkable_module_assets_srcs.setdefault(module, {})[so_name_path] = native_linkable.stripped_lib
            elif native_linkable.can_be_asset and package_native_libs_as_assets_enabled:
                expect(not native_linkable.for_primary_apk, "{} which is marked as needing to be in the primary APK cannot be an asset".format(native_linkable_target))
                so_name_path = paths.join(_get_native_libs_as_assets_dir(module), abi_directory, so_name)
                stripped_native_linkable_assets_for_primary_apk_srcs[so_name_path] = native_linkable.stripped_lib
            else:
                so_name_path = paths.join(abi_directory, so_name)
                if native_linkable.for_primary_apk:
                    stripped_native_linkables_always_in_primary_apk_srcs[so_name_path] = native_linkable.stripped_lib
                else:
                    stripped_native_linkables_srcs[so_name_path] = native_linkable.stripped_lib

    stripped_native_linkables = ctx.actions.symlinked_dir(
        "stripped_native_linkables",
        stripped_native_linkables_srcs,
    )
    stripped_native_linkables_always_in_primary_apk = ctx.actions.symlinked_dir(
        "stripped_native_linkables_always_in_primary_apk",
        stripped_native_linkables_always_in_primary_apk_srcs,
    )
    stripped_native_linkable_assets_for_primary_apk = ctx.actions.symlinked_dir(
        "stripped_native_linkables_assets_for_primary_apk",
        stripped_native_linkable_assets_for_primary_apk_srcs,
    ) if stripped_native_linkable_assets_for_primary_apk_srcs else None
    stripped_native_linkable_module_assets_map = {}
    for module, srcs in stripped_native_linkable_module_assets_srcs.items():
        stripped_native_linkable_module_assets_map[module] = ctx.actions.symlinked_dir(
            "stripped_native_linkable_assets_for_module_{}".format(module),
            srcs,
        )

    return (
        stripped_native_linkables,
        stripped_native_linkables_always_in_primary_apk,
        stripped_native_linkable_assets_for_primary_apk,
        stripped_native_linkable_module_assets_map,
    )

def _get_native_libs_as_assets_metadata(
        ctx: AnalysisContext,
        native_lib_assets: list["artifact"],
        module: str) -> ("artifact", "artifact"):
    native_lib_assets_file = ctx.actions.write("{}/native_lib_assets".format(module), [cmd_args([native_lib_asset, _get_native_libs_as_assets_dir(module)], delimiter = "/") for native_lib_asset in native_lib_assets])
    metadata_output = ctx.actions.declare_output("{}/native_libs_as_assets_metadata.txt".format(module))
    native_library_paths = ctx.actions.declare_output("{}/native_libs_as_assets_paths.txt".format(module))
    metadata_cmd = cmd_args([
        ctx.attrs._android_toolchain[AndroidToolchainInfo].native_libs_as_assets_metadata[RunInfo],
        "--native-library-dirs",
        native_lib_assets_file,
        "--metadata-output",
        metadata_output.as_output(),
        "--native-library-paths-output",
        native_library_paths.as_output(),
    ]).hidden(native_lib_assets)
    ctx.actions.run(metadata_cmd, category = "get_native_libs_as_assets_metadata", identifier = module)
    return metadata_output, native_library_paths

def _get_compressed_native_libs_as_assets(
        ctx: AnalysisContext,
        native_lib_assets: list["artifact"],
        native_library_paths: "artifact",
        module: str) -> "artifact":
    output_dir = ctx.actions.declare_output("{}/compressed_native_libs_as_assets_dir".format(module))
    compressed_libraries_cmd = cmd_args([
        ctx.attrs._android_toolchain[AndroidToolchainInfo].compress_libraries[RunInfo],
        "--libraries",
        native_library_paths,
        "--output-dir",
        output_dir.as_output(),
        "--compression-type",
        ctx.attrs.asset_compression_algorithm or "xz",
        "--xz-compression-level",
        str(ctx.attrs.xz_compression_level),
    ]).hidden(native_lib_assets)
    ctx.actions.run(compressed_libraries_cmd, category = "compress_native_libs_as_assets", identifier = module)
    return output_dir

def _get_native_libs_as_assets_dir(module: str) -> str:
    return "assets/{}".format("lib" if is_root_module(module) else module)
