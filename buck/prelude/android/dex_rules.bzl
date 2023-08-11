# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//android:android_providers.bzl", "DexFilesInfo", "ExopackageDexInfo")
load("@prelude//android:voltron.bzl", "ROOT_MODULE", "get_apk_module_graph_info", "get_root_module_only_apk_module_graph_info", "is_root_module")
load("@prelude//java:dex.bzl", "get_dex_produced_from_java_library")
load("@prelude//java:dex_toolchain.bzl", "DexToolchainInfo")
load("@prelude//java:java_library.bzl", "compile_to_jar")
load("@prelude//utils:utils.bzl", "expect", "flatten")
load("@prelude//paths.bzl", "paths")

# Android builds use a tool called `d8` to compile Java bytecode is DEX (Dalvik EXecutable)
# bytecode that runs on Android devices. Our Android builds have two distinct ways of
# doing that:
# 1) With pre-dexing enabled (this is the most common case for debug builds). That means that
#    d8 runs on every individual .jar file (to produce a .jar.dex file), and then at the APK
#    level we run d8 again to combine all the individual .jar.dex files.
# 2) With pre-dexing disabled (this is the case if it is explicitly disabled, we are running
#    proguard, or we preprocess the java classes at the APK level). This means that we run
#    d8 at the APK level on all the .jar files.
#
# The .dex files that we package into the APK consist of a single classes.dex "primary DEX"
# file, and N secondary DEX files. The classes that are put into the primary DEX are those
# that are required at startup, and are specified via `primary_dex_patterns` (classes which
# match one of those patterns are put into the primary DEX).
#
# The primary DEX is always stored in the root directory of the APK as `classes.dex`.
#
# We have 4 different ways of storing our secondary DEX files, which are specified via the
# `dex_compression` attribute:
# 1) `raw` compression. This means that we create `classes2.dex`, `classes3.dex`, ...,
#    `classesN.dex` and store each of them in the root directory of the APK.
# 2) `jar` compression. For each secondary DEX file, we put a `classes.dex` entry into a
#    JAR file, and store it as an asset at `assets/secondary-program-dex-jars/secondary-I.dex.jar`
# 3) `xz` compression. This is the same as `jar` compression, except that we run `xz` on the
#    JAR file to produce `assets/secondary-program-dex-jars/secondary-I.dex.jar.xz`.
# 4) `xzs` compression. We do the same as `jar` compression, then concatenate all the jars
#    together and do `xz` compression on the result to produce a single
#    `assets/secondary-program-dex-jars/secondary.dex.jar.xzs`.
#
# For all compression types, we also package a `assets/secondary-program-dex-jars/metadata.txt`,
# which has an entry for each secondary DEX file:
# <secondary DEX file name> <sha1 hash of secondary DEX> <canary class name>
#
# A "canary class" is a Java class that we add to every secondary DEX. It is a known class that
# can be used for DEX verification when loading the DEX on a device.
#
# For compression types other than raw, we also include a metadata file per secondary DEX, which
# consists of a single line of the form:
# jar:<size of secondary dex jar (in bytes)> dex:<size of uncompressed dex file (in bytes)>
#
# If an APK has Voltron modules, then we produce a separate group of secondary DEX files for each
# module, and we put them into `assets/<module_name>` instead of `assets/secondary-program-dex-jars`.
# We produce a `metadata.txt` file for each Voltron module.

_DEX_MERGE_OPTIONS = ["--no-desugar", "--no-optimize"]

SplitDexMergeConfig = record(
    dex_compression = str,
    primary_dex_patterns = [str],
    secondary_dex_weight_limit_bytes = int,
)

def _get_dex_compression(ctx: AnalysisContext) -> str:
    is_exopackage_enabled_for_secondary_dexes = _is_exopackage_enabled_for_secondary_dex(ctx)
    default_dex_compression = "jar" if is_exopackage_enabled_for_secondary_dexes else "raw"
    dex_compression = getattr(ctx.attrs, "dex_compression", None) or default_dex_compression
    expect(
        dex_compression in ["raw", "jar", "xz", "xzs"],
        "Only 'raw', 'jar', 'xz' and 'xzs' dex compression are supported at this time!",
    )

    return dex_compression

def get_split_dex_merge_config(
        ctx: AnalysisContext,
        android_toolchain: "AndroidToolchainInfo") -> "SplitDexMergeConfig":
    secondary_dex_weight_limit = getattr(ctx.attrs, "secondary_dex_weight_limit", None) or android_toolchain.secondary_dex_weight_limit
    return SplitDexMergeConfig(
        dex_compression = _get_dex_compression(ctx),
        primary_dex_patterns = ctx.attrs.primary_dex_patterns,
        secondary_dex_weight_limit_bytes = secondary_dex_weight_limit,
    )

