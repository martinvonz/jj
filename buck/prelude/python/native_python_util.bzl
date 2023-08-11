# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//linking:link_info.bzl",
    "LinkInfo",
    "LinkInfos",
    "LinkStyle",
    "MergedLinkInfo",
    "ObjectsLinkable",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "DlopenableLibraryInfo",
    "LinkableRootInfo",
)
load(
    "@prelude//linking:linkables.bzl",
    "LinkableProviders",  # @unused Used as type
    "linkable",
)
load("@prelude//linking:strip.bzl", "strip_debug_info")

LinkableProvidersTSet = transitive_set()

# Info required to link cxx_python_extensions into native python binaries
CxxExtensionLinkInfo = provider(
    fields = [
        "linkable_providers",  # LinkableProvidersTSet.type
        "artifacts",  # {str: "_a"}
        "python_module_names",  # {str: str}
        "dlopen_deps",  # {"label": LinkableProviders.type}
        # Native python extensions that can't be linked into the main executable.
        "unembeddable_extensions",  # {str: LinkableProviders.type}
        # Native libraries that are only available as shared libs.
        "shared_only_libs",  # {Label: LinkableProviders.type}
    ],
)

def merge_cxx_extension_info(
        actions: "actions",
        deps: list[Dependency],
        linkable_providers: [LinkableProviders.type, None] = None,
        artifacts: dict[str, "_a"] = {},
        python_module_names: dict[str, str] = {},
        unembeddable_extensions: dict[str, LinkableProviders.type] = {},
        shared_deps: list[Dependency] = []) -> CxxExtensionLinkInfo.type:
    linkable_provider_children = []
    artifacts = dict(artifacts)
    python_module_names = dict(python_module_names)
    unembeddable_extensions = dict(unembeddable_extensions)

    dlopen_deps = {}
    shared_only_libs = {}
    for dep in shared_deps:
        # Libs that should be linked into their own, standalone link groups
        if DlopenableLibraryInfo in dep:
            dlopen_deps[dep.label] = linkable(dep)
            continue

        # Try to detect prebuilt, shared-only libraries.
        # TODO(agallagher): We need a more general way to support this, which
        # should *just* use `preferred_linkage` (and so it supports non-prebuilt
        # libs too), but this will require hoisting the rules first-order deps
        # up the tree as `dlopen_deps` so that we link them properly.
        if MergedLinkInfo in dep and LinkableRootInfo not in dep:
            shared_only_libs[dep.label] = linkable(dep)

    for dep in deps:
        cxx_extension_info = dep.get(CxxExtensionLinkInfo)
        if cxx_extension_info == None:
            continue
        linkable_provider_children.append(cxx_extension_info.linkable_providers)
        artifacts.update(cxx_extension_info.artifacts)
        python_module_names.update(cxx_extension_info.python_module_names)
        unembeddable_extensions.update(cxx_extension_info.unembeddable_extensions)
        dlopen_deps.update(cxx_extension_info.dlopen_deps)
        shared_only_libs.update(cxx_extension_info.shared_only_libs)
    linkable_providers_kwargs = {}
    if linkable_providers != None:
        linkable_providers_kwargs["value"] = linkable_providers
    linkable_providers_kwargs["children"] = linkable_provider_children
    return CxxExtensionLinkInfo(
        linkable_providers = actions.tset(LinkableProvidersTSet, **linkable_providers_kwargs),
        artifacts = artifacts,
        python_module_names = python_module_names,
        unembeddable_extensions = unembeddable_extensions,
        dlopen_deps = dlopen_deps,
        shared_only_libs = shared_only_libs,
    )

def rewrite_static_symbols(
        ctx: AnalysisContext,
        suffix: str,
        pic_objects: list["artifact"],
        non_pic_objects: list["artifact"],
        libraries: dict[LinkStyle.type, LinkInfos.type],
        cxx_toolchain: "CxxToolchainInfo",
        suffix_all: bool = False) -> dict[LinkStyle.type, LinkInfos.type]:
    symbols_file = _write_syms_file(
        ctx = ctx,
        name = ctx.label.name + "_rename_syms",
        objects = non_pic_objects,
        suffix = suffix,
        cxx_toolchain = cxx_toolchain,
        suffix_all = suffix_all,
    )
    static_objects, stripped_static_objects = suffix_symbols(ctx, suffix, non_pic_objects, symbols_file, cxx_toolchain)

    symbols_file_pic = _write_syms_file(
        ctx = ctx,
        name = ctx.label.name + "_rename_syms_pic",
        objects = pic_objects,
        suffix = suffix,
        cxx_toolchain = cxx_toolchain,
        suffix_all = suffix_all,
    )
    static_pic_objects, stripped_static_pic_objects = suffix_symbols(ctx, suffix, pic_objects, symbols_file_pic, cxx_toolchain)

    static_info = libraries[LinkStyle("static")].default
    updated_static_info = LinkInfo(
        name = static_info.name,
        pre_flags = static_info.pre_flags,
        post_flags = static_info.post_flags,
        linkables = [static_objects],
        external_debug_info = static_info.external_debug_info,
    )
    updated_stripped_static_info = None
    static_info = libraries[LinkStyle("static")].stripped
    if static_info != None:
        updated_stripped_static_info = LinkInfo(
            name = static_info.name,
            pre_flags = static_info.pre_flags,
            post_flags = static_info.post_flags,
            linkables = [stripped_static_objects],
        )

    static_pic_info = libraries[LinkStyle("static_pic")].default
    updated_static_pic_info = LinkInfo(
        name = static_pic_info.name,
        pre_flags = static_pic_info.pre_flags,
        post_flags = static_pic_info.post_flags,
        linkables = [static_pic_objects],
        external_debug_info = static_pic_info.external_debug_info,
    )
    updated_stripped_static_pic_info = None
    static_pic_info = libraries[LinkStyle("static_pic")].stripped
    if static_pic_info != None:
        updated_stripped_static_pic_info = LinkInfo(
            name = static_pic_info.name,
            pre_flags = static_pic_info.pre_flags,
            post_flags = static_pic_info.post_flags,
            linkables = [stripped_static_pic_objects],
        )
    updated_libraries = {
        LinkStyle("static"): LinkInfos(default = updated_static_info, stripped = updated_stripped_static_info),
        LinkStyle("static_pic"): LinkInfos(default = updated_static_pic_info, stripped = updated_stripped_static_pic_info),
    }
    return updated_libraries

