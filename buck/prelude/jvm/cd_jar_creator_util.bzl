# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//java:java_providers.bzl",
    "JavaClasspathEntry",
    "JavaCompilingDepsTSet",
    "JavaLibraryInfo",
    "create_abi",
    "derive_compiling_deps",
)
load("@prelude//java:java_toolchain.bzl", "AbiGenerationMode")
load("@prelude//java/utils:java_utils.bzl", "declare_prefixed_name")
load("@prelude//utils:utils.bzl", "expect")

def add_java_7_8_bootclasspath(target_level: int, bootclasspath_entries: list["artifact"], java_toolchain: "JavaToolchainInfo") -> list["artifact"]:
    if target_level == 7:
        return bootclasspath_entries + java_toolchain.bootclasspath_7
    if target_level == 8:
        return bootclasspath_entries + java_toolchain.bootclasspath_8
    return bootclasspath_entries

def declare_prefixed_output(actions: "actions", prefix: [str, None], output: str, dir: bool = False) -> "artifact":
    return actions.declare_output(declare_prefixed_name(output, prefix), dir = dir)

# The library and the toolchain can both set a specific abi generation
# mode. The toolchain's setting is effectively the "highest" form of abi
# that the toolchain supports and then the same for the target and we will choose
# the "highest" that both support.
def _resolve_abi_generation_mode(abi_generation_mode: [AbiGenerationMode.type, None], java_toolchain: "JavaToolchainInfo") -> "AbiGenerationMode":
    if abi_generation_mode == None:
        return java_toolchain.abi_generation_mode
    for mode in [AbiGenerationMode("none"), AbiGenerationMode("class"), AbiGenerationMode("source"), AbiGenerationMode("source_only")]:
        if mode in (java_toolchain.abi_generation_mode, abi_generation_mode):
            return mode
    fail("resolving abi generation mode failed. had `{}` and `{}`".format(java_toolchain.abi_generation_mode, abi_generation_mode))

def get_abi_generation_mode(
        abi_generation_mode: [AbiGenerationMode.type, None],
        java_toolchain: "JavaToolchainInfo",
        srcs: list["artifact"],
        ap_params: list["AnnotationProcessorParams"]) -> "AbiGenerationMode":
    resolved_mode = AbiGenerationMode("none") if not srcs else _resolve_abi_generation_mode(abi_generation_mode, java_toolchain)
    if resolved_mode == AbiGenerationMode("source_only"):
        def plugins_support_source_only_abi():
            for ap in ap_params:
                if ap.affects_abi and not ap.supports_source_only_abi:
                    return False
            return True

        if not plugins_support_source_only_abi():
            resolved_mode = AbiGenerationMode("source")
    return resolved_mode

# We need to construct a complex protobuf message. We do it by constructing
# a bunch of nested structs and then use write_json to get a json-encoded
# protobuf message.
#
# The definition is in xplat/build_infra/buck_client/src/com/facebook/buck/cd/resources/proto/javacd.proto
# and is, sadly, poorly documented.
#
# As we are generally trying to match buck1 for now, you can get buck1 to dump the protos for a build
# by running `export JAVACD_DUMP_PROTOS=1; buck build foo -c javacd.pass_env_variables_to_javacd=true`

# Our protobuf format mostly encodes paths in RelPath/AbsPath structs with a single "path" field.
# Note that we don't actually use abspath and instead enable JAVACD_ABSOLUTE_PATHS_ARE_RELATIVE_TO_CWD
TargetType = enum("library", "source_abi", "source_only_abi")

def encode_abi_generation_mode(mode: AbiGenerationMode.type) -> str:
    return {
        AbiGenerationMode("none"): "NONE",
        AbiGenerationMode("class"): "CLASS",
        AbiGenerationMode("source"): "SOURCE",
        AbiGenerationMode("source_only"): "SOURCE_ONLY",
    }[mode]

