# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//:resources.bzl",
    "ResourceInfo",
    "gather_resources",
)
load("@prelude//java:class_to_srcs.bzl", "JavaClassToSourceMapInfo")
load("@prelude//java:dex.bzl", "get_dex_produced_from_java_library")
load("@prelude//java:dex_toolchain.bzl", "DexToolchainInfo")
load("@prelude//java/utils:java_utils.bzl", "get_path_separator")
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "merge_shared_libraries",
)
load("@prelude//utils:utils.bzl", "expect")

# JAVA PROVIDER DOCS
#
# Our core Java provider is JavaLibraryInfo. At a basic level, this provider needs to give
# its dependents the ability to do two things: compilation and packaging.
#
# Compilation
#
# When we compile, we need to add all of our dependencies to the classpath. That includes
# anything in `deps`, `exported_deps`, `provided_deps` and `exported_provided_deps`, (but
# not `runtime_deps`). Additionally, it includes anything that these dependencies export
# (via `exported_deps` or `exported_provided_deps`). For example, if A depends upon B,
# and B has an exported dependency on C, then we need to add both B and C to the classpath
# when compiling A, i.e. both B and C need to be part of B's `compiling_deps`.
#
# Therefore, the `compiling_deps` consist of the library's own output (if it exists) plus
# the `compiling_deps` of any `exported_deps` and `exported_provided_deps`.
#
# When we compile, we don't need to compile against the full library - instead, we just
# compile against the library's public interface, or ABI.
#
# Packaging
#
# When we package our Java code into a `java_binary`, we need to include all of the Java
# code that is need to run the application - i.e. all the transitive dependencies. That
# includes anything in `deps`, `exported_deps` and `runtime_deps` (but not `provided_deps`
# or `exported_provided_deps`). For example, if A depends upon B, and B has a `dep` on C
# and a `provided_dep` on D, then if we package A we also need to include B and C, but
# not D.
#
# Therefore, the `packaging_deps` consist of the library's own output (if it exists) plus
# the `packaging_deps` of any `deps`, `exported_deps` and `runtime_deps`.
#
# When we package, we need to use the full library (since we are actually going to be
# running the code contained in the library).
#
# We also need to package up any native code that is declared transitively. The
# `SharedLibraryInfo` also consists of the `SharedLibraryInfo` of any `deps`,
# `exported_deps` and `runtime_deps`.
#
# Android
#
# Because Android uses Java, and we don't currently have the ability to "overlay" our
# providers, the core Java providers are extended to support Android's requirements.
# This introduces some additional complexity.
#
# Android doesn't package Java bytecode, but instead it converts the Java bytecode
# .dex (Dalvik Executable) files that are packaged into the Android binary (either an
# APK or an AAB). Therefore, our `packaging_deps` contain not just a `jar` field but
# also a `dex` field. If the `dex` field is empty, then the dep should not be
# packaged into the APK - this is useful for things like `android_build_config` where
# we want the output (.jar) to be present in any Java annotation processors that we
# run, but not in the final binary (since we rewrite the build config at the binary
# level anyway).
#
# Android also provides the ability to run Proguard on your binary in order to
# remove unused classes etc. Each Java library can specify any classes that it wants
# to always keep etc, via a `proguard_config`. This config also needs to be added to
# the `packaging_deps`.
#
# Java-like rules also provide a "special" function that can be used inside queries:
# "classpath". `classpath(A)` returns all of the packaging deps of A, while
# `classpath(A, 1)` returns all of the first-order packaging deps of A.

JavaClasspathEntry = record(
    full_library = field("artifact"),
    abi = field("artifact"),
    # abi_as_dir is the abi .jar unzipped into a directory. If available, it is used to provide
    # .class level granularity for javacd and kotlincd dep-files.
    abi_as_dir = field(["artifact", None]),
    required_for_source_only_abi = field(bool),
)

def _args_for_ast_dumper(entry: JavaClasspathEntry.type):
    return [
        "--dependency",
        '"{}"'.format(entry.abi.owner),
        entry.abi,
    ]

