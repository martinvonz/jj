# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:artifacts.bzl", "unpack_artifact_map")
load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//:resources.bzl",
    "ResourceInfo",
    "gather_resources",
)
load("@prelude//cxx:cxx_link_utility.bzl", "shared_libs_symlink_tree_name")
load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxPlatformInfo")
load(
    "@prelude//cxx:omnibus.bzl",
    "get_excluded",
    "get_roots",
)
load(
    "@prelude//linking:link_info.bzl",
    "LinkedObject",  # @unused Used as a type
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "LinkableRootInfo",
    "create_linkable_graph",
    "create_linkable_graph_node",
)
load("@prelude//linking:shared_libraries.bzl", "SharedLibraryInfo", "merge_shared_libraries")
load("@prelude//python:toolchain.bzl", "PythonPlatformInfo", "get_platform_attr")
load("@prelude//utils:utils.bzl", "expect", "flatten", "from_named_set")
load(":compile.bzl", "PycInvalidationMode", "compile_manifests")
load(
    ":manifest.bzl",
    "ManifestInfo",  # @unused Used as a type
    "create_dep_manifest_for_source_map",
    "create_manifest_for_source_map",
)
load(
    ":native_python_util.bzl",
    "merge_cxx_extension_info",
)
load(":needed_coverage.bzl", "PythonNeededCoverageInfo")
load(":python.bzl", "PythonLibraryInfo", "PythonLibraryManifests", "PythonLibraryManifestsTSet")
load(":source_db.bzl", "create_python_source_db_info", "create_source_db", "create_source_db_no_deps")
load(":toolchain.bzl", "PythonToolchainInfo")

def dest_prefix(label: Label, base_module: [None, str]) -> str:
    """
    Find the prefix to use for placing files inside of the python link tree

    This uses the label's package path if `base_module` is `None`, or `base_module`,
    with '.' replaced by '/', if not None. If non-empty, the returned prefix will
    end with a '/'
    """
    if base_module == None:
        prefix = label.package
    else:
        prefix = base_module.replace(".", "/")

    # Add a leading slash if we need to, but don't do that for an empty base_module
    if prefix != "":
        prefix += "/"

    return prefix

def qualify_srcs(
        label: Label,
        base_module: [None, str],
        srcs: dict[str, "_a"]) -> dict[str, "_a"]:
    """
    Fully qualify package-relative sources with the rule's base module.

    Arguments:
        label: The label for the `python_library`. Used for errors, and to construct
               the path for each source file
        base_module: If provided, the module to prefix all files from `srcs` with in
                     the eventual binary. If `None`, use the package path.
                     Usage of this is discouraged, because it makes on-disk paths
                     not match the module in execution.
        srcs: A dictionary of {relative destination path: source file}. The derived
              base module will be prepended to the destination.
    """
    prefix = dest_prefix(label, base_module)

    # Use `path.normalize` here in case items in `srcs` contains relative paths.
    return {paths.normalize(prefix + dest): src for dest, src in srcs.items()}

def create_python_needed_coverage_info(
        label: Label,
        base_module: [None, str],
        srcs: list[str]) -> PythonNeededCoverageInfo.type:
    prefix = dest_prefix(label, base_module)
    return PythonNeededCoverageInfo(
        modules = {src: prefix + src for src in srcs},
    )

def create_python_library_info(
        actions: "actions",
        label: Label,
        srcs: [ManifestInfo.type, None] = None,
        src_types: [ManifestInfo.type, None] = None,
        bytecode: [dict[PycInvalidationMode.type, ManifestInfo.type], None] = None,
        dep_manifest: [ManifestInfo.type, None] = None,
        resources: [(ManifestInfo.type, list["_arglike"]), None] = None,
        extensions: [dict[str, LinkedObject.type], None] = None,
        deps: list["PythonLibraryInfo"] = [],
        shared_libraries: list["SharedLibraryInfo"] = []):
    """
    Create a `PythonLibraryInfo` for a set of sources and deps

    Arguments:
        label: The label for the `python_library`. Used for errors, and to construct
               the path for each source file
        srcs: A dictionary of {relative destination path: source file}.
        resources: A dictionary of {relative destination path: source file}.
        prebuilt_libraries: Prebuilt python libraries to include.
        deps: A list of `PythonLibraryInfo` objects from dependencies. These are merged
              into the resulting `PythonLibraryInfo`, as python needs all files present
              in the end
    Return:
        A fully merged `PythonLibraryInfo` provider, or fails if deps and/or srcs
        have destination paths that collide.
    """

    manifests = PythonLibraryManifests(
        label = label,
        srcs = srcs,
        src_types = src_types,
        resources = resources,
        dep_manifest = dep_manifest,
        bytecode = bytecode,
        extensions = extensions,
    )

    new_shared_libraries = merge_shared_libraries(
        actions,
        deps = shared_libraries + [dep.shared_libraries for dep in deps],
    )

    return PythonLibraryInfo(
        manifests = actions.tset(PythonLibraryManifestsTSet, value = manifests, children = [dep.manifests for dep in deps]),
        shared_libraries = new_shared_libraries,
    )