def encode_target_type(target_type: TargetType.type) -> str:
    if target_type == TargetType("library"):
        return "LIBRARY"
    if target_type == TargetType("source_abi"):
        return "SOURCE_ABI"
    if target_type == TargetType("source_only_abi"):
        return "SOURCE_ONLY_ABI"
    fail()

OutputPaths = record(
    jar_parent = "artifact",
    jar = "artifact",
    classes = "artifact",
    annotations = "artifact",
    scratch = "artifact",
)

def qualified_name_with_subtarget(label: Label) -> str:
    if label.sub_target:
        return "{}:{}[{}]".format(label.path, label.name, label.sub_target[0])
    return "{}:{}".format(label.path, label.name)

# Converted to str so that we get the right result when written as json.
def base_qualified_name(label: Label) -> str:
    return "{}:{}".format(label.path, label.name)

def get_qualified_name(label: Label, target_type: TargetType.type) -> str:
    # These should match the names for subtargets in java_library.bzl
    return {
        TargetType("library"): base_qualified_name(label),
        TargetType("source_abi"): base_qualified_name(label) + "[source-abi]",
        TargetType("source_only_abi"): base_qualified_name(label) + "[source-only-abi]",
    }[target_type]

def define_output_paths(actions: "actions", prefix: [str, None], label: Label) -> OutputPaths.type:
    # currently, javacd requires that at least some outputs are in the root
    # output dir. so we put all of them there. If javacd is updated we
    # could consolidate some of these into one subdir.
    jar_parent = declare_prefixed_output(actions, prefix, "jar", dir = True)
    return OutputPaths(
        jar_parent = jar_parent,
        jar = jar_parent.project("{}.jar".format(label.name)),
        classes = declare_prefixed_output(actions, prefix, "__classes__", dir = True),
        annotations = declare_prefixed_output(actions, prefix, "__gen__", dir = True),
        scratch = declare_prefixed_output(actions, prefix, "scratch", dir = True),
    )

# buildifier: disable=uninitialized
def add_output_paths_to_cmd_args(cmd: cmd_args, output_paths: OutputPaths.type, path_to_class_hashes: ["artifact", None]) -> "cmd_args":
    if path_to_class_hashes != None:
        cmd.hidden(path_to_class_hashes.as_output())
    cmd.hidden(output_paths.jar_parent.as_output())
    cmd.hidden(output_paths.jar.as_output())
    cmd.hidden(output_paths.classes.as_output())
    cmd.hidden(output_paths.annotations.as_output())
    cmd.hidden(output_paths.scratch.as_output())
    return cmd

def encode_output_paths(label: Label, paths: OutputPaths.type, target_type: TargetType.type) -> struct.type:
    paths = struct(
        classesDir = paths.classes.as_output(),
        outputJarDirPath = paths.jar_parent.as_output(),
        annotationPath = paths.annotations.as_output(),
        pathToSourcesList = cmd_args([paths.scratch.as_output(), "/", "__srcs__"], delimiter = ""),
        workingDirectory = paths.scratch.as_output(),
        outputJarPath = paths.jar.as_output(),
    )

    return struct(
        libraryPaths = paths if target_type == TargetType("library") else None,
        sourceAbiPaths = paths if target_type == TargetType("source_abi") else None,
        sourceOnlyAbiPaths = paths if target_type == TargetType("source_only_abi") else None,
        libraryTargetFullyQualifiedName = base_qualified_name(label),
    )

def encode_jar_params(remove_classes: list[str], output_paths: OutputPaths.type) -> struct.type:
    return struct(
        jarPath = output_paths.jar.as_output(),
        removeEntryPredicate = struct(
            patterns = remove_classes,
        ),
        entriesToJar = [output_paths.classes.as_output()],
        duplicatesLogLevel = "FINE",
    )

def command_abi_generation_mode(target_type: TargetType.type, abi_generation_mode: [AbiGenerationMode.type, None]) -> [AbiGenerationMode.type, None]:
    # We want the library target to have the real generation mode, but the source one's just use their own.
    # The generation mode will be used elsewhere to setup the target's classpath entry to use the correct abi.
    if target_type == TargetType("source_abi"):
        return AbiGenerationMode("source")
    if target_type == TargetType("source_only_abi"):
        return AbiGenerationMode("source_only")
    return abi_generation_mode

