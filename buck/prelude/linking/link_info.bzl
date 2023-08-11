# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//:artifact_tset.bzl",
    "ArtifactTSet",
    "make_artifact_tset",
)
load("@prelude//cxx:cxx_toolchain_types.bzl", "PicBehavior")
load(
    "@prelude//cxx:linker.bzl",
    "get_link_whole_args",
    "get_no_as_needed_shared_libs_flags",
    "get_objects_as_library_args",
)
load(
    "@prelude//utils:utils.bzl",
    "flatten",
)

# Represents an archive (.a file)
Archive = record(
    artifact = field("artifact"),
    # For a thin archive, this contains all the referenced .o files
    external_objects = field(["artifact"], []),
)

# The different ways libraries can contribute towards a link.
LinkStyle = enum(
    # Link using a static archive of non-PIC native objects.
    "static",
    # Link using a static archive containing PIC native objects.
    "static_pic",
    # Link using a native shared library.
    "shared",
)

STATIC_LINK_STYLES = [LinkStyle("static"), LinkStyle("static_pic")]

# Ways a library can request to be linked (e.g. usually specific via a rule
# param like `preferred_linkage`.  The actual link style used for a library is
# usually determined by a combination of this and the link style being exported
# via a provider.
Linkage = enum(
    "static",
    "shared",
    "any",
)

# This is used to mark each of the linkable types with a type so we can have
# different behavior for different types. Ideally, starlark records would
# each have the appropriate type automatically.
LinkableType = enum(
    "archive",
    "frameworks",
    "shared",
    "objects",
    "swiftmodule",
    "swift_runtime",
)

# An archive.
ArchiveLinkable = record(
    # Artifact in the .a format from ar
    archive = field(Archive.type),
    # If a bitcode bundle was created for this artifact it will be present here
    bitcode_bundle = field(["artifact", None], None),
    linker_type = field(str),
    link_whole = field(bool, False),
    # Indicates if this archive may contain LTO bit code.  Can be set to `False`
    # to e.g. tell dist LTO handling that a potentially expensive archive doesn't
    # need to be processed.
    supports_lto = field(bool, True),
    _type = field(LinkableType.type, LinkableType("archive")),
)

# A shared lib.
SharedLibLinkable = record(
    lib = field("artifact"),
    link_without_soname = field(bool, False),
    _type = field(LinkableType.type, LinkableType("shared")),
)

# A list of objects.
ObjectsLinkable = record(
    objects = field([["artifact"], None], None),
    # Any of the objects that are in bitcode format
    bitcode_bundle = field(["artifact", None], None),
    linker_type = field(str),
    link_whole = field(bool, False),
    _type = field(LinkableType.type, LinkableType("objects")),
)

# Framework + library information for Apple/Cxx targets.
FrameworksLinkable = record(
    # A list of trimmed framework paths, example: ["Foundation", "UIKit"]
    # Used to construct `-framework` args.
    framework_names = field([str], []),
    # A list of unresolved framework paths (i.e., containing $SDKROOT, etc).
    # Used to construct `-F` args for compilation and linking.
    #
    # Framework path resolution _must_ happen at the target site because
    # different targets might use different toolchains. For example,
    # an `apple_library()` might get _compiled_ using one toolchain
    # and then linked by as part of an `apple_binary()` using another
    # compatible toolchain. The resolved framework directories passed
    # using `-F` would be different for the compilation and the linking.
    unresolved_framework_paths = field([str], []),
    # A list of library names, used to construct `-l` args.
    library_names = field([str], []),
    _type = field(LinkableType.type, LinkableType("frameworks")),
)

SwiftmoduleLinkable = record(
    tset = field("SwiftmodulePathsTSet"),
    _type = field(LinkableType.type, LinkableType("swiftmodule")),
)

# Represents the Swift runtime as a linker input.
SwiftRuntimeLinkable = record(
    # Only store whether the runtime is required, so that linker flags
    # are only materialized _once_ (no duplicates) on the link line.
    runtime_required = field(bool, False),
    _type = field(LinkableType.type, LinkableType("swift_runtime")),
)

