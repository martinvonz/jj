# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:resources.bzl", "ResourceInfo", "gather_resources")
load(
    "@prelude//android:android_providers.bzl",
    "merge_android_packageable_info",
)
load("@prelude//cxx:cxx_toolchain_types.bzl", "PicBehavior")
load(
    "@prelude//cxx:linker.bzl",
    "PDB_SUB_TARGET",
    "get_default_shared_library_name",
)
load(
    "@prelude//cxx:omnibus.bzl",
    "create_linkable_root",
    "is_known_omnibus_root",
)
load(
    "@prelude//linking:link_groups.bzl",
    "merge_link_group_lib_info",
)
load(
    "@prelude//linking:link_info.bzl",
    "Archive",
    "ArchiveLinkable",
    "LinkInfo",
    "LinkInfos",
    "LinkStyle",
    "Linkage",
    "LinkedObject",
    "MergedLinkInfo",
    "STATIC_LINK_STYLES",
    "SharedLibLinkable",
    "create_merged_link_info",
    "get_actual_link_style",
    "merge_link_infos",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "AnnotatedLinkableRoot",
    "DlopenableLibraryInfo",
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "create_shared_libraries",
    "merge_shared_libraries",
)
load("@prelude//os_lookup:defs.bzl", "OsLookup")
load(
    ":build.bzl",
    "RustcOutput",  # @unused Used as a type
    "compile_context",
    "generate_rustdoc",
    "generate_rustdoc_test",
    "rust_compile",
    "rust_compile_multi",
)
load(
    ":build_params.bzl",
    "BuildParams",  # @unused Used as a type
    "Emit",
    "LinkageLang",
    "RuleType",
    "build_params",
    "crate_type_transitive_deps",
)
load(
    ":context.bzl",
    "CompileContext",  # @unused Used as a type
)
load(
    ":link_info.bzl",
    "CrateName",  # @unused Used as a type
    "DEFAULT_STATIC_LINK_STYLE",
    "RustLinkInfo",
    "RustLinkStyleInfo",
    "attr_crate",
    "inherited_non_rust_exported_link_deps",
    "inherited_non_rust_link_info",
    "inherited_non_rust_shared_libs",
    "resolve_deps",
    "style_info",
)
load(":resources.bzl", "rust_attr_resources")
load(":targets.bzl", "targets")

def prebuilt_rust_library_impl(ctx: AnalysisContext) -> list["provider"]:
    providers = []

    # Default output.
    providers.append(
        DefaultInfo(
            default_output = ctx.attrs.rlib,
        ),
    )

    # Rust link provider.
    crate = attr_crate(ctx)
    styles = {}
    for style in LinkStyle:
        tdeps, tmetadeps = _compute_transitive_deps(ctx, style)
        styles[style] = RustLinkStyleInfo(
            rlib = ctx.attrs.rlib,
            transitive_deps = tdeps,
            rmeta = ctx.attrs.rlib,
            transitive_rmeta_deps = tmetadeps,
            pdb = None,
        )
    providers.append(
        RustLinkInfo(
            crate = crate,
            styles = styles,
            non_rust_exported_link_deps = inherited_non_rust_exported_link_deps(ctx),
            non_rust_link_info = inherited_non_rust_link_info(ctx),
            non_rust_shared_libs = merge_shared_libraries(
                ctx.actions,
                deps = inherited_non_rust_shared_libs(ctx),
            ),
        ),
    )

    # Native link provier.
    link = LinkInfo(
        linkables = [ArchiveLinkable(
            archive = Archive(artifact = ctx.attrs.rlib),
            linker_type = "unknown",
        )],
    )
    providers.append(
        create_merged_link_info(
            ctx,
            PicBehavior("supported"),
            {link_style: LinkInfos(default = link) for link_style in LinkStyle},
            exported_deps = [d[MergedLinkInfo] for d in ctx.attrs.deps],
            # TODO(agallagher): This matches v1 behavior, but some of these libs
            # have prebuilt DSOs which might be usable.
            preferred_linkage = Linkage("static"),
        ),
    )

    # Native link graph setup.
    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            linkable_node = create_linkable_node(
                ctx = ctx,
                preferred_linkage = Linkage("static"),
                exported_deps = ctx.attrs.deps,
                link_infos = {link_style: LinkInfos(default = link) for link_style in LinkStyle},
            ),
        ),
        deps = ctx.attrs.deps,
    )
    providers.append(linkable_graph)

    providers.append(merge_link_group_lib_info(deps = ctx.attrs.deps))

    providers.append(merge_android_packageable_info(ctx.label, ctx.actions, ctx.attrs.deps))

    return providers