# TODO(cjhopman): Get correct output root (and figure out whether its even actually necessary).
# TODO(cjhopman): Get correct ignore paths.
filesystem_params = struct(
    # For buck2, everything is relative to the project root.
    rootPath = "",
    configuredBuckOut = "buck-out/v2",
    globIgnorePaths = [],
)

def get_compiling_deps_tset(
        actions: "actions",
        deps: list[Dependency],
        additional_classpath_entries: list["artifact"]) -> [JavaCompilingDepsTSet.type, None]:
    compiling_deps_tset = derive_compiling_deps(actions, None, deps)
    if additional_classpath_entries:
        children = [compiling_deps_tset] if compiling_deps_tset else []
        for entry in additional_classpath_entries:
            children.append(actions.tset(JavaCompilingDepsTSet, value = JavaClasspathEntry(
                full_library = entry,
                abi = entry,
                abi_as_dir = None,
                required_for_source_only_abi = True,
            )))
        compiling_deps_tset = actions.tset(JavaCompilingDepsTSet, children = children)

    return compiling_deps_tset

def _get_source_only_abi_compiling_deps(compiling_deps_tset: [JavaCompilingDepsTSet.type, None], source_only_abi_deps: list[Dependency]) -> list["JavaClasspathEntry"]:
    source_only_abi_compiling_deps = []
    if compiling_deps_tset:
        source_only_abi_deps_filter = {}
        for d in source_only_abi_deps:
            info = d.get(JavaLibraryInfo)
            if not info:
                fail("source_only_abi_deps must produce a JavaLibraryInfo but {} does not, please remove it".format(d))
            if info.library_output:
                source_only_abi_deps_filter[info.library_output.abi] = True

        def filter_compiling_deps(dep):
            return dep.abi in source_only_abi_deps_filter or dep.required_for_source_only_abi

        source_only_abi_compiling_deps = [compiling_dep for compiling_dep in list(compiling_deps_tset.traverse()) if filter_compiling_deps(compiling_dep)]
    return source_only_abi_compiling_deps

# buildifier: disable=unused-variable
def encode_ap_params(ap_params: list["AnnotationProcessorParams"], target_type: TargetType.type) -> [struct.type, None]:
    # buck1 oddly only inspects annotation processors, not plugins for
    # abi/source-only abi related things, even though the plugin rules
    # support the flags. we apply it to both.
    encoded_ap_params = None
    if ap_params:
        encoded_ap_params = struct(
            parameters = [],
            pluginProperties = [],
        )
        for ap in ap_params:
            # We should also filter out non-abi-affecting APs for source-abi, but buck1 doesn't and so we have lots that depend on not filtering them there.
            if target_type == TargetType("source_only_abi") and not ap.affects_abi:
                continue
            encoded_ap_params.parameters.extend(ap.params)
            if ap.deps or ap.processors:
                encoded_ap_params.pluginProperties.append(
                    struct(
                        canReuseClassLoader = not ap.isolate_class_loader,
                        doesNotAffectAbi = not ap.affects_abi,
                        supportsAbiGenerationFromSource = ap.supports_source_only_abi,
                        processorNames = ap.processors,
                        classpath = ap.deps.project_as_json("javacd_json") if ap.deps else [],
                        pathParams = {},
                    ),
                )
    return encoded_ap_params

def encode_plugin_params(plugin_params: ["PluginParams", None]) -> [struct.type, None]:
    # TODO(cjhopman): We should change plugins to not be merged together just like APs.
    encoded_plugin_params = None
    if plugin_params:
        encoded_plugin_params = struct(
            parameters = [],
            pluginProperties = [struct(
                canReuseClassLoader = False,
                doesNotAffectAbi = False,
                supportsAbiGenerationFromSource = False,
                processorNames = plugin_params.processors,
                classpath = plugin_params.deps.project_as_json("javacd_json") if plugin_params.deps else [],
                pathParams = {},
            )],
        )
    return encoded_plugin_params

