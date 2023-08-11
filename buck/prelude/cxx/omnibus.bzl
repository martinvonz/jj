# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:local_only.bzl", "get_resolved_cxx_binary_link_execution_preference")
load("@prelude//cxx:cxx_toolchain_types.bzl", "PicBehavior")
load(
    "@prelude//cxx:link.bzl",
    "CxxLinkResult",  # @unused Used as a type
    "cxx_link_shared_library",
)
load("@prelude//linking:execution_preference.bzl", "LinkExecutionPreference")
load(
    "@prelude//linking:link_info.bzl",
    "LinkArgs",
    "LinkInfo",
    "LinkInfos",
    "LinkStyle",
    "Linkage",
    "LinkedObject",
    "SharedLibLinkable",
    "get_actual_link_style",
    "link_info_to_args",
    get_link_info_from_link_infos = "get_link_info",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "AnnotatedLinkableRoot",
    "LinkableGraph",  # @unused Used as a type
    "LinkableNode",
    "LinkableRootAnnotation",
    "LinkableRootInfo",
    "get_deps_for_link",
    "get_link_info",
    "linkable_deps",
    "linkable_graph",
)
load(
    "@prelude//utils:graph_utils.bzl",
    "breadth_first_traversal_by",
    "post_order_traversal",
)
load("@prelude//utils:utils.bzl", "expect", "flatten", "value_or")
load("@prelude//open_source.bzl", "is_open_source")
load(":cxx_context.bzl", "get_cxx_toolchain_info")
load(
    ":linker.bzl",
    "get_default_shared_library_name",
    "get_ignore_undefined_symbols_flags",
    "get_no_as_needed_shared_libs_flags",
    "get_shared_library_name",
)
load(
    ":symbols.bzl",
    "create_global_symbols_version_script",
    "extract_global_syms",
    "extract_symbol_names",
    "extract_undefined_syms",
    "get_undefined_symbols_args",
)

OmnibusEnvironment = provider(fields = [
    "dummy_omnibus",
    "exclusions",
    "roots",
    "enable_explicit_roots",
    "prefer_stripped_objects",
    "shared_root_ld_flags",
    "force_hybrid_links",
])

Disposition = enum("root", "excluded", "body", "omitted")

OmnibusGraph = record(
    nodes = field({"label": LinkableNode.type}),
    # All potential root notes for an omnibus link (e.g. C++ libraries,
    # C++ Python extensions).
    roots = field({"label": AnnotatedLinkableRoot.type}),
    # All nodes that should be excluded from libomnibus.
    excluded = field({"label": None}),
)

# Bookkeeping information used to setup omnibus link rules.
OmnibusSpec = record(
    body = field({"label": None}, {}),
    excluded = field({"label": None}, {}),
    roots = field({"label": AnnotatedLinkableRoot.type}, {}),
    exclusion_roots = field(["label"]),
    # All link infos.
    link_infos = field({"label": LinkableNode.type}, {}),
    dispositions = field({"label": Disposition.type}),
)

OmnibusPrivateRootProductCause = record(
    category = field(str),
    # Miss-assigned label
    label = field(["label", None], default = None),
    # Its actual disposiiton
    disposition = field([Disposition.type, None], default = None),
)

OmnibusRootProduct = record(
    shared_library = field(LinkedObject.type),
    undefined_syms = field("artifact"),
    global_syms = field("artifact"),
    # If set, this explains why we had to use a private root for this product.
    # If unset, this means the root was a shared root we reused.
    private = field([OmnibusPrivateRootProductCause.type, None]),
)

AnnotatedOmnibusRootProduct = record(
    product = field(OmnibusRootProduct.type),
    annotation = field([LinkableRootAnnotation.type, None]),
)

SharedOmnibusRoot = record(
    product = field(OmnibusRootProduct.type),
    linker_type = field(str),
    required_body = field(["label"]),
    required_exclusions = field(["label"]),
    prefer_stripped_objects = field(bool),
)