def _args_for_compiling(entry: JavaClasspathEntry.type):
    return entry.abi

def _javacd_json(v):
    return v.abi

def _abi_to_abi_dir(entry: JavaClasspathEntry.type):
    if entry.abi_as_dir:
        return cmd_args([entry.abi, entry.abi_as_dir], delimiter = " ")
    return []

JavaCompilingDepsTSet = transitive_set(
    args_projections = {
        "abi_to_abi_dir": _abi_to_abi_dir,
        "args_for_ast_dumper": _args_for_ast_dumper,
        "args_for_compiling": _args_for_compiling,
    },
    json_projections = {
        "javacd_json": _javacd_json,
    },
)

JavaPackagingDep = record(
    label = Label,
    jar = ["artifact", None],
    dex = ["DexLibraryInfo", None],
    is_prebuilt_jar = bool,
    proguard_config = ["artifact", None],

    # An output that is used solely by the system to have an artifact bound to the target (that the core can then use to find
    # the right target from the given artifact).
    output_for_classpath_macro = "artifact",
)

def _full_jar_args(dep: JavaPackagingDep.type):
    if dep.jar:
        return cmd_args(dep.jar)
    return cmd_args()

def _args_for_classpath_macro(dep: JavaPackagingDep.type):
    return dep.output_for_classpath_macro

def _packaging_dep_javacd_json(dep: JavaPackagingDep.type):
    return dep.jar or ""

JavaPackagingDepTSet = transitive_set(
    args_projections = {
        "args_for_classpath_macro": _args_for_classpath_macro,
        "full_jar_args": _full_jar_args,
    },
    json_projections = {
        "javacd_json": _packaging_dep_javacd_json,
    },
)

JavaLibraryInfo = provider(
    doc = "Information about a java library and its dependencies",
    fields = [
        # Java dependencies exposed to dependent targets and supposed to be used during compilation.
        # Consisting of this library's own output, and the "compiling_deps" of any exported_deps and exported_provided_deps.
        #
        "compiling_deps",  # ["JavaCompilingDepsTSet", None]

        # An output of the library. If present then already included into `compiling_deps` field.
        "library_output",  # ["JavaClasspathEntry", None]

        # An output that is used solely by the system to have an artifact bound to the target (that the core can then use to find
        # the right target from the given artifact).
        "output_for_classpath_macro",  # "artifact"
    ],
)

JavaLibraryIntellijInfo = provider(
    doc = "Information about a java library that is required for Intellij project generation",
    fields = [
        # All the artifacts that were used in order to compile this library
        "compiling_classpath",  # ["artifact"]
        "generated_sources",  # ["artifact"]
        # Directory containing external annotation jars
        "annotation_jars_dir",  # ["artifact", None]
    ],
)

JavaPackagingInfo = provider(
    fields = [
        # Presents all java dependencies used to build this library and it's dependencies (all transitive deps except provided ones).
        # These deps must be included into the final artifact.
        "packaging_deps",  # ["JavaPackagingDepTSet", None],
    ],
)

KeystoreInfo = provider(
    fields = [
        "store",  # artifact
        "properties",  # artifact
    ],
)

JavaCompileOutputs = record(
    full_library = "artifact",
    class_abi = ["artifact", None],
    source_abi = ["artifact", None],
    source_only_abi = ["artifact", None],
    classpath_entry = JavaClasspathEntry.type,
    annotation_processor_output = ["artifact", None],
)

JavaProviders = record(
    java_library_info = JavaLibraryInfo.type,
    java_library_intellij_info = JavaLibraryIntellijInfo.type,
    java_packaging_info = JavaPackagingInfo.type,
    shared_library_info = SharedLibraryInfo.type,
    cxx_resource_info = ResourceInfo.type,
    template_placeholder_info = TemplatePlaceholderInfo.type,
    default_info = DefaultInfo.type,
    class_to_src_map = [JavaClassToSourceMapInfo.type, None],
)