def encode_base_jar_command(
        javac_tool: [str, "RunInfo", "artifact", None],
        target_type: TargetType.type,
        output_paths: OutputPaths.type,
        remove_classes: list[str],
        label: Label,
        compiling_deps_tset: [JavaCompilingDepsTSet.type, None],
        classpath_jars_tag: "artifact_tag",
        bootclasspath_entries: list["artifact"],
        source_level: int,
        target_level: int,
        abi_generation_mode: [AbiGenerationMode.type, None],
        srcs: list["artifact"],
        resources_map: dict[str, "artifact"],
        ap_params: list["AnnotationProcessorParams"],
        plugin_params: ["PluginParams", None],
        extra_arguments: cmd_args,
        source_only_abi_compiling_deps: list["JavaClasspathEntry"],
        track_class_usage: bool) -> struct.type:
    library_jar_params = encode_jar_params(remove_classes, output_paths)
    qualified_name = get_qualified_name(label, target_type)
    if target_type == TargetType("source_only_abi"):
        compiling_classpath = classpath_jars_tag.tag_artifacts([dep.abi for dep in source_only_abi_compiling_deps])
    else:
        expect(len(source_only_abi_compiling_deps) == 0)
        compiling_classpath = classpath_jars_tag.tag_artifacts(
            compiling_deps_tset.project_as_json("javacd_json") if compiling_deps_tset else None,
        )

    build_target_value = struct(
        fullyQualifiedName = qualified_name,
        buildTargetConfigHash = label.configured_target().config().hash,
        type = encode_target_type(target_type),
    )
    if javac_tool:
        resolved_javac = {
            "externalJavac": {
                "commandPrefix": [javac_tool],
                "shortName": str(javac_tool),
            },
        }
    else:
        resolved_javac = {"jsr199Javac": {}}
    resolved_java_options = struct(
        bootclasspathList = bootclasspath_entries,
        languageLevelOptions = struct(
            sourceLevel = source_level,
            targetLevel = target_level,
        ),
        debug = True,
        javaAnnotationProcessorParams = encode_ap_params(ap_params, target_type),
        standardJavacPluginParams = encode_plugin_params(plugin_params),
        extraArguments = extra_arguments,
    )

    return struct(
        outputPathsValue = encode_output_paths(label, output_paths, target_type),
        compileTimeClasspathPaths = compiling_classpath,
        javaSrcs = srcs,
        # TODO(cjhopman): populate jar infos. I think these are only used for unused dependencies (and appear to be broken in buck1 w/javacd anyway).
        fullJarInfos = [],
        abiJarInfos = [],
        # We use "class" abi compatibility to match buck1 (other compatibility modes are used for abi verification.
        abiCompatibilityMode = encode_abi_generation_mode(AbiGenerationMode("class")),
        abiGenerationMode = encode_abi_generation_mode(command_abi_generation_mode(target_type, abi_generation_mode)),
        trackClassUsage = track_class_usage,
        filesystemParams = filesystem_params,
        buildTargetValue = build_target_value,
        # TODO(cjhopman): Populate this or remove it.
        cellToPathMappings = {},
        resourcesMap = [
            {
                "key": v,
                "value": cmd_args([output_paths.classes.as_output(), "/", k], delimiter = ""),
            }
            for (k, v) in resources_map.items()
        ],
        resolvedJavac = resolved_javac,
        resolvedJavacOptions = resolved_java_options,
        libraryJarParameters = library_jar_params,
    )

