# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load("@prelude//utils:utils.bzl", "expect", "from_named_set", "is_any", "map_val", "value_or")
load(":cxx_context.bzl", "get_cxx_toolchain_info")
load(":platform.bzl", "cxx_by_platform")

# Defines the varying bits of implementation affecting on how the end user
# should include the headers.
# Given there are 2 headers which are defined:
# a) one header in a list, as ["foo/bar/foobar.h"]
# b) one header in a dict (aka named header), as {"wfh/baz.h": "path/header.h"}
#
# `apple`:
# 1) header from the list should be included as NAMESPACE/PATH_BASENAME:
# #include "namespace/foobar.h"
# 2) header from the dict should be included as DICT_KEY (aka header name):
# #include "wfh/baz.h"
# 3) it should be possible to include list header from the same target via basename:
# #include "foobar.h"
#
# `regular`:
# 1) header from the list should be included as NAMESPACE/PATH:
# #include "namespace/foo/bar/foobar.h"
# 2) header from the dict should be included as NAMESPACE/DICT_KEY:
# #include "namespace/wfh/baz.h"
CxxHeadersNaming = enum("apple", "regular")

# Modes supporting implementing the `headers` parameter of C++ rules using raw
# headers instead of e.g. symlink trees.
HeadersAsRawHeadersMode = enum(
    # Require that all headers be implemented as raw headers, failing if this
    # is not possible.
    "required",
    # Attempt to implement headers via raw headers, falling to header maps or
    # symlink tress when raw headers cannot be used (e.g. rule contains a
    # generated header or remaps a header to an incompatible location in the
    # header namespace).
    "preferred",
    "disabled",
)

HeaderMode = enum(
    # Creates the header map that references the headers directly in the source
    # tree.
    "header_map_only",
    # Creates the tree of symbolic links of headers.
    "symlink_tree_only",
    # Creates the tree of symbolic links of headers and creates the header map
    # that references the symbolic links to the headers.
    "symlink_tree_with_header_map",
)

HeaderStyle = enum(
    "local",
    "system",
)

Headers = record(
    include_path = field(cmd_args),
    # NOTE(agallagher): Used for module hack replacement.
    symlink_tree = field(["artifact", None], None),
    # args that map symlinked private headers to source path
    file_prefix_args = field([cmd_args, None], None),
)

CHeader = record(
    # `"artifact"` pointing to the actual header file
    artifact = "artifact",
    # Basename as it should appear in include directive
    name = str,
    # Prefix before the basename as it should appear in include directive
    namespace = str,
    # Whether or not this header is provided via dict, where the corresponding key is a new name
    named = bool,
)

# Parameters controlling the varying aspects of headers-related behavior.
# The contract on how headers could be used (i.e. end user inclusion rules)
# is different for `apple_library` and `cxx_library`. Those parameters
# allows generalizing the C++ rules implementation and are provided
# by top-level user-facing wrappers around those generalized methods.
CxxHeadersLayout = record(
    # Prefix part of the header path in the include statement. Header name might
    # not always be prepended by the namespace, `naming` parameter is controlling
    # that behavior. The value is ready to be used and abstracts different naming
    # for such prefix in user-facing attributes (e.g. `apple_binary.header_path_prefix`
    # vs `cxx_binary.header_namespace`) and different default values when those
    # attributes are omitted (package path for regular C++ rules vs target name for
    # Apple-specific rules).
    namespace = str,
    # Selects the behavior in the implementation to support the specific way of how
    # headers are allowed to be included (e.g. if header namespace is applied for
    # headers from dicts). For more information see comment for `CxxHeadersNaming`
    naming = CxxHeadersNaming.type,
)

CPrecompiledHeaderInfo = provider(fields = [
    # Actual precompiled header ready to be used during compilation, "artifact"
    "header",
])

def cxx_attr_header_namespace(ctx: AnalysisContext) -> str:
    return value_or(ctx.attrs.header_namespace, ctx.label.package)

def cxx_attr_exported_headers(ctx: AnalysisContext, headers_layout: CxxHeadersLayout.type) -> list[CHeader.type]:
    headers = _get_attr_headers(ctx.attrs.exported_headers, headers_layout.namespace, headers_layout.naming)
    platform_headers = _get_attr_headers(_headers_by_platform(ctx, ctx.attrs.exported_platform_headers), headers_layout.namespace, headers_layout.naming)
    return headers + platform_headers

def cxx_attr_headers(ctx: AnalysisContext, headers_layout: CxxHeadersLayout.type) -> list[CHeader.type]:
    headers = _get_attr_headers(ctx.attrs.headers, headers_layout.namespace, headers_layout.naming)
    platform_headers = _get_attr_headers(_headers_by_platform(ctx, ctx.attrs.platform_headers), headers_layout.namespace, headers_layout.naming)
    return headers + platform_headers

