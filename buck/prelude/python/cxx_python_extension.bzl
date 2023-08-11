# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load(
    "@prelude//cxx:cxx_library.bzl",
    "cxx_library_parameterized",
)
load(
    "@prelude//cxx:cxx_library_utility.bzl",
    "cxx_attr_deps",
)
load(
    "@prelude//cxx:cxx_sources.bzl",
    "get_srcs_with_flags",
)
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxPlatformInfo")
load(
    "@prelude//cxx:cxx_types.bzl",
    "CxxRuleConstructorParams",
    "CxxRuleProviderParams",
    "CxxRuleSubTargetParams",
)
load("@prelude//cxx:headers.bzl", "cxx_get_regular_cxx_headers_layout")
load("@prelude//cxx:linker.bzl", "PDB_SUB_TARGET")
load(
    "@prelude//cxx:omnibus.bzl",
    "create_linkable_root",
    "explicit_roots_enabled",
    "get_roots",
)
load(
    "@prelude//linking:link_groups.bzl",
    "merge_link_group_lib_info",
)
load(
    "@prelude//linking:link_info.bzl",
    "LinkInfo",
    "LinkInfos",
    "LinkStyle",
    "Linkage",
    "create_merged_link_info",
    "wrap_link_infos",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "AnnotatedLinkableRoot",
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
)
load(
    "@prelude//linking:linkables.bzl",
    "LinkableProviders",
    "linkables",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "merge_shared_libraries",
)
load("@prelude//os_lookup:defs.bzl", "OsLookup")
load("@prelude//python:toolchain.bzl", "PythonPlatformInfo", "get_platform_attr")
load("@prelude//utils:utils.bzl", "expect", "flatten", "value_or")
load(":manifest.bzl", "create_manifest_for_source_map")
load(
    ":native_python_util.bzl",
    "merge_cxx_extension_info",
    "rewrite_static_symbols",
)
load(":python.bzl", "PythonLibraryInfo")
load(":python_library.bzl", "create_python_library_info", "dest_prefix", "gather_dep_libraries", "qualify_srcs")