def get_single_primary_dex(
        ctx: AnalysisContext,
        android_toolchain: "AndroidToolchainInfo",
        java_library_jars: list["artifact"],
        is_optimized: bool = False) -> "DexFilesInfo":
    expect(
        not _is_exopackage_enabled_for_secondary_dex(ctx),
        "It doesn't make sense to enable secondary dex exopackage for single dex builds!",
    )
    d8_cmd = cmd_args(android_toolchain.d8_command[RunInfo])

    output_dex_file = ctx.actions.declare_output("classes.dex")
    d8_cmd.add(["--output-dex-file", output_dex_file.as_output()])

    jar_to_dex_file = ctx.actions.write("jar_to_dex_file.txt", java_library_jars)
    d8_cmd.add(["--files-to-dex-list", jar_to_dex_file])
    d8_cmd.hidden(java_library_jars)

    d8_cmd.add(["--android-jar", android_toolchain.android_jar])
    if not is_optimized:
        d8_cmd.add("--no-optimize")

    ctx.actions.run(d8_cmd, category = "d8", identifier = "{}:{}".format(ctx.label.package, ctx.label.name))

    return DexFilesInfo(
        primary_dex = output_dex_file,
        root_module_secondary_dex_dirs = [],
        non_root_module_secondary_dex_dirs = [],
        secondary_dex_exopackage_info = None,
        proguard_text_files_path = None,
        primary_dex_class_names = None,
    )

def get_multi_dex(
        ctx: AnalysisContext,
        android_toolchain: "AndroidToolchainInfo",
        java_library_jars_to_owners: dict["artifact", "target_label"],
        primary_dex_patterns: list[str],
        proguard_configuration_output_file: ["artifact", None] = None,
        proguard_mapping_output_file: ["artifact", None] = None,
        is_optimized: bool = False,
        apk_module_graph_file: ["artifact", None] = None) -> "DexFilesInfo":
    expect(
        not _is_exopackage_enabled_for_secondary_dex(ctx),
        "secondary dex exopackage can only be enabled on pre-dexed builds!",
    )
    primary_dex_file = ctx.actions.declare_output("classes.dex")
    primary_dex_class_names = ctx.actions.declare_output("primary_dex_class_names.txt")
    root_module_secondary_dex_output_dir = ctx.actions.declare_output("root_module_secondary_dex_output_dir", dir = True)
    secondary_dex_dir = ctx.actions.declare_output("secondary_dex_output_dir", dir = True)

    # dynamic actions are not valid with no input, but it's easier to use the same code regardless,
    # so just create an empty input.
    inputs = [apk_module_graph_file] if apk_module_graph_file else [ctx.actions.write("empty_artifact_for_multi_dex_dynamic_action", [])]
    outputs = [primary_dex_file, primary_dex_class_names, root_module_secondary_dex_output_dir, secondary_dex_dir]

    def do_multi_dex(ctx: AnalysisContext, artifacts, outputs):
        apk_module_graph_info = get_apk_module_graph_info(ctx, apk_module_graph_file, artifacts) if apk_module_graph_file else get_root_module_only_apk_module_graph_info()
        target_to_module_mapping_function = apk_module_graph_info.target_to_module_mapping_function
        module_to_jars = {}
        for java_library_jar, owner in java_library_jars_to_owners.items():
            module = target_to_module_mapping_function(str(owner))
            module_to_jars.setdefault(module, []).append(java_library_jar)

        secondary_dex_dir_srcs = {}
        for module, jars in module_to_jars.items():
            multi_dex_cmd = cmd_args(android_toolchain.multi_dex_command[RunInfo])
            secondary_dex_compression_cmd = cmd_args(android_toolchain.secondary_dex_compression_command[RunInfo])

            uncompressed_secondary_dex_output_dir = ctx.actions.declare_output("uncompressed_secondary_dex_output_dir_for_module_{}".format(module), dir = True)
            multi_dex_cmd.add("--secondary-dex-output-dir", uncompressed_secondary_dex_output_dir.as_output())
            secondary_dex_compression_cmd.add("--raw-secondary-dexes-dir", uncompressed_secondary_dex_output_dir)
            if is_root_module(module):
                primary_dex_patterns_file = ctx.actions.write("primary_dex_patterns", primary_dex_patterns)
                if getattr(ctx.attrs, "minimize_primary_dex_size", False):
                    primary_dex_jars, jars_to_dex = _get_primary_dex_and_secondary_dex_jars(
                        ctx,
                        jars,
                        java_library_jars_to_owners,
                        primary_dex_patterns_file,
                        proguard_configuration_output_file,
                        proguard_mapping_output_file,
                        android_toolchain,
                    )

                    primary_dex_jar_to_dex_file = ctx.actions.write("primary_dex_jars_to_dex_file_for_root_module.txt", primary_dex_jars)
                    multi_dex_cmd.add("--primary-dex-files-to-dex-list", primary_dex_jar_to_dex_file)
                    multi_dex_cmd.hidden(primary_dex_jars)
                    multi_dex_cmd.add("--minimize-primary-dex")
                else:
                    jars_to_dex = jars
                    multi_dex_cmd.add("--primary-dex-patterns-path", primary_dex_patterns_file)

                multi_dex_cmd.add("--primary-dex", outputs[primary_dex_file].as_output())
                multi_dex_cmd.add("--primary-dex-class-names", outputs[primary_dex_class_names].as_output())
                secondary_dex_compression_cmd.add("--secondary-dex-output-dir", outputs[root_module_secondary_dex_output_dir].as_output())
            else:
                secondary_dex_dir_for_module = ctx.actions.declare_output("secondary_dex_output_dir_for_module_{}".format(module), dir = True)
                secondary_dex_subdir = secondary_dex_dir_for_module.project(_get_secondary_dex_subdir(module))
                secondary_dex_dir_srcs[_get_secondary_dex_subdir(module)] = secondary_dex_subdir
                secondary_dex_compression_cmd.add("--module-deps", ctx.actions.write("module_deps_for_{}".format(module), apk_module_graph_info.module_to_module_deps_function(module)))
                secondary_dex_compression_cmd.add("--secondary-dex-output-dir", secondary_dex_dir_for_module.as_output())
                jars_to_dex = jars

            multi_dex_cmd.add("--module", module)
            multi_dex_cmd.add("--canary-class-name", apk_module_graph_info.module_to_canary_class_name_function(module))
            secondary_dex_compression_cmd.add("--module", module)
            secondary_dex_compression_cmd.add("--canary-class-name", apk_module_graph_info.module_to_canary_class_name_function(module))

            jar_to_dex_file = ctx.actions.write("jars_to_dex_file_for_module_{}.txt".format(module), jars_to_dex)
            multi_dex_cmd.add("--files-to-dex-list", jar_to_dex_file)
            multi_dex_cmd.hidden(jars_to_dex)

            multi_dex_cmd.add("--android-jar", android_toolchain.android_jar)
            if not is_optimized:
                multi_dex_cmd.add("--no-optimize")

            if proguard_configuration_output_file:
                multi_dex_cmd.add("--proguard-configuration-file", proguard_configuration_output_file)
                multi_dex_cmd.add("--proguard-mapping-file", proguard_mapping_output_file)

            ctx.actions.run(multi_dex_cmd, category = "multi_dex", identifier = "{}:{}_module_{}".format(ctx.label.package, ctx.label.name, module))

            secondary_dex_compression_cmd.add("--compression", _get_dex_compression(ctx))
            secondary_dex_compression_cmd.add("--xz-compression-level", str(getattr(ctx.attrs, "xz_compression_level", 4)))

            ctx.actions.run(secondary_dex_compression_cmd, category = "secondary_dex_compression", identifier = "{}:{}_module_{}".format(ctx.label.package, ctx.label.name, module))

        ctx.actions.symlinked_dir(outputs[secondary_dex_dir], secondary_dex_dir_srcs)

    ctx.actions.dynamic_output(dynamic = inputs, inputs = [], outputs = outputs, f = do_multi_dex)

    return DexFilesInfo(
        primary_dex = primary_dex_file,
        root_module_secondary_dex_dirs = [root_module_secondary_dex_output_dir],
        non_root_module_secondary_dex_dirs = [secondary_dex_dir],
        secondary_dex_exopackage_info = None,
        proguard_text_files_path = None,
        primary_dex_class_names = primary_dex_class_names,
    )

