# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//java:java_library.bzl", "compile_to_jar")
load("@prelude//java:java_providers.bzl", "JavaClasspathEntry", "JavaLibraryInfo", "derive_compiling_deps")
load("@prelude//utils:set.bzl", "set")

RDotJavaSourceCode = record(
    r_dot_java_source_code_dir = "artifact",
    r_dot_java_source_code_zipped = "artifact",
    strings_source_code_dir = ["artifact", None],
    strings_source_code_zipped = ["artifact", None],
    ids_source_code_dir = ["artifact", None],
    ids_source_code_zipped = ["artifact", None],
)

def get_dummy_r_dot_java(
        ctx: AnalysisContext,
        merge_android_resources_tool: RunInfo.type,
        android_resources: list["AndroidResourceInfo"],
        union_package: [str, None]) -> "JavaLibraryInfo":
    r_dot_java_source_code = _generate_r_dot_java_source_code(ctx, merge_android_resources_tool, android_resources, "dummy_r_dot_java", union_package = union_package)
    library_output = _generate_and_compile_r_dot_java(
        ctx,
        r_dot_java_source_code.r_dot_java_source_code_zipped,
        "dummy_r_dot_java",
    )
    return JavaLibraryInfo(
        compiling_deps = derive_compiling_deps(ctx.actions, library_output, []),
        library_output = library_output,
        output_for_classpath_macro = library_output.full_library,
    )

def generate_r_dot_javas(
        ctx: AnalysisContext,
        merge_android_resources_tool: RunInfo.type,
        android_resources: list["AndroidResourceInfo"],
        banned_duplicate_resource_types: list[str],
        uber_r_dot_txt_files: list["artifact"],
        override_symbols_paths: list["artifact"],
        duplicate_resources_allowlist: ["artifact", None],
        union_package: [str, None],
        referenced_resources_lists: list["artifact"],
        generate_strings_and_ids_separately: [bool, None] = True) -> list["JavaLibraryInfo"]:
    r_dot_java_source_code = _generate_r_dot_java_source_code(
        ctx,
        merge_android_resources_tool,
        android_resources,
        "r_dot_java",
        generate_strings_and_ids_separately = generate_strings_and_ids_separately,
        force_final_resources_ids = True,
        banned_duplicate_resource_types = banned_duplicate_resource_types,
        uber_r_dot_txt_files = uber_r_dot_txt_files,
        override_symbols_paths = override_symbols_paths,
        duplicate_resources_allowlist = duplicate_resources_allowlist,
        union_package = union_package,
        referenced_resources_lists = referenced_resources_lists,
    )

    main_library_output = _generate_and_compile_r_dot_java(
        ctx,
        r_dot_java_source_code.r_dot_java_source_code_zipped,
        "main_r_dot_java",
    )
    if generate_strings_and_ids_separately:
        strings_library_output = _generate_and_compile_r_dot_java(
            ctx,
            r_dot_java_source_code.strings_source_code_zipped,
            "strings_r_dot_java",
            remove_classes = [".R$"],
        )
        ids_library_output = _generate_and_compile_r_dot_java(
            ctx,
            r_dot_java_source_code.ids_source_code_zipped,
            "ids_r_dot_java",
            remove_classes = [".R$"],
        )
    else:
        strings_library_output = None
        ids_library_output = None

    return [JavaLibraryInfo(
        compiling_deps = derive_compiling_deps(ctx.actions, library_output, []),
        library_output = library_output,
        output_for_classpath_macro = library_output.full_library,
    ) for library_output in filter(None, [main_library_output, strings_library_output, ids_library_output])]

