# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",  # @unused Used as a type
    "make_artifact_tset",
)
load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//:resources.bzl",
    "ResourceInfo",
    "gather_resources",
)
load(
    "@prelude//android:android_providers.bzl",
    "merge_android_packageable_info",
)
load(
    "@prelude//apple:apple_frameworks.bzl",
    "apple_build_link_args_with_deduped_flags",
    "apple_create_frameworks_linkable",
    "apple_get_link_info_by_deduping_link_infos",
)
load(
    "@prelude//apple:xcode.bzl",
    "apple_get_xcode_absolute_path_prefix",
)
load(
    "@prelude//apple/swift:swift_runtime.bzl",
    "create_swift_runtime_linkable",
)
load(
    "@prelude//ide_integrations:xcode.bzl",
    "XCODE_DATA_SUB_TARGET",
    "XcodeDataInfo",
    "generate_xcode_data",
)
load(
    "@prelude//java:java_providers.bzl",
    "get_java_packaging_info",
)
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference", "get_link_execution_preference")
load(
    "@prelude//linking:link_groups.bzl",
    "LinkGroupLib",  # @unused Used as a type
    "LinkGroupLibInfo",
    "gather_link_group_libs",
    "merge_link_group_lib_info",
)
load(
    "@prelude//linking:link_info.bzl",
    "ArchiveLinkable",
    "FrameworksLinkable",  # @unused Used as a type
    "LinkArgs",
    "LinkInfo",
    "LinkInfos",
    "LinkOrdering",
    "LinkStyle",
    "Linkage",
    "LinkedObject",  # @unused Used as a type
    "ObjectsLinkable",
    "STATIC_LINK_STYLES",
    "SharedLibLinkable",
    "SwiftRuntimeLinkable",  # @unused Used as a type
    "SwiftmoduleLinkable",  # @unused Used as a type
    "create_merged_link_info",
    "get_actual_link_style",
    "get_link_args",
    "get_link_styles_for_linkage",
    "process_link_style_for_pic_behavior",
    "unpack_link_args",
    "wrap_link_info",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "AnnotatedLinkableRoot",
    "DlopenableLibraryInfo",
    "LinkableRootInfo",
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
    "get_linkable_graph_node_map_func",
    "linkable_deps",
)
load("@prelude//linking:shared_libraries.bzl", "SharedLibraryInfo", "create_shared_libraries", "merge_shared_libraries")
load("@prelude//linking:strip.bzl", "strip_debug_info")
load(
    "@prelude//utils:utils.bzl",
    "expect",
    "flatten",
    "is_any",
    "map_val",
    "value_or",
)
load(":archive.bzl", "make_archive")
load(
    ":argsfiles.bzl",
    "ABS_ARGSFILES_SUBTARGET",
    "ARGSFILES_SUBTARGET",
    "get_argsfiles_output",
)
load(":bitcode.bzl", "BitcodeBundle", "BitcodeBundleInfo", "BitcodeTSet", "make_bitcode_bundle")
load(
    ":comp_db.bzl",
    "CxxCompilationDbInfo",
    "create_compilation_database",
    "make_compilation_db_info",
)
load(
    ":compile.bzl",
    "CxxCompileCommandOutput",
    "compile_cxx",
    "create_compile_cmds",
)
load(":cxx_context.bzl", "get_cxx_platform_info", "get_cxx_toolchain_info")
load(
    ":cxx_library_utility.bzl",
    "OBJECTS_SUBTARGET",
    "cxx_attr_deps",
    "cxx_attr_exported_deps",
    "cxx_attr_exported_linker_flags",
    "cxx_attr_exported_post_linker_flags",
    "cxx_attr_link_style",
    "cxx_attr_linker_flags",
    "cxx_attr_preferred_linkage",
    "cxx_attr_resources",
    "cxx_inherited_link_info",
    "cxx_is_gnu",
    "cxx_mk_shlib_intf",
    "cxx_objects_sub_target",
    "cxx_platform_supported",
    "cxx_use_shlib_intfs",
)
load(":cxx_toolchain_types.bzl", "is_bitcode_format")
load(
    ":cxx_types.bzl",
    "CxxRuleConstructorParams",  # @unused Used as a type
)
load(
    ":link.bzl",
    "CxxLinkResult",  # @unused Used as a type
    "CxxLinkerMapData",
    "cxx_link_shared_library",
)
load(
    ":link_groups.bzl",
    "LINK_GROUP_MAP_DATABASE_SUB_TARGET",
    "get_filtered_labels_to_links_map",
    "get_filtered_links",
    "get_filtered_targets",
    "get_link_group",
    "get_link_group_info",
    "get_link_group_map_json",
    "get_link_group_preferred_linkage",
)
load(
    ":linker.bzl",
    "get_default_shared_library_name",
    "get_ignore_undefined_symbols_flags",
    "get_shared_library_name",
    "get_shared_library_name_for_param",
)
load(
    ":omnibus.bzl",
    "create_linkable_root",
)
load(":platform.bzl", "cxx_by_platform")
load(
    ":preprocessor.bzl",
    "CPreprocessor",  # @unused Used as a type
    "CPreprocessorForTestsInfo",
    "CPreprocessorInfo",  # @unused Used as a type
    "cxx_exported_preprocessor_info",
    "cxx_inherited_preprocessor_infos",
    "cxx_merge_cpreprocessors",
    "cxx_private_preprocessor_info",
)

# An output of a `cxx_library`, consisting of required `default` artifact and optional
# `other` outputs that should also be materialized along with it.
CxxLibraryOutput = record(
    default = field("artifact"),
    # The object files used to create the artifact in `default`.
    object_files = field(["artifact"], []),
    # Additional outputs that are implicitly used along with the above output
    # (e.g. external object files referenced by a thin archive).
    #
    # Note: It's possible that this can contain some of the artifacts which are
    # also present in object_files.
    other = field(["artifact"], []),
    bitcode_bundle = field([BitcodeBundle.type, None], None),
    # Additional debug info which is not included in the library output.
    external_debug_info = field(ArtifactTSet.type, ArtifactTSet()),
    # A shared shared library may have an associated dwp file with
    # its corresponding DWARF debug info.
    # May be None when Split DWARF is disabled, for static/static-pic libraries,
    # for some types of synthetic link objects or for pre-built shared libraries.
    dwp = field(["artifact", None], None),
    # A shared shared library may have an associated PDB file with
    # its corresponding Windows debug info.
    pdb = field(["artifact", None], None),
    linker_map = field([CxxLinkerMapData.type, None], None),
)

# The outputs of either archiving or linking the outputs of the library
_CxxAllLibraryOutputs = record(
    # The outputs for each type of link style.
    outputs = field({LinkStyle.type: [CxxLibraryOutput.type, None]}),
    # The link infos that are part of each output based on link style.
    libraries = field({LinkStyle.type: LinkInfos.type}),
    # Extra sub targets to be returned as outputs of this rule, by link style.
    sub_targets = field({LinkStyle.type: {str: [DefaultInfo.type]}}, default = {}),
    # Extra providers to be returned consumers of this rule.
    providers = field(["provider"], default = []),
    # Shared object name to shared library mapping.
    solibs = field({str: LinkedObject.type}),
)