def _get_primary_dex_and_secondary_dex_jars(
        ctx: AnalysisContext,
        jars: list["artifact"],
        java_library_jars_to_owners: dict["artifact", "target_label"],
        primary_dex_patterns_file: "artifact",
        proguard_configuration_output_file: ["artifact", None],
        proguard_mapping_output_file: ["artifact", None],
        android_toolchain: "AndroidToolchainInfo") -> (list["artifact"], list["artifact"]):
    primary_dex_jars = []
    secondary_dex_jars = []
    for jar in jars:
        jar_splitter_cmd = cmd_args(android_toolchain.jar_splitter_command[RunInfo])
        owner = java_library_jars_to_owners[jar]
        identifier = "{}/{}/{}".format(owner.package, owner.name, jar.short_path)
        primary_dex_jar = ctx.actions.declare_output("root_module_primary_dex_jars/{}".format(identifier))
        secondary_dex_jar = ctx.actions.declare_output("root_module_secondary_dex_jars/{}".format(identifier))
        jar_splitter_cmd.add([
            "--input-jar",
            jar,
            "--primary-dex-patterns",
            primary_dex_patterns_file,
            "--primary-dex-classes-jar",
            primary_dex_jar.as_output(),
            "--secondary-dex-classes-jar",
            secondary_dex_jar.as_output(),
        ])
        if proguard_configuration_output_file:
            jar_splitter_cmd.add("--proguard-configuration-file", proguard_configuration_output_file)
            jar_splitter_cmd.add("--proguard-mapping-file", proguard_mapping_output_file)

        ctx.actions.run(jar_splitter_cmd, category = "jar_splitter", identifier = identifier)

        primary_dex_jars.append(primary_dex_jar)
        secondary_dex_jars.append(secondary_dex_jar)

    return primary_dex_jars, secondary_dex_jars

def merge_to_single_dex(
        ctx: AnalysisContext,
        android_toolchain: "AndroidToolchainInfo",
        pre_dexed_libs: list["DexLibraryInfo"]) -> "DexFilesInfo":
    expect(
        not _is_exopackage_enabled_for_secondary_dex(ctx),
        "It doesn't make sense to enable secondary dex exopackage for single dex builds!",
    )
    output_dex_file = ctx.actions.declare_output("classes.dex")
    pre_dexed_artifacts_to_dex_file = ctx.actions.declare_output("pre_dexed_artifacts_to_dex_file.txt")
    pre_dexed_artifacts = [pre_dexed_lib.dex for pre_dexed_lib in pre_dexed_libs if pre_dexed_lib.dex != None]
    _merge_dexes(ctx, android_toolchain, output_dex_file, pre_dexed_artifacts, pre_dexed_artifacts_to_dex_file)

    return DexFilesInfo(
        primary_dex = output_dex_file,
        root_module_secondary_dex_dirs = [],
        non_root_module_secondary_dex_dirs = [],
        secondary_dex_exopackage_info = None,
        proguard_text_files_path = None,
        primary_dex_class_names = None,
    )

