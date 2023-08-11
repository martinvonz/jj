# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Implementation of the Haskell build rules.

load("@prelude//:paths.bzl", "paths")
load("@prelude//cxx:archive.bzl", "make_archive")
load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "CxxPlatformInfo",
    "CxxToolchainInfo",
    "PicBehavior",
)
load(
    "@prelude//cxx:preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorArgs",
    "cxx_inherited_preprocessor_infos",
    "cxx_merge_cpreprocessors",
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
    "SharedLibLinkable",
    "create_merged_link_info",
    "get_actual_link_style",
    "get_link_args",
    "get_link_styles_for_linkage",
    "merge_link_infos",
    "unpack_link_args",
)
load(
    "@prelude//linking:linkable_graph.bzl",
    "create_linkable_graph",
    "create_linkable_graph_node",
    "create_linkable_node",
)
load(
    "@prelude//linking:shared_libraries.bzl",
    "SharedLibraryInfo",
    "create_shared_libraries",
    "merge_shared_libraries",
    "traverse_shared_library_info",
)
load(
    "@prelude//python:python.bzl",
    "PythonLibraryInfo",
)
load("@prelude//utils:platform_flavors_util.bzl", "by_platform")
load("@prelude//utils:utils.bzl", "flatten")

_HASKELL_EXTENSIONS = [
    ".hs",
    ".lhs",
    ".hsc",
    ".chs",
    ".x",
    ".y",
]

HaskellPlatformInfo = provider(fields = [
    "name",
])

HaskellToolchainInfo = provider(fields = [
    "compiler",
    "compiler_flags",
    "linker",
    "linker_flags",
    "haddock",
    "compiler_major_version",
    "package_name_prefix",
    "packager",
    "use_argsfile",
    "support_expose_package",
    "archive_contents",
    "ghci_script_template",
    "ghci_iserv_template",
    "ide_script_template",
    "ghci_binutils_path",
    "ghci_lib_path",
    "ghci_ghc_path",
    "ghci_iserv_path",
    "ghci_iserv_prof_path",
    "ghci_cxx_path",
    "ghci_cc_path",
    "ghci_cpp_path",
    "ghci_packager",
    "cache_links",
    "script_template_processor",
])

# A list of `HaskellLibraryInfo`s.
HaskellLinkInfo = provider(
    # Contains a list of HaskellLibraryInfo records.
    fields = [
        "info",  # { LinkStyle.type : [HaskellLibraryInfo] } # TODO use a tset
    ],
)

HaskellIndexingTSet = transitive_set()

# A list of hie dirs
HaskellIndexInfo = provider(
    fields = [
        "info",  # { LinkStyle.type : HaskellIndexingTset }
    ],
)

# If the target is a haskell library, the HaskellLibraryProvider
# contains its HaskellLibraryInfo. (in contrast to a HaskellLinkInfo,
# which contains the HaskellLibraryInfo for all the transitive
# dependencies). Direct dependencies are treated differently from
# indirect dependencies for the purposes of module visibility.
HaskellLibraryProvider = provider(
    fields = [
        "lib",  # { LinkStyle.type : HaskellLibraryInfo }
    ],
)

# A record of a Haskell library.
HaskellLibraryInfo = record(
    # The library target name: e.g. "rts"
    name = str,
    # package config database: e.g. platform009/build/ghc/lib/package.conf.d
    db = "artifact",
    # e.g. "base-4.13.0.0"
    id = str,
    import_dirs = ["artifact"],
    stub_dirs = ["artifact"],

    # This field is only used as hidden inputs to compilation, to
    # support Template Haskell which may need access to the libraries
    # at compile time.  The real library flags are propagated up the
    # dependency graph via MergedLinkInfo.
    libs = field(["artifact"], []),
    # Package version, used to specify the full package when exposing it,
    # e.g. filepath-1.4.2.1, deepseq-1.4.4.0.
    # Internal packages default to 1.0.0, e.g. `fbcode-dsi-logger-hs-types-1.0.0`.
    version = str,
    is_prebuilt = bool,
)

# --

def _by_platform(ctx: AnalysisContext, xs: list[(str, list["_a"])]) -> list["_a"]:
    platform = ctx.attrs._cxx_toolchain[CxxPlatformInfo].name
    return flatten(by_platform([platform], xs))