def setup_dep_files(
        actions: "actions",
        actions_identifier: [str, None],
        cmd: cmd_args,
        classpath_jars_tag: "artifact_tag",
        used_classes_json_outputs: list["artifact"],
        abi_to_abi_dir_map: ["transitive_set_args_projection", list[cmd_args], None],
        hidden = ["artifact"]) -> cmd_args:
    dep_file = declare_prefixed_output(actions, actions_identifier, "dep_file.txt")

    new_cmd = cmd_args()
    new_cmd.add(cmd)
    new_cmd.add([
        "--used-classes",
    ] + [
        used_classes_json.as_output()
        for used_classes_json in used_classes_json_outputs
    ] + [
        "--dep-file",
        classpath_jars_tag.tag_artifacts(dep_file.as_output()),
    ])

    if abi_to_abi_dir_map:
        abi_to_abi_dir_map_file = declare_prefixed_output(actions, actions_identifier, "abi_to_abi_dir_map")
        actions.write(abi_to_abi_dir_map_file, abi_to_abi_dir_map)
        new_cmd.add([
            "--jar-to-jar-dir-map",
            abi_to_abi_dir_map_file,
        ])
        if type(abi_to_abi_dir_map) == "transitive_set_args_projection":
            new_cmd.hidden(classpath_jars_tag.tag_artifacts(abi_to_abi_dir_map))
        for hidden_artifact in hidden:
            new_cmd.hidden(classpath_jars_tag.tag_artifacts(hidden_artifact))

    return new_cmd

def prepare_cd_exe(
        qualified_name: str,
        java: RunInfo.type,
        class_loader_bootstrapper: "artifact",
        compiler: "artifact",
        main_class: str,
        worker: WorkerInfo.type,
        debug_port: [int, None],
        debug_target: [Label, None],
        extra_jvm_args: list[str],
        extra_jvm_args_target: [Label, None]) -> tuple.type:
    local_only = False
    jvm_args = ["-XX:-MaxFDLimit"]

    if extra_jvm_args_target:
        if qualified_name == qualified_name_with_subtarget(extra_jvm_args_target):
            jvm_args = jvm_args + extra_jvm_args
            local_only = True
    else:
        jvm_args = jvm_args + extra_jvm_args

    if debug_port and qualified_name == qualified_name_with_subtarget(debug_target):
        # Do not use a worker when debugging is enabled
        local_only = True
        jvm_args.extend(["-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address={}".format(debug_port)])

    non_worker_args = cmd_args([java, jvm_args, "-cp", compiler, "-jar", class_loader_bootstrapper, main_class])

    if local_only:
        return RunInfo(args = non_worker_args), True
    else:
        worker_run_info = WorkerRunInfo(
            # Specifies the command to compile using a non-worker process, on RE or if workers are disabled
            exe = non_worker_args,
            # Specifies the command to initialize a new worker process.
            # This is used for local execution if `build.use_persistent_workers=True`
            worker = worker,
        )
        return worker_run_info, False

# If there's additional compiled srcs, we need to merge them in and if the
# caller specified an output artifact we need to make sure the jar is in that
# location.
def prepare_final_jar(
        actions: "actions",
        actions_identifier: [str, None],
        output: ["artifact", None],
        output_paths: OutputPaths.type,
        additional_compiled_srcs: ["artifact", None],
        jar_builder: "RunInfo") -> "artifact":
    if not additional_compiled_srcs:
        if output:
            actions.copy_file(output.as_output(), output_paths.jar)
            return output
        return output_paths.jar

    merged_jar = output
    if not merged_jar:
        merged_jar = declare_prefixed_output(actions, actions_identifier, "merged.jar")
    files_to_merge = [output_paths.jar, additional_compiled_srcs]
    files_to_merge_file = actions.write(declare_prefixed_name("files_to_merge.txt", actions_identifier), files_to_merge)
    actions.run(
        cmd_args([
            jar_builder,
            "--output",
            merged_jar.as_output(),
            "--entries-to-jar",
            files_to_merge_file,
        ]).hidden(files_to_merge),
        category = "merge_additional_srcs",
        identifier = actions_identifier,
    )
    return merged_jar