# The output of compiling all the source files in the library, containing
# the commands use to compile them and all the object file variants.
_CxxCompiledSourcesOutput = record(
    # Compile commands used to compile the source files or generate object files
    compile_cmds = field(CxxCompileCommandOutput.type),
    # Non-PIC object files
    objects = field([["artifact"], None]),
    # Those outputs which are bitcode
    bitcode_objects = field([["artifact"], None]),
    # yaml file with optimization remarks about clang compilation
    clang_remarks = field([["artifact"], None]),
    # json file with trace information about clang compilation
    clang_traces = field([["artifact"], None]),
    # Externally referenced debug info, which doesn't get linked with the
    # object (e.g. the above `.o` when using `-gsplit-dwarf=single` or the
    # the `.dwo` when using `-gsplit-dwarf=split`).
    external_debug_info = field([["artifact"], None]),
    objects_have_external_debug_info = field(bool, False),
    # PIC Object files
    pic_objects = field([["artifact"], None]),
    pic_objects_have_external_debug_info = field(bool, False),
    pic_external_debug_info = field([["artifact"], None]),
    # yaml file with optimization remarks about clang compilation
    pic_clang_remarks = field([["artifact"], None]),
    # json file with trace information about clang compilation
    pic_clang_traces = field([["artifact"], None]),
    # Non-PIC object files stripped of debug information
    stripped_objects = field([["artifact"], None]),
    # PIC object files stripped of debug information
    stripped_pic_objects = field([["artifact"], None]),
    objects_sub_target = field(["provider"]),
)

# The outputs of a cxx_library_parameterized rule.
_CxxLibraryParameterizedOutput = record(
    # The default output of a cxx library rule
    default_output = field([CxxLibraryOutput.type, None], None),
    # The other outputs available
    all_outputs = field([_CxxAllLibraryOutputs.type, None], None),
    # Any generated sub targets as requested by impl_params
    sub_targets = field({str: ["provider"]}),
    # Any generated providers as requested by impl_params
    providers = field(["provider"]),
    # XcodeDataInfo provider, returned separately as we cannot check
    # provider type from providers above
    xcode_data_info = field([XcodeDataInfo.type, None], None),
    # CxxCompilationDbInfo provider, returned separately as we cannot check
    # provider type from providers above
    cxx_compilationdb_info = field([CxxCompilationDbInfo.type, None], None),
    # LinkableRootInfo provider, same as above.
    linkable_root = field([LinkableRootInfo.type, None], None),
    # A bundle of all bitcode files as a subtarget
    bitcode_bundle = field([BitcodeBundle.type, None], None),
)