# The result of the omnibus link.
OmnibusSharedLibraries = record(
    omnibus = field([CxxLinkResult.type, None], None),
    libraries = field({str: LinkedObject.type}, {}),
    roots = field({"label": AnnotatedOmnibusRootProduct.type}, {}),
    exclusion_roots = field(["label"]),
    excluded = field(["label"]),
    dispositions = field({"label": Disposition.type}),
)

def get_omnibus_graph(graph: LinkableGraph.type, roots: dict[Label, AnnotatedLinkableRoot.type], excluded: dict[Label, None]) -> OmnibusGraph.type:
    graph_nodes = graph.nodes.traverse()
    nodes = {}
    for node in filter(None, graph_nodes):
        if node.linkable:
            nodes[node.label] = node.linkable

        for root, annotated in node.roots.items():
            # When building ou graph, we prefer un-annotated roots. Annotations
            # tell us if a root was discovered implicitly, but if was
            # discovered explicitly (in which case it has no annotation) then
            # we would rather record that, since the annotation wasn't
            # necessary.
            if annotated.annotation:
                roots.setdefault(root, annotated)
            else:
                roots[root] = annotated
        excluded.update(node.excluded)

    return OmnibusGraph(nodes = nodes, roots = roots, excluded = excluded)

def get_roots(label: Label, deps: list[Dependency]) -> dict[Label, AnnotatedLinkableRoot.type]:
    roots = {}
    for dep in deps:
        if LinkableRootInfo in dep:
            roots[dep.label] = AnnotatedLinkableRoot(
                root = dep[LinkableRootInfo],
                annotation = LinkableRootAnnotation(dependent = label),
            )
    return roots

def get_excluded(deps: list[Dependency] = []) -> dict[Label, None]:
    excluded_nodes = {}
    for dep in deps:
        dep_info = linkable_graph(dep)
        if dep_info != None:
            excluded_nodes[dep_info.label] = None
    return excluded_nodes