LinkableTypes = [ArchiveLinkable.type, SharedLibLinkable.type, ObjectsLinkable.type, FrameworksLinkable.type, SwiftmoduleLinkable.type, SwiftRuntimeLinkable.type]

# Contains the information required to add an item (often corresponding to a single library) to a link command line.
LinkInfo = record(
    # An informative name for this LinkInfo. This may be used in user messages
    # or when constructing intermediate output paths and does not need to be unique.
    name = field([str, None], None),
    # Opaque cmd_arg-likes to be added pre/post this item on a linker command line.
    pre_flags = field([""], []),
    post_flags = field([""], []),
    # Primary input to the linker, one of the Linkable types above.
    linkables = field([LinkableTypes], []),
    # Debug info which is referenced -- but not included -- by linkables in the
    # link info.  For example, this may include `.dwo` files, or the original
    # `.o` files if they contain debug info that doesn't follow the link.
    external_debug_info = field(ArtifactTSet.type, ArtifactTSet()),
)

# The ordering to use when traversing linker libs transitive sets.
LinkOrdering = enum(
    # Preorder traversal, the default behavior which traverses depth-first returning the current
    # node, and then its children left-to-right.
    "preorder",
    # Topological sort, such that nodes are listed after all nodes that have them as descendants.
    "topological",
)

def set_linkable_link_whole(
        linkable: [ArchiveLinkable.type, ObjectsLinkable.type, SharedLibLinkable.type, FrameworksLinkable.type]) -> [ArchiveLinkable.type, ObjectsLinkable.type, SharedLibLinkable.type, FrameworksLinkable.type]:
    if linkable._type == LinkableType("archive"):
        return ArchiveLinkable(
            archive = linkable.archive,
            linker_type = linkable.linker_type,
            link_whole = True,
            supports_lto = linkable.supports_lto,
            _type = linkable._type,
        )
    elif linkable._type == LinkableType("objects"):
        return ObjectsLinkable(
            objects = linkable.objects,
            linker_type = linkable.linker_type,
            link_whole = True,
            _type = linkable._type,
        )
    return linkable

# Helper to wrap a LinkInfo with additional pre/post-flags.
def wrap_link_info(
        inner: LinkInfo.type,
        pre_flags: list[""] = [],
        post_flags: list[""] = []) -> LinkInfo.type:
    pre_flags = pre_flags + inner.pre_flags
    post_flags = inner.post_flags + post_flags
    return LinkInfo(
        name = inner.name,
        pre_flags = pre_flags,
        post_flags = post_flags,
        linkables = inner.linkables,
        external_debug_info = inner.external_debug_info,
    )

# Adds appropriate args representing `linkable` to `args`
def append_linkable_args(args: cmd_args, linkable: LinkableTypes):
    if linkable._type == LinkableType("archive"):
        if linkable.link_whole:
            args.add(get_link_whole_args(linkable.linker_type, [linkable.archive.artifact]))
        elif linkable.linker_type == "darwin":
            pass
        else:
            args.add(linkable.archive.artifact)

        # When using thin archives, object files are implicitly used as inputs
        # to the link, so make sure track them as inputs so that they're
        # materialized/tracked properly.
        args.add(cmd_args().hidden(linkable.archive.external_objects))
    elif linkable._type == LinkableType("shared"):
        if linkable.link_without_soname:
            args.add(cmd_args(linkable.lib, format = "-L{}").parent())
            args.add("-l" + linkable.lib.basename.removeprefix("lib").removesuffix(linkable.lib.extension))
        else:
            args.add(linkable.lib)
    elif linkable._type == LinkableType("objects"):
        # We depend on just the filelist for darwin linker and don't add the normal args
        if linkable.linker_type != "darwin":
            # We need to export every symbol when link groups are used, but enabling
            # --whole-archive with --start-lib is undefined behavior in gnu linkers:
            # https://reviews.llvm.org/D120443. We need to export symbols from every
            # linkable in the link_info
            if not linkable.link_whole:
                args.add(get_objects_as_library_args(linkable.linker_type, linkable.objects))
            else:
                args.add(linkable.objects)
    elif linkable._type == LinkableType("frameworks") or linkable._type == LinkableType("swiftmodule") or linkable._type == LinkableType("swift_runtime"):
        # These flags are handled separately so they can be deduped.
        #
        # We've seen in apps with larger dependency graphs that failing
        # to dedupe these args results in linker.argsfile which are too big.
        pass
    else:
        fail("Encountered unhandled linkable of type {}".format(linkable._type))