def cxx_library_parameterized(ctx: AnalysisContext, impl_params: "CxxRuleConstructorParams") -> _CxxLibraryParameterizedOutput.type:
    """
    Defines the outputs for a cxx library, return the default output and any subtargets and providers based upon the requested params.
    """

    if not cxx_platform_supported(ctx):
        sub_targets = {}

        # Needed to handle cases of the named output (e.g. [static-pic]) being called directly.
        for link_style in get_link_styles_for_linkage(cxx_attr_preferred_linkage(ctx)):
            sub_targets[link_style.value.replace("_", "-")] = [DefaultInfo(default_output = None)]

        return _CxxLibraryParameterizedOutput(
            providers = [
                DefaultInfo(default_output = None, sub_targets = sub_targets),
                SharedLibraryInfo(set = None),
            ],
            sub_targets = sub_targets,
        )

    non_exported_deps = cxx_attr_deps(ctx)
    exported_deps = cxx_attr_exported_deps(ctx)

    # TODO(T110378095) right now we implement reexport of exported_* flags manually, we should improve/automate that in the macro layer

    absolute_path_prefix = apple_get_xcode_absolute_path_prefix()

    # Gather preprocessor inputs.
    (own_non_exported_preprocessor_info, test_preprocessor_infos) = cxx_private_preprocessor_info(
        ctx = ctx,
        headers_layout = impl_params.headers_layout,
        extra_preprocessors = impl_params.extra_preprocessors,
        non_exported_deps = non_exported_deps,
        is_test = impl_params.is_test,
        absolute_path_prefix = absolute_path_prefix,
    )
    own_exported_preprocessor_info = cxx_exported_preprocessor_info(ctx, impl_params.headers_layout, impl_params.extra_exported_preprocessors, absolute_path_prefix)
    own_preprocessors = [own_non_exported_preprocessor_info, own_exported_preprocessor_info] + test_preprocessor_infos

    inherited_non_exported_preprocessor_infos = cxx_inherited_preprocessor_infos(
        non_exported_deps + filter(None, [ctx.attrs.precompiled_header]),
    )
    inherited_exported_preprocessor_infos = cxx_inherited_preprocessor_infos(exported_deps)

    preferred_linkage = cxx_attr_preferred_linkage(ctx)

    compiled_srcs = cxx_compile_srcs(
        ctx = ctx,
        impl_params = impl_params,
        own_preprocessors = own_preprocessors,
        inherited_non_exported_preprocessor_infos = inherited_non_exported_preprocessor_infos,
        inherited_exported_preprocessor_infos = inherited_exported_preprocessor_infos,
        absolute_path_prefix = absolute_path_prefix,
        preferred_linkage = preferred_linkage,
    )

    sub_targets = {}
    providers = []

    if len(ctx.attrs.tests) > 0 and impl_params.generate_providers.preprocessor_for_tests:
        providers.append(
            CPreprocessorForTestsInfo(
                test_names = [test_target.name for test_target in ctx.attrs.tests],
                own_non_exported_preprocessor = own_non_exported_preprocessor_info,
            ),
        )

    if impl_params.generate_sub_targets.argsfiles:
        sub_targets[ARGSFILES_SUBTARGET] = [get_argsfiles_output(ctx, compiled_srcs.compile_cmds.argsfiles.relative, "argsfiles")]
        if absolute_path_prefix:
            sub_targets[ABS_ARGSFILES_SUBTARGET] = [get_argsfiles_output(ctx, compiled_srcs.compile_cmds.argsfiles.absolute, "abs-argsfiles")]

    if impl_params.generate_sub_targets.clang_remarks:
        if compiled_srcs.clang_remarks:
            sub_targets["clang-remarks"] = [DefaultInfo(
                default_outputs = compiled_srcs.clang_remarks,
            )]

        if compiled_srcs.pic_clang_remarks:
            sub_targets["pic-clang-remarks"] = [DefaultInfo(
                default_outputs = compiled_srcs.pic_clang_remarks,
            )]

    if impl_params.generate_sub_targets.clang_traces:
        if compiled_srcs.clang_traces:
            sub_targets["clang-trace"] = [DefaultInfo(
                default_outputs = compiled_srcs.clang_traces,
            )]

        if compiled_srcs.pic_clang_traces:
            sub_targets["pic-clang-trace"] = [DefaultInfo(
                default_outputs = compiled_srcs.pic_clang_traces,
            )]

    if impl_params.generate_sub_targets.objects:
        sub_targets[OBJECTS_SUBTARGET] = compiled_srcs.objects_sub_target

    # Compilation DB.
    if impl_params.generate_sub_targets.compilation_database:
        comp_db = create_compilation_database(ctx, compiled_srcs.compile_cmds.comp_db_compile_cmds)
        sub_targets["compilation-database"] = [comp_db]
    comp_db_info = None
    if impl_params.generate_providers.compilation_database:
        comp_db_info = make_compilation_db_info(compiled_srcs.compile_cmds.comp_db_compile_cmds, get_cxx_toolchain_info(ctx), get_cxx_platform_info(ctx))
        providers.append(comp_db_info)

    # Link Groups
    link_group = get_link_group(ctx)
    link_group_info = get_link_group_info(ctx)

    if link_group_info:
        link_groups = link_group_info.groups
        link_group_mappings = link_group_info.mappings
        link_group_deps = [link_group_info.graph]
        link_group_libs = gather_link_group_libs(
            deps = non_exported_deps + exported_deps,
        )
        providers.append(link_group_info)
    else:
        link_groups = {}
        link_group_mappings = {}
        link_group_deps = []
        link_group_libs = {}
    link_group_preferred_linkage = get_link_group_preferred_linkage(link_groups.values())

    # Create the linkable graph from the library's deps, exported deps and any link group deps.
    linkable_graph_deps = non_exported_deps + exported_deps
    deps_linkable_graph = create_linkable_graph(
        ctx,
        deps = linkable_graph_deps,
        children = link_group_deps,
    )

    frameworks_linkable = apple_create_frameworks_linkable(ctx)
    swiftmodule_linkable = impl_params.swiftmodule_linkable
    swift_runtime_linkable = create_swift_runtime_linkable(ctx)
    shared_links, link_group_map, link_execution_preference = _get_shared_library_links(
        ctx,
        get_linkable_graph_node_map_func(deps_linkable_graph),
        link_group,
        link_group_mappings,
        link_group_preferred_linkage,
        link_group_libs,
        exported_deps,
        non_exported_deps,
        impl_params.force_link_group_linking,
        frameworks_linkable,
        force_static_follows_dependents = impl_params.link_groups_force_static_follows_dependents,
        swiftmodule_linkable = swiftmodule_linkable,
        swift_runtime_linkable = swift_runtime_linkable,
    )
    if impl_params.generate_sub_targets.link_group_map and link_group_map:
        sub_targets[LINK_GROUP_MAP_DATABASE_SUB_TARGET] = [link_group_map]

    extra_static_linkables = []
    if frameworks_linkable:
        extra_static_linkables.append(frameworks_linkable)
    if swiftmodule_linkable:
        extra_static_linkables.append(swiftmodule_linkable)
    if swift_runtime_linkable:
        extra_static_linkables.append(swift_runtime_linkable)

    library_outputs = _form_library_outputs(
        ctx = ctx,
        impl_params = impl_params,
        compiled_srcs = compiled_srcs,
        preferred_linkage = preferred_linkage,
        shared_links = shared_links,
        extra_static_linkables = extra_static_linkables,
        gnu_use_link_groups = cxx_is_gnu(ctx) and bool(link_group_mappings),
        link_execution_preference = link_execution_preference,
    )

    for _, link_style_sub_targets in library_outputs.sub_targets.items():
        for key in link_style_sub_targets.keys():
            expect(not key in sub_targets, "The subtarget `{}` already exists!".format(key))
        sub_targets.update(link_style_sub_targets)

    providers.extend(library_outputs.providers)

    pic_behavior = get_cxx_toolchain_info(ctx).pic_behavior
    actual_link_style = get_actual_link_style(cxx_attr_link_style(ctx), preferred_linkage, pic_behavior)

    # Output sub-targets for all link-styles.
    if impl_params.generate_sub_targets.link_style_outputs or impl_params.generate_providers.link_style_outputs:
        actual_link_style_providers = []

        for link_style, output in library_outputs.outputs.items():
            link_style_sub_targets, link_style_providers = impl_params.link_style_sub_targets_and_providers_factory(
                link_style,
                ctx,
                output,
            )

            # Add any subtargets for this link style.
            link_style_sub_targets.update(library_outputs.sub_targets.get(link_style, {}))

            if impl_params.generate_sub_targets.link_style_outputs:
                sub_target_providers = []
                if output:
                    sub_target_providers.append(DefaultInfo(
                        default_output = output.default,
                        other_outputs = output.other,
                        sub_targets = link_style_sub_targets,
                    ))
                if link_style_providers:
                    sub_target_providers.extend(link_style_providers)

                sub_targets[link_style.value.replace("_", "-")] = sub_target_providers

                if link_style == actual_link_style:
                    # If we have additional providers for the current link style,
                    # add them to the list of default providers
                    actual_link_style_providers += link_style_providers

                    # In addition, ensure any subtargets for the active link style
                    # can be accessed as a default subtarget
                    for link_style_sub_target_name, link_style_sub_target_providers in link_style_sub_targets.items():
                        sub_targets[link_style_sub_target_name] = link_style_sub_target_providers

        if impl_params.generate_providers.link_style_outputs:
            providers += actual_link_style_providers

    # Create the default output for the library rule given it's link style and preferred linkage
    default_output = library_outputs.outputs[actual_link_style]

    if default_output and default_output.bitcode_bundle:
        sub_targets["bitcode"] = [DefaultInfo(default_output = default_output.bitcode_bundle.artifact)]

    # Define the xcode data sub target
    xcode_data_info = None
    if impl_params.generate_sub_targets.xcode_data:
        xcode_data_default_info, xcode_data_info = generate_xcode_data(
            ctx,
            rule_type = impl_params.rule_type,
            output = default_output.default if default_output else None,
            populate_rule_specific_attributes_func = impl_params.cxx_populate_xcode_attributes_func,
            srcs = impl_params.srcs + impl_params.additional.srcs,
            argsfiles = compiled_srcs.compile_cmds.argsfiles.absolute if absolute_path_prefix else compiled_srcs.compile_cmds.argsfiles.relative,
            product_name = get_default_cxx_library_product_name(ctx, impl_params),
        )
        sub_targets[XCODE_DATA_SUB_TARGET] = xcode_data_default_info
        providers.append(xcode_data_info)

    # Propagate link info provider.
    if impl_params.generate_providers.merged_native_link_info or impl_params.generate_providers.template_placeholders:
        # Gather link inputs.
        inherited_non_exported_link = cxx_inherited_link_info(ctx, non_exported_deps)
        inherited_exported_link = cxx_inherited_link_info(ctx, exported_deps)

        merged_native_link_info = create_merged_link_info(
            ctx,
            pic_behavior,
            # Add link info for each link style,
            library_outputs.libraries,
            preferred_linkage = preferred_linkage,
            # Export link info from non-exported deps (when necessary).
            deps = [inherited_non_exported_link],
            # Export link info from out (exported) deps.
            exported_deps = [inherited_exported_link],
            frameworks_linkable = frameworks_linkable,
            swift_runtime_linkable = swift_runtime_linkable,
        )
        if impl_params.generate_providers.merged_native_link_info:
            providers.append(merged_native_link_info)
    else:
        # This code sets merged_native_link_info only in some cases, leaving it unassigned in others.
        # Add a fake definition set to None so the assignment checker is satisfied.
        merged_native_link_info = None

    # Propagate shared libraries up the tree.
    if impl_params.generate_providers.shared_libraries:
        providers.append(merge_shared_libraries(
            ctx.actions,
            create_shared_libraries(ctx, library_outputs.solibs),
            filter(None, [x.get(SharedLibraryInfo) for x in non_exported_deps]) +
            filter(None, [x.get(SharedLibraryInfo) for x in exported_deps]),
        ))

    propagated_preprocessor_merge_list = inherited_exported_preprocessor_infos
    if _attr_reexport_all_header_dependencies(ctx):
        propagated_preprocessor_merge_list = inherited_non_exported_preprocessor_infos + propagated_preprocessor_merge_list
    propagated_preprocessor = cxx_merge_cpreprocessors(ctx, [own_exported_preprocessor_info], propagated_preprocessor_merge_list)
    if impl_params.generate_providers.preprocessors:
        providers.append(propagated_preprocessor)

    # Propagated_exported_preprocessor_info is used for pcm compilation, which isn't possible for non-modular targets.
    propagated_exported_preprocessor_info = propagated_preprocessor if impl_params.rule_type == "apple_library" and ctx.attrs.modular else None
    additional_providers = impl_params.additional.additional_providers_factory(propagated_exported_preprocessor_info) if impl_params.additional.additional_providers_factory else []

    # For v1's `#headers` functionality.
    if impl_params.generate_sub_targets.headers:
        sub_targets["headers"] = [propagated_preprocessor, create_merged_link_info(
            ctx,
            pic_behavior = pic_behavior,
            preferred_linkage = Linkage("static"),
            frameworks_linkable = frameworks_linkable,
        ), LinkGroupLibInfo(libs = {}), SharedLibraryInfo(set = None)] + additional_providers

    if getattr(ctx.attrs, "supports_header_symlink_subtarget", False):
        header_symlink_mapping = {}
        for records in propagated_preprocessor.set.traverse():
            for record in records:
                for header in record.headers:
                    header_path = header.name
                    if header.namespace:
                        header_path = paths.join(header.namespace, header_path)
                    header_symlink_mapping[paths.normalize(header_path)] = header.artifact

        sub_targets["header-symlink-tree"] = [DefaultInfo(
            default_output = ctx.actions.symlinked_dir("header_symlink_tree", header_symlink_mapping),
        )]

    for additional_subtarget, subtarget_providers in impl_params.additional.subtargets.items():
        sub_targets[additional_subtarget] = subtarget_providers

    # Omnibus root provider.
    linkable_root = None
    if impl_params.generate_providers.omnibus_root:
        if impl_params.use_soname:
            soname = _soname(ctx, impl_params)
        else:
            soname = None
        linker_type = get_cxx_toolchain_info(ctx).linker_info.type
        linkable_root = create_linkable_root(
            ctx,
            name = soname,
            link_infos = LinkInfos(
                default = LinkInfo(
                    pre_flags = cxx_attr_linker_flags(ctx) + cxx_attr_exported_linker_flags(ctx),
                    post_flags = _attr_post_linker_flags(ctx) + cxx_attr_exported_post_linker_flags(ctx),
                    linkables = [ObjectsLinkable(
                        objects = compiled_srcs.pic_objects,
                        linker_type = linker_type,
                        link_whole = True,
                    )],
                    external_debug_info = make_artifact_tset(
                        actions = ctx.actions,
                        label = ctx.label,
                        artifacts = (
                            compiled_srcs.pic_external_debug_info +
                            (compiled_srcs.pic_objects if compiled_srcs.pic_objects_have_external_debug_info else [])
                        ),
                        children = impl_params.additional.static_external_debug_info,
                    ),
                ),
                stripped = LinkInfo(
                    pre_flags = cxx_attr_linker_flags(ctx) + cxx_attr_exported_linker_flags(ctx),
                    post_flags = _attr_post_linker_flags(ctx) + cxx_attr_exported_post_linker_flags(ctx),
                    linkables = [ObjectsLinkable(
                        objects = compiled_srcs.stripped_pic_objects,
                        linker_type = linker_type,
                        link_whole = True,
                    )],
                ),
            ),
            deps = non_exported_deps + exported_deps,
            graph = deps_linkable_graph,
            create_shared_root = impl_params.is_omnibus_root or impl_params.force_emit_omnibus_shared_root,
        )
        providers.append(linkable_root)

        if linkable_root.shared_root:
            sub_targets["omnibus-shared-root"] = [DefaultInfo(
                default_output = linkable_root.shared_root.product.shared_library.output,
            )]

        # Mark libraries that support `dlopen`.
        if getattr(ctx.attrs, "supports_python_dlopen", False):
            providers.append(DlopenableLibraryInfo())

    # Augment and provide the linkable graph.
    if impl_params.generate_providers.linkable_graph:
        roots = {}
        if linkable_root != None and impl_params.is_omnibus_root:
            roots[ctx.label] = AnnotatedLinkableRoot(root = linkable_root)

        merged_linkable_graph = create_linkable_graph(
            ctx,
            node = create_linkable_graph_node(
                ctx,
                linkable_node = create_linkable_node(
                    ctx = ctx,
                    preferred_linkage = preferred_linkage,
                    deps = non_exported_deps,
                    exported_deps = exported_deps,
                    # If we don't have link input for this link style, we pass in `None` so
                    # that omnibus knows to avoid it.
                    link_infos = library_outputs.libraries,
                    shared_libs = library_outputs.solibs,
                ),
                roots = roots,
                excluded = {ctx.label: None} if not value_or(ctx.attrs.supports_merged_linking, True) else {},
            ),
            children = [deps_linkable_graph],
        )
        providers.append(merged_linkable_graph)

    # C++ resource.
    if impl_params.generate_providers.resources:
        providers.append(ResourceInfo(resources = gather_resources(
            label = ctx.label,
            resources = cxx_attr_resources(ctx),
            deps = non_exported_deps + exported_deps,
        )))

    if impl_params.generate_providers.template_placeholders:
        templ_vars = {}

        # Some rules, e.g. fbcode//thrift/lib/cpp:thrift-core-module
        # define preprocessor flags as things like: -DTHRIFT_PLATFORM_CONFIG=<thrift/facebook/PlatformConfig.h>
        # and unless they get quoted, they break shell syntax.
        cxx_preprocessor_flags = cmd_args()
        cxx_compiler_info = get_cxx_toolchain_info(ctx).cxx_compiler_info
        cxx_preprocessor_flags.add(cmd_args(cxx_compiler_info.preprocessor_flags or [], quote = "shell"))
        cxx_preprocessor_flags.add(cmd_args(propagated_preprocessor.set.project_as_args("args"), quote = "shell"))
        cxx_preprocessor_flags.add(propagated_preprocessor.set.project_as_args("include_dirs"))
        templ_vars["cxxppflags"] = cxx_preprocessor_flags

        c_preprocessor_flags = cmd_args()
        c_compiler_info = get_cxx_toolchain_info(ctx).c_compiler_info
        c_preprocessor_flags.add(cmd_args(c_compiler_info.preprocessor_flags or [], quote = "shell"))
        c_preprocessor_flags.add(cmd_args(propagated_preprocessor.set.project_as_args("args"), quote = "shell"))
        c_preprocessor_flags.add(propagated_preprocessor.set.project_as_args("include_dirs"))
        templ_vars["cppflags"] = c_preprocessor_flags

        # Add in ldflag macros.
        for link_style in (LinkStyle("static"), LinkStyle("static_pic")):
            name = "ldflags-" + link_style.value.replace("_", "-")
            args = cmd_args()
            linker_info = get_cxx_toolchain_info(ctx).linker_info
            args.add(linker_info.linker_flags or [])
            args.add(unpack_link_args(
                get_link_args(
                    merged_native_link_info,
                    link_style,
                ),
            ))
            templ_vars[name] = args

        # TODO(T110378127): To implement `$(ldflags-shared ...)` properly, we'd need
        # to setup a symink tree rule for all transitive shared libs.  Since this
        # currently would be pretty costly (O(N^2)?), and since it's not that
        # commonly used anyway, just use `static-pic` instead.  Longer-term, once
        # v1 is gone, macros that use `$(ldflags-shared ...)` (e.g. Haskell's
        # hsc2hs) can move to a v2 rules-based API to avoid needing this macro.
        templ_vars["ldflags-shared"] = templ_vars["ldflags-static-pic"]

        providers.append(TemplatePlaceholderInfo(keyed_variables = templ_vars))

    # It is possible (e.g. in a java binary or an Android APK) to have C++ libraries that depend
    # upon Java libraries (through JNI). In some cases those Java libraries are not depended upon
    # anywhere else, so we need to expose them here to ensure that they are packaged into the
    # final binary.
    if impl_params.generate_providers.java_packaging_info:
        providers.append(get_java_packaging_info(ctx, non_exported_deps + exported_deps))

    # TODO(T107163344) this shouldn't be in cxx_library itself, use overlays to remove it.
    if impl_params.generate_providers.android_packageable_info:
        providers.append(merge_android_packageable_info(ctx.label, ctx.actions, non_exported_deps + exported_deps))

    bitcode_bundle = default_output.bitcode_bundle if default_output != None else None
    if bitcode_bundle:
        bc_provider = BitcodeBundleInfo(bitcode = bitcode_bundle, bitcode_bundle = ctx.actions.tset(BitcodeTSet, value = bitcode_bundle))
        additional_providers.append(bc_provider)

    if impl_params.generate_providers.default:
        providers.append(DefaultInfo(
            default_output = default_output.default if default_output != None else None,
            other_outputs = default_output.other if default_output != None else [],
            sub_targets = sub_targets,
        ))

    # Propagate all transitive link group lib roots up the tree, so that the
    # final executable can use them.
    if impl_params.generate_providers.merged_native_link_info:
        providers.append(
            merge_link_group_lib_info(
                label = ctx.label,
                name = link_group,
                shared_libs = library_outputs.solibs,
                shared_link_infos = library_outputs.libraries.get(LinkStyle("shared")),
                deps = exported_deps + non_exported_deps,
            ),
        )

    return _CxxLibraryParameterizedOutput(
        default_output = default_output,
        all_outputs = library_outputs,
        sub_targets = sub_targets,
        providers = providers + additional_providers,
        xcode_data_info = xcode_data_info,
        cxx_compilationdb_info = comp_db_info,
        linkable_root = linkable_root,
        bitcode_bundle = bitcode_bundle,
    )