def generate_abi_jars(
        actions: "actions",
        actions_identifier: [str, None],
        label: Label,
        abi_generation_mode: [AbiGenerationMode.type, None],
        additional_compiled_srcs: ["artifact", None],
        is_building_android_binary: bool,
        class_abi_generator: Dependency,
        final_jar: "artifact",
        compiling_deps_tset: [JavaCompilingDepsTSet.type, None],
        source_only_abi_deps: list[Dependency],
        class_abi_jar: ["artifact", None],
        class_abi_output_dir: ["artifact", None],
        encode_abi_command: "function",
        define_action: "function") -> tuple.type:
    class_abi = None
    source_abi = None
    source_only_abi = None
    classpath_abi = None
    classpath_abi_dir = None

    # If we are merging additional compiled_srcs, we can't produce source/source-only abis. Otherwise we
    # always generation the source/source-only abis and setup the classpath entry to use the appropriate
    # abi. This allows us to build/inspect/debug source/source-only abi for rules that don't have it enabled.
    if not additional_compiled_srcs:
        if abi_generation_mode == AbiGenerationMode("source") or not is_building_android_binary:
            source_abi_identifier = declare_prefixed_name("source_abi", actions_identifier)
            source_abi_target_type = TargetType("source_abi")
            source_abi_qualified_name = get_qualified_name(label, source_abi_target_type)
            source_abi_output_paths = define_output_paths(actions, source_abi_identifier, label)
            source_abi_classpath_jars_tag = actions.artifact_tag()
            source_abi_dir = declare_prefixed_output(actions, source_abi_identifier, "source-abi-dir", dir = True)
            source_abi_command = encode_abi_command(source_abi_output_paths, source_abi_target_type, source_abi_classpath_jars_tag)
            define_action(
                "source_abi_",
                source_abi_identifier,
                source_abi_command,
                source_abi_qualified_name,
                source_abi_output_paths,
                source_abi_classpath_jars_tag,
                source_abi_dir,
                source_abi_target_type,
                path_to_class_hashes = None,
            )
            source_abi = source_abi_output_paths.jar

            if abi_generation_mode == AbiGenerationMode("source"):
                classpath_abi = source_abi
                classpath_abi_dir = source_abi_dir

        if abi_generation_mode == AbiGenerationMode("source_only") or not is_building_android_binary:
            source_only_abi_identifier = declare_prefixed_name("source_only_abi", actions_identifier)
            source_only_abi_target_type = TargetType("source_only_abi")
            source_only_abi_qualified_name = get_qualified_name(label, source_only_abi_target_type)
            source_only_abi_output_paths = define_output_paths(actions, source_only_abi_identifier, label)
            source_only_abi_classpath_jars_tag = actions.artifact_tag()
            source_only_abi_dir = declare_prefixed_output(actions, source_only_abi_identifier, "dir", dir = True)
            source_only_abi_compiling_deps = _get_source_only_abi_compiling_deps(compiling_deps_tset, source_only_abi_deps)
            source_only_abi_command = encode_abi_command(source_only_abi_output_paths, source_only_abi_target_type, source_only_abi_classpath_jars_tag, source_only_abi_compiling_deps)
            define_action(
                "source_only_abi_",
                source_only_abi_identifier,
                source_only_abi_command,
                source_only_abi_qualified_name,
                source_only_abi_output_paths,
                source_only_abi_classpath_jars_tag,
                source_only_abi_dir,
                source_only_abi_target_type,
                path_to_class_hashes = None,
                source_only_abi_compiling_deps = source_only_abi_compiling_deps,
            )
            source_only_abi = source_only_abi_output_paths.jar

            if abi_generation_mode == AbiGenerationMode("source_only"):
                classpath_abi = source_only_abi
                classpath_abi_dir = source_only_abi_dir

        if abi_generation_mode == AbiGenerationMode("none"):
            classpath_abi = final_jar

    if classpath_abi == None or not is_building_android_binary:
        class_abi = class_abi_jar or create_abi(actions, class_abi_generator, final_jar)
        if classpath_abi == None:
            classpath_abi = class_abi
            if class_abi_output_dir:
                classpath_abi_dir = class_abi_output_dir

    return class_abi, source_abi, source_only_abi, classpath_abi, classpath_abi_dir