def link_info_to_args(value: LinkInfo.type) -> cmd_args:
    args = cmd_args(value.pre_flags)
    for linkable in value.linkables:
        append_linkable_args(args, linkable)
    if value.post_flags != None:
        args.add(value.post_flags)
    return args

# List of inputs to pass to the darwin linker via the `-filelist` param.
# TODO(agallagher): It might be nicer to leave these inlined in the args
# above and extract them at link time via reflection.  This way we'd hide
# platform-specific details from this level.
# NOTE(agallagher): Using filelist out-of-band means objects/archives get
# linked out of order of their corresponding flags.
def link_info_filelist(value: LinkInfo.type) -> list["artifact"]:
    filelists = []
    for linkable in value.linkables:
        if linkable._type == LinkableType("archive"):
            if linkable.linker_type == "darwin" and not linkable.link_whole:
                filelists.append(linkable.archive.artifact)
        elif linkable._type == LinkableType("shared"):
            pass
        elif linkable._type == LinkableType("objects"):
            if linkable.linker_type == "darwin":
                filelists += linkable.objects
        elif linkable._type == LinkableType("frameworks") or linkable._type == LinkableType("swiftmodule") or linkable._type == LinkableType("swift_runtime"):
            pass
        else:
            fail("Encountered unhandled linkable of type {}".format(linkable._type))
    return filelists

# Encapsulate all `LinkInfo`s provided by a given rule's link style.
#
# We provide both the "default" and (optionally) a pre-"stripped" LinkInfo. For a consumer that doesn't care
# about debug info (for example, who is going to produce stripped output anyway), it can be significantly
# cheaper to consume the pre-stripped LinkInfo.
LinkInfos = record(
    # Link info to use by default.
    default = field(LinkInfo.type),
    # Link info stripped of debug symbols.
    stripped = field([LinkInfo.type, None], None),
)

# The output of a native link (e.g. a shared library or an executable).
LinkedObject = record(
    output = field(["artifact", "promise"]),
    # The combined bitcode from this linked object and any static libraries
    bitcode_bundle = field(["artifact", None], None),
    # the generated linked output before running bolt, may be None if bolt is not used.
    prebolt_output = field(["artifact", None], None),
    # A linked object (binary/shared library) may have an associated dwp file with
    # its corresponding DWARF debug info.
    # May be None when Split DWARF is disabled or for some types of synthetic link objects.
    dwp = field(["artifact", None], None),
    # Additional dirs or paths that contain debug info referenced by the linked
    # object (e.g. split dwarf files or PDB file).
    external_debug_info = field(ArtifactTSet.type, ArtifactTSet()),
    # This argsfile is generated in the `cxx_link` step and contains a list of arguments
    # passed to the linker. It is being exposed as a sub-target for debugging purposes.
    linker_argsfile = field(["artifact", None], None),
    # This sub-target is only available for distributed thinLTO builds.
    index_argsfile = field(["artifact", None], None),
    # Import library for linking with DLL on Windows.
    # If not on Windows it's always None.
    import_library = field(["artifact", None], None),
    # A linked object (binary/shared library) may have an associated PDB file with
    # its corresponding Windows debug info.
    # If not on Windows it's always None.
    pdb = field(["artifact", None], None),
    # Split-debug info generated by the link.
    split_debug_output = field(["artifact", None], None),
)

def _link_info_default_args(infos: "LinkInfos"):
    info = infos.default
    return link_info_to_args(info)

def _link_info_default_shared_link_args(infos: "LinkInfos"):
    info = infos.default
    return link_info_to_args(info)