def rust_library_impl(ctx: AnalysisContext) -> list["provider"]:
    compile_ctx = compile_context(ctx)
    toolchain_info = compile_ctx.toolchain_info

    # Multiple styles and language linkages could generate the same crate types
    # (eg procmacro or using preferred_linkage), so we need to see how many
    # distinct kinds of build we actually need to deal with.
    param_lang, lang_style_param = _build_params_for_styles(ctx, compile_ctx)

    artifacts = _build_library_artifacts(ctx, compile_ctx, param_lang)

    rust_param_artifact = {}
    native_param_artifact = {}
    check_artifacts = None

    for (lang, params), (link, meta) in artifacts.items():
        if lang == LinkageLang("rust"):
            # Grab the check output for all kinds of builds to use
            # in the check subtarget. The link style doesn't matter
            # so pick the first.
            if check_artifacts == None:
                check_artifacts = {"check": meta.output}
                check_artifacts.update(meta.diag)

            rust_param_artifact[params] = _handle_rust_artifact(ctx, params, link, meta)
        elif lang == LinkageLang("c++"):
            native_param_artifact[params] = link.output
        else:
            fail("Unhandled lang {}".format(lang))

    # Among {rustdoc, doctests, macro expand}, doctests are the only one which
    # cares about linkage. So if there is a required link style set for the
    # doctests, reuse those same dependency artifacts for the other build
    # outputs where static vs static_pic does not make a difference.
    if ctx.attrs.doctest_link_style:
        static_link_style = {
            "shared": DEFAULT_STATIC_LINK_STYLE,
            "static": LinkStyle("static"),
            "static_pic": LinkStyle("static_pic"),
        }[ctx.attrs.doctest_link_style]
    else:
        static_link_style = DEFAULT_STATIC_LINK_STYLE

    static_library_params = lang_style_param[(LinkageLang("rust"), static_link_style)]
    default_roots = ["lib.rs"]
    rustdoc = generate_rustdoc(
        ctx = ctx,
        compile_ctx = compile_ctx,
        params = static_library_params,
        default_roots = default_roots,
        document_private_items = False,
    )

    # If doctests=True or False is set on the individual target, respect that.
    # Otherwise look at the global setting on the toolchain.
    doctests_enabled = ctx.attrs.doctests if ctx.attrs.doctests != None else toolchain_info.doctests

    rustdoc_test = None
    if doctests_enabled and toolchain_info.rustc_target_triple == targets.exec_triple(ctx):
        if ctx.attrs.doctest_link_style:
            doctest_link_style = LinkStyle(ctx.attrs.doctest_link_style)
        else:
            doctest_link_style = {
                "any": LinkStyle("shared"),
                "shared": LinkStyle("shared"),
                "static": DEFAULT_STATIC_LINK_STYLE,
            }[ctx.attrs.preferred_linkage]
        rustdoc_test_params = build_params(
            rule = RuleType("binary"),
            proc_macro = ctx.attrs.proc_macro,
            link_style = doctest_link_style,
            preferred_linkage = Linkage(ctx.attrs.preferred_linkage),
            lang = LinkageLang("rust"),
            linker_type = compile_ctx.cxx_toolchain_info.linker_info.type,
            target_os_type = ctx.attrs._target_os_type[OsLookup],
        )
        rustdoc_test = generate_rustdoc_test(
            ctx = ctx,
            compile_ctx = compile_ctx,
            link_style = rustdoc_test_params.dep_link_style,
            library = rust_param_artifact[static_library_params],
            params = rustdoc_test_params,
            default_roots = default_roots,
        )

    expand = rust_compile(
        ctx = ctx,
        compile_ctx = compile_ctx,
        emit = Emit("expand"),
        params = static_library_params,
        link_style = DEFAULT_STATIC_LINK_STYLE,
        default_roots = default_roots,
    )

    providers = []

    providers += _default_providers(
        ctx = ctx,
        lang_style_param = lang_style_param,
        param_artifact = rust_param_artifact,
        rustdoc = rustdoc,
        rustdoc_test = rustdoc_test,
        check_artifacts = check_artifacts,
        expand = expand.output,
        sources = compile_ctx.symlinked_srcs,
    )
    providers += _rust_providers(
        ctx = ctx,
        lang_style_param = lang_style_param,
        param_artifact = rust_param_artifact,
    )
    providers += _native_providers(
        ctx = ctx,
        compile_ctx = compile_ctx,
        lang_style_param = lang_style_param,
        param_artifact = native_param_artifact,
    )

    deps = [dep.dep for dep in resolve_deps(ctx)]
    providers.append(ResourceInfo(resources = gather_resources(
        label = ctx.label,
        resources = rust_attr_resources(ctx),
        deps = deps,
    )))

    providers.append(merge_android_packageable_info(ctx.label, ctx.actions, deps))

    return providers

