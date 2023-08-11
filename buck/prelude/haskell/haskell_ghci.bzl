# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//cxx:cxx_toolchain_types.bzl", "PicBehavior")
load(
    "@prelude//cxx:link.bzl",
    "cxx_link_shared_library",
)
load(
    "@prelude//haskell:haskell.bzl",
    "HaskellLibraryProvider",
    "HaskellToolchainInfo",
    "PackagesInfo",
    "attr_deps",
    "get_packages_info",
)
load(
    "@prelude//linking:link_info.bzl",
    "LinkArgs",
    "LinkInfo",
    "LinkStyle",
    "Linkage",
    "get_actual_link_style",
    "set_linkable_link_whole",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableRootInfo",
    "create_linkable_graph",
    "get_deps_for_link",
    "get_link_info",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "traverse_shared_library_info",
)
load(
    "@prelude//utils:graph_utils.bzl",
    "breadth_first_traversal",
    "breadth_first_traversal_by",
)
load("@prelude//utils:utils.bzl", "flatten")

GHCiPreloadDepsInfo = record(
    preload_symlinks = {str: "artifact"},
    preload_deps_root = "artifact",
)

USER_GHCI_PATH = "user_ghci_path"
BINUTILS_PATH = "binutils_path"
GHCI_LIB_PATH = "ghci_lib_path"
CC_PATH = "cc_path"
CPP_PATH = "cpp_path"
CXX_PATH = "cxx_path"
GHCI_PACKAGER = "ghc_pkg_path"
GHCI_GHC_PATH = "ghc_path"

HaskellOmnibusData = record(
    omnibus = "artifact",
    so_symlinks_root = "artifact",
)

def _write_final_ghci_script(
        ctx: AnalysisContext,
        omnibus_data: HaskellOmnibusData.type,
        packages_info: PackagesInfo.type,
        packagedb_args: "cmd_args",
        prebuilt_packagedb_args: "cmd_args",
        iserv_script: "artifact",
        start_ghci_file: "artifact",
        ghci_bin: "artifact",
        haskell_toolchain: HaskellToolchainInfo.type,
        ghci_script_template: "artifact") -> "artifact":
    srcs = " ".join(
        [
            paths.normalize(
                paths.join(
                    paths.relativize(str(ctx.label.path), "fbcode"),
                    s,
                ),
            )
            for s in ctx.attrs.srcs
        ],
    )

    # Collect compiler flags
    compiler_flags = cmd_args(
        # TODO(gustavoavena): do I need to filter these flags?
        filter(lambda x: x == "-O", haskell_toolchain.compiler_flags),
        delimiter = " ",
    )

    compiler_flags.add([
        "-fPIC",
        "-fexternal-dynamic-refs",
    ])

    if (ctx.attrs.enable_profiling):
        compiler_flags.add([
            "-prof",
            "-osuf p_o",
            "-hisuf p_hi",
        ])

    compiler_flags.add(ctx.attrs.compiler_flags)
    omnibus_so = omnibus_data.omnibus

    final_ghci_script = _replace_macros_in_script_template(
        ctx,
        script_template = ghci_script_template,
        haskell_toolchain = haskell_toolchain,
        ghci_bin = ghci_bin,
        exposed_package_args = packages_info.exposed_package_args,
        packagedb_args = packagedb_args,
        prebuilt_packagedb_args = prebuilt_packagedb_args,
        start_ghci = start_ghci_file,
        iserv_script = iserv_script,
        squashed_so = omnibus_so,
        compiler_flags = compiler_flags,
        srcs = srcs,
        output_name = ctx.label.name,
    )

    return final_ghci_script