def _link_info_stripped_args(infos: "LinkInfos"):
    info = infos.stripped or infos.default
    return link_info_to_args(info)

def _link_info_stripped_shared_link_args(infos: "LinkInfos"):
    info = infos.stripped or infos.default
    return link_info_to_args(info)

def _link_info_default_filelist(infos: "LinkInfos"):
    info = infos.default
    return link_info_filelist(info)

def _link_info_stripped_filelist(infos: "LinkInfos"):
    info = infos.stripped or infos.default
    return link_info_filelist(info)

def _link_info_has_default_filelist(children: list[bool], infos: ["LinkInfos", None]) -> bool:
    if infos:
        info = infos.default
        if link_info_filelist(info):
            return True
    return any(children)

def _link_info_has_stripped_filelist(children: list[bool], infos: ["LinkInfos", None]) -> bool:
    if infos:
        info = infos.stripped or infos.default
        if link_info_filelist(info):
            return True
    return any(children)

# TransitiveSet of LinkInfos.
LinkInfosTSet = transitive_set(
    args_projections = {
        "default": _link_info_default_args,
        "default_filelist": _link_info_default_filelist,
        "default_shared": _link_info_default_shared_link_args,
        "stripped": _link_info_stripped_args,
        "stripped_filelist": _link_info_stripped_filelist,
        "stripped_shared": _link_info_stripped_shared_link_args,
    },
    reductions = {
        "has_default_filelist": _link_info_has_default_filelist,
        "has_stripped_filelist": _link_info_has_stripped_filelist,
    },
)

# A map of native linkable infos from transitive dependencies.
MergedLinkInfo = provider(fields = [
    "_infos",  # {LinkStyle.type: LinkInfosTSet.type}
    "_external_debug_info",  # {LinkStyle.type: ArtifactTSet.type}
    # Apple framework linker args must be deduped to avoid overflow in our argsfiles.
    #
    # To save on repeated computation of transitive LinkInfos, we store a dedupped
    # structure, based on the link-style.
    "frameworks",  # {LinkStyle.type: [FrameworksLinkable.type, None]}
    "swift_runtime",  # {LinkStyle.type: [SwiftRuntimeLinkable.type, None]}
])

# A map of linkages to all possible link styles it supports.
_LINK_STYLE_FOR_LINKAGE = {
    Linkage("any"): [LinkStyle("static"), LinkStyle("static_pic"), LinkStyle("shared")],
    Linkage("static"): [LinkStyle("static"), LinkStyle("static_pic")],
    Linkage("shared"): [LinkStyle("shared")],
}

# Helper to wrap a LinkInfos with additional pre/post-flags.
def wrap_link_infos(
        inner: LinkInfos.type,
        pre_flags: list[""] = [],
        post_flags: list[""] = []) -> LinkInfos.type:
    return LinkInfos(
        default = wrap_link_info(
            inner.default,
            pre_flags = pre_flags,
            post_flags = post_flags,
        ),
        stripped = None if inner.stripped == None else wrap_link_info(
            inner.stripped,
            pre_flags = pre_flags,
            post_flags = post_flags,
        ),
    )

