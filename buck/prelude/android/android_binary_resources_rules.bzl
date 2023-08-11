# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:resources.bzl", "gather_resources")
load("@prelude//android:aapt2_link.bzl", "get_aapt2_link", "get_module_manifest_in_proto_format")
load("@prelude//android:android_manifest.bzl", "generate_android_manifest")
load("@prelude//android:android_providers.bzl", "AndroidBinaryResourcesInfo", "AndroidResourceInfo", "ExopackageResourcesInfo")
load("@prelude//android:android_resource.bzl", "aapt2_compile")
load("@prelude//android:android_toolchain.bzl", "AndroidToolchainInfo")
load("@prelude//android:r_dot_java.bzl", "generate_r_dot_javas")
load("@prelude//android:voltron.bzl", "ROOT_MODULE", "get_apk_module_graph_info", "is_root_module")
load("@prelude//utils:set.bzl", "set_type")  # @unused Used as a type
load("@prelude//utils:utils.bzl", "expect")
load("@prelude//decls/android_rules.bzl", "RType")

def get_android_binary_resources_info(
        ctx: AnalysisContext,
        deps: list[Dependency],
        android_packageable_info: "AndroidPackageableInfo",
        java_packaging_deps: list["JavaPackagingDep"],
        use_proto_format: bool,
        referenced_resources_lists: list["artifact"],
        apk_module_graph_file: ["artifact", None] = None,
        manifest_entries: dict = {},
        resource_infos_to_exclude: [set_type, None] = None,
        generate_strings_and_ids_separately: [bool, None] = True,
        aapt2_min_sdk: [str, None] = None,
        aapt2_preferred_density: [str, None] = None) -> "AndroidBinaryResourcesInfo":
    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]
    unfiltered_resource_infos = [
        resource_info
        for resource_info in list(android_packageable_info.resource_infos.traverse() if android_packageable_info.resource_infos else [])
        if not (resource_infos_to_exclude and resource_infos_to_exclude.contains(resource_info.raw_target))
    ]
    resource_infos, override_symbols, string_files_list, string_files_res_dirs = _maybe_filter_resources(
        ctx,
        unfiltered_resource_infos,
        android_toolchain,
    )

    android_manifest = _get_manifest(ctx, android_packageable_info, manifest_entries)

    non_proto_format_aapt2_link_info, proto_format_aapt2_link_info = get_aapt2_link(
        ctx,
        ctx.attrs._android_toolchain[AndroidToolchainInfo],
        resource_infos,
        android_manifest,
        includes_vector_drawables = getattr(ctx.attrs, "includes_vector_drawables", False),
        no_auto_version = getattr(ctx.attrs, "no_auto_version_resources", False),
        no_version_transitions = getattr(ctx.attrs, "no_version_transitions_resources", False),
        no_auto_add_overlay = getattr(ctx.attrs, "no_auto_add_overlay_resources", False),
        no_resource_removal = True,
        package_id_offset = 0,
        should_keep_raw_values = getattr(ctx.attrs, "aapt2_keep_raw_values", False),
        resource_stable_ids = getattr(ctx.attrs, "resource_stable_ids", None),
        compiled_resource_apks = [],
        additional_aapt2_params = getattr(ctx.attrs, "additional_aapt_params", []),
        extra_filtered_resources = getattr(ctx.attrs, "extra_filtered_resources", []),
        locales = getattr(ctx.attrs, "locales", []) or getattr(ctx.attrs, "locales_for_binary_resources", []),
        filter_locales = getattr(ctx.attrs, "aapt2_locale_filtering", False) or bool(getattr(ctx.attrs, "locales_for_binary_resources", [])),
        min_sdk = aapt2_min_sdk,
        preferred_density = aapt2_preferred_density,
    )

    module_manifests = _get_module_manifests(
        ctx,
        android_packageable_info,
        manifest_entries,
        apk_module_graph_file,
        use_proto_format,
        non_proto_format_aapt2_link_info.primary_resources_apk,
    )

    aapt2_link_info = proto_format_aapt2_link_info if use_proto_format else non_proto_format_aapt2_link_info

    prebuilt_jars = [packaging_dep.jar for packaging_dep in java_packaging_deps if packaging_dep.is_prebuilt_jar]

    cxx_resources = _get_cxx_resources(ctx, deps)
    is_exopackaged_enabled_for_resources = "resources" in getattr(ctx.attrs, "exopackage_modes", [])
    primary_resources_apk, exopackaged_assets, exopackaged_assets_hash = _merge_assets(
        ctx,
        is_exopackaged_enabled_for_resources,
        aapt2_link_info.primary_resources_apk,
        resource_infos,
        cxx_resources,
    )

    if is_exopackaged_enabled_for_resources:
        r_dot_txt = ctx.actions.declare_output("after_exo/R.txt")
        primary_resources_apk = ctx.actions.declare_output("after_exo/primary_resources_apk.apk")
        exo_resources = ctx.actions.declare_output("exo_resources.apk")
        exo_resources_hash = ctx.actions.declare_output("exo_resources.apk.hash")
        ctx.actions.run(cmd_args([
            android_toolchain.exo_resources_rewriter[RunInfo],
            "--original-r-dot-txt",
            aapt2_link_info.r_dot_txt,
            "--new-r-dot-txt",
            r_dot_txt.as_output(),
            "--original-primary-apk-resources",
            aapt2_link_info.primary_resources_apk,
            "--new-primary-apk-resources",
            primary_resources_apk.as_output(),
            "--exo-resources",
            exo_resources.as_output(),
            "--exo-resources-hash",
            exo_resources_hash.as_output(),
            "--zipalign-tool",
            android_toolchain.zipalign[RunInfo],
        ]), category = "write_exo_resources")

        exopackage_info = ExopackageResourcesInfo(
            assets = exopackaged_assets,
            assets_hash = exopackaged_assets_hash,
            res = exo_resources,
            res_hash = exo_resources_hash,
        )
    else:
        exopackage_info = None
        r_dot_txt = aapt2_link_info.r_dot_txt

    override_symbols_paths = [override_symbols] if override_symbols else []
    resources = [resource for resource in resource_infos if resource.res != None]
    r_dot_javas = [] if len(resources) == 0 else generate_r_dot_javas(
        ctx,
        ctx.attrs._android_toolchain[AndroidToolchainInfo].merge_android_resources[RunInfo],
        resources,
        get_effective_banned_duplicate_resource_types(
            getattr(ctx.attrs, "duplicate_resource_behavior", "allow_by_default"),
            getattr(ctx.attrs, "allowed_duplicate_resource_types", []),
            getattr(ctx.attrs, "banned_duplicate_resource_types", []),
        ),
        [r_dot_txt],
        override_symbols_paths,
        getattr(ctx.attrs, "duplicate_resource_whitelist", None),
        getattr(ctx.attrs, "resource_union_package", None),
        referenced_resources_lists,
        generate_strings_and_ids_separately = generate_strings_and_ids_separately,
    )
    string_source_map = _maybe_generate_string_source_map(
        ctx.actions,
        getattr(ctx.attrs, "build_string_source_map", False),
        resources,
        android_toolchain,
    )
    voltron_string_source_map = _maybe_generate_string_source_map(
        ctx.actions,
        getattr(ctx.attrs, "is_voltron_language_pack_enabled", False),
        resources,
        android_toolchain,
        is_voltron_string_source_map = True,
    )

    packaged_string_assets = _maybe_package_strings_as_assets(
        ctx,
        string_files_list,
        string_files_res_dirs,
        r_dot_txt,
        android_toolchain,
    )

    return AndroidBinaryResourcesInfo(
        exopackage_info = exopackage_info,
        manifest = android_manifest,
        module_manifests = module_manifests,
        packaged_string_assets = packaged_string_assets,
        primary_resources_apk = primary_resources_apk,
        proguard_config_file = aapt2_link_info.proguard_config_file,
        r_dot_javas = r_dot_javas,
        string_source_map = string_source_map,
        voltron_string_source_map = voltron_string_source_map,
        jar_files_that_may_contain_resources = prebuilt_jars,
        unfiltered_resource_infos = unfiltered_resource_infos,
    )