def get_default_cxx_library_product_name(ctx, impl_params) -> str:
    preferred_linkage = cxx_attr_preferred_linkage(ctx)
    link_style = get_actual_link_style(cxx_attr_link_style(ctx), preferred_linkage, get_cxx_toolchain_info(ctx).pic_behavior)
    if link_style in STATIC_LINK_STYLES:
        return _base_static_library_name(ctx, False)
    else:
        return _soname(ctx, impl_params)

def cxx_compile_srcs(
        ctx: AnalysisContext,
        impl_params: CxxRuleConstructorParams.type,
        own_preprocessors: list[CPreprocessor.type],
        inherited_non_exported_preprocessor_infos: list[CPreprocessorInfo.type],
        inherited_exported_preprocessor_infos: list[CPreprocessorInfo.type],
        absolute_path_prefix: [str, None],
        preferred_linkage: Linkage.type) -> _CxxCompiledSourcesOutput.type:
    """
    Compile objects we'll need for archives and shared libraries.
    """

    # Create the commands and argsfiles to use for compiling each source file
    compile_cmd_output = create_compile_cmds(
        ctx = ctx,
        impl_params = impl_params,
        own_preprocessors = own_preprocessors,
        inherited_preprocessor_infos = inherited_non_exported_preprocessor_infos + inherited_exported_preprocessor_infos,
        absolute_path_prefix = absolute_path_prefix,
    )

    # Define object files.
    objects = None
    objects_have_external_debug_info = False
    external_debug_info = None
    stripped_objects = []
    clang_remarks = []
    clang_traces = []
    pic_cxx_outs = compile_cxx(ctx, compile_cmd_output.src_compile_cmds, pic = True)
    pic_objects = [out.object for out in pic_cxx_outs]
    pic_objects_have_external_debug_info = is_any(lambda out: out.object_has_external_debug_info, pic_cxx_outs)
    pic_external_debug_info = [out.external_debug_info for out in pic_cxx_outs if out.external_debug_info]
    pic_clang_remarks = [out.clang_remarks for out in pic_cxx_outs if out.clang_remarks != None]
    pic_clang_traces = [out.clang_trace for out in pic_cxx_outs if out.clang_trace != None]
    stripped_pic_objects = _strip_objects(ctx, pic_objects)

    bitcode_outs = [
        out.object
        for out in pic_cxx_outs
        if is_bitcode_format(out.object_format)
    ]
    if len(bitcode_outs) == 0:
        bitcode_outs = None

    all_outs = []
    all_outs.extend(pic_cxx_outs)

    if preferred_linkage != Linkage("shared"):
        cxx_outs = compile_cxx(ctx, compile_cmd_output.src_compile_cmds, pic = False)
        all_outs.extend(cxx_outs)
        objects = [out.object for out in cxx_outs]
        objects_have_external_debug_info = is_any(lambda out: out.object_has_external_debug_info, cxx_outs)
        external_debug_info = [out.external_debug_info for out in cxx_outs if out.external_debug_info]
        stripped_objects = _strip_objects(ctx, objects)
        clang_remarks = [out.clang_remarks for out in cxx_outs if out.clang_remarks != None]
        clang_traces = [out.clang_trace for out in cxx_outs if out.clang_trace != None]

    # Add in additional objects, after setting up stripped objects.
    pic_objects += impl_params.extra_link_input
    stripped_pic_objects += impl_params.extra_link_input
    if preferred_linkage != Linkage("shared"):
        objects += impl_params.extra_link_input
        stripped_objects += impl_params.extra_link_input

    objects_sub_target = cxx_objects_sub_target(all_outs)

    return _CxxCompiledSourcesOutput(
        compile_cmds = compile_cmd_output,
        objects = objects,
        bitcode_objects = bitcode_outs,
        objects_have_external_debug_info = objects_have_external_debug_info,
        external_debug_info = external_debug_info,
        clang_remarks = clang_remarks,
        clang_traces = clang_traces,
        pic_objects = pic_objects,
        pic_objects_have_external_debug_info = pic_objects_have_external_debug_info,
        pic_external_debug_info = pic_external_debug_info,
        pic_clang_remarks = pic_clang_remarks,
        pic_clang_traces = pic_clang_traces,
        stripped_objects = stripped_objects,
        stripped_pic_objects = stripped_pic_objects,
        objects_sub_target = objects_sub_target,
    )