def _build_params_for_styles(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type) -> (
    dict[BuildParams.type, list[LinkageLang.type]],
    dict[(LinkageLang.type, LinkStyle.type), BuildParams.type],
):
    """
    For a given rule, return two things:
    - a set of build params we need for all combinations of linkage langages and
      link styles, mapped to which languages they apply to
    - a mapping from linkage language and link style to build params

    This is needed because different combinations may end up using the same set
    of params, and we want to minimize invocations to rustc, both for
    efficiency's sake, but also to avoid duplicate objects being linked
    together.
    """

    param_lang = {}  # param -> linkage_lang
    style_param = {}  # (linkage_lang, link_style) -> param

    target_os_type = ctx.attrs._target_os_type[OsLookup]
    linker_type = compile_ctx.cxx_toolchain_info.linker_info.type

    # Styles+lang linkage to params
    for linkage_lang in LinkageLang:
        # Skip proc_macro + c++ combination
        if ctx.attrs.proc_macro and linkage_lang == LinkageLang("c++"):
            continue

        for link_style in LinkStyle:
            params = build_params(
                rule = RuleType("library"),
                proc_macro = ctx.attrs.proc_macro,
                link_style = link_style,
                preferred_linkage = Linkage(ctx.attrs.preferred_linkage),
                lang = linkage_lang,
                linker_type = linker_type,
                target_os_type = target_os_type,
            )
            if params not in param_lang:
                param_lang[params] = []
            param_lang[params] = param_lang[params] + [linkage_lang]
            style_param[(linkage_lang, link_style)] = params

    return (param_lang, style_param)

def _build_library_artifacts(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        param_lang: dict[BuildParams.type, list[LinkageLang.type]]) -> dict[(LinkageLang.type, BuildParams.type), (RustcOutput.type, RustcOutput.type)]:
    """
    Generate the actual actions to build various output artifacts. Given the set
    parameters we need, return a mapping to the linkable and metadata artifacts.
    """
    param_artifact = {}

    for params, langs in param_lang.items():
        link_style = params.dep_link_style

        # Separate actions for each emit type
        #
        # In principle we don't really need metadata for C++-only artifacts, but I don't think it hurts
        link, meta = rust_compile_multi(
            ctx = ctx,
            compile_ctx = compile_ctx,
            emits = [Emit("link"), Emit("metadata")],
            params = params,
            link_style = link_style,
            default_roots = ["lib.rs"],
        )

        for lang in langs:
            param_artifact[(lang, params)] = (link, meta)

    return param_artifact

def _handle_rust_artifact(
        ctx: AnalysisContext,
        params: BuildParams.type,
        link: RustcOutput.type,
        meta: RustcOutput.type) -> RustLinkStyleInfo.type:
    """
    Return the RustLinkInfo for a given set of artifacts. The main consideration
    is computing the right set of dependencies.
    """

    link_style = params.dep_link_style

    # If we're a crate where our consumers should care about transitive deps,
    # then compute them (specifically, not proc-macro).
    tdeps, tmetadeps = ({}, {})
    if crate_type_transitive_deps(params.crate_type):
        tdeps, tmetadeps = _compute_transitive_deps(ctx, link_style)

    if not ctx.attrs.proc_macro:
        return RustLinkStyleInfo(
            rlib = link.output,
            transitive_deps = tdeps,
            rmeta = meta.output,
            transitive_rmeta_deps = tmetadeps,
            pdb = link.pdb,
        )
    else:
        # Proc macro deps are always the real thing
        return RustLinkStyleInfo(
            rlib = link.output,
            transitive_deps = tdeps,
            rmeta = link.output,
            transitive_rmeta_deps = tdeps,
            pdb = link.pdb,
        )