def create_merged_link_info(
        # Target context for which to create the link info.
        ctx: AnalysisContext,
        pic_behavior: PicBehavior.type,
        # The link infos provided by this rule, as a map from link style (as
        # used by dependents) to `LinkInfo`.
        link_infos: dict[LinkStyle.type, LinkInfos.type] = {},
        # How the rule requests to be linked.  This will be used to determine
        # which actual link style to propagate for each "requested" link style.
        preferred_linkage: Linkage.type = Linkage("any"),
        # Link info to propagate from non-exported deps for static link styles.
        deps: list["MergedLinkInfo"] = [],
        # Link info to always propagate from exported deps.
        exported_deps: list["MergedLinkInfo"] = [],
        frameworks_linkable: [FrameworksLinkable.type, None] = None,
        swift_runtime_linkable: [SwiftRuntimeLinkable.type, None] = None) -> "MergedLinkInfo":
    """
    Create a `MergedLinkInfo` provider.
    """

    infos = {}
    external_debug_info = {}
    frameworks = {}
    swift_runtime = {}

    # We don't know how this target will be linked, so we generate the possible
    # link info given the target's preferred linkage, to be consumed by the
    # ultimate linking target.
    for link_style in LinkStyle:
        actual_link_style = get_actual_link_style(link_style, preferred_linkage, pic_behavior)

        children = []
        external_debug_info_children = []
        framework_linkables = []
        swift_runtime_linkables = []

        # When we're being linked statically, we also need to export all private
        # linkable input (e.g. so that any unresolved symbols we have are
        # resolved properly when we're linked).
        if actual_link_style != LinkStyle("shared"):
            # We never want to propagate the linkables used to build a shared library.
            #
            # Doing so breaks the encapsulation of what is in linked in the library vs. the main executable.
            framework_linkables.append(frameworks_linkable)
            framework_linkables += [dep_info.frameworks[link_style] for dep_info in exported_deps]

            swift_runtime_linkables.append(swift_runtime_linkable)
            swift_runtime_linkables += [dep_info.swift_runtime[link_style] for dep_info in exported_deps]

            for dep_info in deps:
                children.append(dep_info._infos[link_style])
                external_debug_info_children.append(dep_info._external_debug_info[link_style])
                framework_linkables.append(dep_info.frameworks[link_style])
                swift_runtime_linkables.append(dep_info.swift_runtime[link_style])

        # We always export link info for exported deps.
        for dep_info in exported_deps:
            children.append(dep_info._infos[link_style])
            external_debug_info_children.append(dep_info._external_debug_info[link_style])

        frameworks[link_style] = merge_framework_linkables(framework_linkables)
        swift_runtime[link_style] = merge_swift_runtime_linkables(swift_runtime_linkables)
        if actual_link_style in link_infos:
            infos[link_style] = ctx.actions.tset(
                LinkInfosTSet,
                value = link_infos[actual_link_style],
                children = children,
            )
            external_debug_info[link_style] = make_artifact_tset(
                actions = ctx.actions,
                label = ctx.label,
                children = (
                    [link_infos[actual_link_style].default.external_debug_info] +
                    external_debug_info_children
                ),
            )

    return MergedLinkInfo(
        _infos = infos,
        _external_debug_info = external_debug_info,
        frameworks = frameworks,
        swift_runtime = swift_runtime,
    )

def merge_link_infos(
        ctx: AnalysisContext,
        xs: list["MergedLinkInfo"]) -> "MergedLinkInfo":
    merged = {}
    merged_external_debug_info = {}
    frameworks = {}
    swift_runtime = {}
    for link_style in LinkStyle:
        merged[link_style] = ctx.actions.tset(
            LinkInfosTSet,
            children = filter(None, [x._infos.get(link_style) for x in xs]),
        )
        merged_external_debug_info[link_style] = make_artifact_tset(
            actions = ctx.actions,
            label = ctx.label,
            children = filter(None, [x._external_debug_info.get(link_style) for x in xs]),
        )
        frameworks[link_style] = merge_framework_linkables([x.frameworks[link_style] for x in xs])
        swift_runtime[link_style] = merge_swift_runtime_linkables([x.swift_runtime[link_style] for x in xs])
    return MergedLinkInfo(
        _infos = merged,
        _external_debug_info = merged_external_debug_info,
        frameworks = frameworks,
        swift_runtime = swift_runtime,
    )

def get_link_info(
        infos: LinkInfos.type,
        prefer_stripped: bool = False) -> LinkInfo.type:
    """
    Helper for getting a `LinkInfo` out of a `LinkInfos`.
    """

    # When requested, prefer using pre-stripped link info.
    if prefer_stripped and infos.stripped != None:
        return infos.stripped

    return infos.default

LinkArgsTSet = record(
    infos = field(LinkInfosTSet.type),
    external_debug_info = field(ArtifactTSet.type, ArtifactTSet()),
    prefer_stripped = field(bool.type, False),
)