def _form_library_outputs(
        ctx: AnalysisContext,
        impl_params: CxxRuleConstructorParams.type,
        compiled_srcs: _CxxCompiledSourcesOutput.type,
        preferred_linkage: Linkage.type,
        shared_links: LinkArgs.type,
        extra_static_linkables: list[[FrameworksLinkable.type, SwiftmoduleLinkable.type, SwiftRuntimeLinkable.type]],
        gnu_use_link_groups: bool,
        link_execution_preference: LinkExecutionPreference.type) -> _CxxAllLibraryOutputs.type:
    # Build static/shared libs and the link info we use to export them to dependents.
    outputs = {}
    libraries = {}
    solibs = {}
    sub_targets = {}
    providers = []

    # Add in exported linker flags.
    def ldflags(inner: LinkInfo.type) -> LinkInfo.type:
        return wrap_link_info(
            inner = inner,
            pre_flags = cxx_attr_exported_linker_flags(ctx),
            post_flags = cxx_attr_exported_post_linker_flags(ctx),
        )

    # We don't know which outputs consumers may want, so we must define them all.
    for link_style in get_link_styles_for_linkage(preferred_linkage):
        output = None
        stripped = None
        info = None

        # Generate the necessary libraries and
        # add them to the exported link info.
        if link_style in (LinkStyle("static"), LinkStyle("static_pic")):
            # Only generate an archive if we have objects to include
            if compiled_srcs.objects or compiled_srcs.pic_objects:
                pic = _use_pic(link_style)
                output, info = _static_library(
                    ctx,
                    impl_params,
                    compiled_srcs.pic_objects if pic else compiled_srcs.objects,
                    objects_have_external_debug_info = compiled_srcs.pic_objects_have_external_debug_info if pic else compiled_srcs.objects_have_external_debug_info,
                    external_debug_info = make_artifact_tset(
                        ctx.actions,
                        label = ctx.label,
                        artifacts = (compiled_srcs.pic_external_debug_info if pic else compiled_srcs.external_debug_info),
                        children = impl_params.additional.static_external_debug_info,
                    ),
                    pic = pic,
                    stripped = False,
                    extra_linkables = extra_static_linkables,
                    bitcode_objects = compiled_srcs.bitcode_objects,
                )
                _, stripped = _static_library(
                    ctx,
                    impl_params,
                    compiled_srcs.stripped_pic_objects if pic else compiled_srcs.stripped_objects,
                    pic = pic,
                    stripped = True,
                    extra_linkables = extra_static_linkables,
                    bitcode_objects = compiled_srcs.bitcode_objects,
                )
            else:
                # Header only libraries can have `extra_static_linkables`
                info = LinkInfo(
                    name = ctx.label.name,
                    linkables = extra_static_linkables,
                )
        else:  # shared
            # We still generate a shared library with no source objects because it can still point to dependencies.
            # i.e. a rust_python_extension is an empty .so depending on a rust shared object
            if compiled_srcs.pic_objects or impl_params.build_empty_so:
                external_debug_artifacts = compiled_srcs.pic_external_debug_info
                if compiled_srcs.pic_objects_have_external_debug_info:
                    external_debug_artifacts.extend(compiled_srcs.pic_objects)
                if impl_params.extra_link_input_has_external_debug_info:
                    external_debug_artifacts.extend(impl_params.extra_link_input)
                external_debug_info = make_artifact_tset(
                    actions = ctx.actions,
                    label = ctx.label,
                    artifacts = external_debug_artifacts,
                    children = impl_params.additional.shared_external_debug_info,
                )

                extra_linker_flags, extra_linker_outputs = impl_params.extra_linker_outputs_factory(ctx)
                result = _shared_library(
                    ctx,
                    impl_params,
                    compiled_srcs.pic_objects,
                    external_debug_info,
                    shared_links,
                    gnu_use_link_groups,
                    extra_linker_flags = extra_linker_flags,
                    link_ordering = map_val(LinkOrdering, ctx.attrs.link_ordering),
                    link_execution_preference = link_execution_preference,
                )
                shlib = result.link_result.linked_object
                info = result.info
                output = CxxLibraryOutput(
                    default = shlib.output,
                    object_files = compiled_srcs.pic_objects,
                    external_debug_info = shlib.external_debug_info,
                    dwp = shlib.dwp,
                    linker_map = result.link_result.linker_map_data,
                    pdb = shlib.pdb,
                )
                solibs[result.soname] = shlib
                sub_targets[link_style] = extra_linker_outputs
                providers.append(result.link_result.link_execution_preference_info)

        # you cannot link against header only libraries so create an empty link info
        info = info if info != None else LinkInfo()
        outputs[link_style] = output
        libraries[link_style] = LinkInfos(
            default = ldflags(info),
            stripped = ldflags(stripped) if stripped != None else None,
        )

    return _CxxAllLibraryOutputs(
        outputs = outputs,
        libraries = libraries,
        sub_targets = sub_targets,
        providers = providers,
        solibs = solibs,
    )