DexInputWithSpecifiedClasses = record(
    lib = "DexLibraryInfo",
    dex_class_names = [str],
)

DexInputsWithClassNamesAndWeightEstimatesFile = record(
    libs = ["DexLibraryInfo"],
    weight_estimate_and_filtered_class_names_file = "artifact",
)

# When using jar compression, the secondary dex directory consists of N secondary dex jars, each
# of which has a corresponding .meta file (the secondary_dex_metadata_file) containing a single
# line of the form:
# jar:<size of secondary dex jar (in bytes)> dex:<size of uncompressed dex file (in bytes)>
#
# It also contains a metadata.txt file, which consists on N lines, one for each secondary dex
# jar. Those lines consist of:
# <secondary dex file name> <sha1 hash of secondary dex> <canary class>
#
# We write the line that needs to be added to metadata.txt for this secondary dex jar to
# secondary_dex_metadata_line, and we use the secondary_dex_canary_class_name for the
# <canary class>.
#
# When we have finished building all of the secondary dexes, we read each of the
# secondary_dex_metadata_line artifacts and write them to a single metadata.txt file.
# We do that for raw compression too, since it also has a metadata.txt file.
SecondaryDexMetadataConfig = record(
    secondary_dex_compression = str,
    secondary_dex_metadata_path = [str, None],
    secondary_dex_metadata_file = ["artifact", None],
    secondary_dex_metadata_line = "artifact",
    secondary_dex_canary_class_name = str,
)

def _get_secondary_dex_jar_metadata_config(
        actions: "actions",
        secondary_dex_path: str,
        module: str,
        module_to_canary_class_name_function: "function",
        index: int) -> SecondaryDexMetadataConfig.type:
    secondary_dex_metadata_path = secondary_dex_path + ".meta"
    return SecondaryDexMetadataConfig(
        secondary_dex_compression = "jar",
        secondary_dex_metadata_path = secondary_dex_metadata_path,
        secondary_dex_metadata_file = actions.declare_output(secondary_dex_metadata_path),
        secondary_dex_metadata_line = actions.declare_output("metadata_line_artifacts/{}/{}".format(module, index + 1)),
        secondary_dex_canary_class_name = _get_fully_qualified_canary_class_name(module, module_to_canary_class_name_function, index + 1),
    )

def _get_secondary_dex_raw_metadata_config(
        actions: "actions",
        module: str,
        module_to_canary_class_name_function: "function",
        index: int) -> SecondaryDexMetadataConfig.type:
    return SecondaryDexMetadataConfig(
        secondary_dex_compression = "raw",
        secondary_dex_metadata_path = None,
        secondary_dex_metadata_file = None,
        secondary_dex_metadata_line = actions.declare_output("metadata_line_artifacts/{}/{}".format(module, index + 1)),
        secondary_dex_canary_class_name = _get_fully_qualified_canary_class_name(module, module_to_canary_class_name_function, index + 1),
    )

def _get_filter_dex_batch_size() -> int:
    return 100

def _filter_pre_dexed_libs(
        actions: "actions",
        android_toolchain: "AndroidToolchainInfo",
        primary_dex_patterns_file: "artifact",
        pre_dexed_libs: list["DexLibraryInfo"],
        batch_number: int) -> DexInputsWithClassNamesAndWeightEstimatesFile.type:
    weight_estimate_and_filtered_class_names_file = actions.declare_output("class_names_and_weight_estimates_for_batch_{}".format(batch_number))

    filter_dex_cmd = cmd_args([
        android_toolchain.filter_dex_class_names[RunInfo],
        "--primary-dex-patterns",
        primary_dex_patterns_file,
        "--dex-target-identifiers",
        [lib.identifier for lib in pre_dexed_libs],
        "--class-names",
        [lib.class_names for lib in pre_dexed_libs],
        "--weight-estimates",
        [lib.weight_estimate for lib in pre_dexed_libs],
        "--output",
        weight_estimate_and_filtered_class_names_file.as_output(),
    ])
    actions.run(filter_dex_cmd, category = "filter_dex", identifier = "batch_{}".format(batch_number))

    return DexInputsWithClassNamesAndWeightEstimatesFile(libs = pre_dexed_libs, weight_estimate_and_filtered_class_names_file = weight_estimate_and_filtered_class_names_file)

_SortedPreDexedInputs = record(
    module = str,
    primary_dex_inputs = [DexInputWithSpecifiedClasses.type],
    secondary_dex_inputs = [[DexInputWithSpecifiedClasses.type]],
)