def _build_haskell_omnibus_so(
        ctx: AnalysisContext) -> HaskellOmnibusData.type:
    link_style = LinkStyle("static_pic")

    # pic_behavior = PicBehavior("always_enabled")
    pic_behavior = PicBehavior("supported")
    preload_deps = ctx.attrs.preload_deps

    all_deps = attr_deps(ctx) + preload_deps + ctx.attrs.template_deps

    linkable_graph_ = create_linkable_graph(
        ctx,
        deps = all_deps,
    )

    # Keep only linkable nodes
    graph_nodes = {
        n.label: n.linkable
        for n in linkable_graph_.nodes.traverse()
        if n.linkable
    }

    # Map node label to its dependencies' labels
    dep_graph = {
        nlabel: get_deps_for_link(n, link_style, pic_behavior)
        for nlabel, n in graph_nodes.items()
    }

    all_direct_deps = [dep.label for dep in all_deps]
    dep_graph[ctx.label] = all_direct_deps

    # Need to exclude all transitive deps of excluded deps
    all_nodes_to_exclude = breadth_first_traversal(
        dep_graph,
        [dep.label for dep in preload_deps],
    )

    # Body nodes should support haskell omnibus (e.g. cxx_library)
    # and can't be prebuilt tp dependencies
    body_nodes = {}

    # Prebuilt (i.e. third-party) nodes shouldn't be statically linked on
    # the omnibus, but we need to keep track of them because they're a
    # dependency of it and are linked dynamically.
    prebuilt_so_deps = {}

    # Helper to get body nodes and prebuilt dependencies of the
    # omnibus SO (which should dynamically linked) during BFS traversal
    def find_deps_for_body(node_label: Label):
        deps = dep_graph[node_label]

        final_deps = []
        for node_label in deps:
            node = graph_nodes[node_label]

            # We process these libs even if they're excluded, as they need to
            # be added to the link line.
            if "prebuilt_so_for_haskell_omnibus" in node.labels:
                # If the library is marked as force-static, then it won't provide
                # shared libs and we'll have to link is statically.
                if node.preferred_linkage == Linkage("static"):
                    body_nodes[node_label] = None
                else:
                    prebuilt_so_deps[node_label] = None

            if node_label in all_nodes_to_exclude:
                continue

            if "supports_haskell_omnibus" in node.labels and "prebuilt_so_for_haskell_omnibus" not in node.labels:
                body_nodes[node_label] = None

            final_deps.append(node_label)

        return final_deps

    # This is not the final set of body nodes, because it still includes
    # nodes that don't support omnibus (e.g. haskell_library nodes)
    breadth_first_traversal_by(
        dep_graph,
        [ctx.label],
        find_deps_for_body,
    )

    # After collecting all the body nodes, get all their linkables (e.g. `.a`
    # files) that will be part of the omnibus SO.
    body_link_infos = {}

    for node_label in body_nodes.keys():
        node = graph_nodes[node_label]

        node_target = node_label.raw_target()
        if (node_target in body_link_infos):
            # Not skipping these leads to duplicate symbol errors
            continue

        actual_link_style = get_actual_link_style(
            link_style,
            node.preferred_linkage,
            pic_behavior = pic_behavior,
        )

        li = get_link_info(node, actual_link_style)
        linkables = [
            # All symbols need to be included in the omnibus so, even if
            # they're not being referenced yet, so we should enable
            # link_whole which passes the `--whole-archive` linker flag.
            set_linkable_link_whole(linkable)
            for linkable in li.linkables
        ]
        new_li = LinkInfo(
            name = li.name,
            pre_flags = li.pre_flags,
            post_flags = li.post_flags,
            linkables = linkables,
            external_debug_info = li.external_debug_info,
        )
        body_link_infos[node_target] = new_li

    # Handle third-party dependencies of the omnibus SO
    tp_deps_shared_link_infos = {}
    so_symlinks = {}

    for node_label in prebuilt_so_deps.keys():
        node = graph_nodes[node_label]

        shared_li = node.link_infos.get(LinkStyle("shared"), None)
        if shared_li != None:
            tp_deps_shared_link_infos[node_label] = shared_li.default
        for libname, linkObject in node.shared_libs.items():
            so_symlinks[libname] = linkObject.output

    # Create symlinks to the TP dependencies' SOs
    so_symlinks_root_path = ctx.label.name + ".so-symlinks"
    so_symlinks_root = ctx.actions.symlinked_dir(
        so_symlinks_root_path,
        so_symlinks,
    )

    linker_info = get_cxx_toolchain_info(ctx).linker_info
    soname = "libghci_dependencies.so"
    extra_ldflags = [
        "-rpath",
        "$ORIGIN/{}".format(so_symlinks_root_path),
    ]
    link_result = cxx_link_shared_library(
        ctx,
        soname,
        links = [
            LinkArgs(flags = extra_ldflags),
            LinkArgs(infos = body_link_infos.values()),
            LinkArgs(infos = tp_deps_shared_link_infos.values()),
        ],
        category_suffix = "omnibus",
        link_weight = linker_info.link_weight,
        identifier = soname,
    )
    omnibus = link_result.linked_object.output

    return HaskellOmnibusData(
        omnibus = omnibus,
        so_symlinks_root = so_symlinks_root,
    )