# This extension is basically cxx_library, plus base_module.
# So we augment with default attributes so it has everything cxx_library has, and then call cxx_library_parameterized and work from that.
def cxx_python_extension_impl(ctx: AnalysisContext) -> list["provider"]:
    providers = []

    if ctx.attrs._target_os_type[OsLookup].platform == "windows":
        library_extension = ".pyd"
    else:
        library_extension = ".so"
    module_name = value_or(ctx.attrs.module_name, ctx.label.name)
    name = module_name + library_extension
    base_module = dest_prefix(ctx.label, ctx.attrs.base_module)

    sub_targets = CxxRuleSubTargetParams(
        argsfiles = True,
        compilation_database = True,
        headers = False,
        link_group_map = False,
        link_style_outputs = False,
        xcode_data = False,
    )

    cxx_providers = CxxRuleProviderParams(
        compilation_database = True,
        default = False,  # We need to do some postprocessing to make sure the shared library is our default output
        java_packaging_info = False,
        linkable_graph = False,  # We create this here so we can correctly apply exclusions
        link_style_outputs = False,
        merged_native_link_info = False,
        omnibus_root = True,
        preprocessors = False,
        resources = True,
        shared_libraries = False,
        template_placeholders = False,
        preprocessor_for_tests = False,
    )

    impl_params = CxxRuleConstructorParams(
        build_empty_so = True,
        rule_type = "cxx_python_extension",
        headers_layout = cxx_get_regular_cxx_headers_layout(ctx),
        srcs = get_srcs_with_flags(ctx),
        soname = name,
        use_soname = False,
        generate_providers = cxx_providers,
        generate_sub_targets = sub_targets,
        is_omnibus_root = explicit_roots_enabled(ctx),
    )

    cxx_library_info = cxx_library_parameterized(ctx, impl_params)
    libraries = cxx_library_info.all_outputs
    shared_output = libraries.outputs[LinkStyle("shared")]

    shared_objects = libraries.solibs.values()
    expect(len(shared_objects) == 1, "Expected exactly 1 so for cxx_python_extension: {}".format(ctx.label))
    extension = shared_objects[0]

    sub_targets = cxx_library_info.sub_targets
    if extension.pdb:
        sub_targets[PDB_SUB_TARGET] = [DefaultInfo(default_output = extension.pdb)]

    providers.append(DefaultInfo(
        default_output = shared_output.default,
        other_outputs = shared_output.other,
        sub_targets = sub_targets,
    ))

    cxx_deps = [dep for dep in cxx_attr_deps(ctx)]

    extension_artifacts = {}
    python_module_names = {}
    unembeddable_extensions = {}

    link_infos = libraries.libraries

    # For python_cxx_extensions we need to mangle the symbol names in order to avoid collisions
    # when linking into the main binary
    embeddable = ctx.attrs.allow_embedding and libraries.outputs[LinkStyle("static")] != None
    if embeddable:
        if not ctx.attrs.allow_suffixing:
            pyinit_symbol = "PyInit_{}".format(module_name)
        else:
            suffix = base_module.replace("/", "$") + module_name
            static_output = libraries.outputs[LinkStyle("static")]
            static_pic_output = libraries.outputs[LinkStyle("static_pic")]
            cxx_toolchain = get_cxx_toolchain_info(ctx)
            link_infos = rewrite_static_symbols(
                ctx,
                suffix,
                pic_objects = static_pic_output.object_files,
                non_pic_objects = static_output.object_files,
                libraries = libraries.libraries,
                cxx_toolchain = cxx_toolchain,
                suffix_all = ctx.attrs.suffix_all,
            )
            pyinit_symbol = "PyInit_{}_{}".format(module_name, suffix)

        if base_module != "":
            lines = ["# auto generated stub for {}\n".format(ctx.label.raw_target())]
            stub_name = module_name + ".empty_stub"
            extension_artifacts.update(qualify_srcs(ctx.label, ctx.attrs.base_module, {stub_name: ctx.actions.write(stub_name, lines)}))

        python_module_names[base_module.replace("/", ".") + module_name] = pyinit_symbol

    # Add a dummy shared link info to avoid marking this node as preferred
    # linkage being "static", which has a special meaning for various link
    # strategies
    link_infos[LinkStyle("shared")] = LinkInfos(default = LinkInfo())

    # Create linkable providers for the extension.
    link_deps = linkables(cxx_deps)
    linkable_providers = LinkableProviders(
        link_group_lib_info = merge_link_group_lib_info(deps = cxx_deps),
        linkable_graph = create_linkable_graph(
            ctx = ctx,
            node = create_linkable_graph_node(
                ctx = ctx,
                linkable_node = create_linkable_node(
                    ctx = ctx,
                    deps = cxx_deps,
                    preferred_linkage = Linkage("any"),
                    link_infos = link_infos,
                ),
            ),
            children = [d.linkable_graph for d in link_deps],
        ),
        merged_link_info = create_merged_link_info(
            ctx = ctx,
            pic_behavior = get_cxx_toolchain_info(ctx).pic_behavior,
            link_infos = link_infos,
            preferred_linkage = Linkage("static"),
            deps = [d.merged_link_info for d in link_deps],
        ),
        shared_library_info = merge_shared_libraries(
            actions = ctx.actions,
            deps = [d.shared_library_info for d in link_deps],
        ),
        linkable_root_info = create_linkable_root(
            ctx = ctx,
            link_infos = wrap_link_infos(
                link_infos[LinkStyle("static_pic")],
                pre_flags = ctx.attrs.linker_flags,
                post_flags = ctx.attrs.post_linker_flags,
            ),
            graph = create_linkable_graph(
                ctx,
                deps = cxx_deps,
            ),
            deps = cxx_deps,
        ),
    )

    if not embeddable:
        unembeddable_extensions[base_module + name] = linkable_providers
        linkable_providers = None

    providers.append(merge_cxx_extension_info(
        actions = ctx.actions,
        deps = cxx_deps,
        linkable_providers = linkable_providers,
        artifacts = extension_artifacts,
        python_module_names = python_module_names,
        unembeddable_extensions = unembeddable_extensions,
    ))
    providers.extend(cxx_library_info.providers)

    # If a type stub was specified, create a manifest for export.
    src_type_manifest = None
    if ctx.attrs.type_stub != None:
        src_type_manifest = create_manifest_for_source_map(
            ctx,
            "type_stub",
            qualify_srcs(
                ctx.label,
                ctx.attrs.base_module,
                {module_name + ".pyi": ctx.attrs.type_stub},
            ),
        )

    # Export library info.
    python_platform = ctx.attrs._python_toolchain[PythonPlatformInfo]
    cxx_platform = ctx.attrs._cxx_toolchain[CxxPlatformInfo]
    raw_deps = (
        [ctx.attrs.deps] +
        get_platform_attr(python_platform, cxx_platform, ctx.attrs.platform_deps)
    )
    deps, shared_deps = gather_dep_libraries(raw_deps)
    providers.append(create_python_library_info(
        ctx.actions,
        ctx.label,
        extensions = qualify_srcs(ctx.label, ctx.attrs.base_module, {name: extension}),
        deps = deps,
        shared_libraries = shared_deps,
        src_types = src_type_manifest,
    ))

    # Omnibus providers

    # Handle the case where C++ Python extensions depend on other C++ Python
    # extensions, which should also be treated as roots.
    roots = get_roots(ctx.label, [
        dep
        for dep in flatten(raw_deps)
        # We only want to handle C++ Python extension deps, but not other native
        # linkable deps like C++ libraries.
        if PythonLibraryInfo in dep
    ])
    roots[ctx.label] = AnnotatedLinkableRoot(root = cxx_library_info.linkable_root)

    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            roots = roots,
        ),
        deps = flatten(raw_deps),
    )
    providers.append(linkable_graph)
    return providers