def merge_to_split_dex(
        ctx: AnalysisContext,
        android_toolchain: "AndroidToolchainInfo",
        pre_dexed_libs: list["DexLibraryInfo"],
        split_dex_merge_config: "SplitDexMergeConfig",
        apk_module_graph_file: ["artifact", None] = None) -> "DexFilesInfo":
    is_exopackage_enabled_for_secondary_dex = _is_exopackage_enabled_for_secondary_dex(ctx)
    if is_exopackage_enabled_for_secondary_dex:
        expect(
            split_dex_merge_config.dex_compression == "jar",
            "Exopackage can only be enabled for secondary dexes when the dex compression is 'jar', but the dex compression is '{}'".format(split_dex_merge_config.dex_compression),
        )
    primary_dex_patterns_file = ctx.actions.write("primary_dex_patterns_file", split_dex_merge_config.primary_dex_patterns)

    pre_dexed_libs_with_class_names_and_weight_estimates_files = []

    batch_size = _get_filter_dex_batch_size()
    for (batch_number, start_index) in enumerate(range(0, len(pre_dexed_libs), batch_size)):
        end_index = min(start_index + batch_size, len(pre_dexed_libs))
        pre_dexed_libs_with_class_names_and_weight_estimates_files.append(
            _filter_pre_dexed_libs(
                ctx.actions,
                android_toolchain,
                primary_dex_patterns_file,
                pre_dexed_libs[start_index:end_index],
                batch_number,
            ),
        )

    input_artifacts = [
        input.weight_estimate_and_filtered_class_names_file
        for input in pre_dexed_libs_with_class_names_and_weight_estimates_files
    ] + ([apk_module_graph_file] if apk_module_graph_file else [])
    primary_dex_artifact_list = ctx.actions.declare_output("pre_dexed_artifacts_for_primary_dex.txt")
    primary_dex_output = ctx.actions.declare_output("classes.dex")
    primary_dex_class_names_list = ctx.actions.declare_output("primary_dex_class_names_list.txt")
    root_module_secondary_dexes_dir = ctx.actions.declare_output("root_module_secondary_dexes_dir", dir = True)
    root_module_secondary_dexes_subdir = root_module_secondary_dexes_dir.project(_get_secondary_dex_subdir(ROOT_MODULE))
    root_module_secondary_dexes_metadata = root_module_secondary_dexes_dir.project(paths.join(_get_secondary_dex_subdir(ROOT_MODULE), "metadata.txt"))
    non_root_module_secondary_dexes_dir = ctx.actions.declare_output("non_root_module_secondary_dexes_dir", dir = True)

    outputs = [primary_dex_output, primary_dex_artifact_list, primary_dex_class_names_list, root_module_secondary_dexes_dir, non_root_module_secondary_dexes_dir]

    def merge_pre_dexed_libs(ctx: AnalysisContext, artifacts, outputs):
        apk_module_graph_info = get_apk_module_graph_info(ctx, apk_module_graph_file, artifacts) if apk_module_graph_file else get_root_module_only_apk_module_graph_info()
        module_to_canary_class_name_function = apk_module_graph_info.module_to_canary_class_name_function
        sorted_pre_dexed_inputs = _sort_pre_dexed_files(
            ctx,
            artifacts,
            pre_dexed_libs_with_class_names_and_weight_estimates_files,
            split_dex_merge_config,
            get_module_from_target = apk_module_graph_info.target_to_module_mapping_function,
            module_to_canary_class_name_function = module_to_canary_class_name_function,
        )

        root_module_secondary_dexes_for_symlinking = {}
        non_root_module_secondary_dexes_for_symlinking = {}
        metadata_line_artifacts_by_module = {}
        metadata_dot_txt_files_by_module = {}

        for sorted_pre_dexed_input in sorted_pre_dexed_inputs:
            module = sorted_pre_dexed_input.module
            secondary_dexes_for_symlinking = root_module_secondary_dexes_for_symlinking if is_root_module(module) else non_root_module_secondary_dexes_for_symlinking
            primary_dex_inputs = sorted_pre_dexed_input.primary_dex_inputs
            pre_dexed_artifacts = [primary_dex_input.lib.dex for primary_dex_input in primary_dex_inputs if primary_dex_input.lib.dex]
            if pre_dexed_artifacts:
                expect(is_root_module(module), "module {} should not have a primary dex!".format(module))
                ctx.actions.write(
                    outputs[primary_dex_class_names_list].as_output(),
                    flatten([primary_dex_input.dex_class_names for primary_dex_input in primary_dex_inputs]),
                )

                _merge_dexes(
                    ctx,
                    android_toolchain,
                    outputs[primary_dex_output],
                    pre_dexed_artifacts,
                    outputs[primary_dex_artifact_list],
                    class_names_to_include = primary_dex_class_names_list,
                )
            else:
                expect(
                    not is_root_module(module),
                    "No primary dex classes were specified! Please add primary_dex_patterns to ensure that at least one class exists in the primary dex.",
                )

            secondary_dex_inputs = sorted_pre_dexed_input.secondary_dex_inputs
            raw_secondary_dexes_for_compressing = {}
            for i in range(len(secondary_dex_inputs)):
                if split_dex_merge_config.dex_compression == "jar" or split_dex_merge_config.dex_compression == "raw":
                    if split_dex_merge_config.dex_compression == "jar":
                        secondary_dex_path = _get_jar_secondary_dex_path(i, module)
                        secondary_dex_metadata_config = _get_secondary_dex_jar_metadata_config(ctx.actions, secondary_dex_path, module, module_to_canary_class_name_function, i)
                        secondary_dexes_for_symlinking[secondary_dex_metadata_config.secondary_dex_metadata_path] = secondary_dex_metadata_config.secondary_dex_metadata_file
                    else:
                        secondary_dex_path = _get_raw_secondary_dex_path(i, module)
                        secondary_dex_metadata_config = _get_secondary_dex_raw_metadata_config(ctx.actions, module, module_to_canary_class_name_function, i)

                    secondary_dex_output = ctx.actions.declare_output(secondary_dex_path)
                    secondary_dexes_for_symlinking[secondary_dex_path] = secondary_dex_output
                    metadata_line_artifacts_by_module.setdefault(module, []).append(secondary_dex_metadata_config.secondary_dex_metadata_line)
                else:
                    secondary_dex_name = _get_raw_secondary_dex_name(i, module)
                    secondary_dex_output = ctx.actions.declare_output("{}/{}".format(module, secondary_dex_name))
                    raw_secondary_dexes_for_compressing[secondary_dex_name] = secondary_dex_output
                    secondary_dex_metadata_config = None

                secondary_dex_artifact_list = ctx.actions.declare_output("pre_dexed_artifacts_for_secondary_dex_{}_for_module_{}.txt".format(i + 2, module))
                secondary_dex_class_list = ctx.actions.write(
                    "class_list_for_secondary_dex_{}_for_module_{}.txt".format(i + 2, module),
                    flatten([secondary_dex_input.dex_class_names for secondary_dex_input in secondary_dex_inputs[i]]),
                )
                pre_dexed_artifacts = [secondary_dex_input.lib.dex for secondary_dex_input in secondary_dex_inputs[i] if secondary_dex_input.lib.dex]
                _merge_dexes(
                    ctx,
                    android_toolchain,
                    secondary_dex_output,
                    pre_dexed_artifacts,
                    secondary_dex_artifact_list,
                    class_names_to_include = secondary_dex_class_list,
                    secondary_dex_metadata_config = secondary_dex_metadata_config,
                )

            if split_dex_merge_config.dex_compression == "jar" or split_dex_merge_config.dex_compression == "raw":
                metadata_dot_txt_path = "{}/metadata.txt".format(_get_secondary_dex_subdir(module))
                metadata_dot_txt_file = ctx.actions.declare_output(metadata_dot_txt_path)
                secondary_dexes_for_symlinking[metadata_dot_txt_path] = metadata_dot_txt_file
                metadata_dot_txt_files_by_module[module] = metadata_dot_txt_file
            else:
                raw_secondary_dexes_dir = ctx.actions.symlinked_dir("raw_secondary_dexes_dir_for_module_{}".format(module), raw_secondary_dexes_for_compressing)
                secondary_dex_dir_for_module = ctx.actions.declare_output("secondary_dexes_dir_for_{}".format(module), dir = True)
                secondary_dex_subdir = secondary_dex_dir_for_module.project(_get_secondary_dex_subdir(module))

                multi_dex_cmd = cmd_args(android_toolchain.secondary_dex_compression_command[RunInfo])
                multi_dex_cmd.add("--secondary-dex-output-dir", secondary_dex_dir_for_module.as_output())
                multi_dex_cmd.add("--raw-secondary-dexes-dir", raw_secondary_dexes_dir)
                multi_dex_cmd.add("--compression", _get_dex_compression(ctx))
                multi_dex_cmd.add("--xz-compression-level", str(ctx.attrs.xz_compression_level))
                multi_dex_cmd.add("--module", module)
                multi_dex_cmd.add("--canary-class-name", module_to_canary_class_name_function(module))
                if not is_root_module(module):
                    multi_dex_cmd.add("--module-deps", ctx.actions.write("module_deps_for_{}".format(module), apk_module_graph_info.module_to_module_deps_function(module)))

                ctx.actions.run(multi_dex_cmd, category = "multi_dex_from_raw_dexes", identifier = "{}:{}_module_{}".format(ctx.label.package, ctx.label.name, module))

                secondary_dexes_for_symlinking[_get_secondary_dex_subdir(module)] = secondary_dex_subdir

        if metadata_dot_txt_files_by_module:
            def write_metadata_dot_txts(ctx: AnalysisContext, artifacts, outputs):
                for voltron_module, metadata_dot_txt in metadata_dot_txt_files_by_module.items():
                    metadata_line_artifacts = metadata_line_artifacts_by_module[voltron_module]
                    expect(metadata_line_artifacts != None, "Should have metadata lines!")

                    metadata_lines = [".id {}".format(voltron_module)]
                    metadata_lines.extend([".requires {}".format(module_dep) for module_dep in apk_module_graph_info.module_to_module_deps_function(voltron_module)])
                    if split_dex_merge_config.dex_compression == "raw" and is_root_module(voltron_module):
                        metadata_lines.append(".root_relative")
                    for metadata_line_artifact in metadata_line_artifacts:
                        metadata_lines.append(artifacts[metadata_line_artifact].read_string().strip())
                    ctx.actions.write(outputs[metadata_dot_txt], metadata_lines)

            ctx.actions.dynamic_output(dynamic = flatten(metadata_line_artifacts_by_module.values()), inputs = [], outputs = metadata_dot_txt_files_by_module.values(), f = write_metadata_dot_txts)

        ctx.actions.symlinked_dir(
            outputs[root_module_secondary_dexes_dir],
            root_module_secondary_dexes_for_symlinking,
        )
        ctx.actions.symlinked_dir(
            outputs[non_root_module_secondary_dexes_dir],
            non_root_module_secondary_dexes_for_symlinking,
        )

    ctx.actions.dynamic_output(dynamic = input_artifacts, inputs = [], outputs = outputs, f = merge_pre_dexed_libs)

    if is_exopackage_enabled_for_secondary_dex:
        root_module_secondary_dex_dirs = []
        secondary_dex_exopackage_info = ExopackageDexInfo(
            metadata = root_module_secondary_dexes_metadata,
            directory = root_module_secondary_dexes_subdir,
        )
    else:
        root_module_secondary_dex_dirs = [root_module_secondary_dexes_dir]
        secondary_dex_exopackage_info = None

    return DexFilesInfo(
        primary_dex = primary_dex_output,
        root_module_secondary_dex_dirs = root_module_secondary_dex_dirs,
        non_root_module_secondary_dex_dirs = [non_root_module_secondary_dexes_dir],
        secondary_dex_exopackage_info = secondary_dex_exopackage_info,
        proguard_text_files_path = None,
        primary_dex_class_names = primary_dex_class_names_list,
    )