def attr_deps(ctx: AnalysisContext) -> list[Dependency]:
    return ctx.attrs.deps + _by_platform(ctx, ctx.attrs.platform_deps)

# Disable until we have a need to call this.
# def _attr_deps_merged_link_infos(ctx: AnalysisContext) -> ["MergedLinkInfo"]:
#     return filter(None, [d[MergedLinkInfo] for d in attr_deps(ctx)])

def _attr_deps_haskell_link_infos(ctx: AnalysisContext) -> list["HaskellLinkInfo"]:
    return filter(
        None,
        [
            d.get(HaskellLinkInfo)
            for d in attr_deps(ctx) + ctx.attrs.template_deps
        ],
    )

def _attr_deps_haskell_lib_infos(
        ctx: AnalysisContext,
        link_style: LinkStyle.type) -> list["HaskellLibraryInfo"]:
    return [
        x.lib[link_style]
        for x in filter(None, [
            d.get(HaskellLibraryProvider)
            for d in attr_deps(ctx) + ctx.attrs.template_deps
        ])
    ]

def _cxx_toolchain_link_style(ctx: AnalysisContext) -> LinkStyle.type:
    return ctx.attrs._cxx_toolchain[CxxToolchainInfo].linker_info.link_style

def _attr_link_style(ctx: AnalysisContext) -> LinkStyle.type:
    if ctx.attrs.link_style != None:
        return LinkStyle(ctx.attrs.link_style)
    else:
        return _cxx_toolchain_link_style(ctx)

def _attr_preferred_linkage(ctx: AnalysisContext) -> Linkage.type:
    preferred_linkage = ctx.attrs.preferred_linkage

    # force_static is deprecated, but it has precedence over preferred_linkage
    if getattr(ctx.attrs, "force_static", False):
        preferred_linkage = "static"

    return Linkage(preferred_linkage)

# --

def _is_haskell_src(x: str) -> bool:
    _, ext = paths.split_extension(x)
    return ext in _HASKELL_EXTENSIONS

def _src_to_module_name(x: str) -> str:
    base, _ext = paths.split_extension(x)
    return base.replace("/", ".")

def haskell_prebuilt_library_impl(ctx: AnalysisContext) -> list["provider"]:
    native_infos = []
    haskell_infos = []
    shared_library_infos = []
    for dep in ctx.attrs.deps:
        used = False
        if HaskellLinkInfo in dep:
            used = True
            haskell_infos.append(dep[HaskellLinkInfo])
        if MergedLinkInfo in dep:
            used = True
            native_infos.append(dep[MergedLinkInfo])
        if SharedLibraryInfo in dep:
            used = True
            shared_library_infos.append(dep[SharedLibraryInfo])
        if PythonLibraryInfo in dep:
            used = True
        if not used:
            fail("Unexpected link info encountered")

    hlibinfos = {}
    hlinkinfos = {}
    link_infos = {}
    for link_style in LinkStyle:
        libs = []
        if ctx.attrs.enable_profiling:
            if link_style == LinkStyle("static"):
                libs = ctx.attrs.profiled_static_libs
            if link_style == LinkStyle("static_pic"):
                libs = ctx.attrs.pic_profiled_static_libs
        elif link_style == LinkStyle("shared"):
            libs = ctx.attrs.shared_libs.values()
        elif link_style == LinkStyle("static"):
            libs = ctx.attrs.static_libs
        elif link_style == LinkStyle("static_pic"):
            libs = ctx.attrs.pic_static_libs
        hlibinfo = HaskellLibraryInfo(
            name = ctx.attrs.name,
            db = ctx.attrs.db,
            import_dirs = ctx.attrs.import_dirs,
            stub_dirs = [],
            id = ctx.attrs.id,
            libs = libs,
            version = ctx.attrs.version,
            is_prebuilt = True,
        )

        def archive_linkable(lib):
            return ArchiveLinkable(
                archive = Archive(artifact = lib),
                linker_type = "gnu",
            )

        def shared_linkable(lib):
            return SharedLibLinkable(
                lib = lib,
            )

        linkables = [
            (shared_linkable if link_style == LinkStyle("shared") else archive_linkable)(lib)
            for lib in libs
        ]

        hlibinfos[link_style] = hlibinfo
        hlinkinfos[link_style] = [hlibinfo]
        link_infos[link_style] = LinkInfos(
            default = LinkInfo(
                pre_flags = ctx.attrs.exported_linker_flags,
                linkables = linkables,
            ),
        )

    haskell_link_infos = HaskellLinkInfo(info = hlinkinfos)
    haskell_lib_provider = HaskellLibraryProvider(lib = hlibinfos)

    merged_link_info = create_merged_link_info(
        ctx,
        # We don't have access to a CxxToolchain here (yet).
        # Give that it's already built, this doesn't mean much, use a sane default.
        pic_behavior = PicBehavior("supported"),
        link_infos = link_infos,
        exported_deps = native_infos,
    )

    solibs = {}
    for soname, lib in ctx.attrs.shared_libs.items():
        solibs[soname] = LinkedObject(output = lib)

    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            linkable_node = create_linkable_node(
                ctx = ctx,
                exported_deps = ctx.attrs.deps,
                link_infos = link_infos,
                shared_libs = solibs,
            ),
        ),
        deps = ctx.attrs.deps,
    )

    inherited_pp_info = cxx_inherited_preprocessor_infos(ctx.attrs.deps)
    own_pp_info = CPreprocessor(
        relative_args = CPreprocessorArgs(args = flatten([["-isystem", d] for d in ctx.attrs.cxx_header_dirs])),
    )

    return [
        DefaultInfo(),
        haskell_lib_provider,
        cxx_merge_cpreprocessors(ctx, [own_pp_info], inherited_pp_info),
        merge_shared_libraries(
            ctx.actions,
            create_shared_libraries(ctx, solibs),
            shared_library_infos,
        ),
        merge_link_group_lib_info(deps = ctx.attrs.deps),
        merge_haskell_link_infos(haskell_infos + [haskell_link_infos]),
        merged_link_info,
        linkable_graph,
    ]