def _maybe_filter_resources(
        ctx: AnalysisContext,
        resources: list[AndroidResourceInfo.type],
        android_toolchain: AndroidToolchainInfo.type) -> (list[AndroidResourceInfo.type], ["artifact", None], ["artifact", None], list["artifact"]):
    resources_filter_strings = getattr(ctx.attrs, "resource_filter", [])
    resources_filter = _get_resources_filter(resources_filter_strings)
    resource_compression_mode = getattr(ctx.attrs, "resource_compression", "disabled")
    is_store_strings_as_assets = _is_store_strings_as_assets(resource_compression_mode)
    locales = getattr(ctx.attrs, "locales", None)
    use_aapt2_locale_filtering = getattr(ctx.attrs, "aapt2_locale_filtering", False)
    needs_resource_filtering_for_locales = locales != None and len(locales) > 0 and not use_aapt2_locale_filtering
    post_filter_resources_cmd = getattr(ctx.attrs, "post_filter_resources_cmd", None)

    needs_resource_filtering = (
        resources_filter != None or
        is_store_strings_as_assets or
        needs_resource_filtering_for_locales or
        post_filter_resources_cmd != None
    )

    if not needs_resource_filtering:
        return resources, None, None, []

    res_info_to_out_res_dir = {}
    voltron_res_info_to_out_res_dir = {}
    res_infos_with_no_res = []
    skip_crunch_pngs = getattr(ctx.attrs, "skip_crunch_pngs", None) or False
    is_voltron_language_pack_enabled = getattr(ctx.attrs, "is_voltron_language_pack_enabled", False)
    for i, resource in enumerate(resources):
        if resource.res == None:
            res_infos_with_no_res.append(resource)
        else:
            filtered_res = ctx.actions.declare_output("filtered_res_{}".format(i), dir = True)
            res_info_to_out_res_dir[resource] = filtered_res

            if is_voltron_language_pack_enabled:
                filtered_res_for_voltron = ctx.actions.declare_output("filtered_res_for_voltron_{}".format(i), dir = True)
                voltron_res_info_to_out_res_dir[resource] = filtered_res_for_voltron

    filter_resources_cmd = cmd_args(android_toolchain.filter_resources[RunInfo])
    in_res_dir_to_out_res_dir_dict = {
        in_res.res: out_res
        for in_res, out_res in res_info_to_out_res_dir.items()
    }
    in_res_dir_to_out_res_dir_map = ctx.actions.write_json("in_res_dir_to_out_res_dir_map", {"res_dir_map": in_res_dir_to_out_res_dir_dict})
    in_res_dirs = [in_res.res for in_res in res_info_to_out_res_dir.keys()]
    filter_resources_cmd.hidden(in_res_dirs)
    filter_resources_cmd.hidden([out_res.as_output() for out_res in res_info_to_out_res_dir.values()])
    filter_resources_cmd.add([
        "--in-res-dir-to-out-res-dir-map",
        in_res_dir_to_out_res_dir_map,
    ])

    if is_voltron_language_pack_enabled:
        voltron_in_res_dir_to_out_res_dir_dict = {
            in_res.res: out_res
            for in_res, out_res in voltron_res_info_to_out_res_dir.items()
        }
        voltron_in_res_dir_to_out_res_dir_map = ctx.actions.write_json("voltron_in_res_dir_to_out_res_dir_map", {"res_dir_map": voltron_in_res_dir_to_out_res_dir_dict})
        filter_resources_cmd.hidden([out_res.as_output() for out_res in voltron_res_info_to_out_res_dir.values()])
        filter_resources_cmd.add([
            "--voltron-in-res-dir-to-out-res-dir-map",
            voltron_in_res_dir_to_out_res_dir_map,
        ])

    if resources_filter:
        filter_resources_cmd.add([
            "--target-densities",
            ",".join(resources_filter.densities),
        ])

    all_strings_files_list = None
    all_strings_files_res_dirs = []
    if is_store_strings_as_assets:
        all_strings_files_list = ctx.actions.declare_output("all_strings_files")
        all_strings_files_res_dirs = in_res_dirs
        filter_resources_cmd.add([
            "--enable-string-as-assets-filtering",
            "--string-files-list-output",
            all_strings_files_list.as_output(),
        ])

        packaged_locales = getattr(ctx.attrs, "packaged_locales", [])
        if packaged_locales:
            filter_resources_cmd.add([
                "--packaged-locales",
                ",".join(packaged_locales),
            ])

        not_filtered_string_dirs = [resource.res for resource in resources if not resource.allow_strings_as_assets_resource_filtering]
        if not_filtered_string_dirs:
            filter_resources_cmd.add([
                "--not-filtered-string-dirs",
                ctx.actions.write("not_filtered_string_dirs", not_filtered_string_dirs),
            ])

    if needs_resource_filtering_for_locales:
        filter_resources_cmd.add([
            "--locales",
            ",".join(locales),
        ])

    override_symbols_artifact = None
    if post_filter_resources_cmd != None:
        override_symbols_artifact = ctx.actions.declare_output("post_filter_resources_cmd/R.json")
        filter_resources_cmd.add([
            "--post-filter-resources-cmd",
            post_filter_resources_cmd,
            "--post-filter-resources-cmd-override-symbols-output",
            override_symbols_artifact.as_output(),
        ])

    ctx.actions.run(
        filter_resources_cmd,
        local_only = "run_post_filter_resources_cmd_locally" in ctx.attrs.labels,
        category = "filter_resources",
    )

    filtered_resource_infos = []
    for i, resource in enumerate(resources):
        if resource.res == None:
            continue

        filtered_res = res_info_to_out_res_dir[resource]
        filtered_aapt2_compile_output = aapt2_compile(
            ctx,
            filtered_res,
            android_toolchain,
            skip_crunch_pngs = skip_crunch_pngs,
            identifier = "filtered_res_{}".format(i),
        )
        filtered_resource = AndroidResourceInfo(
            raw_target = resource.raw_target,
            aapt2_compile_output = filtered_aapt2_compile_output,
            assets = resource.assets,
            manifest_file = resource.manifest_file,
            r_dot_java_package = resource.r_dot_java_package,
            res = filtered_res,
            text_symbols = resource.text_symbols,
        )
        filtered_resource_infos.append(filtered_resource)

    return (
        res_infos_with_no_res + filtered_resource_infos,
        override_symbols_artifact,
        all_strings_files_list,
        all_strings_files_res_dirs,
    )