def _merge_dexes(
        ctx: AnalysisContext,
        android_toolchain: "AndroidToolchainInfo",
        output_dex_file: "artifact",
        pre_dexed_artifacts: list["artifact"],
        pre_dexed_artifacts_file: "artifact",
        class_names_to_include: ["artifact", None] = None,
        secondary_output_dex_file: ["artifact", None] = None,
        secondary_dex_metadata_config: [SecondaryDexMetadataConfig.type, None] = None):
    d8_cmd = cmd_args(android_toolchain.d8_command[RunInfo])
    d8_cmd.add(["--output-dex-file", output_dex_file.as_output()])

    pre_dexed_artifacts_to_dex_file = ctx.actions.write(pre_dexed_artifacts_file.as_output(), pre_dexed_artifacts)
    d8_cmd.add(["--files-to-dex-list", pre_dexed_artifacts_to_dex_file])
    d8_cmd.hidden(pre_dexed_artifacts)

    d8_cmd.add(["--android-jar", android_toolchain.android_jar])
    d8_cmd.add(_DEX_MERGE_OPTIONS)

    if class_names_to_include:
        d8_cmd.add(["--primary-dex-class-names-path", class_names_to_include])

    if secondary_output_dex_file:
        d8_cmd.add(["--secondary-output-dex-file", secondary_output_dex_file.as_output()])

    if secondary_dex_metadata_config:
        d8_cmd.add(["--secondary-dex-compression", secondary_dex_metadata_config.secondary_dex_compression])
        if secondary_dex_metadata_config.secondary_dex_metadata_file:
            d8_cmd.add(["--secondary-dex-metadata-file", secondary_dex_metadata_config.secondary_dex_metadata_file.as_output()])
        d8_cmd.add(["--secondary-dex-metadata-line", secondary_dex_metadata_config.secondary_dex_metadata_line.as_output()])
        d8_cmd.add(["--secondary-dex-canary-class-name", secondary_dex_metadata_config.secondary_dex_canary_class_name])

    ctx.actions.run(
        d8_cmd,
        category = "d8",
        identifier = "{}:{} {}".format(ctx.label.package, ctx.label.name, output_dex_file.short_path),
    )