def _default_providers(
        ctx: AnalysisContext,
        lang_style_param: dict[(LinkageLang.type, LinkStyle.type), BuildParams.type],
        param_artifact: dict[BuildParams.type, RustLinkStyleInfo.type],
        rustdoc: "artifact",
        rustdoc_test: [cmd_args, None],
        check_artifacts: dict[str, "artifact"],
        expand: "artifact",
        sources: "artifact") -> list["provider"]:
    targets = {}
    targets.update(check_artifacts)
    targets["sources"] = sources
    targets["expand"] = expand
    targets["doc"] = rustdoc
    sub_targets = {
        k: [DefaultInfo(default_output = v)]
        for (k, v) in targets.items()
    }

    # Add provider for default output, and for each link-style...
    for link_style in LinkStyle:
        link_style_info = param_artifact[lang_style_param[(LinkageLang("rust"), link_style)]]
        nested_sub_targets = {}
        if link_style_info.pdb:
            nested_sub_targets[PDB_SUB_TARGET] = [DefaultInfo(default_output = link_style_info.pdb)]
        sub_targets[link_style.value] = [DefaultInfo(
            default_output = link_style_info.rlib,
            sub_targets = nested_sub_targets,
        )]

    providers = []

    if rustdoc_test:
        # Pass everything in env + doc_env, except ones with value None in doc_env.
        doc_env = dict(ctx.attrs.env)
        for k, v in ctx.attrs.doc_env.items():
            if v == None:
                doc_env.pop(k, None)
            else:
                doc_env[k] = v
        doc_env["RUSTC_BOOTSTRAP"] = "1"  # for `-Zunstable-options`

        rustdoc_test_info = ExternalRunnerTestInfo(
            type = "rustdoc",
            command = [rustdoc_test],
            run_from_project_root = True,
            env = doc_env,
        )

        # Run doc test as part of `buck2 test :crate`
        providers.append(rustdoc_test_info)

        # Run doc test as part of `buck2 test :crate[doc]`
        sub_targets["doc"].append(rustdoc_test_info)

    providers.append(DefaultInfo(
        default_output = check_artifacts["check"],
        sub_targets = sub_targets,
    ))

    return providers

def _rust_providers(
        ctx: AnalysisContext,
        lang_style_param: dict[(LinkageLang.type, LinkStyle.type), BuildParams.type],
        param_artifact: dict[BuildParams.type, RustLinkStyleInfo.type]) -> list["provider"]:
    """
    Return the set of providers for Rust linkage.
    """
    crate = attr_crate(ctx)

    style_info = {
        link_style: param_artifact[lang_style_param[(LinkageLang("rust"), link_style)]]
        for link_style in LinkStyle
    }

    # Inherited link input and shared libraries.  As in v1, this only includes
    # non-Rust rules, found by walking through -- and ignoring -- Rust libraries
    # to find non-Rust native linkables and libraries.
    if not ctx.attrs.proc_macro:
        inherited_non_rust_link_deps = inherited_non_rust_exported_link_deps(ctx)
        inherited_non_rust_link = inherited_non_rust_link_info(ctx)
        inherited_non_rust_shlibs = inherited_non_rust_shared_libs(ctx)
    else:
        # proc-macros are just used by the compiler and shouldn't propagate
        # their native deps to the link line of the target.
        inherited_non_rust_link = merge_link_infos(ctx, [])
        inherited_non_rust_shlibs = []
        inherited_non_rust_link_deps = []

    providers = []

    # Create rust library provider.
    providers.append(RustLinkInfo(
        crate = crate,
        styles = style_info,
        non_rust_link_info = inherited_non_rust_link,
        non_rust_exported_link_deps = inherited_non_rust_link_deps,
        non_rust_shared_libs = merge_shared_libraries(
            ctx.actions,
            deps = inherited_non_rust_shlibs,
        ),
    ))

    return providers