def create_linkable_root(
        ctx: AnalysisContext,
        graph: LinkableGraph.type,
        link_infos: LinkInfos.type,
        name: [str, None] = None,
        deps: list[Dependency] = [],
        create_shared_root: bool = False) -> LinkableRootInfo.type:
    # Only include dependencies that are linkable.
    deps = linkable_deps(deps)

    def create_shared_root_impl():
        env = ctx.attrs._omnibus_environment
        if not env:
            return (None, OmnibusPrivateRootProductCause(category = "no_omnibus_environment"))

        env = env[OmnibusEnvironment]
        prefer_stripped_objects = env.prefer_stripped_objects

        if not create_shared_root:
            return (None, OmnibusPrivateRootProductCause(category = "no_shared_root"))

        omnibus_graph = get_omnibus_graph(graph, {}, {})

        inputs = []
        toolchain_info = get_cxx_toolchain_info(ctx)
        linker_info = toolchain_info.linker_info
        linker_type = linker_info.type
        inputs.append(LinkInfo(
            pre_flags =
                get_no_as_needed_shared_libs_flags(linker_type) +
                get_ignore_undefined_symbols_flags(linker_type),
        ))

        inputs.append(get_link_info_from_link_infos(
            link_infos,
            prefer_stripped = prefer_stripped_objects,
        ))

        inputs.append(LinkInfo(linkables = [SharedLibLinkable(lib = env.dummy_omnibus)]))

        env_excluded = _exclusions_from_env(env, omnibus_graph)

        required_body = []
        required_exclusions = []

        for dep in _link_deps(omnibus_graph.nodes, deps, toolchain_info.pic_behavior):
            node = omnibus_graph.nodes[dep]

            actual_link_style = get_actual_link_style(
                LinkStyle("shared"),
                node.preferred_linkage,
                toolchain_info.pic_behavior,
            )

            if actual_link_style != LinkStyle("shared"):
                inputs.append(
                    get_link_info(
                        node,
                        actual_link_style,
                        prefer_stripped = prefer_stripped_objects,
                    ),
                )
                continue

            is_excluded = dep in env_excluded or dep in omnibus_graph.excluded
            is_root = dep in omnibus_graph.roots

            if is_excluded or (_is_shared_only(node) and not is_root):
                inputs.append(get_link_info(node, actual_link_style, prefer_stripped = prefer_stripped_objects))
                required_exclusions.append(dep)
                continue

            if is_root:
                dep_root = omnibus_graph.roots[dep].root.shared_root

                if dep_root == None:
                    # If we know our dep is a root, but our dep didn't know
                    # that and didn't produce a shared root, then there is no
                    # point in producing anything a reusable root here since it
                    # wo'nt actually *be* reusable due to the root mismatch.
                    return (None, OmnibusPrivateRootProductCause(category = "dep_no_shared_root", label = dep))

                inputs.append(LinkInfo(pre_flags = [
                    cmd_args(dep_root.product.shared_library.output),
                ]))
                continue

            required_body.append(dep)

        output = "omnibus/" + value_or(name, get_default_shared_library_name(linker_info, ctx.label))
        link_result = cxx_link_shared_library(
            ctx = ctx,
            output = output,
            name = name,
            links = [LinkArgs(flags = env.shared_root_ld_flags), LinkArgs(infos = inputs)],
            category_suffix = "omnibus_root",
            identifier = name or output,
        )
        shared_library = link_result.linked_object

        return (
            SharedOmnibusRoot(
                product = OmnibusRootProduct(
                    shared_library = shared_library,
                    global_syms = extract_global_syms(
                        ctx,
                        shared_library.output,
                        category_prefix = "omnibus",
                        prefer_local = False,
                    ),
                    undefined_syms = extract_undefined_syms(
                        ctx,
                        shared_library.output,
                        category_prefix = "omnibus",
                        prefer_local = False,
                    ),
                    private = None,
                ),
                required_body = required_body,
                required_exclusions = required_exclusions,
                prefer_stripped_objects = prefer_stripped_objects,
                linker_type = linker_type,
            ),
            None,
        )

    (shared_root, no_shared_root_reason) = create_shared_root_impl()

    return LinkableRootInfo(
        name = name,
        link_infos = link_infos,
        deps = deps,
        shared_root = shared_root,
        no_shared_root_reason = no_shared_root_reason,
    )

def _exclusions_from_env(env: OmnibusEnvironment.type, graph: OmnibusGraph.type):
    excluded = [
        label
        for label, info in graph.nodes.items()
        if _is_excluded_by_environment(label, env) and not _is_static_only(info)
    ]

    return {label: None for label in excluded}

def _is_excluded_by_environment(label: Label, env: OmnibusEnvironment.type) -> bool:
    return label.raw_target() in env.exclusions

def _omnibus_soname(ctx):
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    return get_shared_library_name(linker_info, "omnibus")

def create_dummy_omnibus(ctx: AnalysisContext, extra_ldflags: list[""] = []) -> "artifact":
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    link_result = cxx_link_shared_library(
        ctx = ctx,
        output = get_shared_library_name(linker_info, "omnibus-dummy"),
        name = _omnibus_soname(ctx),
        links = [LinkArgs(flags = extra_ldflags)],
        category_suffix = "dummy_omnibus",
    )
    return link_result.linked_object.output

def _link_deps(
        link_infos: dict[Label, LinkableNode.type],
        deps: list[Label],
        pic_behavior: PicBehavior.type) -> list[Label]:
    """
    Return transitive deps required to link dynamically against the given deps.
    This will following through deps of statically linked inputs and exported
    deps of everything else (see https://fburl.com/diffusion/rartsbkw from v1).
    """

    def find_deps(node: Label):
        return get_deps_for_link(link_infos[node], LinkStyle("shared"), pic_behavior)

    return breadth_first_traversal_by(link_infos, deps, find_deps)

