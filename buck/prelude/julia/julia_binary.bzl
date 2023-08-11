# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//linking:shared_libraries.bzl", "merge_shared_libraries", "traverse_shared_library_info")
load("@prelude//utils:utils.bzl", "flatten")
load(":julia_info.bzl", "JuliaLibraryInfo", "JuliaLibraryTSet", "JuliaToolchainInfo")

def write_overrides_file(ctx: AnalysisContext):
    """Compiles a JSON file containing all required info for jlls.

    We need to create a JSON file that contains all the relevant jlls, all the
    shlibs that that particular jll needs to link to, and the solib locations.
    Julia requires an absolute path. We get around this by establishing the
    paths of all the libraries relative to the current JSON file, and then pull
    the absolute path of the JSON file during runtime with a python script.

    We populate the JSON file with the following structure:
    [
       ("first_jll", "uuid",
          [
             ("julia_foo_solib_name", symlink_dir_location, shlib_label),
             ("julia_bar_solib_name", symlink_dir_location, shlib_label),
          ]
       )
      ... etc
    ]
    """
    json_info_file = ctx.actions.declare_output("artifacts/Overrides.json")

    # build a tree for the jlls
    deps = filter(None, [dep.get(JuliaLibraryInfo) for dep in ctx.attrs.deps])
    julia_library_info = ctx.actions.tset(
        JuliaLibraryTSet,
        children = [j.julia_tsets for j in deps],
    )

    # build a tree for the c/c++ shlibs
    shlibs = traverse_shared_library_info(merge_shared_libraries(
        ctx.actions,
        None,
        filter(None, [d.shared_library_info for d in deps]),
    ))

    shared_libs_symlink_tree = ctx.actions.symlinked_dir(
        "__shared_libs_symlink_tree__",
        {name: shlib.lib.output for name, shlib in shlibs.items()},
    )

    shlib_label_to_soname = {shlib.label: name for name, shlib in shlibs.items()}

    # iterate through all the jll libraries
    json_info = []
    for jli in julia_library_info.traverse():
        jll = jli.jll
        if jll == None:
            continue

        # iterate through all the shlib dependencies for the current jll
        artifact_info = []
        for julia_name, label in jll.libs.items():
            symlink_dir = cmd_args(shared_libs_symlink_tree, delimiter = "")
            symlink_dir.relative_to(json_info_file)  # That cannot be produced by a tset projection
            artifact_info.append((julia_name, symlink_dir, shlib_label_to_soname[label]))
        json_info.append((jll.name, jli.uuid, artifact_info))

    return ctx.actions.write_json(json_info_file, json_info, with_inputs = True)

def build_load_path_symtree(ctx: AnalysisContext):
    """Builds symtree of all julia library files."""
    dep_julia_library_infos = filter(None, [dep.get(JuliaLibraryInfo) for dep in ctx.attrs.deps])

    julia_library_info = ctx.actions.tset(
        JuliaLibraryTSet,
        children = [j.julia_tsets for j in dep_julia_library_infos],
    )
    traversed = list(julia_library_info.traverse())
    src_labels = flatten([t.src_labels for t in traversed])
    srcs = flatten([t.srcs for t in traversed])

    dict_from_tree = {
        k: p
        for k, p in zip(src_labels, srcs)
    }
    symlink_dir = ctx.actions.symlinked_dir("_modules_", dict_from_tree)

    return symlink_dir

def build_julia_command(ctx):
    """
    run a command of the form:
    $ julia -flag_1 -flag_2 -- my_script.jl arg1 arg2

    https://docs.julialang.org/en/v1/manual/command-line-options/
    """
    symlink_dir = build_load_path_symtree(ctx)
    json_info_file = write_overrides_file(ctx)

    julia_toolchain = ctx.attrs._julia_toolchain[JuliaToolchainInfo]

    # python processor
    cmd = cmd_args(julia_toolchain.cmd_processor)

    # toolchain env variables
    if len(julia_toolchain.env) > 0:
        cmd.add("--env")

        # We need to not only separate, by prepend our commands with our
        # delimiter to "trick" argparse into allowing arguments containing "-"
        # or "--" (this is mostly a problem on RE).
        joined_args = '";;{}"'.format(";;".join(julia_toolchain.env))
        cmd.add(joined_args)

    # library load path
    cmd.add("--lib-path")
    cmd.add(symlink_dir)

    # json path
    cmd.add("--json-path")
    cmd.add(json_info_file)

    # julia binary
    cmd.add("--julia-binary")
    cmd.add(julia_toolchain.julia)

    # add julia flags
    if len(ctx.attrs.julia_flags) > 0:
        cmd.add("--julia-flags")
        joined_args = '\";;{}\"'.format(";;".join(ctx.attrs.julia_flags))
        cmd.add(joined_args)

    # build symdir for sources
    srcs_by_path = {f.short_path: f for f in ctx.attrs.srcs}
    srcs = ctx.actions.symlinked_dir("srcs_tree", srcs_by_path)
    if ctx.attrs.main not in srcs_by_path:
        fail("main should be in srcs!")

    # add the main source file
    cmd.add("--main")
    cmd.add(srcs.project(ctx.attrs.main))

    # add the command arguments
    if len(ctx.attrs.julia_args) > 0:
        cmd.add("--main-args")
        joined_args = '";;{}"'.format(";;".join(ctx.attrs.julia_args))
        cmd.add(joined_args)

    # add all relevant source files
    cmd.hidden(srcs)
    cmd.hidden(symlink_dir)  # julia lib srcs

    return cmd

def julia_binary_impl(ctx: AnalysisContext) -> list["provider"]:
    cmd = build_julia_command(ctx)
    return [DefaultInfo(), RunInfo(cmd)]