def gather_dep_libraries(raw_deps: list[list[Dependency]]) -> (list["PythonLibraryInfo"], list["SharedLibraryInfo"]):
    """
    Takes a list of raw dependencies, and partitions them into python_library / shared library providers.
    Fails if a dependency is not one of these.
    """
    deps = []
    shared_libraries = []
    for raw in raw_deps:
        for dep in raw:
            if PythonLibraryInfo in dep:
                deps.append(dep[PythonLibraryInfo])
            elif SharedLibraryInfo in dep:
                shared_libraries.append(dep[SharedLibraryInfo])
            else:
                # TODO(nmj): This is disabled for the moment because of:
                #                 - the 'genrule-hack' rules that are added as deps
                #                   on third-party whls. Not quite sure what's up
                #                   there, but shouldn't be necessary on v2.
                #                   (e.g. fbsource//third-party/pypi/zstandard:0.12.0-genrule-hack)
                #fail("Dependency {} is neither a python_library, nor a prebuilt_python_library".format(dep.label))
                pass
    return (deps, shared_libraries)

def _exclude_deps_from_omnibus(
        ctx: AnalysisContext,
        srcs: dict[str, "artifact"]) -> bool:
    # User-specified parameter.
    if ctx.attrs.exclude_deps_from_merged_linking:
        return True

    # In some cases, Python library rules package prebuilt native extensions,
    # in which case, we can't support library merging (since we can't re-link
    # these extensions against new libraries).
    for src in srcs:
        # TODO(agallagher): Ideally, we'd prevent sources with these suffixes
        # and requires specifying them another way to make this easier to detect.
        if paths.split_extension(src)[1] in (".so", ".dll", ".pyd"):
            return True

    return False

def _attr_srcs(ctx: AnalysisContext) -> dict[str, "artifact"]:
    python_platform = ctx.attrs._python_toolchain[PythonPlatformInfo]
    cxx_platform = ctx.attrs._cxx_toolchain[CxxPlatformInfo]
    all_srcs = {}
    all_srcs.update(from_named_set(ctx.attrs.srcs))
    for srcs in get_platform_attr(python_platform, cxx_platform, ctx.attrs.platform_srcs):
        all_srcs.update(from_named_set(srcs))
    return all_srcs

def _attr_resources(ctx: AnalysisContext) -> dict[str, [Dependency, "artifact"]]:
    python_platform = ctx.attrs._python_toolchain[PythonPlatformInfo]
    cxx_platform = ctx.attrs._cxx_toolchain[CxxPlatformInfo]
    all_resources = {}
    all_resources.update(from_named_set(ctx.attrs.resources))
    for resources in get_platform_attr(python_platform, cxx_platform, ctx.attrs.platform_resources):
        all_resources.update(from_named_set(resources))
    return all_resources

def py_attr_resources(ctx: AnalysisContext) -> dict[str, ("artifact", list["_arglike"])]:
    """
    Return the resources provided by this rule, as a map of resource name to
    a tuple of the resource artifact and any "other" outputs exposed by it.
    """

    return unpack_artifact_map(_attr_resources(ctx))

def py_resources(
        ctx: AnalysisContext,
        resources: dict[str, ("artifact", list["_arglike"])]) -> (ManifestInfo.type, list["_arglike"]):
    """
    Generate a manifest to wrap this rules resources.
    """
    d = {name: resource for name, (resource, _) in resources.items()}
    hidden = []
    for name, (resource, other) in resources.items():
        for o in other:
            if type(o) == "artifact" and o.basename == shared_libs_symlink_tree_name(resource):
                # Package the binary's shared libs next to the binary
                # (the path is stored in RPATH relative to the binary).
                d[paths.join(paths.dirname(name), o.basename)] = o
            else:
                hidden.append(o)
    manifest = create_manifest_for_source_map(ctx, "resources", d)
    return manifest, dedupe(hidden)

