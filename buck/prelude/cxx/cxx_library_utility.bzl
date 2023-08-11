# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//linking:link_info.bzl",
    "LinkStyle",
    "Linkage",
    "MergedLinkInfo",
    "merge_link_infos",
)
load(
    "@prelude//utils:utils.bzl",
    "expect",
    "flatten",
    "from_named_set",
)
load(
    ":compile.bzl",
    "CxxCompileOutput",  # @unused Used as a type
)
load(":cxx_context.bzl", "get_cxx_platform_info", "get_cxx_toolchain_info")
load(
    ":headers.bzl",
    "cxx_attr_header_namespace",
)
load(
    ":linker.bzl",
    "get_shared_library_name",
)
load(":platform.bzl", "cxx_by_platform")

OBJECTS_SUBTARGET = "objects"

# The dependencies
def cxx_attr_deps(ctx: AnalysisContext) -> list[Dependency]:
    return (
        ctx.attrs.deps +
        flatten(cxx_by_platform(ctx, getattr(ctx.attrs, "platform_deps", []))) +
        (getattr(ctx.attrs, "deps_query", []) or [])
    )

def cxx_attr_exported_deps(ctx: AnalysisContext) -> list[Dependency]:
    return ctx.attrs.exported_deps + flatten(cxx_by_platform(ctx, ctx.attrs.exported_platform_deps))

def cxx_attr_exported_linker_flags(ctx: AnalysisContext) -> list[""]:
    return (
        ctx.attrs.exported_linker_flags +
        flatten(cxx_by_platform(ctx, ctx.attrs.exported_platform_linker_flags))
    )

def cxx_attr_exported_post_linker_flags(ctx: AnalysisContext) -> list[""]:
    return (
        ctx.attrs.exported_post_linker_flags +
        flatten(cxx_by_platform(ctx, ctx.attrs.exported_post_platform_linker_flags))
    )

def cxx_inherited_link_info(ctx, first_order_deps: list[Dependency]) -> MergedLinkInfo.type:
    # We filter out nones because some non-cxx rule without such providers could be a dependency, for example
    # cxx_binary "fbcode//one_world/cli/util/process_wrapper:process_wrapper" depends on
    # python_library "fbcode//third-party-buck/$platform/build/glibc:__project__"
    return merge_link_infos(ctx, filter(None, [x.get(MergedLinkInfo) for x in first_order_deps]))

# Linker flags
def cxx_attr_linker_flags(ctx: AnalysisContext) -> list[""]:
    return (
        ctx.attrs.linker_flags +
        flatten(cxx_by_platform(ctx, ctx.attrs.platform_linker_flags))
    )

def cxx_attr_link_style(ctx: AnalysisContext) -> LinkStyle.type:
    if ctx.attrs.link_style != None:
        return LinkStyle(ctx.attrs.link_style)
    if ctx.attrs.defaults != None:
        # v1 equivalent code is in CxxConstructorArg::getDefaultFlavors and ParserWithConfigurableAttributes::applyDefaultFlavors
        # Only values in the map are used by v1 as flavors, copy this behavior and return the first value which is compatible with link style.
        v1_flavors = ctx.attrs.defaults.values()
        for s in [LinkStyle("static"), LinkStyle("static_pic"), LinkStyle("shared")]:
            if s.value in v1_flavors:
                return s
    return get_cxx_toolchain_info(ctx).linker_info.link_style

def cxx_attr_preferred_linkage(ctx: AnalysisContext) -> Linkage.type:
    preferred_linkage = ctx.attrs.preferred_linkage

    # force_static is deprecated, but it has precedence over preferred_linkage
    if getattr(ctx.attrs, "force_static", False):
        preferred_linkage = "static"

    return Linkage(preferred_linkage)

def cxx_attr_resources(ctx: AnalysisContext) -> dict[str, ("artifact", list["_arglike"])]:
    """
    Return the resources provided by this rule, as a map of resource name to
    a tuple of the resource artifact and any "other" outputs exposed by it.
    """

    resources = {}
    namespace = cxx_attr_header_namespace(ctx)

    # Use getattr, as apple rules don't have a `resources` parameter.
    for name, resource in from_named_set(getattr(ctx.attrs, "resources", {})).items():
        if type(resource) == "artifact":
            other = []
        else:
            info = resource[DefaultInfo]
            expect(
                len(info.default_outputs) == 1,
                "expected exactly one default output from {} ({})"
                    .format(resource, info.default_outputs),
            )
            [resource] = info.default_outputs
            other = info.other_outputs
        resources[paths.join(namespace, name)] = (resource, other)

    return resources

def cxx_mk_shlib_intf(
        ctx: AnalysisContext,
        name: str,
        shared_lib: ["artifact", "promise"]) -> "artifact":
    """
    Convert the given shared library into an interface used for linking.
    """
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    args = cmd_args(linker_info.mk_shlib_intf[RunInfo])
    args.add(shared_lib)
    output = ctx.actions.declare_output(
        get_shared_library_name(linker_info, name + "-interface"),
    )
    args.add(output.as_output())
    ctx.actions.run(
        args,
        category = "generate_shared_library_interface",
        identifier = name,
    )
    return output

def cxx_is_gnu(ctx: AnalysisContext) -> bool:
    return get_cxx_toolchain_info(ctx).linker_info.type == "gnu"

def cxx_use_shlib_intfs(ctx: AnalysisContext) -> bool:
    """
    Return whether we should use shared library interfaces for linking.
    """

    # Per-rule opt-out.
    if not getattr(ctx.attrs, "supports_shlib_interfaces", True):
        return False

    # TODO(T110378128): Apple currently uses the same configuration as fbcode
    # platforms, so only explicitly enable for linux until this is fixed.
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    return linker_info.shlib_interfaces != "disabled" and linker_info.type == "gnu"

def cxx_platform_supported(ctx: AnalysisContext) -> bool:
    """
    Return whether this rule's `supported_platforms_regex` matches the current
    platform name.
    """

    if ctx.attrs.supported_platforms_regex == None:
        return True

    return regex_match(
        ctx.attrs.supported_platforms_regex,
        get_cxx_platform_info(ctx).name,
    )

def cxx_objects_sub_target(outs: list[CxxCompileOutput.type]) -> list["provider"]:
    objects_sub_targets = {}
    for obj in outs:
        sub_targets = {}
        if obj.clang_trace:
            sub_targets["clang-trace"] = [DefaultInfo(obj.clang_trace)]
        if obj.clang_remarks:
            sub_targets["clang-remarks"] = [DefaultInfo(obj.clang_remarks)]
        objects_sub_targets[obj.object.short_path] = [DefaultInfo(
            obj.object,
            sub_targets = sub_targets,
        )]
    return [DefaultInfo(sub_targets = objects_sub_targets)]
