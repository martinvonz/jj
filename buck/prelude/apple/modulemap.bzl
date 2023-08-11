# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//apple:apple_toolchain_types.bzl", "AppleToolsInfo")
load(
    "@prelude//cxx:headers.bzl",
    "CHeader",  # @unused Used as a type
)
load(
    "@prelude//cxx:preprocessor.bzl",
    "CPreprocessor",
    "CPreprocessorArgs",
)
load(":apple_utility.bzl", "get_module_name")

def preprocessor_info_for_modulemap(ctx: AnalysisContext, name: str, headers: list[CHeader.type], swift_header: ["artifact", None]) -> CPreprocessor.type:
    # We don't want to name this module.modulemap to avoid implicit importing
    if name == "module":
        fail("Don't use the name `module` for modulemaps, this will allow for implicit importing.")

    module_name = get_module_name(ctx)

    # Create a map of header import path to artifact location
    header_map = {}
    for h in headers:
        if h.namespace:
            header_map["{}/{}".format(h.namespace, h.name)] = h.artifact
        else:
            header_map[h.name] = h.artifact

    # We need to include the Swift header in the symlink tree too
    swift_header_name = "{}/{}-Swift.h".format(module_name, module_name)
    if swift_header:
        header_map[swift_header_name] = swift_header

    # Create a symlink dir for the headers to import
    symlink_tree = ctx.actions.symlinked_dir(name + "_symlink_tree", header_map)

    # Create a modulemap at the root of that tree
    output = ctx.actions.declare_output(name + ".modulemap")
    cmd = cmd_args(ctx.attrs._apple_tools[AppleToolsInfo].make_modulemap)
    cmd.add([
        "--output",
        output.as_output(),
        "--name",
        get_module_name(ctx),
        "--symlink-tree",
        symlink_tree,
    ])

    if swift_header:
        cmd.add([
            "--swift-header",
            swift_header,
        ])

    if ctx.attrs.use_submodules:
        cmd.add("--use-submodules")

    for hdr in sorted(header_map.keys()):
        # Don't include the Swift header in the mappings, this is handled separately.
        if hdr != swift_header_name:
            cmd.add(hdr)

    ctx.actions.run(cmd, category = "modulemap", identifier = name)

    return CPreprocessor(
        relative_args = CPreprocessorArgs(args = _exported_preprocessor_args(symlink_tree)),
        absolute_args = CPreprocessorArgs(args = _exported_preprocessor_args(symlink_tree)),
        modular_args = _args_for_modulemap(output, symlink_tree, swift_header),
        modulemap_path = cmd_args(output).hidden(cmd_args(symlink_tree)),
    )

def _args_for_modulemap(
        modulemap: "artifact",
        symlink_tree: "artifact",
        swift_header: ["artifact", None]) -> list[cmd_args]:
    cmd = cmd_args(modulemap, format = "-fmodule-map-file={}")
    cmd.hidden(symlink_tree)
    if swift_header:
        cmd.hidden(swift_header)

    return [cmd]

def _exported_preprocessor_args(symlink_tree: "artifact") -> list[cmd_args]:
    return [cmd_args(symlink_tree, format = "-I./{}")]