# An enum. Only one field should be set. The variants here represent different
# ways in which we might obtain linker commands: through a t-set of propagated
# dependencies (used for deps propagated unconditionally up a tree), through a
# series of LinkInfo (used for link groups, Omnibus linking), or simply through
# raw arguments we want to include (used for e.g. per-target link flags).
LinkArgs = record(
    # A LinkInfosTSet + a flag indicating if stripped is preferred.
    tset = field([LinkArgsTSet.type, None], None),
    # A list of LinkInfos
    infos = field([[LinkInfo.type], None], None),
    # A bunch of flags.
    flags = field(["_arglike", None], None),
)

def unpack_link_args(args: LinkArgs.type, is_shared: [bool, None] = None, link_ordering: [LinkOrdering.type, None] = None) -> "_arglike":
    if args.tset != None:
        ordering = link_ordering.value if link_ordering else "preorder"

        tset = args.tset.infos
        if is_shared:
            if args.tset.prefer_stripped:
                return tset.project_as_args("stripped_shared", ordering = ordering)
            return tset.project_as_args("default_shared", ordering = ordering)
        else:
            if args.tset.prefer_stripped:
                return tset.project_as_args("stripped", ordering = ordering)
            return tset.project_as_args("default", ordering = ordering)

    if args.infos != None:
        return cmd_args([link_info_to_args(info) for info in args.infos])

    if args.flags != None:
        return args.flags

    fail("Unpacked invalid empty link args")

def unpack_link_args_filelist(args: LinkArgs.type) -> ["_arglike", None]:
    if args.tset != None:
        tset = args.tset.infos
        stripped = args.tset.prefer_stripped
        if not tset.reduce("has_stripped_filelist" if stripped else "has_default_filelist"):
            return None
        return tset.project_as_args("stripped_filelist" if stripped else "default_filelist")

    if args.infos != None:
        filelist = flatten([link_info_filelist(info) for info in args.infos])
        if not filelist:
            return None

        # Actually create cmd_args so the API is consistent between the 2 branches.
        args = cmd_args()
        args.add(filelist)
        return args

    if args.flags != None:
        return None

    fail("Unpacked invalid empty link args")

def unpack_external_debug_info(actions: "actions", args: LinkArgs.type) -> ArtifactTSet.type:
    if args.tset != None:
        if args.tset.prefer_stripped:
            return ArtifactTSet()
        return args.tset.external_debug_info

    if args.infos != None:
        return make_artifact_tset(
            actions = actions,
            children = [info.external_debug_info for info in args.infos],
        )

    if args.flags != None:
        return ArtifactTSet()

    fail("Unpacked invalid empty link args")

def map_to_link_infos(links: list[LinkArgs.type]) -> list["LinkInfo"]:
    res = []

    def append(v):
        if v.pre_flags or v.post_flags or v.linkables:
            res.append(v)

    for link in links:
        if link.tset != None:
            for info in link.tset.infos.traverse():
                if link.tset.prefer_stripped:
                    append(info.stripped or info.default)
                else:
                    append(info.default)
            continue
        if link.infos != None:
            for link in link.infos:
                append(link)
            continue
        if link.flags != None:
            append(LinkInfo(pre_flags = link.flags))
            continue
        fail("Unpacked invalid empty link args")
    return res

def get_link_args(
        merged: "MergedLinkInfo",
        link_style: LinkStyle.type,
        prefer_stripped: bool = False) -> LinkArgs.type:
    """
    Return `LinkArgs` for `MergedLinkInfo`  given a link style and a strip preference.
    """

    return LinkArgs(
        tset = LinkArgsTSet(
            infos = merged._infos[link_style],
            external_debug_info = merged._external_debug_info[link_style],
            prefer_stripped = prefer_stripped,
        ),
    )

