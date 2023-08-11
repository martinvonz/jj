# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//java:java_providers.bzl",
    "JavaLibraryInfo",
    "make_compile_outputs",
)
load("@prelude//java:java_resources.bzl", "get_resources_map")
load("@prelude//java:java_toolchain.bzl", "AbiGenerationMode", "DepFiles")
load(
    "@prelude//jvm:cd_jar_creator_util.bzl",
    "OutputPaths",
    "TargetType",
    "add_output_paths_to_cmd_args",
    "base_qualified_name",
    "declare_prefixed_output",
    "define_output_paths",
    "encode_base_jar_command",
    "encode_jar_params",
    "generate_abi_jars",
    "get_compiling_deps_tset",
    "prepare_cd_exe",
    "prepare_final_jar",
    "setup_dep_files",
)
load("@prelude//kotlin:kotlin_utils.bzl", "get_kotlinc_compatible_target")
load("@prelude//utils:utils.bzl", "expect", "map_idx")

buckPaths = struct(
    configuredBuckOut = "buck-out/v2",
    includeTargetConfigHash = True,
)

def create_jar_artifact_kotlincd(
        actions: "actions",
        actions_identifier: [str, None],
        abi_generation_mode: [AbiGenerationMode.type, None],
        java_toolchain: "JavaToolchainInfo",
        kotlin_toolchain: "KotlinToolchainInfo",
        javac_tool: [str, "RunInfo", "artifact", None],
        label: Label,
        srcs: list["artifact"],
        remove_classes: list[str],
        resources: list["artifact"],
        resources_root: [str, None],
        ap_params: list["AnnotationProcessorParams"],
        plugin_params: ["PluginParams", None],
        source_level: int,
        target_level: int,
        deps: list[Dependency],
        required_for_source_only_abi: bool,
        source_only_abi_deps: list[Dependency],
        extra_arguments: list[str],
        additional_classpath_entries: list["artifact"],
        bootclasspath_entries: list["artifact"],
        is_building_android_binary: bool,
        friend_paths: list[Dependency],
        kotlin_compiler_plugins: dict,
        extra_kotlinc_arguments: list[str],
        extra_non_source_only_abi_kotlinc_arguments: list[str]) -> "JavaCompileOutputs":
    resources_map = get_resources_map(
        java_toolchain = java_toolchain,
        package = label.package,
        resources = resources,
        resources_root = resources_root,
    )

    expect(abi_generation_mode != AbiGenerationMode("source"), "abi_generation_mode: source is not supported in kotlincd")
    actual_abi_generation_mode = abi_generation_mode or AbiGenerationMode("class") if srcs else AbiGenerationMode("none")

    output_paths = define_output_paths(actions, actions_identifier, label)
    path_to_class_hashes_out = declare_prefixed_output(actions, actions_identifier, "classes.txt")

    should_create_class_abi = actual_abi_generation_mode == AbiGenerationMode("class") or not is_building_android_binary
    if should_create_class_abi:
        class_abi_jar = declare_prefixed_output(actions, actions_identifier, "class-abi.jar")
        class_abi_output_dir = declare_prefixed_output(actions, actions_identifier, "class_abi_dir", dir = True)
        jvm_abi_gen = output_paths.jar_parent.project("jvm-abi-gen.jar")
        should_use_jvm_abi_gen = True
    else:
        class_abi_jar = None
        class_abi_output_dir = None
        jvm_abi_gen = None
        should_use_jvm_abi_gen = False

    def encode_kotlin_extra_params(kotlin_compiler_plugins):
        return struct(
            extraClassPaths = bootclasspath_entries,
            standardLibraryClassPath = kotlin_toolchain.kotlin_stdlib[JavaLibraryInfo].library_output.full_library,
            annotationProcessingClassPath = kotlin_toolchain.annotation_processing_jar[JavaLibraryInfo].library_output.full_library,
            compilationTracerPlugin = kotlin_toolchain.compilation_tracer_plugin,
            qpldDotslash = kotlin_toolchain.qpld_dotslash,
            jvmAbiGenPlugin = kotlin_toolchain.jvm_abi_gen_plugin,
            kotlinCompilerPlugins = {plugin: {"params": plugin_options} if plugin_options else {} for plugin, plugin_options in kotlin_compiler_plugins.items()},
            kosabiPluginOptions = struct(
                kosabi_stubs_gen_plugin = kotlin_toolchain.kosabi_stubs_gen_plugin,
                kosabi_applicability_plugin = kotlin_toolchain.kosabi_applicability_plugin,
                kosabi_jvm_abi_gen_plugin = kotlin_toolchain.kosabi_jvm_abi_gen_plugin,
            ),
            friendPaths = [friend_path.library_output.abi for friend_path in map_idx(JavaLibraryInfo, friend_paths) if friend_path.library_output],
            kotlinHomeLibraries = kotlin_toolchain.kotlin_home_libraries,
            jvmTarget = get_kotlinc_compatible_target(str(target_level)),
            kosabiJvmAbiGenEarlyTerminationMessagePrefix = "exception: java.lang.RuntimeException: Terminating compilation. We're done with ABI.",
            kosabiSupportedKspProviders = kotlin_toolchain.kosabi_supported_ksp_providers,
            shouldUseCompilationTracer = kotlin_toolchain.should_use_compilation_tracer,
            shouldUseJvmAbiGen = should_use_jvm_abi_gen,
            shouldVerifySourceOnlyAbiConstraints = actual_abi_generation_mode == AbiGenerationMode("source_only"),
            shouldGenerateAnnotationProcessingStats = True,
            extraKotlincArguments = extra_kotlinc_arguments,
            extraNonSourceOnlyAbiKotlincArguments = extra_non_source_only_abi_kotlinc_arguments,
            shouldRemoveKotlinCompilerFromClassPath = True,
            depTrackerPlugin = kotlin_toolchain.track_class_usage_plugin,
        )

    kotlin_extra_params = encode_kotlin_extra_params(kotlin_compiler_plugins)

    compiling_deps_tset = get_compiling_deps_tset(actions, deps, additional_classpath_entries)

    # external javac does not support used classes
    track_class_usage = javac_tool == None

    def encode_library_command(
            output_paths: OutputPaths.type,
            path_to_class_hashes: "artifact",
            classpath_jars_tag: "artifact_tag") -> struct.type:
        target_type = TargetType("library")
        base_jar_command = encode_base_jar_command(
            javac_tool,
            target_type,
            output_paths,
            remove_classes,
            label,
            compiling_deps_tset,
            classpath_jars_tag,
            bootclasspath_entries,
            source_level,
            target_level,
            actual_abi_generation_mode,
            srcs,
            resources_map,
            ap_params = ap_params,
            plugin_params = plugin_params,
            extra_arguments = cmd_args(extra_arguments),
            source_only_abi_compiling_deps = [],
            track_class_usage = track_class_usage,
        )

        return struct(
            baseCommandParams = struct(
                withDownwardApi = True,
                hasAnnotationProcessing = True,
            ),
            libraryJarCommand = struct(
                kotlinExtraParams = kotlin_extra_params,
                baseJarCommand = base_jar_command,
                libraryJarBaseCommand = struct(
                    pathToClasses = output_paths.jar.as_output(),
                    rootOutput = output_paths.jar_parent.as_output(),
                    pathToClassHashes = path_to_class_hashes.as_output(),
                    annotationsPath = output_paths.annotations.as_output(),
                ),
            ),
        )

    def encode_abi_command(
            output_paths: OutputPaths.type,
            target_type: TargetType.type,
            classpath_jars_tag: "artifact_tag",
            source_only_abi_compiling_deps: list["JavaClasspathEntry"] = []) -> struct.type:
        base_jar_command = encode_base_jar_command(
            javac_tool,
            target_type,
            output_paths,
            remove_classes,
            label,
            compiling_deps_tset,
            classpath_jars_tag,
            bootclasspath_entries,
            source_level,
            target_level,
            actual_abi_generation_mode,
            srcs,
            resources_map,
            ap_params,
            plugin_params,
            cmd_args(extra_arguments),
            source_only_abi_compiling_deps = source_only_abi_compiling_deps,
            track_class_usage = True,
        )
        abi_params = encode_jar_params(remove_classes, output_paths)
        abi_command = struct(
            kotlinExtraParams = kotlin_extra_params,
            baseJarCommand = base_jar_command,
            abiJarParameters = abi_params,
        )

        return struct(
            baseCommandParams = struct(
                withDownwardApi = True,
            ),
            abiJarCommand = abi_command,
        )

    # buildifier: disable=uninitialized
    # buildifier: disable=unused-variable
    def define_kotlincd_action(
            category_prefix: str,
            actions_identifier: [str, None],
            encoded_command: struct.type,
            qualified_name: str,
            output_paths: OutputPaths.type,
            classpath_jars_tag: "artifact_tag",
            abi_dir: ["artifact", None],
            target_type: TargetType.type,
            path_to_class_hashes: ["artifact", None],
            source_only_abi_compiling_deps: list["JavaClasspathEntry"] = []):
        _unused = source_only_abi_compiling_deps

        proto = declare_prefixed_output(actions, actions_identifier, "jar_command.proto.json")
        proto_with_inputs = actions.write_json(proto, encoded_command, with_inputs = True)

        compiler = kotlin_toolchain.kotlinc[DefaultInfo].default_outputs[0]
        exe, local_only = prepare_cd_exe(
            qualified_name,
            java = java_toolchain.java[RunInfo],
            class_loader_bootstrapper = kotlin_toolchain.class_loader_bootstrapper,
            compiler = compiler,
            main_class = kotlin_toolchain.kotlincd_main_class,
            worker = kotlin_toolchain.kotlincd_worker[WorkerInfo],
            debug_port = kotlin_toolchain.kotlincd_debug_port,
            debug_target = kotlin_toolchain.kotlincd_debug_target,
            extra_jvm_args = kotlin_toolchain.kotlincd_jvm_args,
            extra_jvm_args_target = kotlin_toolchain.kotlincd_jvm_args_target,
        )

        args = cmd_args()
        args.add(
            "--action-id",
            qualified_name,
            "--command-file",
            proto_with_inputs,
        )

        if target_type == TargetType("library") and should_create_class_abi:
            args.add(
                "--full-library",
                output_paths.jar.as_output(),
                "--class-abi-output",
                class_abi_jar.as_output(),
                "--jvm-abi-gen-output",
                jvm_abi_gen.as_output(),
                "--abi-output-dir",
                class_abi_output_dir.as_output(),
            )

        if target_type == TargetType("source_abi") or target_type == TargetType("source_only_abi"):
            args.add(
                "--kotlincd-abi-output",
                output_paths.jar.as_output(),
                "--abi-output-dir",
                abi_dir.as_output(),
            )

        args = add_output_paths_to_cmd_args(args, output_paths, path_to_class_hashes)

        event_pipe_out = declare_prefixed_output(actions, actions_identifier, "events.data")

        dep_files = {}
        if srcs and (kotlin_toolchain.dep_files == DepFiles("per_jar") or kotlin_toolchain.dep_files == DepFiles("per_class")) and target_type == TargetType("library") and track_class_usage:
            used_classes_json_outputs = [
                output_paths.jar_parent.project("used-classes.json"),
                output_paths.jar_parent.project("kotlin-used-classes.json"),
            ]
            args = setup_dep_files(
                actions,
                actions_identifier,
                args,
                classpath_jars_tag,
                used_classes_json_outputs,
                compiling_deps_tset.project_as_args("abi_to_abi_dir") if kotlin_toolchain.dep_files == DepFiles("per_class") and compiling_deps_tset else None,
            )

            dep_files["classpath_jars"] = classpath_jars_tag

        actions.run(
            args,
            env = {
                "BUCK_CLASSPATH": compiler,
                "BUCK_EVENT_PIPE": event_pipe_out.as_output(),
                "JAVACD_ABSOLUTE_PATHS_ARE_RELATIVE_TO_CWD": "1",
            },
            category = "{}kotlincd_jar".format(category_prefix),
            identifier = actions_identifier,
            dep_files = dep_files,
            exe = exe,
            local_only = local_only,
            low_pass_filter = False,
            weight = 5,
        )

    library_classpath_jars_tag = actions.artifact_tag()
    command = encode_library_command(output_paths, path_to_class_hashes_out, library_classpath_jars_tag)
    define_kotlincd_action(
        category_prefix = "",
        actions_identifier = actions_identifier,
        encoded_command = command,
        qualified_name = base_qualified_name(label),
        output_paths = output_paths,
        classpath_jars_tag = library_classpath_jars_tag,
        abi_dir = class_abi_output_dir if should_create_class_abi else None,
        target_type = TargetType("library"),
        path_to_class_hashes = path_to_class_hashes_out,
    )

    final_jar = prepare_final_jar(
        actions = actions,
        actions_identifier = actions_identifier,
        output = None,
        output_paths = output_paths,
        additional_compiled_srcs = None,
        jar_builder = java_toolchain.jar_builder,
    )

    # kotlincd does not support source abi
    class_abi, _, source_only_abi, classpath_abi, classpath_abi_dir = generate_abi_jars(
        actions = actions,
        actions_identifier = actions_identifier,
        label = label,
        abi_generation_mode = actual_abi_generation_mode,
        additional_compiled_srcs = None,
        is_building_android_binary = is_building_android_binary,
        class_abi_generator = java_toolchain.class_abi_generator,
        final_jar = final_jar,
        compiling_deps_tset = compiling_deps_tset,
        source_only_abi_deps = source_only_abi_deps,
        class_abi_jar = class_abi_jar,
        class_abi_output_dir = class_abi_output_dir,
        encode_abi_command = encode_abi_command,
        define_action = define_kotlincd_action,
    )
    return make_compile_outputs(
        full_library = final_jar,
        class_abi = class_abi,
        source_only_abi = source_only_abi,
        classpath_abi = classpath_abi,
        classpath_abi_dir = classpath_abi_dir,
        required_for_source_only_abi = required_for_source_only_abi,
        annotation_processor_output = output_paths.annotations,
    )