def _generate_r_dot_java_source_code(
        ctx: AnalysisContext,
        merge_android_resources_tool: RunInfo.type,
        android_resources: list["AndroidResourceInfo"],
        identifier: str,
        force_final_resources_ids = False,
        generate_strings_and_ids_separately = False,
        banned_duplicate_resource_types: list[str] = [],
        uber_r_dot_txt_files: list["artifact"] = [],
        override_symbols_paths: list["artifact"] = [],
        duplicate_resources_allowlist: ["artifact", None] = None,
        union_package: [str, None] = None,
        referenced_resources_lists: list["artifact"] = []) -> RDotJavaSourceCode.type:
    merge_resources_cmd = cmd_args(merge_android_resources_tool)

    r_dot_txt_info = cmd_args()
    deduped_android_resources = set([(android_resource.text_symbols, android_resource.r_dot_java_package) for android_resource in android_resources])
    for (text_symbols, r_dot_java_package) in deduped_android_resources.list():
        r_dot_txt_info.add(cmd_args([text_symbols, r_dot_java_package, "_"], delimiter = " "))  # pass target name

    r_dot_txt_info_file = ctx.actions.write("r_dot_txt_info_file_for_{}.txt".format(identifier), r_dot_txt_info)
    merge_resources_cmd.add(["--symbol-file-info", r_dot_txt_info_file])
    merge_resources_cmd.hidden([android_resource.r_dot_java_package for android_resource in android_resources])
    merge_resources_cmd.hidden([android_resource.text_symbols for android_resource in android_resources])

    output_dir = ctx.actions.declare_output("{}_source_code".format(identifier), dir = True)
    merge_resources_cmd.add(["--output-dir", output_dir.as_output()])
    output_dir_zipped = ctx.actions.declare_output("{}.src.zip".format(identifier))
    merge_resources_cmd.add(["--output-dir-zipped", output_dir_zipped.as_output()])

    if generate_strings_and_ids_separately:
        strings_output_dir = ctx.actions.declare_output("strings_source_code", dir = True)
        merge_resources_cmd.add(["--strings-output-dir", strings_output_dir.as_output()])
        strings_output_dir_zipped = ctx.actions.declare_output("strings.src.zip")
        merge_resources_cmd.add(["--strings-output-dir-zipped", strings_output_dir_zipped.as_output()])
        ids_output_dir = ctx.actions.declare_output("ids_source_code", dir = True)
        merge_resources_cmd.add(["--ids-output-dir", ids_output_dir.as_output()])
        ids_output_dir_zipped = ctx.actions.declare_output("ids.src.zip")
        merge_resources_cmd.add(["--ids-output-dir-zipped", ids_output_dir_zipped.as_output()])
    else:
        strings_output_dir = None
        strings_output_dir_zipped = None
        ids_output_dir = None
        ids_output_dir_zipped = None

    if force_final_resources_ids:
        merge_resources_cmd.add("--force-final-resource-ids")

    if len(banned_duplicate_resource_types) > 0:
        banned_duplicate_resource_types_file = ctx.actions.write("banned_duplicate_resource_types_file", banned_duplicate_resource_types)
        merge_resources_cmd.add(["--banned-duplicate-resource-types", banned_duplicate_resource_types_file])

    if len(uber_r_dot_txt_files) > 0:
        uber_r_dot_txt_files_list = ctx.actions.write("uber_r_dot_txt_files_list", uber_r_dot_txt_files)
        merge_resources_cmd.add(["--uber-r-dot-txt", uber_r_dot_txt_files_list])
        merge_resources_cmd.hidden(uber_r_dot_txt_files)

    if len(override_symbols_paths) > 0:
        override_symbols_paths_list = ctx.actions.write("override_symbols_paths_list", override_symbols_paths)
        merge_resources_cmd.add(["--override-symbols", override_symbols_paths_list])
        merge_resources_cmd.hidden(override_symbols_paths)

    if duplicate_resources_allowlist != None:
        merge_resources_cmd.add(["--duplicate-resource-allowlist-path", duplicate_resources_allowlist])

    if union_package != None:
        merge_resources_cmd.add(["--union-package", union_package])

    if referenced_resources_lists:
        referenced_resources_file = ctx.actions.write("referenced_resources_lists", referenced_resources_lists)
        merge_resources_cmd.add(["--referenced-resources-lists", referenced_resources_file])
        merge_resources_cmd.hidden(referenced_resources_lists)

    ctx.actions.run(merge_resources_cmd, category = "r_dot_java_merge_resources", identifier = identifier)

    return RDotJavaSourceCode(
        r_dot_java_source_code_dir = output_dir,
        r_dot_java_source_code_zipped = output_dir_zipped,
        strings_source_code_dir = strings_output_dir,
        strings_source_code_zipped = strings_output_dir_zipped,
        ids_source_code_dir = ids_output_dir,
        ids_source_code_zipped = ids_output_dir_zipped,
    )

def _generate_and_compile_r_dot_java(
        ctx: AnalysisContext,
        r_dot_java_source_code_zipped: "artifact",
        identifier: str,
        remove_classes: list[str] = []) -> JavaClasspathEntry.type:
    r_dot_java_out = ctx.actions.declare_output("{}.jar".format(identifier))

    compile_to_jar(
        ctx,
        output = r_dot_java_out,
        actions_identifier = identifier,
        javac_tool = None,
        srcs = [r_dot_java_source_code_zipped],
        remove_classes = remove_classes,
    )

    # Extracting an abi is unnecessary as there's not really anything to strip.
    outputs = JavaClasspathEntry(
        full_library = r_dot_java_out,
        abi = r_dot_java_out,
        abi_as_dir = None,
        required_for_source_only_abi = False,
    )

    return outputs