def merge_haskell_link_infos(deps: list[HaskellLinkInfo.type]) -> HaskellLinkInfo.type:
    merged = {}
    for link_style in LinkStyle:
        children = []
        for dep in deps:
            if link_style in dep.info:
                children.extend(dep.info[link_style])
        merged[link_style] = dedupe(children)

    return HaskellLinkInfo(info = merged)

PackagesInfo = record(
    exposed_package_args = cmd_args,
    packagedb_args = cmd_args,
    transitive_deps = field([HaskellLibraryInfo.type]),
)

def get_packages_info(
        ctx: AnalysisContext,
        link_style: LinkStyle.type,
        specify_pkg_version: bool) -> PackagesInfo.type:
    # Collect library dependencies. Note that these don't need to be in a
    # particular order and we really want to remove duplicates (there
    # are a *lot* of duplicates).
    libs = {}
    transitive_deps = []
    direct_deps_link_info = _attr_deps_haskell_link_infos(ctx)
    for lib in merge_haskell_link_infos(direct_deps_link_info).info[link_style]:
        libs[lib.db] = lib  # lib.db is a good enough unique key
        transitive_deps.append(lib)

    # base is special and gets exposed by default
    exposed_package_args = cmd_args(["-expose-package", "base"])

    packagedb_args = cmd_args()

    for lib in libs.values():
        exposed_package_args.hidden(lib.import_dirs)
        exposed_package_args.hidden(lib.stub_dirs)

        # libs of dependencies might be needed at compile time if
        # we're using Template Haskell:
        exposed_package_args.hidden(lib.libs)

        packagedb_args.hidden(lib.import_dirs)
        packagedb_args.hidden(lib.stub_dirs)
        packagedb_args.hidden(lib.libs)

    for lib in libs.values():
        # These we need to add for all the packages/dependencies, i.e.
        # direct and transitive (e.g. `fbcode-common-hs-util-hs-array`)
        packagedb_args.add("-package-db", lib.db)

    haskell_direct_deps_lib_infos = _attr_deps_haskell_lib_infos(ctx, link_style)

    # Expose only the packages we depend on directly
    for lib in haskell_direct_deps_lib_infos:
        pkg_name = lib.name
        if (specify_pkg_version):
            pkg_name += "-{}".format(lib.version)

        exposed_package_args.add("-expose-package", pkg_name)

    return PackagesInfo(
        exposed_package_args = exposed_package_args,
        packagedb_args = packagedb_args,
        transitive_deps = libs.values(),
    )

# The type of the return value of the `_compile()` function.
CompileResultInfo = record(
    objects = field("artifact"),
    hi = field("artifact"),
    stubs = field("artifact"),
    producing_indices = field("bool"),
)

