# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//java:class_to_srcs.bzl",
    "JavaClassToSourceMapInfo",  # @unused Used as a type
    "merge_class_to_source_map_from_jar",
)
load("@prelude//java:java_library.bzl", "build_java_library")
load("@prelude//java:java_providers.bzl", "get_all_java_packaging_deps_tset")
load("@prelude//java:java_toolchain.bzl", "JavaTestToolchainInfo", "JavaToolchainInfo")
load("@prelude//java/utils:java_utils.bzl", "get_path_separator")
load("@prelude//linking:shared_libraries.bzl", "SharedLibraryInfo", "merge_shared_libraries", "traverse_shared_library_info")
load("@prelude//utils:utils.bzl", "expect")
load("@prelude//test/inject_test_run_info.bzl", "inject_test_run_info")

def java_test_impl(ctx: AnalysisContext) -> list["provider"]:
    if ctx.attrs._build_only_native_code:
        return [DefaultInfo()]

    java_providers = build_java_library(ctx, ctx.attrs.srcs)
    external_runner_test_info = build_junit_test(ctx, java_providers.java_library_info, java_providers.java_packaging_info, java_providers.class_to_src_map)

    return inject_test_run_info(ctx, external_runner_test_info) + [
        java_providers.java_library_intellij_info,
        java_providers.java_library_info,
        java_providers.java_packaging_info,
        java_providers.template_placeholder_info,
        java_providers.default_info,
    ]