def _native_providers(
        ctx: AnalysisContext,
        compile_ctx: CompileContext.type,
        lang_style_param: dict[(LinkageLang.type, LinkStyle.type), BuildParams.type],
        param_artifact: dict[BuildParams.type, "artifact"]) -> list["provider"]:
    """
    Return the set of providers needed to link Rust as a dependency for native
    (ie C/C++) code, along with relevant dependencies.

    TODO: This currently assumes `staticlib`/`cdylib` behaviour, where all
    dependencies are bundled into the Rust crate itself. We need to break out of
    this mode of operation.
    """
    inherited_non_rust_link_deps = inherited_non_rust_exported_link_deps(ctx)
    inherited_non_rust_link = inherited_non_rust_link_info(ctx)
    inherited_non_rust_shlibs = inherited_non_rust_shared_libs(ctx)
    linker_info = compile_ctx.cxx_toolchain_info.linker_info
    linker_type = linker_info.type

    providers = []

    if ctx.attrs.proc_macro:
        # Proc-macros never have a native form
        return providers

    libraries = {
        link_style: param_artifact[lang_style_param[(LinkageLang("c++"), link_style)]]
        for link_style in LinkStyle
    }

    link_infos = {}
    for link_style, arg in libraries.items():
        if link_style in STATIC_LINK_STYLES:
            link_infos[link_style] = LinkInfos(default = LinkInfo(linkables = [ArchiveLinkable(archive = Archive(artifact = arg), linker_type = linker_type)]))
        else:
            link_infos[link_style] = LinkInfos(default = LinkInfo(linkables = [SharedLibLinkable(lib = arg)]))

    preferred_linkage = Linkage(ctx.attrs.preferred_linkage)

    # Native link provider.
    providers.append(create_merged_link_info(
        ctx,
        compile_ctx.cxx_toolchain_info.pic_behavior,
        link_infos,
        exported_deps = [inherited_non_rust_link],
        preferred_linkage = preferred_linkage,
    ))

    solibs = {}

    # Add the shared library to the list of shared libs.
    linker_info = compile_ctx.cxx_toolchain_info.linker_info
    shlib_name = get_default_shared_library_name(linker_info, ctx.label)

    # Only add a shared library if we generated one.
    if get_actual_link_style(LinkStyle("shared"), preferred_linkage, compile_ctx.cxx_toolchain_info.pic_behavior) == LinkStyle("shared"):
        solibs[shlib_name] = LinkedObject(output = libraries[LinkStyle("shared")])

    # Native shared library provider.
    providers.append(merge_shared_libraries(
        ctx.actions,
        create_shared_libraries(ctx, solibs),
        inherited_non_rust_shlibs,
    ))

    # Create, augment and provide the linkable graph.
    deps_linkable_graph = create_linkable_graph(
        ctx,
        deps = inherited_non_rust_link_deps,
    )

    # Omnibus root provider.
    known_omnibus_root = is_known_omnibus_root(ctx)

    linkable_root = create_linkable_root(
        ctx,
        name = get_default_shared_library_name(linker_info, ctx.label),
        link_infos = LinkInfos(
            default = LinkInfo(
                linkables = [ArchiveLinkable(archive = Archive(artifact = libraries[LinkStyle("static_pic")]), linker_type = linker_type, link_whole = True)],
            ),
        ),
        deps = inherited_non_rust_link_deps,
        graph = deps_linkable_graph,
        create_shared_root = known_omnibus_root,
    )
    providers.append(linkable_root)

    # Mark libraries that support `dlopen`.
    if getattr(ctx.attrs, "supports_python_dlopen", False):
        providers.append(DlopenableLibraryInfo())

    roots = {}

    if known_omnibus_root:
        roots[ctx.label] = AnnotatedLinkableRoot(root = linkable_root)

    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            linkable_node = create_linkable_node(
                ctx = ctx,
                preferred_linkage = preferred_linkage,
                exported_deps = inherited_non_rust_link_deps,
                link_infos = link_infos,
                shared_libs = solibs,
            ),
            roots = roots,
        ),
        children = [deps_linkable_graph],
    )

    providers.append(linkable_graph)

    providers.append(merge_link_group_lib_info(deps = inherited_non_rust_link_deps))

    return providers

# Compute transitive deps. Caller decides whether this is necessary.
def _compute_transitive_deps(ctx: AnalysisContext, link_style: LinkStyle.type) -> (dict["artifact", CrateName.type], dict["artifact", CrateName.type]):
    transitive_deps = {}
    transitive_rmeta_deps = {}
    for dep in resolve_deps(ctx):
        info = dep.dep.get(RustLinkInfo)
        if info == None:
            continue

        style = style_info(info, link_style)
        transitive_deps[style.rlib] = info.crate
        transitive_deps.update(style.transitive_deps)

        transitive_rmeta_deps[style.rmeta] = info.crate
        transitive_rmeta_deps.update(style.transitive_rmeta_deps)

    return (transitive_deps, transitive_rmeta_deps)