# Use the script_template_processor.py script to generate a script from a
# script template.
def _replace_macros_in_script_template(
        ctx: AnalysisContext,
        script_template: "artifact",
        haskell_toolchain: HaskellToolchainInfo.type,
        # Optional artifacts
        ghci_bin: ["artifact", None] = None,
        start_ghci: ["artifact", None] = None,
        iserv_script: ["artifact", None] = None,
        squashed_so: ["artifact", None] = None,
        # Optional cmd_args
        exposed_package_args: ["cmd_args", None] = None,
        packagedb_args: ["cmd_args", None] = None,
        prebuilt_packagedb_args: ["cmd_args", None] = None,
        compiler_flags: ["cmd_args", None] = None,
        # Optional string args
        srcs: [str, None] = None,
        output_name: [str, None] = None,
        ghci_iserv_path: [str, None] = None,
        preload_libs: [str, None] = None) -> "artifact":
    toolchain_paths = {
        BINUTILS_PATH: haskell_toolchain.ghci_binutils_path,
        GHCI_LIB_PATH: haskell_toolchain.ghci_lib_path,
        CC_PATH: haskell_toolchain.ghci_cc_path,
        CPP_PATH: haskell_toolchain.ghci_cpp_path,
        CXX_PATH: haskell_toolchain.ghci_cxx_path,
        GHCI_PACKAGER: haskell_toolchain.ghci_packager,
        GHCI_GHC_PATH: haskell_toolchain.ghci_ghc_path,
    }

    if ghci_bin != None:
        toolchain_paths[USER_GHCI_PATH] = ghci_bin.short_path

    final_script = ctx.actions.declare_output(
        script_template.basename if not output_name else output_name,
    )
    script_template_processor = haskell_toolchain.script_template_processor[RunInfo]

    replace_cmd = cmd_args(script_template_processor)
    replace_cmd.add(cmd_args(script_template, format = "--script_template={}"))
    for name, path in toolchain_paths.items():
        replace_cmd.add(cmd_args("--{}={}".format(name, path)))

    replace_cmd.add(cmd_args(
        final_script.as_output(),
        format = "--output={}",
    ))

    replace_cmd.add(cmd_args(
        ctx.label.name,
        format = "--target_name={}",
    ))

    exposed_package_args = exposed_package_args if exposed_package_args != None else ""
    replace_cmd.add(cmd_args(
        cmd_args(exposed_package_args, delimiter = " "),
        format = "--exposed_packages={}",
    ))

    if packagedb_args != None:
        replace_cmd.add(cmd_args(
            packagedb_args,
            format = "--package_dbs={}",
        ))
    if prebuilt_packagedb_args != None:
        replace_cmd.add(cmd_args(
            prebuilt_packagedb_args,
            format = "--prebuilt_package_dbs={}",
        ))

    # Tuple containing orig value (for null check), macro value and flag name
    optional_flags = [
        (
            start_ghci,
            start_ghci.short_path if start_ghci != None else "",
            "--start_ghci",
        ),
        (iserv_script, "iserv", "--iserv_path"),
        (
            squashed_so,
            squashed_so.short_path if squashed_so != None else "",
            "--squashed_so",
        ),
        (compiler_flags, compiler_flags, "--compiler_flags"),
        (srcs, srcs, "--srcs"),
        (ghci_iserv_path, ghci_iserv_path, "--ghci_iserv_path"),
        (preload_libs, preload_libs, "--preload_libs"),
    ]

    for (orig_val, macro_value, flag) in optional_flags:
        if orig_val != None:
            replace_cmd.add(cmd_args(
                macro_value,
                format = flag + "={}",
            ))

    ctx.actions.run(
        replace_cmd,
        category = "replace_template_{}".format(
            script_template.basename.replace("-", "_"),
        ),
        local_only = True,
    )

    return final_script

def _write_iserv_script(
        ctx: AnalysisContext,
        preload_deps_info: GHCiPreloadDepsInfo.type,
        haskell_toolchain: HaskellToolchainInfo.type) -> "artifact":
    ghci_iserv_template = haskell_toolchain.ghci_iserv_template

    if (not ghci_iserv_template):
        fail("ghci_iserv_template missing in haskell_toolchain")

    preload_libs = ":".join(
        [paths.join(
            "${DIR}",
            preload_deps_info.preload_deps_root.short_path,
            so,
        ) for so in sorted(preload_deps_info.preload_symlinks)],
    )

    if ctx.attrs.enable_profiling:
        ghci_iserv_path = haskell_toolchain.ghci_iserv_prof_path
    else:
        ghci_iserv_path = haskell_toolchain.ghci_iserv_path

    iserv_script_name = "iserv"
    if ctx.attrs.enable_profiling:
        iserv_script_name += "-prof"

    iserv_script = _replace_macros_in_script_template(
        ctx,
        script_template = ghci_iserv_template,
        output_name = iserv_script_name,
        haskell_toolchain = haskell_toolchain,
        ghci_iserv_path = ghci_iserv_path,
        preload_libs = preload_libs,
    )
    return iserv_script