def to_list(java_providers: JavaProviders.type) -> list["provider"]:
    providers = [
        java_providers.java_library_info,
        java_providers.java_library_intellij_info,
        java_providers.java_packaging_info,
        java_providers.shared_library_info,
        java_providers.cxx_resource_info,
        java_providers.template_placeholder_info,
        java_providers.default_info,
    ]
    if java_providers.class_to_src_map != None:
        providers.append(java_providers.class_to_src_map)
    return providers

# Creates a JavaCompileOutputs. `classpath_abi` can be set to specify a
# specific artifact to be used as the abi for the JavaClasspathEntry.
def make_compile_outputs(
        full_library: "artifact",
        class_abi: ["artifact", None] = None,
        source_abi: ["artifact", None] = None,
        source_only_abi: ["artifact", None] = None,
        classpath_abi: ["artifact", None] = None,
        classpath_abi_dir: ["artifact", None] = None,
        required_for_source_only_abi: bool = False,
        annotation_processor_output: ["artifact", None] = None) -> JavaCompileOutputs.type:
    expect(classpath_abi != None or classpath_abi_dir == None, "A classpath_abi_dir should only be provided if a classpath_abi is provided!")
    return JavaCompileOutputs(
        full_library = full_library,
        class_abi = class_abi,
        source_abi = source_abi,
        source_only_abi = source_only_abi,
        classpath_entry = JavaClasspathEntry(
            full_library = full_library,
            abi = classpath_abi or class_abi or full_library,
            abi_as_dir = classpath_abi_dir,
            required_for_source_only_abi = required_for_source_only_abi,
        ),
        annotation_processor_output = annotation_processor_output,
    )

def create_abi(actions: "actions", class_abi_generator: Dependency, library: "artifact") -> "artifact":
    # It's possible for the library to be created in a subdir that is
    # itself some actions output artifact, so we replace directory
    # separators to get a path that we can uniquely own.
    # TODO(cjhopman): This probably should take in the output path.
    class_abi = actions.declare_output("{}-class-abi.jar".format(library.short_path.replace("/", "_")))
    actions.run(
        [
            class_abi_generator[RunInfo],
            library,
            class_abi.as_output(),
        ],
        category = "class_abi_generation",
        identifier = library.short_path,
    )
    return class_abi

# Accumulate deps necessary for compilation, which consist of this library's output and compiling_deps of its exported deps
def derive_compiling_deps(
        actions: "actions",
        library_output: [JavaClasspathEntry.type, None],
        children: list[Dependency]) -> ["JavaCompilingDepsTSet", None]:
    if children:
        filtered_children = filter(
            None,
            [exported_dep.compiling_deps for exported_dep in filter(None, [x.get(JavaLibraryInfo) for x in children])],
        )
        children = filtered_children

    if not library_output and not children:
        return None

    if library_output:
        return actions.tset(JavaCompilingDepsTSet, children = children, value = library_output)
    else:
        return actions.tset(JavaCompilingDepsTSet, children = children)

def create_java_packaging_dep(
        ctx: AnalysisContext,
        library_jar: ["artifact", None] = None,
        output_for_classpath_macro: ["artifact", None] = None,
        needs_desugar: bool = False,
        desugar_deps: list["artifact"] = [],
        is_prebuilt_jar: bool = False,
        has_srcs: bool = True,
        dex_weight_factor: int = 1,
        proguard_config: ["artifact", None] = None) -> "JavaPackagingDep":
    dex_toolchain = getattr(ctx.attrs, "_dex_toolchain", None)
    if library_jar != None and has_srcs and dex_toolchain != None and ctx.attrs._dex_toolchain[DexToolchainInfo].d8_command != None:
        dex = get_dex_produced_from_java_library(
            ctx,
            ctx.attrs._dex_toolchain[DexToolchainInfo],
            library_jar,
            needs_desugar,
            desugar_deps,
            dex_weight_factor,
        )
    else:
        dex = None

    expect(library_jar != None or output_for_classpath_macro != None, "Must provide an output_for_classpath_macro if no library_jar is provided!")

    return JavaPackagingDep(
        label = ctx.label,
        jar = library_jar,
        dex = dex,
        is_prebuilt_jar = is_prebuilt_jar,
        proguard_config = proguard_config or getattr(ctx.attrs, "proguard_config", None),
        output_for_classpath_macro = output_for_classpath_macro or library_jar,
    )