def _write_syms_file(
        ctx: AnalysisContext,
        name: str,
        objects: list["artifact"],
        suffix: str,
        cxx_toolchain: "CxxToolchainInfo",
        suffix_all: bool = False) -> "artifact":
    """
    Take a list of objects and append a suffix to all  defined symbols.
    """
    nm = cxx_toolchain.binary_utilities_info.nm
    symbols_file = ctx.actions.declare_output(name)

    objects_argsfile = ctx.actions.write(name + ".objects.argsfile", objects)
    objects_args = cmd_args(objects_argsfile).hidden(objects)

    script_env = {
        "NM": nm,
        "OBJECTS": objects_args,
        "SYMSFILE": symbols_file.as_output(),
    }

    # Compile symbols defined by all object files into a de-duplicated list of symbols to rename
    # --no-sort tells nm not to sort the output because we are sorting it to dedupe anyway
    # --defined-only prints only the symbols defined by this extension this way we won't rename symbols defined externally e.g. PyList_GetItem, etc...
    # -j print only the symbol name
    # sed removes filenames generated from objcopy (lines ending with ":") and empty lines
    # sort -u sorts the combined list of symbols and removes any duplicate entries
    # using awk we format the symbol names 'PyInit_hello' followed by the symbol name with the suffix appended to create the input file for objcopy
    # objcopy uses a list of symbol name followed by updated name e.g. 'PyInit_hello PyInit_hello_package_module'
    script = (
        "set -euo pipefail; " +  # fail if any command in the script fails
        '"$NM" --no-sort --defined-only -j @"$OBJECTS" | sed "/:$/d;/^$/d" | sort -u'
    )

    if not suffix_all:
        script += ' | grep "^PyInit_"'

    # Don't suffix asan symbols, as they shouldn't conflict, and suffixing
    # prevents deduplicating all the module constructors, which can be really
    # expensive to run.
    script += ' | grep -v "^\\(__\\)\\?\\(a\\|t\\)san"'

    script += (
        ' | awk \'{{print $1" "$1"_{suffix}"}}\' > '.format(suffix = suffix) +
        '"$SYMSFILE";'
    )

    ctx.actions.run(
        [
            "/usr/bin/env",
            "bash",
            "-c",
            script,
        ],
        env = script_env,
        category = "write_syms_file",
        identifier = "{}_write_syms_file".format(symbols_file.basename),
    )

    return symbols_file

def suffix_symbols(
        ctx: AnalysisContext,
        suffix: str,
        objects: list["artifact"],
        symbols_file: "artifact",
        cxx_toolchain: "CxxToolchainInfo") -> (ObjectsLinkable.type, ObjectsLinkable.type):
    """
    Take a list of objects and append a suffix to all  defined symbols.
    """
    objcopy = cxx_toolchain.binary_utilities_info.objcopy

    artifacts = []
    stripped_artifacts = []
    for obj in objects:
        base, name = paths.split_extension(obj.short_path)
        updated_name = "_".join([base, suffix, name])
        artifact = ctx.actions.declare_output(updated_name)

        script_env = {
            "OBJCOPY": objcopy,
            "ORIGINAL": obj,
            "OUT": artifact.as_output(),
            "SYMSFILE": symbols_file,
        }

        script = (
            "set -euo pipefail; " +  # fail if any command in the script fails
            '"$OBJCOPY" --redefine-syms="$SYMSFILE" "$ORIGINAL" "$OUT"'  # using objcopy we pass in the symbols file to re-write the original symbol name to the now suffixed version
        )

        # Usage: objcopy [option(s)] in-file [out-file]
        ctx.actions.run(
            [
                "/usr/bin/env",
                "bash",
                "-c",
                script,
            ],
            env = script_env,
            category = "suffix_symbols",
            identifier = updated_name,
        )

        artifacts.append(artifact)
        updated_base, _ = paths.split_extension(artifact.short_path)
        stripped_artifacts.append(strip_debug_info(ctx, updated_base + ".stripped.o", artifact))

    default = ObjectsLinkable(
        objects = artifacts,
        linker_type = cxx_toolchain.linker_info.type,
    )
    stripped = ObjectsLinkable(
        objects = stripped_artifacts,
        linker_type = cxx_toolchain.linker_info.type,
    )
    return default, stripped