ResourcesFilter = record(
    densities = [str],
    downscale = bool,
)

def _get_resources_filter(resources_filter_strings: list[str]) -> [ResourcesFilter.type, None]:
    if not resources_filter_strings:
        return None

    densities = [resources_filter_string for resources_filter_string in resources_filter_strings if resources_filter_string != "downscale"]
    if not densities:
        return None

    downscale = len(densities) < len(resources_filter_strings)
    return ResourcesFilter(densities = densities, downscale = downscale)

def _maybe_generate_string_source_map(
        actions: "actions",
        should_build_source_string_map: bool,
        resource_infos: list[AndroidResourceInfo.type],
        android_toolchain: AndroidToolchainInfo.type,
        is_voltron_string_source_map: bool = False) -> ["artifact", None]:
    if not should_build_source_string_map or len(resource_infos) == 0:
        return None

    prefix = "voltron_" if is_voltron_string_source_map else ""
    res_dirs = [resource_info.res for resource_info in resource_infos]
    output = actions.declare_output("{}string_source_map".format(prefix), dir = True)
    res_dirs_file = actions.write("resource_dirs_for_{}string_source_map".format(prefix), res_dirs)
    generate_string_source_map_cmd = cmd_args([
        android_toolchain.copy_string_resources[RunInfo],
        "--res-dirs",
        res_dirs_file,
        "--output",
        output.as_output(),
    ]).hidden(res_dirs)

    if is_voltron_string_source_map:
        generate_string_source_map_cmd.add("--is-voltron")

    actions.run(generate_string_source_map_cmd, category = "generate_{}string_source_map".format(prefix))

    return output