def _strip_objects(ctx: AnalysisContext, objects: list["artifact"]) -> list["artifact"]:
    """
    Return new objects with debug info stripped.
    """

    # Stripping is not supported on Windows
    linker_type = get_cxx_toolchain_info(ctx).linker_info.type
    if linker_type == "windows":
        return objects

    outs = []

    for obj in objects:
        base, ext = paths.split_extension(obj.short_path)
        expect(ext == ".o")
        outs.append(strip_debug_info(ctx, base + ".stripped.o", obj))

    return outs

def _get_shared_library_links(
        ctx: AnalysisContext,
        linkable_graph_node_map_func,
        link_group: [str, None],
        link_group_mappings: [dict[Label, str], None],
        link_group_preferred_linkage: dict[Label, Linkage.type],
        link_group_libs: dict[str, LinkGroupLib.type],
        exported_deps: list[Dependency],
        non_exported_deps: list[Dependency],
        force_link_group_linking,
        frameworks_linkable: [FrameworksLinkable.type, None],
        force_static_follows_dependents: bool = True,
        swiftmodule_linkable: [SwiftmoduleLinkable.type, None] = None,
        swift_runtime_linkable: [SwiftRuntimeLinkable.type, None] = None) -> ("LinkArgs", [DefaultInfo.type, None], LinkExecutionPreference.type):
    """
    Returns LinkArgs with the content to link, and a link group map json output if applicable.

    TODO(T110378116): Omnibus linking always creates shared libraries by linking
    against shared dependencies. This is not true for link groups and possibly
    other forms of shared libraries. Ideally we consolidate this logic and
    propagate up only the expected links. Until we determine the comprehensive
    logic here, simply diverge behavior on whether link groups are defined.
    """

    pic_behavior = get_cxx_toolchain_info(ctx).pic_behavior

    # If we're not filtering for link groups, link against the shared dependencies
    if not link_group_mappings and not force_link_group_linking:
        link = cxx_inherited_link_info(ctx, dedupe(flatten([non_exported_deps, exported_deps])))

        # Even though we're returning the shared library links, we must still
        # respect the `link_style` attribute of the target which controls how
        # all deps get linked. For example, you could be building the shared
        # output of a library which has `link_style = "static"`.
        #
        # The fallback equivalent code in Buck v1 is in CxxLibraryFactor::createBuildRule()
        # where link style is determined using the `linkableDepType` variable.
        link_style_value = ctx.attrs.link_style if ctx.attrs.link_style != None else "shared"

        # Note if `static` link style is requested, we assume `static_pic`
        # instead, so that code in the shared library can be correctly
        # loaded in the address space of any process at any address.
        link_style_value = "static_pic" if link_style_value == "static" else link_style_value

        # We cannot support deriving link execution preference off the included links, as we've already
        # lost the information on what is in the link.
        # TODO(T152860998): Derive link_execution_preference based upon the included links
        # Not all rules calling `cxx_library_parameterized` have `link_execution_preference`. Notably `cxx_python_extension`.
        link_execution_preference = get_link_execution_preference(ctx, []) if hasattr(ctx.attrs, "link_execution_preference") else LinkExecutionPreference("any")

        return apple_build_link_args_with_deduped_flags(
            ctx,
            link,
            frameworks_linkable,
            # fPIC behaves differently on various combinations of toolchains + platforms.
            # To get the link_style, we have to check the link_style against the toolchain's pic_behavior.
            #
            # For more info, check the PicBehavior.type docs.
            process_link_style_for_pic_behavior(LinkStyle(link_style_value), pic_behavior),
            swiftmodule_linkable = swiftmodule_linkable,
            swift_runtime_linkable = swift_runtime_linkable,
        ), None, link_execution_preference

    # Else get filtered link group links
    prefer_stripped = cxx_is_gnu(ctx) and ctx.attrs.prefer_stripped_objects

    link_style = cxx_attr_link_style(ctx) if cxx_attr_link_style(ctx) != LinkStyle("static") else LinkStyle("static_pic")
    link_style = process_link_style_for_pic_behavior(link_style, pic_behavior)
    filtered_labels_to_links_map = get_filtered_labels_to_links_map(
        linkable_graph_node_map_func(),
        link_group,
        {},
        link_group_mappings,
        link_group_preferred_linkage,
        link_group_libs = {
            name: (lib.label, lib.shared_link_infos)
            for name, lib in link_group_libs.items()
        },
        link_style = link_style,
        roots = linkable_deps(non_exported_deps + exported_deps),
        pic_behavior = pic_behavior,
        prefer_stripped = prefer_stripped,
        force_static_follows_dependents = force_static_follows_dependents,
    )
    filtered_links = get_filtered_links(filtered_labels_to_links_map)
    filtered_targets = get_filtered_targets(filtered_labels_to_links_map)

    link_execution_preference = get_link_execution_preference(ctx, filtered_labels_to_links_map.keys())

    # Unfortunately, link_groups does not use MergedLinkInfo to represent the args
    # for the resolved nodes in the graph.
    additional_links = apple_get_link_info_by_deduping_link_infos(ctx, filtered_links, frameworks_linkable, swiftmodule_linkable, swift_runtime_linkable)
    if additional_links:
        filtered_links.append(additional_links)

    return LinkArgs(infos = filtered_links), get_link_group_map_json(ctx, filtered_targets), link_execution_preference