def _sort_pre_dexed_files(
        ctx: AnalysisContext,
        artifacts,
        pre_dexed_libs_with_class_names_and_weight_estimates_files: list["DexInputsWithClassNamesAndWeightEstimatesFile"],
        split_dex_merge_config: "SplitDexMergeConfig",
        get_module_from_target: "function",
        module_to_canary_class_name_function: "function") -> list[_SortedPreDexedInputs.type]:
    sorted_pre_dexed_inputs_map = {}
    current_secondary_dex_size_map = {}
    current_secondary_dex_inputs_map = {}
    for pre_dexed_libs_with_class_names_and_weight_estimates in pre_dexed_libs_with_class_names_and_weight_estimates_files:
        class_names_and_weight_estimates_json = artifacts[pre_dexed_libs_with_class_names_and_weight_estimates.weight_estimate_and_filtered_class_names_file].read_json()
        for pre_dexed_lib in pre_dexed_libs_with_class_names_and_weight_estimates.libs:
            module = get_module_from_target(str(pre_dexed_lib.dex.owner.raw_target()))
            pre_dexed_lib_info = class_names_and_weight_estimates_json[pre_dexed_lib.identifier]
            weight_estimate_string = pre_dexed_lib_info["weight_estimate"]
            primary_dex_class_names = pre_dexed_lib_info["primary_dex_class_names"]
            secondary_dex_class_names = pre_dexed_lib_info["secondary_dex_class_names"]

            module_pre_dexed_inputs = sorted_pre_dexed_inputs_map.setdefault(module, _SortedPreDexedInputs(
                module = module,
                primary_dex_inputs = [],
                secondary_dex_inputs = [],
            ))
            primary_dex_inputs = module_pre_dexed_inputs.primary_dex_inputs
            secondary_dex_inputs = module_pre_dexed_inputs.secondary_dex_inputs

            if len(primary_dex_class_names) > 0:
                if is_root_module(module):
                    primary_dex_inputs.append(
                        DexInputWithSpecifiedClasses(lib = pre_dexed_lib, dex_class_names = primary_dex_class_names),
                    )
                else:
                    # TODO(T148680617) We shouldn't allow classes that are specified to be in the
                    # primary dex to end up in a non-root module, but buck1 allows it and there are
                    # Voltron configs that rely on this, so we allow it too for migration purposes.
                    # fail("Non-root modules should not have anything that belongs in the primary dex, " +
                    #     "but {} is assigned to module {} and has the following class names in the primary dex: {}\n".format(
                    #         pre_dexed_lib.dex.owner,
                    #         module,
                    #         "\n".join(primary_dex_class_names),
                    #     ),
                    # )
                    secondary_dex_class_names.extend(primary_dex_class_names)

            if len(secondary_dex_class_names) > 0:
                weight_estimate = int(weight_estimate_string)
                current_secondary_dex_size = current_secondary_dex_size_map.get(module, 0)
                if current_secondary_dex_size + weight_estimate > split_dex_merge_config.secondary_dex_weight_limit_bytes:
                    current_secondary_dex_size = 0
                    current_secondary_dex_inputs_map[module] = []

                current_secondary_dex_inputs = current_secondary_dex_inputs_map.setdefault(module, [])
                if len(current_secondary_dex_inputs) == 0:
                    canary_class_dex_input = _create_canary_class(
                        ctx,
                        len(secondary_dex_inputs) + 1,
                        module,
                        module_to_canary_class_name_function,
                        ctx.attrs._dex_toolchain[DexToolchainInfo],
                    )
                    current_secondary_dex_inputs.append(canary_class_dex_input)
                    secondary_dex_inputs.append(current_secondary_dex_inputs)

                current_secondary_dex_size_map[module] = current_secondary_dex_size + weight_estimate
                current_secondary_dex_inputs.append(
                    DexInputWithSpecifiedClasses(lib = pre_dexed_lib, dex_class_names = secondary_dex_class_names),
                )

    return sorted_pre_dexed_inputs_map.values()