def _maybe_package_strings_as_assets(
        ctx: AnalysisContext,
        string_files_list: ["artifact", None],
        string_files_res_dirs: list["artifact"],
        r_dot_txt: "artifact",
        android_toolchain: AndroidToolchainInfo.type) -> ["artifact", None]:
    resource_compression_mode = getattr(ctx.attrs, "resource_compression", "disabled")
    is_store_strings_as_assets = _is_store_strings_as_assets(resource_compression_mode)
    expect(is_store_strings_as_assets == (string_files_list != None))

    if not is_store_strings_as_assets:
        return None

    string_assets_dir = ctx.actions.declare_output("package_strings_as_assets/string_assets", dir = True)
    string_assets_zip = ctx.actions.declare_output("package_strings_as_assets/string_assets_zip.zip")
    all_locales_string_assets_zip = ctx.actions.declare_output("package_strings_as_assets/all_locales_string_assets_zip.zip")

    locales = getattr(ctx.attrs, "locales", [])

    package_strings_as_assets_cmd = cmd_args([
        android_toolchain.package_strings_as_assets[RunInfo],
        "--string-files-list",
        string_files_list,
        "--r-dot-txt",
        r_dot_txt,
        "--string-assets-dir",
        string_assets_dir.as_output(),
        "--string-assets-zip",
        string_assets_zip.as_output(),
        "--all-locales-string-assets-zip",
        all_locales_string_assets_zip.as_output(),
    ]).hidden(string_files_res_dirs)

    if locales:
        package_strings_as_assets_cmd.add("--locales", ",".join(locales))

    ctx.actions.run(package_strings_as_assets_cmd, category = "package_strings_as_assets")

    return string_assets_zip