def _build_preload_deps_root(
        ctx: AnalysisContext,
        haskell_toolchain: HaskellToolchainInfo.type) -> GHCiPreloadDepsInfo.type:
    preload_deps = ctx.attrs.preload_deps

    preload_symlinks = {}
    preload_libs_root = ctx.label.name + ".preload-symlinks"

    for preload_dep in preload_deps:
        if SharedLibraryInfo in preload_dep:
            slib_info = preload_dep[SharedLibraryInfo]

            shlib = traverse_shared_library_info(slib_info).items()

            for shlib_name, shared_lib in shlib:
                preload_symlinks[shlib_name] = shared_lib.lib.output

        # TODO(T150785851): build or get SO for direct preload_deps
        # TODO(T150785851): find out why the only SOs missing are the ones from
        # the preload_deps themselves, even though the ones from their deps are
        # already there.
        if LinkableRootInfo in preload_dep:
            linkable_root_info = preload_dep[LinkableRootInfo]
            preload_so_name = linkable_root_info.name

            linkables = map(lambda x: x.objects, linkable_root_info.link_infos.default.linkables)

            object_file = flatten(linkables)[0]

            preload_so = ctx.actions.declare_output(preload_so_name)
            link = cmd_args(haskell_toolchain.linker)
            link.add(haskell_toolchain.linker_flags)
            link.add(ctx.attrs.linker_flags)
            link.add("-o", preload_so.as_output())

            link.add(
                "-shared",
                "-dynamic",
                "-optl",
                "-Wl,-soname",
                "-optl",
                "-Wl," + preload_so_name,
            )
            link.add(object_file)

            ctx.actions.run(
                link,
                category = "haskell_ghci_link",
                identifier = preload_so_name,
            )

            preload_symlinks[preload_so_name] = preload_so

    preload_deps_root = ctx.actions.symlinked_dir(preload_libs_root, preload_symlinks)
    return GHCiPreloadDepsInfo(
        preload_deps_root = preload_deps_root,
        preload_symlinks = preload_symlinks,
    )

# Symlink the ghci binary that will be used, e.g. the internal fork in Haxlsh
def _symlink_ghci_binary(ctx, ghci_bin: "artifact"):
    # TODO(T155760998): set ghci_ghc_path as a dependency instead of string
    ghci_bin_dep = ctx.attrs.ghci_bin_dep
    if not ghci_bin_dep:
        fail("GHC binary path not specified")

    # NOTE: In the buck1 version we'd symlink the binary only if a custom one
    # was provided, but in buck2 we're always setting `ghci_bin_dep` (i.e.
    # to default one if custom wasn't provided).
    src = ghci_bin_dep[DefaultInfo].default_outputs[0]
    ctx.actions.symlink_file(ghci_bin.as_output(), src)

def _first_order_haskell_deps(ctx: AnalysisContext) -> list["HaskellLibraryInfo"]:
    return dedupe(
        flatten(
            [
                dep[HaskellLibraryProvider].lib.values()
                for dep in ctx.attrs.deps
                if HaskellLibraryProvider in dep
            ],
        ),
    )

# Creates the start.ghci script used to load the packages during startup
def _write_start_ghci(ctx: AnalysisContext, script_file: "artifact"):
    start_cmd = cmd_args()

    # Reason for unsetting `LD_PRELOAD` env var obtained from D6255224:
    # "Certain libraries (like allocators) cannot be loaded after the process
    # has started. When needing to use these libraries, send them to a
    # user-supplied script for handling them appropriately. Running the real
    # iserv with these libraries under LD_PRELOAD accomplishes this.
    # To ensure the LD_PRELOAD env doesn't make it to subsequently forked
    # processes, the very first action of start.ghci is to unset the variable."
    start_cmd.add("System.Environment.unsetEnv \"LD_PRELOAD\"")

    set_cmd = cmd_args(":set", delimiter = " ")
    first_order_deps = list(map(
        lambda dep: dep.name + "-" + dep.version,
        _first_order_haskell_deps(ctx),
    ))
    deduped_deps = {pkg: 1 for pkg in first_order_deps}.keys()
    package_list = cmd_args(
        deduped_deps,
        format = "-package {}",
        delimiter = " ",
    )
    set_cmd.add(package_list)
    set_cmd.add("\n")
    start_cmd.add(set_cmd)

    header_ghci = ctx.actions.declare_output("header.ghci")

    ctx.actions.write(header_ghci.as_output(), start_cmd)

    if ctx.attrs.ghci_init:
        append_ghci_init = cmd_args()
        append_ghci_init.add(
            ["sh", "-c", 'cat "$1" "$2" > "$3"', "--", header_ghci, ctx.attrs.ghci_init, script_file.as_output()],
        )
        ctx.actions.run(append_ghci_init, category = "append_ghci_init")
    else:
        ctx.actions.copy_file(script_file, header_ghci)