def _link_style_extensions(link_style: LinkStyle.type) -> (str, str):
    if link_style == LinkStyle("shared"):
        return ("dyn_o", "dyn_hi")
    elif link_style == LinkStyle("static_pic"):
        return ("o", "hi")  # is this right?
    elif link_style == LinkStyle("static"):
        return ("o", "hi")
    fail("unknown LinkStyle")

def _output_extensions(
        link_style: LinkStyle.type,
        profiled: bool) -> (str, str):
    osuf, hisuf = _link_style_extensions(link_style)
    if profiled:
        return ("p_" + osuf, "p_" + hisuf)
    else:
        return (osuf, hisuf)

def _srcs_to_objfiles(
        ctx: AnalysisContext,
        odir: "artifact",
        osuf: str) -> cmd_args:
    objfiles = cmd_args()
    for src in ctx.attrs.srcs:
        # Don't link boot sources, as they're only meant to be used for compiling.
        if _is_haskell_src(src):
            objfiles.add(cmd_args([odir, "/", paths.replace_extension(src, "." + osuf)], delimiter = ""))
    return objfiles

# Compile all the context's sources.
def _compile(
        ctx: AnalysisContext,
        link_style: LinkStyle.type,
        extra_args = []) -> CompileResultInfo.type:
    haskell_toolchain = ctx.attrs._haskell_toolchain[HaskellToolchainInfo]
    compile_cmd = cmd_args(haskell_toolchain.compiler)
    compile_cmd.add(haskell_toolchain.compiler_flags)

    # Some rules pass in RTS (e.g. `+RTS ... -RTS`) options for GHC, which can't
    # be parsed when inside an argsfile.
    compile_cmd.add(ctx.attrs.compiler_flags)

    compile_args = cmd_args()
    compile_args.add("-no-link", "-i")

    if ctx.attrs.enable_profiling:
        compile_args.add("-prof")

    if link_style == LinkStyle("shared"):
        compile_args.add("-dynamic", "-fPIC")
    elif link_style == LinkStyle("static_pic"):
        compile_args.add("-fPIC", "-fexternal-dynamic-refs")

    osuf, hisuf = _output_extensions(link_style, ctx.attrs.enable_profiling)
    compile_args.add("-osuf", osuf, "-hisuf", hisuf)

    if getattr(ctx.attrs, "main", None) != None:
        compile_args.add(["-main-is", ctx.attrs.main])

    objects = ctx.actions.declare_output("objects-" + link_style.value, dir = True)
    hi = ctx.actions.declare_output("hi-" + link_style.value, dir = True)
    stubs = ctx.actions.declare_output("stubs-" + link_style.value, dir = True)

    compile_args.add(
        "-odir",
        objects.as_output(),
        "-hidir",
        hi.as_output(),
        "-hiedir",
        hi.as_output(),
        "-stubdir",
        stubs.as_output(),
    )

    # Add -package-db and -expose-package flags for each Haskell
    # library dependency.
    packages_info = get_packages_info(
        ctx,
        link_style,
        specify_pkg_version = False,
    )

    compile_args.add(packages_info.exposed_package_args)
    compile_args.add(packages_info.packagedb_args)

    # Add args from preprocess-able inputs.
    inherited_pre = cxx_inherited_preprocessor_infos(ctx.attrs.deps)
    pre = cxx_merge_cpreprocessors(ctx, [], inherited_pre)
    pre_args = pre.set.project_as_args("args")
    compile_args.add(cmd_args(pre_args, format = "-optP={}"))

    compile_args.add(extra_args)

    for (path, src) in ctx.attrs.srcs.items():
        # hs-boot files aren't expected to be an argument to compiler but does need
        # to be included in the directory of the associated src file
        if _is_haskell_src(path):
            compile_args.add(src)
        else:
            compile_args.hidden(src)

    argsfile = ctx.actions.declare_output("haskell_compile_" + link_style.value + ".argsfile")
    ctx.actions.write(argsfile.as_output(), compile_args, allow_args = True)
    hidden_args = [compile_args]
    compile_cmd.add(cmd_args(argsfile, format = "@{}").hidden(hidden_args))

    ctx.actions.run(
        compile_cmd,
        category = "haskell_compile_" + link_style.value,
        no_outputs_cleanup = True,
    )

    producing_indices = "-fwrite-ide-info" in ctx.attrs.compiler_flags

    return CompileResultInfo(
        objects = objects,
        hi = hi,
        stubs = stubs,
        producing_indices = producing_indices,
    )