def get_all_java_packaging_deps(ctx: AnalysisContext, deps: list[Dependency]) -> list["JavaPackagingDep"]:
    return get_all_java_packaging_deps_from_packaging_infos(ctx, filter(None, [x.get(JavaPackagingInfo) for x in deps]))

def get_all_java_packaging_deps_from_packaging_infos(ctx: AnalysisContext, infos: list["JavaPackagingInfo"]) -> list["JavaPackagingDep"]:
    children = filter(None, [info.packaging_deps for info in infos])
    if not children:
        return []

    tset = ctx.actions.tset(JavaPackagingDepTSet, children = children)

    return list(tset.traverse())

def get_all_java_packaging_deps_tset(
        ctx: AnalysisContext,
        java_packaging_infos: list["JavaPackagingInfo"],
        java_packaging_dep: [JavaPackagingDep.type, None] = None) -> [JavaPackagingDepTSet.type, None]:
    packaging_deps_kwargs = {}
    if java_packaging_dep:
        packaging_deps_kwargs["value"] = java_packaging_dep

    packaging_deps_children = filter(None, [info.packaging_deps for info in java_packaging_infos])
    if packaging_deps_children:
        packaging_deps_kwargs["children"] = packaging_deps_children

    return ctx.actions.tset(JavaPackagingDepTSet, **packaging_deps_kwargs) if packaging_deps_kwargs else None

# Accumulate deps necessary for packaging, which consist of all transitive java deps (except provided ones)
def get_java_packaging_info(
        ctx: AnalysisContext,
        raw_deps: list[Dependency],
        java_packaging_dep: [JavaPackagingDep.type, None] = None) -> JavaPackagingInfo.type:
    java_packaging_infos = filter(None, [x.get(JavaPackagingInfo) for x in raw_deps])
    packaging_deps = get_all_java_packaging_deps_tset(ctx, java_packaging_infos, java_packaging_dep)
    return JavaPackagingInfo(packaging_deps = packaging_deps)

def create_native_providers(actions: "actions", label: Label, packaging_deps: list[Dependency]) -> (SharedLibraryInfo.type, ResourceInfo.type):
    shared_library_info = merge_shared_libraries(
        actions,
        deps = filter(None, [x.get(SharedLibraryInfo) for x in packaging_deps]),
    )
    cxx_resource_info = ResourceInfo(resources = gather_resources(
        label,
        deps = packaging_deps,
    ))
    return shared_library_info, cxx_resource_info

def _create_non_template_providers(
        ctx: AnalysisContext,
        library_output: [JavaClasspathEntry.type, None],
        declared_deps: list[Dependency] = [],
        exported_deps: list[Dependency] = [],
        exported_provided_deps: list[Dependency] = [],
        runtime_deps: list[Dependency] = [],
        needs_desugar: bool = False,
        desugar_classpath: list["artifact"] = [],
        is_prebuilt_jar: bool = False,
        has_srcs: bool = True,
        proguard_config: ["artifact", None] = None) -> (JavaLibraryInfo.type, JavaPackagingInfo.type, SharedLibraryInfo.type, ResourceInfo.type):
    """Creates java library providers of type `JavaLibraryInfo` and `JavaPackagingInfo`.

    Args:
        library_output: optional JavaClasspathEntry that represents library output
        declared_deps: declared dependencies (usually comes from `deps` field of the rule)
        exported_deps: dependencies that are exposed to dependent rules as compiling deps
        exported_provided_deps: dependencies that are are exposed to dependent rules and not be included into packaging
        runtime_deps: dependencies that are used for packaging only
    """
    packaging_deps = declared_deps + exported_deps + runtime_deps
    shared_library_info, cxx_resource_info = create_native_providers(ctx.actions, ctx.label, packaging_deps)

    output_for_classpath_macro = library_output.abi if (library_output and library_output.abi.owner != None) else ctx.actions.write("dummy_output_for_classpath_macro.txt", "Unused")
    java_packaging_dep = create_java_packaging_dep(
        ctx,
        library_output.full_library if library_output else None,
        output_for_classpath_macro,
        needs_desugar,
        desugar_classpath,
        is_prebuilt_jar,
        has_srcs,
        proguard_config = proguard_config,
    )

    java_packaging_info = get_java_packaging_info(
        ctx,
        raw_deps = packaging_deps,
        java_packaging_dep = java_packaging_dep,
    )

    return (
        JavaLibraryInfo(
            compiling_deps = derive_compiling_deps(ctx.actions, library_output, exported_deps + exported_provided_deps),
            library_output = library_output,
            output_for_classpath_macro = output_for_classpath_macro,
        ),
        java_packaging_info,
        shared_library_info,
        cxx_resource_info,
    )