def all_deps(
        link_infos: dict[Label, LinkableNode.type],
        roots: list[Label]) -> list[Label]:
    """
    Return all transitive deps from following the given nodes.
    """

    def find_transitive_deps(node: Label):
        return link_infos[node].deps + link_infos[node].exported_deps

    all_deps = breadth_first_traversal_by(link_infos, roots, find_transitive_deps)

    return all_deps

def _create_root(
        ctx: AnalysisContext,
        spec: OmnibusSpec.type,
        annotated_root_products,
        root: LinkableRootInfo.type,
        label: Label,
        link_deps: list[Label],
        omnibus: "artifact",
        pic_behavior: PicBehavior.type,
        extra_ldflags: list[""] = [],
        prefer_stripped_objects: bool = False) -> OmnibusRootProduct.type:
    """
    Link a root omnibus node.
    """

    linker_info = get_cxx_toolchain_info(ctx).linker_info
    linker_type = linker_info.type

    if spec.body:
        if root.shared_root != None:
            # NOTE: This ignores ldflags. We rely on env.shared_root_ld_flags instead.
            private = _requires_private_root(
                root.shared_root,
                linker_type,
                prefer_stripped_objects,
                spec,
            )
            if private == None:
                return root.shared_root.product
        else:
            private = root.no_shared_root_reason
    else:
        private = OmnibusPrivateRootProductCause(category = "no_body")

    inputs = []

    # Since we're linking against a dummy omnibus which has no symbols, we need
    # to make sure the linker won't drop it from the link or complain about
    # missing symbols.
    inputs.append(LinkInfo(
        pre_flags =
            get_no_as_needed_shared_libs_flags(linker_type) +
            get_ignore_undefined_symbols_flags(linker_type),
    ))

    # add native target link input
    inputs.append(
        get_link_info_from_link_infos(
            root.link_infos,
            prefer_stripped = prefer_stripped_objects,
        ),
    )

    # Link to Omnibus
    if spec.body:
        inputs.append(LinkInfo(linkables = [SharedLibLinkable(lib = omnibus)]))

    # Add deps of the root to the link line.
    for dep in link_deps:
        node = spec.link_infos[dep]
        actual_link_style = get_actual_link_style(
            LinkStyle("shared"),
            node.preferred_linkage,
            pic_behavior,
        )

        # If this dep needs to be linked statically, then link it directly.
        if actual_link_style != LinkStyle("shared"):
            inputs.append(get_link_info(
                node,
                actual_link_style,
                prefer_stripped = prefer_stripped_objects,
            ))
            continue

        # If this is another root.
        if dep in spec.roots:
            other_root = annotated_root_products[dep]

            # TODO(cjhopman): This should be passing structured linkables
            inputs.append(LinkInfo(pre_flags = [cmd_args(other_root.product.shared_library.output)]))
            continue

        # If this node is in omnibus, just add that to the link line.
        if dep in spec.body:
            continue

        # At this point, this should definitely be an excluded node.
        expect(dep in spec.excluded, str(dep))

        # We should have already handled statically linked nodes above.
        expect(actual_link_style == LinkStyle("shared"))
        inputs.append(get_link_info(node, actual_link_style))

    output = value_or(root.name, get_default_shared_library_name(
        linker_info,
        label,
    ))

    # link the rule
    link_result = cxx_link_shared_library(
        ctx = ctx,
        output = output,
        name = root.name,
        links = [LinkArgs(flags = extra_ldflags), LinkArgs(infos = inputs)],
        category_suffix = "omnibus_root",
        identifier = root.name or output,
        # We prefer local execution because there are lot of cxx_link_omnibus_root
        # running simultaneously, so while their overall load is reasonable,
        # their peak execution load is very high.
        link_execution_preference = LinkExecutionPreference("local"),
    )
    shared_library = link_result.linked_object

    return OmnibusRootProduct(
        shared_library = shared_library,
        global_syms = extract_global_syms(
            ctx,
            shared_library.output,
            category_prefix = "omnibus",
            # Same as above.
            prefer_local = True,
        ),
        undefined_syms = extract_undefined_syms(
            ctx,
            shared_library.output,
            category_prefix = "omnibus",
            # Same as above.
            prefer_local = True,
        ),
        private = private,
    )