_REGISTER_PACKAGE = """\
set -euo pipefail
GHC_PKG=$1
DB=$2
PKGCONF=$3
"$GHC_PKG" init "$DB"
"$GHC_PKG" register --package-conf "$DB" --no-expand-pkgroot "$PKGCONF"
"""

# Create a package
#
# The way we use packages is a bit strange. We're not using them
# at link time at all: all the linking info is in the
# HaskellLibraryInfo and we construct linker command lines
# manually. Packages are used for:
#
#  - finding .hi files at compile time
#
#  - symbol namespacing (so that modules with the same name in
#    different libraries don't clash).
#
#  - controlling module visibility: only dependencies that are
#    directly declared as dependencies may be used
#
#  - Template Haskell: the compiler needs to load libraries itself
#    at compile time, so it uses the package specs to find out
#    which libraries and where.
def _make_package(
        ctx: AnalysisContext,
        link_style: LinkStyle.type,
        pkgname: str,
        libname: str,
        hlis: list[HaskellLibraryInfo.type],
        hi: "artifact",
        lib: "artifact") -> "artifact":
    # Don't expose boot sources, as they're only meant to be used for compiling.
    modules = [_src_to_module_name(x) for x in ctx.attrs.srcs if _is_haskell_src(x)]

    uniq_hlis = {}
    for x in hlis:
        uniq_hlis[x.id] = x

    conf = [
        "name: " + pkgname,
        "version: 1.0.0",
        "id: " + pkgname,
        "key: " + pkgname,
        "exposed: False",
        "exposed-modules: " + ", ".join(modules),
        "import-dirs: \"${pkgroot}/hi-" + link_style.value + "\"",
        "library-dirs: \"${pkgroot}/lib-" + link_style.value + "\"",
        "extra-libraries: " + libname,
        "depends: " + ", ".join(uniq_hlis),
    ]
    pkg_conf = ctx.actions.write("pkg-" + link_style.value + ".conf", conf)

    db = ctx.actions.declare_output("db-" + link_style.value)

    db_deps = {}
    for x in uniq_hlis.values():
        db_deps[repr(x.db)] = x.db

    # So that ghc-pkg can find the DBs for the dependencies. We might
    # be able to use flags for this instead, but this works.
    ghc_package_path = cmd_args(
        db_deps.values(),
        delimiter = ":",
    )

    haskell_toolchain = ctx.attrs._haskell_toolchain[HaskellToolchainInfo]
    ctx.actions.run(
        cmd_args([
            "sh",
            "-c",
            _REGISTER_PACKAGE,
            "",
            haskell_toolchain.packager[RunInfo],
            db.as_output(),
            pkg_conf,
        ]).hidden(hi).hidden(lib),  # needs hi, because ghc-pkg checks that the .hi files exist
        category = "haskell_package_" + link_style.value,
        env = {"GHC_PACKAGE_PATH": ghc_package_path},
    )

    return db

