# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:android_providers.bzl", "Aapt2LinkInfo", "RESOURCE_PRIORITY_LOW")

BASE_PACKAGE_ID = 0x7f
ZIP_NOTHING_TO_DO_EXIT_CODE = 12

def get_aapt2_link(
        ctx: AnalysisContext,
        android_toolchain: "AndroidToolchainInfo",
        resource_infos: list["AndroidResourceInfo"],
        android_manifest: "artifact",
        includes_vector_drawables: bool,
        no_auto_version: bool,
        no_version_transitions: bool,
        no_auto_add_overlay: bool,
        no_resource_removal: bool,
        should_keep_raw_values: bool,
        package_id_offset: int,
        resource_stable_ids: ["artifact", None],
        preferred_density: [str, None],
        min_sdk: [str, None],
        filter_locales: bool,
        locales: list[str],
        compiled_resource_apks: list["artifact"],
        additional_aapt2_params: list[str],
        extra_filtered_resources: list[str]) -> (Aapt2LinkInfo.type, Aapt2LinkInfo.type):
    link_infos = []
    for use_proto_format in [False, True]:
        if use_proto_format:
            identifier = "use_proto_format"
        else:
            identifier = "not_proto_format"

        aapt2_command = cmd_args(android_toolchain.aapt2)
        aapt2_command.add("link")

        # aapt2 only supports @ for -R or input files, not for all args, so we pass in all "normal"
        # args here.
        resources_apk = ctx.actions.declare_output("{}/resource-apk.ap_".format(identifier))
        aapt2_command.add(["-o", resources_apk.as_output()])
        proguard_config = ctx.actions.declare_output("{}/proguard_config.pro".format(identifier))
        aapt2_command.add(["--proguard", proguard_config.as_output()])

        # We don't need the R.java output, but aapt2 won't output R.txt unless we also request R.java.
        r_dot_java = ctx.actions.declare_output("{}/initial-rdotjava".format(identifier), dir = True)
        aapt2_command.add(["--java", r_dot_java.as_output()])
        r_dot_txt = ctx.actions.declare_output("{}/R.txt".format(identifier))
        aapt2_command.add(["--output-text-symbols", r_dot_txt.as_output()])

        aapt2_command.add(["--manifest", android_manifest])
        aapt2_command.add(["-I", android_toolchain.android_jar])

        if includes_vector_drawables:
            aapt2_command.add("--no-version-vectors")
        if no_auto_version:
            aapt2_command.add("--no-auto-version")
        if no_version_transitions:
            aapt2_command.add("--no-version-transitions")
        if not no_auto_add_overlay:
            aapt2_command.add("--auto-add-overlay")
        if use_proto_format:
            aapt2_command.add("--proto-format")
        if no_resource_removal:
            aapt2_command.add("--no-resource-removal")
        if should_keep_raw_values:
            aapt2_command.add("--keep-raw-values")
        if package_id_offset != 0:
            aapt2_command.add(["--package-id", "0x{}".format(BASE_PACKAGE_ID + package_id_offset)])
        if resource_stable_ids != None:
            aapt2_command.add(["--stable-ids", resource_stable_ids])
        if preferred_density != None:
            aapt2_command.add(["--preferred-density", preferred_density])
        if min_sdk != None:
            aapt2_command.add(["--min-sdk-version", min_sdk])
        if filter_locales and len(locales) > 0:
            aapt2_command.add("-c")

            # "NONE" means "en", update the list of locales
            aapt2_command.add(cmd_args([locale if locale != "NONE" else "en" for locale in locales], delimiter = ","))

        for compiled_resource_apk in compiled_resource_apks:
            aapt2_command.add(["-I", compiled_resource_apk])

        # put low priority resources first so that they get overwritten by higher priority resources
        low_priority_aapt2_compile_rules = []
        normal_priority_aapt2_compile_rules = []
        for resource_info in resource_infos:
            if resource_info.aapt2_compile_output:
                (low_priority_aapt2_compile_rules if resource_info.res_priority == RESOURCE_PRIORITY_LOW else normal_priority_aapt2_compile_rules).append(resource_info.aapt2_compile_output)
        aapt2_compile_rules = low_priority_aapt2_compile_rules + normal_priority_aapt2_compile_rules

        aapt2_compile_rules_args_file = ctx.actions.write("{}/aapt2_compile_rules_args_file".format(identifier), cmd_args(aapt2_compile_rules, delimiter = " "))
        aapt2_command.add("-R")
        aapt2_command.add(cmd_args(aapt2_compile_rules_args_file, format = "@{}"))
        aapt2_command.hidden(aapt2_compile_rules)

        aapt2_command.add(additional_aapt2_params)

        ctx.actions.run(aapt2_command, category = "aapt2_link", identifier = identifier)

        # The normal resource filtering apparatus is super slow, because it extracts the whole apk,
        # strips files out of it, then repackages it.
        #
        # This is a faster filtering step that just uses zip -d to remove entries from the archive.
        # It's also superbly dangerous.
        #
        # If zip -d returns that there was nothing to do, then we don't fail.
        if len(extra_filtered_resources) > 0:
            filtered_resources_apk = ctx.actions.declare_output("{}/filtered-resource-apk.ap_".format(identifier))
            filter_resources_sh_cmd = cmd_args([
                "sh",
                "-c",
                'cp "$1" "$2" && chmod 644 "$2"; zip -d "$2" "$3"; if [$? -eq $4]; then\nexit 0\nfi\nexit $?;',
                "--",
                resources_apk,
                filtered_resources_apk.as_output(),
                extra_filtered_resources,
                str(ZIP_NOTHING_TO_DO_EXIT_CODE),
            ])
            ctx.actions.run(filter_resources_sh_cmd, category = "aapt2_filter_resources", identifier = identifier)
            primary_resources_apk = filtered_resources_apk
        else:
            primary_resources_apk = resources_apk

        link_infos.append(Aapt2LinkInfo(
            primary_resources_apk = primary_resources_apk,
            proguard_config_file = proguard_config,
            r_dot_txt = r_dot_txt,
        ))

    return link_infos[0], link_infos[1]

def get_module_manifest_in_proto_format(
        ctx: AnalysisContext,
        android_toolchain: "AndroidToolchainInfo",
        android_manifest: "artifact",
        primary_resources_apk: "artifact",
        module_name: str) -> "artifact":
    aapt2_command = cmd_args(android_toolchain.aapt2)
    aapt2_command.add("link")

    # aapt2 only supports @ for -R or input files, not for all args, so we pass in all "normal"
    # args here.
    resources_apk = ctx.actions.declare_output("{}/resource-apk.ap_".format(module_name))
    aapt2_command.add(["-o", resources_apk.as_output()])
    aapt2_command.add(["--manifest", android_manifest])
    aapt2_command.add(["-I", android_toolchain.android_jar])
    aapt2_command.add(["-I", primary_resources_apk])
    aapt2_command.add("--proto-format")

    ctx.actions.run(aapt2_command, category = "aapt2_link", identifier = module_name)

    proto_manifest_dir = ctx.actions.declare_output("{}/proto_format_manifest".format(module_name))
    proto_manifest = proto_manifest_dir.project("AndroidManifest.xml")
    ctx.actions.run(
        cmd_args(["unzip", resources_apk, "AndroidManifest.xml", "-d", proto_manifest_dir.as_output()]),
        category = "unzip_proto_format_manifest",
        identifier = module_name,
    )

    return proto_manifest