def _requires_private_root(
        candidate: SharedOmnibusRoot.type,
        linker_type: str,
        prefer_stripped_objects: bool,
        spec: OmnibusSpec.type) -> [OmnibusPrivateRootProductCause.type, None]:
    if candidate.linker_type != linker_type:
        return OmnibusPrivateRootProductCause(category = "linker_type")

    if candidate.prefer_stripped_objects != prefer_stripped_objects:
        return OmnibusPrivateRootProductCause(category = "prefer_stripped_objects")

    for required_body in candidate.required_body:
        if not (required_body in spec.body and required_body not in spec.roots):
            return OmnibusPrivateRootProductCause(
                category = "required_body",
                label = required_body,
                disposition = spec.dispositions[required_body],
            )

    for required_exclusion in candidate.required_exclusions:
        if not required_exclusion in spec.excluded:
            return OmnibusPrivateRootProductCause(
                category = "required_exclusion",
                label = required_exclusion,
                disposition = spec.dispositions[required_exclusion],
            )

    return None

def _extract_global_symbols_from_link_args(
        ctx: AnalysisContext,
        name: str,
        link_args: list[["artifact", "resolved_macro", cmd_args, str]],
        prefer_local: bool = False) -> "artifact":
    """
    Extract global symbols explicitly set in the given linker args (e.g.
    `-Wl,--export-dynamic-symbol=<sym>`).
    """

    # TODO(T110378137): This is ported from D24065414, but it might make sense
    # to explicitly tell Buck about the global symbols, rather than us trying to
    # extract it from linker flags (which is brittle).
    output = ctx.actions.declare_output(name)

    # We intentionally drop the artifacts referenced in the args when generating
    # the argsfile -- we just want to parse out symbol name flags and don't need
    # to materialize artifacts to do this.
    argsfile, _ = ctx.actions.write(name + ".args", link_args, allow_args = True)

    # TODO(T110378133): Make this work with other platforms.
    param = "--export-dynamic-symbol"
    pattern = "\\(-Wl,\\)\\?{}[,=]\\([^,]*\\)".format(param)

    # Used sed/grep to filter the symbol name from the relevant flags.
    # TODO(T110378130): As is the case in v1, we don't properly extract flags
    # from argsfiles embedded in existing args.
    script = (
        "set -euo pipefail; " +
        'cat "$@" | (grep -- \'{0}\' || [[ $? == 1 ]]) | sed \'s|{0}|\\2|\' | LC_ALL=C sort -S 10% -u > {{}}'
            .format(pattern)
    )
    ctx.actions.run(
        [
            "/usr/bin/env",
            "bash",
            "-c",
            cmd_args(output.as_output(), format = script),
            "",
            argsfile,
        ],
        category = "omnibus_global_symbol_flags",
        prefer_local = prefer_local,
        weight_percentage = 15,  # 10% + a little padding
    )
    return output

def _create_global_symbols_version_script(
        ctx: AnalysisContext,
        roots: list[AnnotatedOmnibusRootProduct.type],
        excluded: list["artifact"],
        link_args: list[["artifact", "resolved_macro", cmd_args, str]]) -> "artifact":
    """
    Generate a version script exporting symbols from from the given objects and
    link args.
    """

    # Get global symbols from roots.  We set a rule to do this per-rule, as
    # using a single rule to process all roots adds overhead to the critical
    # path of incremental flows (e.g. that only update a single root).
    global_symbols_files = [
        root.product.global_syms
        for root in roots
    ]

    # TODO(T110378126): Processing all excluded libs together may get expensive.
    # We should probably split this up and operate on individual libs.
    if excluded:
        global_symbols_files.append(extract_symbol_names(
            ctx = ctx,
            name = "__excluded_libs__.global_syms.txt",
            objects = excluded,
            dynamic = True,
            global_only = True,
            category = "omnibus_global_syms_excluded_libs",
        ))

    # Extract explicitly globalized symbols from linker args.
    global_symbols_files.append(_extract_global_symbols_from_link_args(
        ctx,
        "__global_symbols_from_args__.txt",
        link_args,
    ))

    return create_global_symbols_version_script(
        actions = ctx.actions,
        name = "__global_symbols__.vers",
        category = "omnibus_version_script",
        symbol_files = global_symbols_files,
    )