def get_actual_link_style(
        requested_link_style: LinkStyle.type,
        preferred_linkage: Linkage.type,
        pic_behavior: PicBehavior.type) -> LinkStyle.type:
    """
    Return how we link a library for a requested link style and preferred linkage.
    -----------------------------------------------------------------------------------|
    |                   |                    requested_link_style                      |
    | preferred_linkage |--------------------------------------------------------------|
    |                   |       static       |     static_pic     |       shared       |
    -----------------------------------------------------------------------------------|
    |      static       | check pic_behavior | check pic_behavior | check pic_behavior |
    |      shared       |       shared       |       shared       |       shared       |
    |       any         | check pic_behavior | check pic_behavior |       shared       |
    ------------------------------------------------------------------------------------
    """
    no_pic_style = _get_link_style_without_pic_behavior(requested_link_style, preferred_linkage)
    return process_link_style_for_pic_behavior(no_pic_style, pic_behavior)

def _get_link_style_without_pic_behavior(requested_link_style: LinkStyle.type, preferred_linkage: Linkage.type) -> LinkStyle.type:
    if preferred_linkage == Linkage("any"):
        return requested_link_style
    elif preferred_linkage == Linkage("shared"):
        return LinkStyle("shared")
    else:  # preferred_linkage = static
        if requested_link_style == LinkStyle("static"):
            return requested_link_style
        else:
            return LinkStyle("static_pic")

def process_link_style_for_pic_behavior(link_style: LinkStyle.type, behavior: PicBehavior.type) -> LinkStyle.type:
    """
    - For targets being built for x86_64, arm64, the fPIC flag isn't respected. Everything is fPIC.
    - For targets being built for Windows, nothing is fPIC. The flag is ignored.
    - There are many platforms (linux, etc.) where the fPIC flag is supported.

    As a result, we can end-up in a place where you pic + non-pic artifacts are requested
    but the platform will produce the exact same output (despite the different files).
    """
    if behavior == PicBehavior("supported") or link_style not in STATIC_LINK_STYLES:
        return link_style
    elif behavior == PicBehavior("not_supported"):
        return LinkStyle("static")
    elif behavior == PicBehavior("always_enabled"):
        return LinkStyle("static_pic")
    else:
        fail("Unknown pic_behavior: {}".format(behavior))

def get_link_styles_for_linkage(linkage: Linkage.type) -> list[LinkStyle.type]:
    """
    Return all possible `LinkStyle`s that apply for the given `Linkage`.
    """
    return _LINK_STYLE_FOR_LINKAGE[linkage]

def merge_swift_runtime_linkables(linkables: list[[SwiftRuntimeLinkable.type, None]]) -> SwiftRuntimeLinkable.type:
    for linkable in linkables:
        if linkable and linkable.runtime_required:
            return SwiftRuntimeLinkable(runtime_required = True)
    return SwiftRuntimeLinkable(runtime_required = False)

def merge_framework_linkables(linkables: list[[FrameworksLinkable.type, None]]) -> FrameworksLinkable.type:
    unique_framework_names = {}
    unique_framework_paths = {}
    unique_library_names = {}
    for linkable in linkables:
        if not linkable:
            continue

        # Avoid building a huge list and then de-duplicating, instead we
        # use a set to track each used entry, order does not matter.
        for framework in linkable.framework_names:
            unique_framework_names[framework] = True
        for framework_path in linkable.unresolved_framework_paths:
            unique_framework_paths[framework_path] = True
        for library_name in linkable.library_names:
            unique_library_names[library_name] = True

    return FrameworksLinkable(
        framework_names = unique_framework_names.keys(),
        unresolved_framework_paths = unique_framework_paths.keys(),
        library_names = unique_library_names.keys(),
    )

def wrap_with_no_as_needed_shared_libs_flags(linker_type: str, link_info: LinkInfo.type) -> LinkInfo.type:
    """
    Wrap link info in args used to prevent linkers from dropping unused shared
    library dependencies from the e.g. DT_NEEDED tags of the link.
    """

    if linker_type == "gnu":
        return wrap_link_info(
            inner = link_info,
            pre_flags = (
                ["-Wl,--push-state"] +
                get_no_as_needed_shared_libs_flags(linker_type)
            ),
            post_flags = ["-Wl,--pop-state"],
        )

    if linker_type == "darwin":
        return link_info

    fail("Linker type {} not supported".format(linker_type))