def cxx_get_regular_cxx_headers_layout(ctx: AnalysisContext) -> CxxHeadersLayout.type:
    namespace = cxx_attr_header_namespace(ctx)
    return CxxHeadersLayout(namespace = namespace, naming = CxxHeadersNaming("regular"))

def cxx_attr_exported_header_style(ctx: AnalysisContext) -> HeaderStyle.type:
    return HeaderStyle(ctx.attrs.exported_header_style)

def _get_attr_headers(xs: "", namespace: str, naming: CxxHeadersNaming.type) -> list[CHeader.type]:
    if type(xs) == type([]):
        return [CHeader(artifact = x, name = _get_list_header_name(x, naming), namespace = namespace, named = False) for x in xs]
    else:
        return [CHeader(artifact = xs[x], name = x, namespace = _get_dict_header_namespace(namespace, naming), named = True) for x in xs]

def _headers_by_platform(ctx: AnalysisContext, xs: list[(str, "")]) -> "":
    res = {}
    for deps in cxx_by_platform(ctx, xs):
        res.update(from_named_set(deps))
    return res

def as_raw_headers(
        ctx: AnalysisContext,
        headers: dict[str, "artifact"],
        mode: HeadersAsRawHeadersMode.type) -> [list["label_relative_path"], None]:
    """
    Return the include directories needed to treat the given headers as raw
    headers, depending on the given `HeadersAsRawHeadersMode` mode.

    Args:
      mode:
        disabled - always return `None`
        preferred - return `None` if conversion isn't possible
        required - fail if conversion isn't possible
    """

    # If we're not supporting raw header conversion, return `None`.
    if mode == HeadersAsRawHeadersMode("disabled"):
        return None

    return _as_raw_headers(
        ctx,
        headers,
        # Don't fail if conversion isn't required.
        no_fail = mode != HeadersAsRawHeadersMode("required"),
    )

def _header_mode(ctx: AnalysisContext) -> HeaderMode.type:
    header_mode = map_val(HeaderMode, getattr(ctx.attrs, "header_mode", None))
    if header_mode != None:
        return header_mode
    return get_cxx_toolchain_info(ctx).header_mode

def prepare_headers(ctx: AnalysisContext, srcs: dict[str, "artifact"], name: str, absolute_path_prefix: [str, None]) -> [Headers.type, None]:
    """
    Prepare all the headers we want to use, depending on the header_mode
    set on the target's toolchain.
        - In the case of a header map, we create a `name`.hmap file and
          return it as part of the include path.
        - In the case of a symlink tree, we create a directory of `name`
          containing the headers and return it as part of the include path.
    """
    if len(srcs) == 0:
        return None

    header_mode = _header_mode(ctx)

    # TODO(T110378135): There's a bug in clang where using header maps w/o
    # explicit `-I` anchors breaks module map lookups.  This will be fixed
    # by https://reviews.llvm.org/D103930 so, until it lands, disable header
    # maps when we see a module map.
    if (header_mode == HeaderMode("symlink_tree_with_header_map") and
        is_any(lambda n: paths.basename(n) == "module.modulemap", srcs.keys())):
        header_mode = HeaderMode("symlink_tree_only")

    output_name = name + "-abs" if absolute_path_prefix else name

    if header_mode == HeaderMode("header_map_only"):
        headers = {h: (a, "{}") for h, a in srcs.items()}
        hmap = _mk_hmap(ctx, output_name, headers, absolute_path_prefix)
        return Headers(
            include_path = cmd_args(hmap).hidden(srcs.values()),
        )
    symlink_dir = ctx.actions.symlinked_dir(output_name, _normalize_header_srcs(srcs))
    if header_mode == HeaderMode("symlink_tree_only"):
        return Headers(include_path = cmd_args(symlink_dir), symlink_tree = symlink_dir)
    if header_mode == HeaderMode("symlink_tree_with_header_map"):
        headers = {h: (symlink_dir, "{}/" + h) for h in srcs}
        hmap = _mk_hmap(ctx, output_name, headers, absolute_path_prefix)
        file_prefix_args = _get_debug_prefix_args(ctx, symlink_dir)
        return Headers(
            include_path = cmd_args(hmap).hidden(symlink_dir),
            symlink_tree = symlink_dir,
            file_prefix_args = file_prefix_args,
        )
    fail("Unsupported header mode: {}".format(header_mode))