def _is_static_only(info: LinkableNode.type) -> bool:
    """
    Return whether this can only be linked statically.
    """
    return info.preferred_linkage == Linkage("static")

def _is_shared_only(info: LinkableNode.type) -> bool:
    """
    Return whether this can only use shared linking
    """
    return info.preferred_linkage == Linkage("shared")

def _create_omnibus(
        ctx: AnalysisContext,
        spec: OmnibusSpec.type,
        annotated_root_products,
        pic_behavior: PicBehavior.type,
        extra_ldflags: list[""] = [],
        prefer_stripped_objects: bool = False) -> CxxLinkResult.type:
    inputs = []

    # Undefined symbols roots...
    non_body_root_undefined_syms = [
        root.product.undefined_syms
        for label, root in annotated_root_products.items()
        if label not in spec.body
    ]
    if non_body_root_undefined_syms:
        inputs.append(LinkInfo(pre_flags = [
            get_undefined_symbols_args(
                ctx = ctx,
                name = "__undefined_symbols__.linker_script",
                symbol_files = non_body_root_undefined_syms,
                category = "omnibus_undefined_symbols",
            ),
        ]))

    # Process all body nodes.
    deps = {}
    global_symbols_link_args = []
    for label in spec.body:
        # If this body node is a root, add the it's output to the link.
        if label in spec.roots:
            root = annotated_root_products[label].product

            # TODO(cjhopman): This should be passing structured linkables
            inputs.append(LinkInfo(pre_flags = [cmd_args(root.shared_library.output)]))
            continue

        node = spec.link_infos[label]

        # Otherwise add in the static input for this node.
        actual_link_style = get_actual_link_style(
            LinkStyle("static_pic"),
            node.preferred_linkage,
            pic_behavior,
        )
        expect(actual_link_style == LinkStyle("static_pic"))
        body_input = get_link_info(
            node,
            actual_link_style,
            prefer_stripped = prefer_stripped_objects,
        )
        inputs.append(body_input)
        global_symbols_link_args.append(link_info_to_args(body_input))

        # Keep track of all first order deps of the omnibus monolith.
        for dep in node.deps + node.exported_deps:
            if dep not in spec.body:
                expect(dep in spec.excluded)
                deps[dep] = None

    toolchain_info = get_cxx_toolchain_info(ctx)

    # Now add deps of omnibus to the link
    for label in _link_deps(spec.link_infos, deps.keys(), toolchain_info.pic_behavior):
        node = spec.link_infos[label]
        actual_link_style = get_actual_link_style(
            LinkStyle("shared"),
            node.preferred_linkage,
            toolchain_info.pic_behavior,
        )
        inputs.append(get_link_info(
            node,
            actual_link_style,
            prefer_stripped = prefer_stripped_objects,
        ))

    linker_info = toolchain_info.linker_info

    # Add global symbols version script.
    # FIXME(agallagher): Support global symbols for darwin.
    if linker_info.type != "darwin":
        global_sym_vers = _create_global_symbols_version_script(
            ctx,
            # Extract symbols from roots...
            annotated_root_products.values(),
            # ... and the shared libs from excluded nodes.
            [
                shared_lib.output
                for label in spec.excluded
                for shared_lib in spec.link_infos[label].shared_libs.values()
            ],
            # Extract explicit global symbol names from flags in all body link args.
            global_symbols_link_args,
        )
        inputs.append(LinkInfo(pre_flags = [
            "-Wl,--version-script",
            global_sym_vers,
            # The version script contains symbols that are not defined. Up to
            # LLVM 15 this behavior was ignored but LLVM 16 turns it into
            # warning by default.
            "-Wl,--undefined-version",
        ]))

    soname = _omnibus_soname(ctx)

    return cxx_link_shared_library(
        ctx = ctx,
        output = soname,
        name = soname,
        links = [LinkArgs(flags = extra_ldflags), LinkArgs(infos = inputs)],
        category_suffix = "omnibus",
        # TODO(T110378138): As with static C++ links, omnibus links are
        # currently too large for RE, so run them locally for now (e.g.
        # https://fb.prod.workplace.com/groups/buck2dev/posts/2953023738319012/).
        # NB: We explicitly pass a value here to override
        # the linker_info.link_libraries_locally that's used by `cxx_link_shared_library`.
        # That's because we do not want to apply the linking behavior universally,
        # just use it for omnibus.
        link_execution_preference = get_resolved_cxx_binary_link_execution_preference(ctx, [], use_hybrid_links_for_libomnibus(ctx), toolchain_info),
        link_weight = linker_info.link_weight,
        enable_distributed_thinlto = ctx.attrs.enable_distributed_thinlto,
        identifier = soname,
    )