def _get_raw_secondary_dex_name(index: int, module: str) -> str:
    # Root module begins at 2 (primary classes.dex is 1)
    # Non-root module begins at 1 (classes.dex)
    if is_root_module(module):
        return "classes{}.dex".format(index + 2)
    elif index == 0:
        return "classes.dex".format(module)
    else:
        return "classes{}.dex".format(module, index + 1)

def _get_raw_secondary_dex_path(index: int, module: str):
    if is_root_module(module):
        return _get_raw_secondary_dex_name(index, module)
    else:
        return "assets/{}/{}".format(module, _get_raw_secondary_dex_name(index, module))

def _get_jar_secondary_dex_path(index: int, module: str):
    return "{}/{}-{}.dex.jar".format(
        _get_secondary_dex_subdir(module),
        "secondary" if is_root_module(module) else module,
        index + 1,
    )

def _get_secondary_dex_subdir(module: str):
    return "assets/{}".format("secondary-program-dex-jars" if is_root_module(module) else module)

# We create "canary" classes and add them to each secondary dex jar to ensure each jar has a class
# that can be safely loaded on any system. This class is used during secondary dex verification.
_CANARY_FULLY_QUALIFIED_CLASS_NAME_TEMPLATE = "{}.dex{}.Canary"
_CANARY_FILE_NAME_TEMPLATE = "canary_classes/{}/dex{}/Canary.java"
_CANARY_CLASS_PACKAGE_TEMPLATE = "package {}.dex{};\n"
_CANARY_CLASS_INTERFACE_DEFINITION = "public interface Canary {}"

def _create_canary_class(
        ctx: AnalysisContext,
        index: int,
        module: str,
        module_to_canary_class_name_function: "function",
        dex_toolchain: DexToolchainInfo.type) -> DexInputWithSpecifiedClasses.type:
    prefix = module_to_canary_class_name_function(module)
    index_string = str(index)
    if len(index_string) == 1:
        index_string = "0" + index_string
    canary_class_java_file = ctx.actions.write(_CANARY_FILE_NAME_TEMPLATE.format(prefix, index_string), [_CANARY_CLASS_PACKAGE_TEMPLATE.format(prefix, index_string), _CANARY_CLASS_INTERFACE_DEFINITION])
    canary_class_jar = ctx.actions.declare_output("canary_classes/{}/canary_jar_{}.jar".format(prefix, index_string))
    compile_to_jar(ctx, [canary_class_java_file], output = canary_class_jar, actions_identifier = "{}_canary_class{}".format(prefix, index_string))

    dex_library_info = get_dex_produced_from_java_library(ctx, dex_toolchain = dex_toolchain, jar_to_dex = canary_class_jar)

    return DexInputWithSpecifiedClasses(
        lib = dex_library_info,
        dex_class_names = [_get_fully_qualified_canary_class_name(module, module_to_canary_class_name_function, index).replace(".", "/") + ".class"],
    )

def _get_fully_qualified_canary_class_name(module: str, module_to_canary_class_name_function: "function", index: int) -> str:
    prefix = module_to_canary_class_name_function(module)
    index_string = str(index)
    if len(index_string) == 1:
        index_string = "0" + index_string
    return _CANARY_FULLY_QUALIFIED_CLASS_NAME_TEMPLATE.format(prefix, index_string)

def _is_exopackage_enabled_for_secondary_dex(ctx: AnalysisContext) -> bool:
    return "secondary_dex" in getattr(ctx.attrs, "exopackage_modes", [])