def _normalize_header_srcs(srcs: dict) -> dict:
    normalized_srcs = {}
    for key, val in srcs.items():
        normalized_key = paths.normalize(key)
        stored_val = normalized_srcs.get(normalized_key, None)
        expect(
            stored_val == None or stored_val == val,
            "Got different values {} and {} for the same normalized header {}".format(
                val,
                stored_val,
                normalized_key,
            ),
        )
        normalized_srcs[normalized_key] = val

    return normalized_srcs

def _as_raw_headers(
        ctx: AnalysisContext,
        headers: dict[str, "artifact"],
        # Return `None` instead of failing.
        no_fail: bool = False) -> [list["label_relative_path"], None]:
    """
    Return the include directories needed to treat the given headers as raw
    headers.
    """

    # Find the all the include dirs needed to treat the given headers as raw
    # headers.
    inc_dirs = {}
    for name, header in headers.items():
        inc_dir = _as_raw_header(
            ctx,
            name,
            header,
            no_fail = no_fail,
        )

        # If the conversion wasn't possible, `inc_dir` will be `None` and we
        # should bail now.
        if inc_dir == None:
            return None
        inc_dirs[inc_dir] = None

    return [ctx.label.path.add(p) for p in inc_dirs]

def _as_raw_header(
        ctx: AnalysisContext,
        # The full name used to include the header.
        name: str,
        header: "artifact",
        # Return `None` instead of failing.
        no_fail: bool = False) -> [str, None]:
    """
    Return path to pass to `include_directories` to treat the given header as
    a raw header.
    """

    # We can't handle generated headers.
    if not header.is_source:
        if no_fail:
            return None
        fail("generated headers cannot be used as raw headers ({})"
            .format(header))

    # To include the header via its name using raw headers and include dirs,
    # it needs to be a suffix of its original path, and we'll strip the include
    # name to get the include dir used to include it.
    path = paths.join(ctx.label.package, header.short_path)
    base = paths.strip_suffix(path, name)
    if base == None:
        if no_fail:
            return None
        fail("header name must be a path suffix of the header path to be " +
             "used as a raw header ({} => {})".format(name, header))

    # If the include dir is underneath our package, then just relativize to find
    # out package-relative path.
    if len(base) > len(ctx.label.package):
        return paths.relativize(base, ctx.label.package)

    # Otherwise, this include dir needs to reference a parent dir.
    expect(ctx.label.package.startswith(base))
    num_parents = (
        len(ctx.label.package.split("/")) -
        (0 if not base else len(base.split("/")))
    )
    return "/".join([".."] * num_parents)

def _get_list_header_name(header: "artifact", naming: CxxHeadersNaming.type) -> str:
    if naming.value == "regular":
        return header.short_path
    elif naming.value == "apple":
        return header.basename
    else:
        fail("Unsupported header naming: {}".format(naming))

def _get_dict_header_namespace(namespace: str, naming: CxxHeadersNaming.type) -> str:
    if naming.value == "regular":
        return namespace
    elif naming.value == "apple":
        return ""
    else:
        fail("Unsupported header naming: {}".format(naming))

def _get_debug_prefix_args(ctx: AnalysisContext, header_dir: "artifact") -> [cmd_args.type, None]:
    # NOTE(@christylee): Do we need to enable debug-prefix-map for darwin and windows?
    if get_cxx_toolchain_info(ctx).linker_info.type != "gnu":
        return None

    debug_prefix_args = cmd_args()
    fmt = "-fdebug-prefix-map={}=" + value_or(header_dir.owner.cell, ".")
    debug_prefix_args.add(
        cmd_args(header_dir, format = fmt),
    )
    return debug_prefix_args

def _mk_hmap(ctx: AnalysisContext, name: str, headers: dict[str, ("artifact", str)], absolute_path_prefix: [str, None]) -> "artifact":
    output = ctx.actions.declare_output(name + ".hmap")
    cmd = cmd_args(get_cxx_toolchain_info(ctx).mk_hmap)
    cmd.add(["--output", output.as_output()])

    header_args = cmd_args()
    for n, (path, fmt) in headers.items():
        header_args.add(n)

        # We don't care about the header contents -- just their names.
        header_args.add(cmd_args(path, format = fmt).ignore_artifacts())

    hmap_args_file = ctx.actions.write(output.basename + ".argsfile", cmd_args(header_args, quote = "shell"))
    cmd.add(["--mappings-file", hmap_args_file]).hidden(header_args)
    if absolute_path_prefix:
        cmd.add(["--absolute-path-prefix", absolute_path_prefix])
    ctx.actions.run(cmd, category = "generate_hmap", identifier = name)
    return output