def create_template_info(packaging_info: JavaPackagingInfo.type, first_order_classpath_libs: list["artifact"]) -> TemplatePlaceholderInfo.type:
    return TemplatePlaceholderInfo(keyed_variables = {
        "classpath": cmd_args(packaging_info.packaging_deps.project_as_args("full_jar_args"), delimiter = get_path_separator()) if packaging_info.packaging_deps else cmd_args(),
        "classpath_including_targets_with_no_output": cmd_args(packaging_info.packaging_deps.project_as_args("args_for_classpath_macro"), delimiter = get_path_separator()),
        "first_order_classpath": cmd_args(first_order_classpath_libs, delimiter = get_path_separator()),
    })

def create_java_library_providers(
        ctx: AnalysisContext,
        library_output: [JavaClasspathEntry.type, None],
        declared_deps: list[Dependency] = [],
        exported_deps: list[Dependency] = [],
        provided_deps: list[Dependency] = [],
        exported_provided_deps: list[Dependency] = [],
        runtime_deps: list[Dependency] = [],
        needs_desugar: bool = False,
        is_prebuilt_jar: bool = False,
        has_srcs: bool = True,
        generated_sources: list["artifact"] = [],
        annotation_jars_dir: ["artifact", None] = None,
        proguard_config: ["artifact", None] = None) -> (JavaLibraryInfo.type, JavaPackagingInfo.type, SharedLibraryInfo.type, ResourceInfo.type, TemplatePlaceholderInfo.type, JavaLibraryIntellijInfo.type):
    first_order_classpath_deps = filter(None, [x.get(JavaLibraryInfo) for x in declared_deps + exported_deps + runtime_deps])
    first_order_classpath_libs = [dep.output_for_classpath_macro for dep in first_order_classpath_deps]

    compiling_deps = derive_compiling_deps(ctx.actions, None, declared_deps + exported_deps + provided_deps + exported_provided_deps)
    compiling_classpath = [dep.full_library for dep in (list(compiling_deps.traverse()) if compiling_deps else [])]
    desugar_classpath = compiling_classpath if needs_desugar else []

    library_info, packaging_info, shared_library_info, cxx_resource_info = _create_non_template_providers(
        ctx,
        library_output = library_output,
        declared_deps = declared_deps,
        exported_deps = exported_deps,
        exported_provided_deps = exported_provided_deps,
        runtime_deps = runtime_deps,
        needs_desugar = needs_desugar,
        desugar_classpath = desugar_classpath,
        is_prebuilt_jar = is_prebuilt_jar,
        has_srcs = has_srcs,
        proguard_config = proguard_config,
    )

    first_order_libs = first_order_classpath_libs + [library_info.library_output.full_library] if library_info.library_output else first_order_classpath_libs
    template_info = create_template_info(packaging_info, first_order_libs)

    intellij_info = JavaLibraryIntellijInfo(
        compiling_classpath = compiling_classpath,
        generated_sources = generated_sources,
        annotation_jars_dir = annotation_jars_dir,
    )

    return (library_info, packaging_info, shared_library_info, cxx_resource_info, template_info, intellij_info)