def _get_manifest(
        ctx: AnalysisContext,
        android_packageable_info: "AndroidPackageableInfo",
        manifest_entries: dict) -> "artifact":
    robolectric_manifest = getattr(ctx.attrs, "robolectric_manifest", None)
    if robolectric_manifest:
        return robolectric_manifest

    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]
    if ctx.attrs.manifest:
        expect(ctx.attrs.manifest_skeleton == None, "Only one of manifest and manifest_skeleton should be declared")
        if type(ctx.attrs.manifest) == "dependency":
            android_manifest = ctx.attrs.manifest[DefaultInfo].default_outputs[0]
        else:
            android_manifest = ctx.attrs.manifest
    else:
        expect(ctx.attrs.manifest_skeleton != None, "Must declare one of manifest and manifest_skeleton")
        if type(ctx.attrs.manifest_skeleton) == "dependency":
            manifest_skeleton = ctx.attrs.manifest_skeleton[DefaultInfo].default_outputs[0]
        else:
            manifest_skeleton = ctx.attrs.manifest_skeleton

        android_manifest, _ = generate_android_manifest(
            ctx,
            android_toolchain.generate_manifest[RunInfo],
            manifest_skeleton,
            ROOT_MODULE,
            android_packageable_info.manifests,
            manifest_entries.get("placeholders", {}),
        )

    if android_toolchain.set_application_id_to_specified_package:
        android_manifest_with_replaced_application_id = ctx.actions.declare_output("android_manifest_with_replaced_application_id")
        replace_application_id_placeholders_cmd = cmd_args([
            ctx.attrs._android_toolchain[AndroidToolchainInfo].replace_application_id_placeholders[RunInfo],
            "--manifest",
            android_manifest,
            "--output",
            android_manifest_with_replaced_application_id.as_output(),
        ])
        if android_toolchain.should_run_sanity_check_for_placeholders:
            replace_application_id_placeholders_cmd.add("--sanity-check-placeholders")

        ctx.actions.run(replace_application_id_placeholders_cmd, category = "replace_application_id_placeholders")
        return android_manifest_with_replaced_application_id
    else:
        return android_manifest

def _get_module_manifests(
        ctx: AnalysisContext,
        android_packageable_info: "AndroidPackageableInfo",
        manifest_entries: dict,
        apk_module_graph_file: ["artifact", None],
        use_proto_format: bool,
        primary_resources_apk: "artifact") -> list["artifact"]:
    if not apk_module_graph_file:
        return []

    if not ctx.attrs.module_manifest_skeleton:
        return []

    if type(ctx.attrs.module_manifest_skeleton) == "dependency":
        module_manifest_skeleton = ctx.attrs.module_manifest_skeleton[DefaultInfo].default_outputs[0]
    else:
        module_manifest_skeleton = ctx.attrs.module_manifest_skeleton

    android_toolchain = ctx.attrs._android_toolchain[AndroidToolchainInfo]

    module_manifests_dir = ctx.actions.declare_output("module_manifests_dir", dir = True)
    android_manifests = list(android_packageable_info.manifests.traverse()) if android_packageable_info.manifests else []

    def get_manifests_modular(ctx: AnalysisContext, artifacts, outputs):
        apk_module_graph_info = get_apk_module_graph_info(ctx, apk_module_graph_file, artifacts)
        get_module_from_target = apk_module_graph_info.target_to_module_mapping_function
        module_to_manifests = {}
        for android_manifest in android_manifests:
            module_name = get_module_from_target(str(android_manifest.target_label))
            if not is_root_module(module_name):
                module_to_manifests.setdefault(module_name, []).append(android_manifest.manifest)

        merged_module_manifests = {}
        for module_name in apk_module_graph_info.module_list:
            if is_root_module(module_name):
                continue

            merged_module_manifest, _ = generate_android_manifest(
                ctx,
                android_toolchain.generate_manifest[RunInfo],
                module_manifest_skeleton,
                module_name,
                module_to_manifests.get(module_name, []),
                manifest_entries.get("placeholders", {}),
            )

            if use_proto_format:
                merged_module_manifest = get_module_manifest_in_proto_format(
                    ctx,
                    android_toolchain,
                    merged_module_manifest,
                    primary_resources_apk,
                    module_name,
                )

            merged_module_manifests["assets/{}/AndroidManifest.xml".format(module_name)] = merged_module_manifest

        ctx.actions.symlinked_dir(outputs[module_manifests_dir], merged_module_manifests)

    ctx.actions.dynamic_output(
        dynamic = [apk_module_graph_file],
        inputs = [],
        outputs = [module_manifests_dir],
        f = get_manifests_modular,
    )

    return [module_manifests_dir]