def _build_omnibus_spec(
        ctx: AnalysisContext,
        graph: OmnibusGraph.type) -> OmnibusSpec.type:
    """
    Divide transitive deps into excluded, root, and body nodes, which we'll
    use to link the various parts of omnibus.
    """

    exclusion_roots = graph.excluded.keys() + _implicit_exclusion_roots(ctx, graph)

    # Build up the set of all nodes that we have to exclude from omnibus linking
    # (any node that is excluded will exclude all it's transitive deps).
    excluded = {
        label: None
        for label in all_deps(
            graph.nodes,
            exclusion_roots,
        )
    }

    # Finalized root nodes, after removing any excluded roots.
    roots = {
        label: root
        for label, root in graph.roots.items()
        if label not in excluded
    }

    # Find the deps of the root nodes.  These form the roots of the nodes
    # included in the omnibus link.
    first_order_root_deps = []
    for label in _link_deps(graph.nodes, flatten([r.root.deps for r in roots.values()]), get_cxx_toolchain_info(ctx).pic_behavior):
        # We only consider deps which aren't *only* statically linked.
        if _is_static_only(graph.nodes[label]):
            continue

        # Don't include a root's dep onto another root.
        if label in roots:
            continue
        first_order_root_deps.append(label)

    # All body nodes.  These included all non-excluded body nodes and any non-
    # excluded roots which are reachable by these body nodes (since they will
    # need to be put on the link line).
    body = {
        label: None
        for label in all_deps(graph.nodes, first_order_root_deps)
        if label not in excluded
    }

    dispositions = {}

    for node, info in graph.nodes.items():
        if _is_static_only(info):
            continue

        if node in roots:
            dispositions[node] = Disposition("root")
            continue

        if node in excluded:
            dispositions[node] = Disposition("excluded")
            continue

        if node in body:
            dispositions[node] = Disposition("body")
            continue

        # Why does that happen? Who knows with Omnibus :(
        dispositions[node] = Disposition("omitted")

    return OmnibusSpec(
        excluded = excluded,
        roots = roots,
        body = body,
        link_infos = graph.nodes,
        exclusion_roots = exclusion_roots,
        dispositions = dispositions,
    )

def _implicit_exclusion_roots(ctx: AnalysisContext, graph: OmnibusGraph.type) -> list[Label]:
    env = ctx.attrs._omnibus_environment
    if not env:
        return []
    env = env[OmnibusEnvironment]

    return [
        label
        for label, info in graph.nodes.items()
        if _is_excluded_by_environment(label, env) or (_is_shared_only(info) and (label not in graph.roots))
    ]