def _use_pic(link_style: LinkStyle.type) -> bool:
    """
    Whether this link style requires PIC objects.
    """
    return link_style != LinkStyle("static")

# Create the objects/archive to use for static linking this rule.
# Returns a tuple of objects/archive to use as the default output for the link
# style(s) it's used in and the `LinkInfo` to export to dependents.
def _static_library(
        ctx: AnalysisContext,
        impl_params: "CxxRuleConstructorParams",
        objects: list["artifact"],
        pic: bool,
        stripped: bool,
        extra_linkables: list[[FrameworksLinkable.type, SwiftmoduleLinkable.type, SwiftRuntimeLinkable.type]],
        objects_have_external_debug_info: bool = False,
        external_debug_info: ArtifactTSet.type = ArtifactTSet(),
        bitcode_objects: [list["artifact"], None] = None) -> (CxxLibraryOutput.type, LinkInfo.type):
    if len(objects) == 0:
        fail("empty objects")

    # No reason to create a static library with just a single object file. We
    # still want to create a static lib to expose as the default output because
    # it's the contract/expectation of external clients of the cmd line
    # interface. Any tools consuming `buck build` outputs should get a
    # consistent output type when building a library, not static lib or object
    # file depending on number of source files.
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    linker_type = linker_info.type

    base_name = _base_static_library_name(ctx, stripped)
    name = _archive_name(base_name, pic = pic, extension = linker_info.static_library_extension)
    archive = make_archive(ctx, name, objects)

    bitcode_bundle = _bitcode_bundle(ctx, bitcode_objects, pic, stripped)
    if bitcode_bundle != None and bitcode_bundle.artifact != None:
        bitcode_artifact = bitcode_bundle.artifact
    else:
        bitcode_artifact = None

    if use_archives(ctx):
        linkable = ArchiveLinkable(
            archive = archive,
            bitcode_bundle = bitcode_artifact,
            linker_type = linker_type,
            link_whole = _attr_link_whole(ctx),
        )
    else:
        linkable = ObjectsLinkable(
            objects = objects,
            bitcode_bundle = bitcode_artifact,
            linker_type = linker_type,
            link_whole = _attr_link_whole(ctx),
        )

    post_flags = []

    if pic:
        post_flags.extend(linker_info.static_pic_dep_runtime_ld_flags or [])
    else:
        post_flags.extend(linker_info.static_dep_runtime_ld_flags or [])

    # On darwin, the linked output references the archive that contains the
    # object files instead of the originating objects.
    object_external_debug_info = []
    if linker_type == "darwin":
        object_external_debug_info.append(archive.artifact)
        object_external_debug_info.extend(archive.external_objects)
    elif objects_have_external_debug_info:
        object_external_debug_info.extend(objects)

    all_external_debug_info = make_artifact_tset(
        actions = ctx.actions,
        label = ctx.label,
        artifacts = object_external_debug_info,
        children = [external_debug_info],
    )

    return (
        CxxLibraryOutput(
            default = archive.artifact,
            object_files = objects,
            bitcode_bundle = bitcode_bundle,
            other = archive.external_objects,
        ),
        LinkInfo(
            name = name,
            # We're propagating object code for linking up the dep tree,
            # so we need to also propagate any necessary link flags required for
            # the object code.
            pre_flags = impl_params.extra_exported_link_flags,
            post_flags = post_flags,
            # Extra linkables are propagated here so they are available to link_groups
            # when they are deducing linker args.
            linkables = [linkable] + extra_linkables,
            external_debug_info = all_external_debug_info,
        ),
    )