def haskell_ghci_impl(ctx: AnalysisContext) -> list["provider"]:
    start_ghci_file = ctx.actions.declare_output("start.ghci")
    _write_start_ghci(ctx, start_ghci_file)

    ghci_bin = ctx.actions.declare_output(ctx.attrs.name + ".bin/ghci")
    _symlink_ghci_binary(ctx, ghci_bin)

    haskell_toolchain = ctx.attrs._haskell_toolchain[HaskellToolchainInfo]
    preload_deps_info = _build_preload_deps_root(ctx, haskell_toolchain)

    ghci_script_template = haskell_toolchain.ghci_script_template

    if (not ghci_script_template):
        fail("ghci_script_template missing in haskell_toolchain")

    iserv_script = _write_iserv_script(ctx, preload_deps_info, haskell_toolchain)

    link_style = LinkStyle("static_pic")

    packages_info = get_packages_info(
        ctx,
        link_style,
        specify_pkg_version = True,
    )

    # Create package db symlinks
    package_symlinks = []

    package_symlinks_root = ctx.label.name + ".packages"

    packagedb_args = cmd_args(delimiter = " ")
    prebuilt_packagedb_args = cmd_args(delimiter = " ")

    for lib in packages_info.transitive_deps:
        if lib.is_prebuilt:
            prebuilt_packagedb_args.add(lib.db)
        else:
            lib_symlinks_root = paths.join(
                package_symlinks_root,
                lib.name,
            )
            lib_symlinks = {
                ("hi-" + link_style.value): lib.import_dirs[0],
                "packagedb": lib.db,
            }
            for o in lib.libs:
                lib_symlinks[o.short_path] = o

            symlinked_things = ctx.actions.symlinked_dir(
                lib_symlinks_root,
                lib_symlinks,
            )

            package_symlinks.append(symlinked_things)

            packagedb_args.add(
                paths.join(
                    lib_symlinks_root,
                    "packagedb",
                ),
            )

    script_templates = []
    for script_template in ctx.attrs.extra_script_templates:
        final_script = _replace_macros_in_script_template(
            ctx,
            script_template = script_template,
            haskell_toolchain = haskell_toolchain,
            ghci_bin = ghci_bin,
            exposed_package_args = packages_info.exposed_package_args,
            packagedb_args = packagedb_args,
            prebuilt_packagedb_args = prebuilt_packagedb_args,
        )
        script_templates.append(final_script)

    omnibus_data = _build_haskell_omnibus_so(ctx)

    final_ghci_script = _write_final_ghci_script(
        ctx,
        omnibus_data,
        packages_info,
        packagedb_args,
        prebuilt_packagedb_args,
        iserv_script,
        start_ghci_file,
        ghci_bin,
        haskell_toolchain,
        ghci_script_template,
    )

    outputs = [
        start_ghci_file,
        ghci_bin,
        preload_deps_info.preload_deps_root,
        iserv_script,
        omnibus_data.omnibus,
        omnibus_data.so_symlinks_root,
        final_ghci_script,
    ]
    outputs.extend(package_symlinks)
    outputs.extend(script_templates)

    # As default output (e.g. used in `$(location )` buck macros), the rule
    # should output a directory containing symlinks to all scripts and resources
    # (e.g. shared objects, package configs)
    output_artifacts = {o.short_path: o for o in outputs}
    root_output_dir = ctx.actions.symlinked_dir(
        "__{}__".format(ctx.label.name),
        output_artifacts,
    )
    run = cmd_args(final_ghci_script).hidden(outputs)

    return [
        DefaultInfo(default_outputs = [root_output_dir]),
        RunInfo(args = run),
    ]