def _src_types(srcs: dict[str, "artifact"], type_stubs: dict[str, "artifact"]) -> dict[str, "artifact"]:
    src_types = {}

    # First, add all `.py` files.
    for name, src in srcs.items():
        _, ext = paths.split_extension(name)
        if ext == ".py" or ext == ".pyi":
            src_types[name] = src

    # Override sources which have a corresponding type stub.
    for name, src in type_stubs.items():
        base, ext = paths.split_extension(name)
        expect(ext == ".pyi", "type stubs must have `.pyi` suffix: {}", name)
        src_types.pop(base + ".py", None)
        src_types[name] = src

    return src_types

def python_library_impl(ctx: AnalysisContext) -> list["provider"]:
    # Versioned params should be intercepted and converted away via the stub.
    expect(not ctx.attrs.versioned_srcs)
    expect(not ctx.attrs.versioned_resources)

    python_platform = ctx.attrs._python_toolchain[PythonPlatformInfo]
    cxx_platform = ctx.attrs._cxx_toolchain[CxxPlatformInfo]

    providers = []
    sub_targets = {}

    srcs = _attr_srcs(ctx)
    qualified_srcs = qualify_srcs(ctx.label, ctx.attrs.base_module, srcs)
    resources = qualify_srcs(ctx.label, ctx.attrs.base_module, py_attr_resources(ctx))
    type_stubs = qualify_srcs(ctx.label, ctx.attrs.base_module, from_named_set(ctx.attrs.type_stubs))
    src_types = _src_types(qualified_srcs, type_stubs)

    src_manifest = create_manifest_for_source_map(ctx, "srcs", qualified_srcs) if qualified_srcs else None
    python_toolchain = ctx.attrs._python_toolchain[PythonToolchainInfo]
    dep_manifest = None
    src_type_manifest = create_manifest_for_source_map(ctx, "type_stubs", src_types) if src_types else None

    # Compile bytecode.
    bytecode = None
    if src_manifest != None:
        bytecode = compile_manifests(ctx, [src_manifest])
        sub_targets["compile"] = [DefaultInfo(default_output = bytecode[PycInvalidationMode("UNCHECKED_HASH")].artifacts[0][0])]
        sub_targets["src-manifest"] = [DefaultInfo(default_output = src_manifest.manifest, other_outputs = [a for a, _ in src_manifest.artifacts])]
        if python_toolchain.emit_dependency_metadata:
            dep_manifest = create_dep_manifest_for_source_map(ctx, python_toolchain, qualified_srcs)
            sub_targets["dep-manifest"] = [DefaultInfo(default_output = dep_manifest.manifest, other_outputs = dep_manifest.artifacts)]

    raw_deps = (
        [ctx.attrs.deps] +
        get_platform_attr(python_platform, cxx_platform, ctx.attrs.platform_deps)
    )
    deps, shared_libraries = gather_dep_libraries(raw_deps)
    library_info = create_python_library_info(
        ctx.actions,
        ctx.label,
        srcs = src_manifest,
        src_types = src_type_manifest,
        resources = py_resources(ctx, resources) if resources else None,
        bytecode = bytecode,
        dep_manifest = dep_manifest,
        deps = deps,
        shared_libraries = shared_libraries,
    )
    providers.append(library_info)

    providers.append(create_python_needed_coverage_info(ctx.label, ctx.attrs.base_module, srcs.keys()))

    # Source DBs.
    sub_targets["source-db"] = [create_source_db(ctx, src_type_manifest, deps)]
    sub_targets["source-db-no-deps"] = [create_source_db_no_deps(ctx, src_types), create_python_source_db_info(library_info.manifests)]
    providers.append(DefaultInfo(sub_targets = sub_targets))

    # Create, augment and provide the linkable graph.
    deps = flatten(raw_deps)
    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            # Add in any potential native root targets from our first-order deps.
            roots = get_roots(ctx.label, deps),
            # Exclude preloaded deps from omnibus linking, to prevent preloading
            # the monolithic omnibus library.
            excluded = get_excluded(
                deps = (
                    (deps if _exclude_deps_from_omnibus(ctx, qualified_srcs) else []) +
                    # We also need to exclude deps that can't be re-linked, via
                    # the `LinkableRootInfo` provider (i.e. `prebuilt_cxx_library_group`).
                    [d for d in deps if LinkableRootInfo not in d]
                ),
            ),
        ),
        deps = deps,
    )
    providers.append(linkable_graph)

    # Link info for native python
    providers.append(
        merge_cxx_extension_info(
            ctx.actions,
            deps,
            shared_deps = deps,
        ),
    )

    # C++ resources.
    providers.append(ResourceInfo(resources = gather_resources(
        label = ctx.label,
        deps = flatten(raw_deps),
    )))

    return providers