# Returns the "primary resources APK" (i.e. the resource that are packaged into the primary APK),
# and optionally an "exopackaged assets APK" and the hash for that APK.
def _merge_assets(
        ctx: AnalysisContext,
        is_exopackaged_enabled_for_resources: bool,
        base_apk: "artifact",
        resource_infos: list["AndroidResourceInfo"],
        cxx_resources: ["artifact", None]) -> ("artifact", ["artifact", None], ["artifact", None]):
    assets_dirs = [resource_info.assets for resource_info in resource_infos if resource_info.assets]
    if cxx_resources != None:
        assets_dirs.extend([cxx_resources])
    if len(assets_dirs) == 0:
        return base_apk, None, None

    merge_assets_cmd = cmd_args(ctx.attrs._android_toolchain[AndroidToolchainInfo].merge_assets[RunInfo])

    merged_assets_output = ctx.actions.declare_output("merged_assets.ap_")
    merge_assets_cmd.add(["--output-apk", merged_assets_output.as_output()])

    if is_exopackaged_enabled_for_resources:
        merged_assets_output_hash = ctx.actions.declare_output("merged_assets.ap_.hash")
        merge_assets_cmd.add(["--output-apk-hash", merged_assets_output_hash.as_output()])
    else:
        merge_assets_cmd.add(["--base-apk", base_apk])
        merged_assets_output_hash = None

    assets_dirs_file = ctx.actions.write("assets_dirs", assets_dirs)
    merge_assets_cmd.add(["--assets-dirs", assets_dirs_file])
    merge_assets_cmd.hidden(assets_dirs)

    ctx.actions.run(merge_assets_cmd, category = "merge_assets")

    if is_exopackaged_enabled_for_resources:
        return base_apk, merged_assets_output, merged_assets_output_hash
    else:
        return merged_assets_output, None, None

def get_effective_banned_duplicate_resource_types(
        duplicate_resource_behavior: str,
        allowed_duplicate_resource_types: list[str],
        banned_duplicate_resource_types: list[str]) -> list[str]:
    if duplicate_resource_behavior == "allow_by_default":
        expect(
            len(allowed_duplicate_resource_types) == 0,
            "Cannot set allowed_duplicate_resource_types if duplicate_resource_behaviour is allow_by_default",
        )
        return banned_duplicate_resource_types
    elif duplicate_resource_behavior == "ban_by_default":
        expect(
            len(banned_duplicate_resource_types) == 0,
            "Cannot set banned_duplicate_resource_types if duplicate_resource_behaviour is ban_by_default",
        )
        return [rtype for rtype in RType if rtype not in allowed_duplicate_resource_types]
    else:
        fail("Unrecognized duplicate_resource_behavior: {}".format(duplicate_resource_behavior))

def _get_cxx_resources(ctx: AnalysisContext, deps: list[Dependency]) -> ["artifact", None]:
    cxx_resources = gather_resources(
        label = ctx.label,
        resources = {},
        deps = deps,
    )

    symlink_tree_dict = {}
    resource_maps = cxx_resources.values()
    for resource_map in resource_maps:
        for name, (resource, _other) in resource_map.items():
            symlink_tree_dict["cxx-resources/{}".format(name)] = resource

    return ctx.actions.symlinked_dir("cxx_resources_dir", symlink_tree_dict) if symlink_tree_dict else None

def _is_store_strings_as_assets(resource_compression: str) -> bool:
    return resource_compression == "enabled_strings_only" or resource_compression == "enabled_with_strings_as_assets"