def build_junit_test(
        ctx: AnalysisContext,
        tests_java_library_info: "JavaLibraryInfo",
        tests_java_packaging_info: "JavaPackagingInfo",
        tests_class_to_source_info: [JavaClassToSourceMapInfo.type, None] = None,
        extra_cmds: list = [],
        extra_classpath_entries: list["artifact"] = []) -> ExternalRunnerTestInfo.type:
    java_test_toolchain = ctx.attrs._java_test_toolchain[JavaTestToolchainInfo]

    java = ctx.attrs.java[RunInfo] if ctx.attrs.java else ctx.attrs._java_toolchain[JavaToolchainInfo].java_for_tests

    cmd = [java] + extra_cmds + ctx.attrs.vm_args + ["-XX:-MaxFDLimit"]
    classpath = []

    if java_test_toolchain.use_java_custom_class_loader:
        cmd.append("-Djava.system.class.loader=" + java_test_toolchain.java_custom_class_loader_class)
        cmd.extend(java_test_toolchain.java_custom_class_loader_vm_args)
        classpath.append(java_test_toolchain.java_custom_class_loader_library_jar)

    classpath.extend(
        [java_test_toolchain.test_runner_library_jar] +
        [
            get_all_java_packaging_deps_tset(ctx, java_packaging_infos = [tests_java_packaging_info])
                .project_as_args("full_jar_args", ordering = "bfs"),
        ] +
        extra_classpath_entries,
    )

    if ctx.attrs.unbundled_resources_root:
        classpath.append(ctx.attrs.unbundled_resources_root)

    labels = ctx.attrs.labels or []
    run_from_cell_root = "buck2_run_from_cell_root" in labels
    uses_java8 = "run_with_java8" in labels

    classpath_args = cmd_args()
    if run_from_cell_root:
        classpath_args.relative_to(ctx.label.cell_root)

    if uses_java8:
        # Java 8 does not support using argfiles, and these tests can have huge classpaths so we need another
        # mechanism to write the classpath to a file.
        # We add "FileClassPathRunner" to the classpath, and then write a line-separated classpath file which we pass
        # to the "FileClassPathRunner" as a system variable. The "FileClassPathRunner" then loads all the jars
        # from that file onto the classpath, and delegates running the test to the junit test runner.
        cmd.extend(["-classpath", cmd_args(java_test_toolchain.test_runner_library_jar)])
        classpath_args.add(cmd_args(classpath))
        classpath_args_file = ctx.actions.write("classpath_args_file", classpath_args)
        cmd.append(cmd_args(classpath_args_file, format = "-Dbuck.classpath_file={}").hidden(classpath_args))
    else:
        # Java 9+ supports argfiles, so just write the classpath to an argsfile. "FileClassPathRunner" will delegate
        # immediately to the junit test runner.
        classpath_args.add("-classpath")
        classpath_args.add(cmd_args(classpath, delimiter = get_path_separator()))
        classpath_args_file = ctx.actions.write("classpath_args_file", classpath_args)
        cmd.append(cmd_args(classpath_args_file, format = "@{}").hidden(classpath_args))

    if (ctx.attrs.test_type == "junit5"):
        cmd.extend(java_test_toolchain.junit5_test_runner_main_class_args)
    elif (ctx.attrs.test_type == "testng"):
        cmd.extend(java_test_toolchain.testng_test_runner_main_class_args)
    else:
        cmd.extend(java_test_toolchain.junit_test_runner_main_class_args)

    if ctx.attrs.test_case_timeout_ms:
        cmd.extend(["--default_test_timeout", ctx.attrs.test_case_timeout_ms])

    expect(tests_java_library_info.library_output != None, "Built test library has no output, likely due to missing srcs")

    class_names = ctx.actions.declare_output("class_names")
    list_class_names_cmd = cmd_args([
        java_test_toolchain.list_class_names[RunInfo],
        "--jar",
        tests_java_library_info.library_output.full_library,
        "--sources",
        ctx.actions.write("sources.txt", ctx.attrs.srcs),
        "--output",
        class_names.as_output(),
    ]).hidden(ctx.attrs.srcs)
    ctx.actions.run(list_class_names_cmd, category = "list_class_names")

    cmd.extend(["--test-class-names-file", class_names])

    native_libs_env = _get_native_libs_env(ctx)
    env = {}
    for d in [ctx.attrs.env, native_libs_env]:
        for key, value in d.items():
            if key in env:
                fail("Duplicate key for java_test env: '{}'".format(key))
            env[key] = value

    if tests_class_to_source_info != None:
        transitive_class_to_src_map = merge_class_to_source_map_from_jar(
            actions = ctx.actions,
            name = ctx.attrs.name + ".transitive_class_to_src.json",
            java_test_toolchain = java_test_toolchain,
            relative_to = ctx.label.cell_root if run_from_cell_root else None,
            deps = [tests_class_to_source_info],
        )
        if run_from_cell_root:
            transitive_class_to_src_map = cmd_args(transitive_class_to_src_map).relative_to(ctx.label.cell_root)
        env["JACOCO_CLASSNAME_SOURCE_MAP"] = transitive_class_to_src_map

    test_info = ExternalRunnerTestInfo(
        type = "junit",
        command = cmd,
        env = env,
        labels = ctx.attrs.labels,
        contacts = ctx.attrs.contacts,
        run_from_project_root = not run_from_cell_root,
        use_project_relative_paths = not run_from_cell_root,
    )
    return test_info

def _get_native_libs_env(ctx: AnalysisContext) -> dict:
    if not ctx.attrs.use_cxx_libraries:
        return {}

    if ctx.attrs.cxx_library_whitelist:
        shared_library_infos = filter(None, [x.get(SharedLibraryInfo) for x in ctx.attrs.cxx_library_whitelist])
    else:
        shared_library_infos = filter(None, [x.get(SharedLibraryInfo) for x in ctx.attrs.deps])

    shared_library_info = merge_shared_libraries(
        ctx.actions,
        deps = shared_library_infos,
    )

    native_linkables = traverse_shared_library_info(shared_library_info)
    cxx_library_symlink_tree_dict = {so_name: shared_lib.lib.output for so_name, shared_lib in native_linkables.items()}
    cxx_library_symlink_tree = ctx.actions.symlinked_dir("cxx_library_symlink_tree", cxx_library_symlink_tree_dict)

    return {"BUCK_LD_SYMLINK_TREE": cxx_library_symlink_tree}