def _ordered_roots(
        spec: OmnibusSpec.type,
        pic_behavior: PicBehavior.type) -> list[(Label, AnnotatedLinkableRoot.type, list[Label])]:
    """
    Return information needed to link the roots nodes.
    """

    # Calculate all deps each root node needs to link against.
    link_deps = {}
    for label, root in spec.roots.items():
        link_deps[label] = _link_deps(spec.link_infos, root.root.deps, pic_behavior)

    # Used the link deps to create the graph of root nodes.
    root_graph = {
        node: [dep for dep in deps if dep in spec.roots]
        for node, deps in link_deps.items()
    }

    ordered_roots = []

    # Emit the root link info in post-order, so that we generate root link rules
    # for dependencies before their dependents.
    for label in post_order_traversal(root_graph):
        root = spec.roots[label]
        deps = link_deps[label]
        ordered_roots.append((label, root, deps))

    return ordered_roots

def create_omnibus_libraries(
        ctx: AnalysisContext,
        graph: OmnibusGraph.type,
        extra_ldflags: list[""] = [],
        prefer_stripped_objects: bool = False) -> OmnibusSharedLibraries.type:
    spec = _build_omnibus_spec(ctx, graph)
    pic_behavior = get_cxx_toolchain_info(ctx).pic_behavior

    # Create dummy omnibus
    dummy_omnibus = create_dummy_omnibus(ctx, extra_ldflags)

    libraries = {}
    root_products = {}

    # Link all root nodes against the dummy libomnibus lib.
    for label, annotated_root, link_deps in _ordered_roots(spec, pic_behavior):
        product = _create_root(
            ctx,
            spec,
            root_products,
            annotated_root.root,
            label,
            link_deps,
            dummy_omnibus,
            pic_behavior,
            extra_ldflags,
            prefer_stripped_objects,
        )
        if annotated_root.root.name != None:
            libraries[annotated_root.root.name] = product.shared_library
        root_products[label] = AnnotatedOmnibusRootProduct(
            product = product,
            annotation = annotated_root.annotation,
        )

    # If we have body nodes, then link them into the monolithic libomnibus.so.
    omnibus = None
    if spec.body:
        omnibus = _create_omnibus(
            ctx,
            spec,
            root_products,
            pic_behavior,
            extra_ldflags,
            prefer_stripped_objects,
        )
        libraries[_omnibus_soname(ctx)] = omnibus.linked_object

    # For all excluded nodes, just add their regular shared libs.
    for label in spec.excluded:
        for name, lib in spec.link_infos[label].shared_libs.items():
            libraries[name] = lib

    return OmnibusSharedLibraries(
        omnibus = omnibus,
        libraries = libraries,
        roots = root_products,
        exclusion_roots = spec.exclusion_roots,
        excluded = spec.excluded.keys(),
        dispositions = spec.dispositions,
    )

def is_known_omnibus_root(ctx: AnalysisContext) -> bool:
    env = ctx.attrs._omnibus_environment
    if not env:
        return False

    env = env[OmnibusEnvironment]

    if not env.enable_explicit_roots:
        return False

    if ctx.attrs.supports_python_dlopen != None:
        return ctx.attrs.supports_python_dlopen

    if ctx.label.raw_target() in env.roots:
        return True

    return False

def explicit_roots_enabled(ctx: AnalysisContext) -> bool:
    env = ctx.attrs._omnibus_environment
    if not env:
        return False
    return env[OmnibusEnvironment].enable_explicit_roots

def use_hybrid_links_for_libomnibus(ctx: AnalysisContext) -> bool:
    env = ctx.attrs._omnibus_environment
    if not env:
        return False
    return env[OmnibusEnvironment].force_hybrid_links

def omnibus_environment_attr():
    default = select({
        # In open source, we don't want to use omnibus
        "DEFAULT": "fbcode//buck2/platform/omnibus:omnibus_environment",
        "fbcode//buck2/platform/omnibus:do_not_inject_omnibus_environment": None,
    }) if not is_open_source() else select({"DEFAULT": None})

    return attrs.option(attrs.dep(), default = default)