# A bitcode bundle is very much like a static library and is generated from object file
# inputs, except the output is a combined bitcode file, which is not machine code.
def _bitcode_bundle(
        ctx: AnalysisContext,
        objects: [list["artifact"], None],
        pic: bool = False,
        stripped: bool = False,
        name_extra = "") -> [BitcodeBundle.type, None]:
    if objects == None or len(objects) == 0:
        return None

    base_name = _base_static_library_name(ctx, False)
    name = name_extra + _bitcode_bundle_name(base_name, pic, stripped)
    return make_bitcode_bundle(ctx, name, objects)

_CxxSharedLibraryResult = record(
    # Result from link, includes the shared lib, linker map data etc
    link_result = CxxLinkResult.type,
    # Shared library name (e.g. SONAME)
    soname = str,
    objects_bitcode_bundle = ["artifact", None],
    # `LinkInfo` used to link against the shared library.
    info = LinkInfo.type,
)

def _shared_library(
        ctx: AnalysisContext,
        impl_params: "CxxRuleConstructorParams",
        objects: list["artifact"],
        external_debug_info: ArtifactTSet.type,
        dep_infos: "LinkArgs",
        gnu_use_link_groups: bool,
        extra_linker_flags: list["_arglike"],
        link_execution_preference: LinkExecutionPreference.type,
        link_ordering: [LinkOrdering.type, None] = None) -> _CxxSharedLibraryResult.type:
    """
    Generate a shared library and the associated native link info used by
    dependents to link against it.
    """

    soname = _soname(ctx, impl_params)
    cxx_toolchain = get_cxx_toolchain_info(ctx)
    linker_info = cxx_toolchain.linker_info

    local_bitcode_bundle = _bitcode_bundle(ctx, objects, name_extra = "objects-")

    # NOTE(agallagher): We add exported link flags here because it's what v1
    # does, but the intent of exported link flags are to wrap the link output
    # that we propagate up the tree, rather than being used locally when
    # generating a link product.
    link_info = LinkInfo(
        pre_flags = (
            cxx_attr_exported_linker_flags(ctx) +
            cxx_attr_linker_flags(ctx)
        ),
        linkables = [ObjectsLinkable(
            objects = objects,
            bitcode_bundle = local_bitcode_bundle.artifact if local_bitcode_bundle else None,
            linker_type = linker_info.type,
            link_whole = True,
        )],
        post_flags = (
            impl_params.extra_exported_link_flags +
            impl_params.extra_link_flags +
            extra_linker_flags +
            _attr_post_linker_flags(ctx) +
            (linker_info.shared_dep_runtime_ld_flags or [])
        ),
        external_debug_info = external_debug_info,
    )
    link_result = cxx_link_shared_library(
        ctx = ctx,
        output = soname,
        name = soname if impl_params.use_soname else None,
        links = [LinkArgs(infos = [link_info]), dep_infos],
        identifier = soname,
        link_ordering = link_ordering,
        shared_library_flags = impl_params.shared_library_flags,
        strip = impl_params.strip_executable,
        strip_args_factory = impl_params.strip_args_factory,
        link_execution_preference = link_execution_preference,
    )
    exported_shlib = link_result.linked_object.output

    # If shared library interfaces are enabled, link that and use it as
    # the shared lib that dependents will link against.
    # TODO(agallagher): There's a bug in shlib intfs interacting with link
    # groups, where we don't include the symbols we're meant to export from
    # deps that get statically linked in.
    if cxx_use_shlib_intfs(ctx) and not gnu_use_link_groups:
        link_info = LinkInfo(
            pre_flags = link_info.pre_flags,
            linkables = link_info.linkables,
            post_flags = (
                (link_info.post_flags or []) +
                get_ignore_undefined_symbols_flags(linker_info.type) +
                (linker_info.independent_shlib_interface_linker_flags or [])
            ),
            external_debug_info = link_info.external_debug_info,
        )
        intf_link_result = cxx_link_shared_library(
            ctx = ctx,
            output = get_shared_library_name(
                linker_info,
                ctx.label.name + "-for-interface",
            ),
            category_suffix = "interface",
            link_ordering = link_ordering,
            name = soname,
            links = [LinkArgs(infos = [link_info])],
            identifier = soname,
            link_execution_preference = link_execution_preference,
        )

        # Convert the shared library into an interface.
        shlib_interface = cxx_mk_shlib_intf(ctx, ctx.label.name, intf_link_result.linked_object.output)

        exported_shlib = shlib_interface

    # Link against import library on Windows.
    if link_result.linked_object.import_library:
        exported_shlib = link_result.linked_object.import_library

    return _CxxSharedLibraryResult(
        link_result = link_result,
        soname = soname,
        objects_bitcode_bundle = local_bitcode_bundle.artifact if local_bitcode_bundle else None,
        info = LinkInfo(
            name = soname,
            linkables = [SharedLibLinkable(
                lib = exported_shlib,
            )],
        ),
    )

def _attr_reexport_all_header_dependencies(ctx: AnalysisContext) -> bool:
    return value_or(ctx.attrs.reexport_all_header_dependencies, False)

def _soname(ctx: AnalysisContext, impl_params) -> str:
    """
    Get the shared library name to set for the given C++ library.
    """
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    explicit_soname = value_or(ctx.attrs.soname, impl_params.soname)
    if explicit_soname != None:
        return get_shared_library_name_for_param(linker_info, explicit_soname)
    return get_default_shared_library_name(linker_info, ctx.label)

def _base_static_library_name(ctx: AnalysisContext, stripped: bool) -> str:
    return ctx.label.name + ".stripped" if stripped else ctx.label.name

def _archive_name(name: str, pic: bool, extension: str) -> str:
    return "lib{}{}.{}".format(name, ".pic" if pic else "", extension)

def _bitcode_bundle_name(name: str, pic: bool, stripped: bool = False) -> str:
    return "{}{}{}.bc".format(name, ".pic" if pic else "", ".stripped" if stripped else "")

def _attr_link_whole(ctx: AnalysisContext) -> bool:
    return value_or(ctx.attrs.link_whole, False)

def use_archives(ctx: AnalysisContext) -> bool:
    """
    Whether this rule should use archives to package objects when producing
    link input for dependents.
    """
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    requires_archives = linker_info.requires_archives
    requires_objects = linker_info.requires_objects

    if requires_archives and requires_objects:
        fail("In cxx linker_info, only one of `requires_archives` and `requires_objects` can be enabled")

    # If the toolchain requires them, then always use them.
    if requires_archives:
        return True

    if requires_objects:
        return False

    # Otherwise, fallback to the rule-specific setting.
    return value_or(ctx.attrs.use_archive, True)

def _attr_post_linker_flags(ctx: AnalysisContext) -> list[""]:
    return (
        ctx.attrs.post_linker_flags +
        flatten(cxx_by_platform(ctx, ctx.attrs.post_platform_linker_flags))
    )