def haskell_library_impl(ctx: AnalysisContext) -> list["provider"]:
    linker_info = ctx.attrs._cxx_toolchain[CxxToolchainInfo].linker_info
    libname = repr(ctx.label.path).replace("//", "_").replace("/", "_") + "_" + ctx.label.name
    pkgname = libname.replace("_", "-")

    # Link the objects into a library
    haskell_toolchain = ctx.attrs._haskell_toolchain[HaskellToolchainInfo]

    preferred_linkage = _attr_preferred_linkage(ctx)
    if ctx.attrs.enable_profiling and preferred_linkage == Linkage("any"):
        preferred_linkage = Linkage("static")

    hlis = []
    nlis = []
    shared_library_infos = []
    for lib in attr_deps(ctx):
        li = lib.get(HaskellLinkInfo)
        if li != None:
            hlis.append(li)
        li = lib.get(MergedLinkInfo)
        if li != None:
            nlis.append(li)
        li = lib.get(SharedLibraryInfo)
        if li != None:
            shared_library_infos.append(li)

    solibs = {}
    link_infos = {}
    hlib_infos = {}
    hlink_infos = {}
    indexing_tsets = {}
    sub_targets = {}

    for link_style in get_link_styles_for_linkage(preferred_linkage):
        osuf, _hisuf = _output_extensions(link_style, ctx.attrs.enable_profiling)

        # Compile the sources
        compiled = _compile(ctx, link_style, ["-this-unit-id", pkgname])

        if link_style == LinkStyle("static_pic"):
            libstem = libname + "_pic"
        else:
            libstem = libname

        if link_style == LinkStyle("shared"):
            libfile = "lib" + libstem + ".so"
        else:
            libfile = "lib" + libstem + ".a"
        lib_short_path = paths.join("lib-{}".format(link_style.value), libfile)

        uniq_infos = dedupe(flatten([x.info[link_style] for x in hlis]))

        objfiles = _srcs_to_objfiles(ctx, compiled.objects, osuf)

        if link_style == LinkStyle("shared"):
            lib = ctx.actions.declare_output(lib_short_path)
            link = cmd_args(haskell_toolchain.linker)
            link.add(haskell_toolchain.linker_flags)
            link.add(ctx.attrs.linker_flags)
            link.add("-o", lib.as_output())
            link.add(
                "-shared",
                "-dynamic",
                "-optl",
                "-Wl,-soname",
                "-optl",
                "-Wl," + libfile,
            )

            link.add(objfiles)
            link.hidden(compiled.stubs)

            infos = get_link_args(merge_link_infos(ctx, nlis), link_style)
            link.add(cmd_args(unpack_link_args(infos), prepend = "-optl"))
            ctx.actions.run(link, category = "haskell_link")

            solibs[libfile] = LinkedObject(output = lib)
            libs = [lib]
            link_infos[link_style] = LinkInfos(
                default = LinkInfo(linkables = [SharedLibLinkable(lib = lib)]),
            )

        else:  # static flavours
            # TODO: avoid making an archive for a single object, like cxx does
            # (but would that work with Template Haskell?)
            archive = make_archive(ctx, lib_short_path, [compiled.objects], objfiles)
            lib = archive.artifact
            libs = [lib] + archive.external_objects
            link_infos[link_style] = LinkInfos(
                default = LinkInfo(
                    linkables = [
                        ArchiveLinkable(
                            archive = archive,
                            linker_type = linker_info.type,
                        ),
                    ],
                ),
            )

        db = _make_package(ctx, link_style, pkgname, libstem, uniq_infos, compiled.hi, lib)

        hlib = HaskellLibraryInfo(
            name = pkgname,
            db = db,
            id = pkgname,
            import_dirs = [compiled.hi],
            stub_dirs = [compiled.stubs],
            libs = libs,
            version = "1.0.0",
            is_prebuilt = False,
        )
        hlib_infos[link_style] = hlib
        hlink_infos[link_style] = [hlib]

        if compiled.producing_indices:
            tset = derive_indexing_tset(ctx.actions, link_style, compiled.hi, attr_deps(ctx))
            indexing_tsets[link_style] = tset

        sub_targets[link_style.value.replace("_", "-")] = [DefaultInfo(
            default_outputs = libs,
        )]

    pic_behavior = ctx.attrs._cxx_toolchain[CxxToolchainInfo].pic_behavior
    merged_link_info = create_merged_link_info(
        ctx,
        pic_behavior = pic_behavior,
        link_infos = link_infos,
        preferred_linkage = preferred_linkage,
        exported_deps = nlis,
    )

    linkable_graph = create_linkable_graph(
        ctx,
        node = create_linkable_graph_node(
            ctx,
            linkable_node = create_linkable_node(
                ctx = ctx,
                preferred_linkage = preferred_linkage,
                exported_deps = ctx.attrs.deps,
                link_infos = link_infos,
                shared_libs = solibs,
            ),
        ),
        deps = ctx.attrs.deps,
    )

    link_style = _cxx_toolchain_link_style(ctx)
    actual_link_style = get_actual_link_style(link_style, preferred_linkage, pic_behavior)
    default_output = hlib_infos[actual_link_style].libs

    inherited_pp_info = cxx_inherited_preprocessor_infos(attr_deps(ctx))

    # We would like to expose the generated _stub.h headers to C++
    # compilations, but it's hard to do that without overbuilding. Which
    # link_style should we pick below? If we pick a different link_style from
    # the one being used by the root rule, we'll end up building all the
    # Haskell libraries multiple times.
    #
    #    pp = [CPreprocessor(
    #        args =
    #            flatten([["-isystem", dir] for dir in hlib_infos[actual_link_style].stub_dirs]),
    #    )]
    pp = []

    providers = [
        DefaultInfo(
            default_outputs = default_output,
            sub_targets = sub_targets,
        ),
        HaskellLibraryProvider(lib = hlib_infos),
        merge_haskell_link_infos(hlis + [HaskellLinkInfo(info = hlink_infos)]),
        merged_link_info,
        linkable_graph,
        cxx_merge_cpreprocessors(ctx, pp, inherited_pp_info),
        merge_shared_libraries(
            ctx.actions,
            create_shared_libraries(ctx, solibs),
            shared_library_infos,
        ),
    ]

    if indexing_tsets:
        providers.append(HaskellIndexInfo(info = indexing_tsets))

    templ_vars = {}

    # Add in ldflag macros.
    for link_style in (LinkStyle("static"), LinkStyle("static_pic")):
        name = "ldflags-" + link_style.value.replace("_", "-")
        args = cmd_args()
        linker_info = ctx.attrs._cxx_toolchain[CxxToolchainInfo].linker_info
        args.add(linker_info.linker_flags)
        args.add(unpack_link_args(
            get_link_args(
                merged_link_info,
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

    providers.append(merge_link_group_lib_info(deps = attr_deps(ctx)))

    return providers

def derive_indexing_tset(
        actions: "actions",
        link_style: LinkStyle.type,
        value: ["artifact", None],
        children: list[Dependency]) -> "HaskellIndexingTSet":
    index_children = []
    for dep in children:
        li = dep.get(HaskellIndexInfo)
        if li:
            if (link_style in li.info):
                index_children.append(li.info[link_style])

    return actions.tset(
        HaskellIndexingTSet,
        value = value,
        children = index_children,
    )

def haskell_binary_impl(ctx: AnalysisContext) -> list["provider"]:
    # Decide what kind of linking we're doing
    link_style = _attr_link_style(ctx)

    # Profiling doesn't support shared libraries
    if ctx.attrs.enable_profiling and link_style == LinkStyle("shared"):
        link_style = LinkStyle("static")

    compiled = _compile(ctx, link_style)

    haskell_toolchain = ctx.attrs._haskell_toolchain[HaskellToolchainInfo]

    output = ctx.actions.declare_output(ctx.attrs.name)
    link = cmd_args(haskell_toolchain.compiler)
    link.add("-o", output.as_output())
    link.add(haskell_toolchain.linker_flags)
    link.add(ctx.attrs.linker_flags)
    link.hidden(compiled.stubs)

    osuf, _hisuf = _output_extensions(link_style, ctx.attrs.enable_profiling)

    objfiles = _srcs_to_objfiles(ctx, compiled.objects, osuf)
    link.add(objfiles)

    hlis = []
    nlis = []
    sos = {}
    indexing_tsets = {}
    for lib in attr_deps(ctx):
        li = lib.get(HaskellLinkInfo)
        if li != None:
            hlis.extend(li.info[link_style])
        li = lib.get(MergedLinkInfo)
        if li != None:
            nlis.append(li)
        li = lib.get(SharedLibraryInfo)
        if li != None:
            # TODO This should probably use merged_shared_libraries to check
            # for soname conflicts.
            for name, shared_lib in traverse_shared_library_info(li).items():
                sos[name] = shared_lib.lib.output

        if compiled.producing_indices:
            tset = derive_indexing_tset(ctx.actions, link_style, compiled.hi, attr_deps(ctx))
            indexing_tsets[link_style] = tset

    nlis = merge_link_infos(ctx, nlis)
    infos = get_link_args(nlis, link_style)
    link.add(cmd_args(unpack_link_args(infos), prepend = "-optl"))

    ctx.actions.run(link, category = "haskell_link")

    run = cmd_args(output)

    if link_style == LinkStyle("shared"):
        link.add("-optl", "-Wl,-rpath", "-optl", "-Wl,$ORIGIN/sos")
        symlink_dir = ctx.actions.symlinked_dir("sos", sos)
        run.hidden(symlink_dir)

    providers = [
        DefaultInfo(default_output = output),
        RunInfo(args = run),
    ]

    if indexing_tsets:
        providers.append(HaskellIndexInfo(info = indexing_tsets))

    return providers
