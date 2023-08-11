# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//linking:shared_libraries.bzl", "SharedLibraryInfo")
load(":julia_info.bzl", "JllInfo", "JuliaLibraryInfo", "create_julia_library_info")

def gather_dep_libraries(raw_deps: list[Dependency]):
    """
    Takes a list of raw dependencies, and partitions them into julia_library / shared library providers.
    Fails if a dependency is not one of these.
    """
    clean_libs = []
    for dep in raw_deps:
        if JuliaLibraryInfo in dep:
            clean_libs.append(dep[JuliaLibraryInfo])
        else:
            fail("Dependency {} is not a julia_library or julia_jll_library!".format(dep.label))
    return clean_libs

def strip_srcs_path(ctx: AnalysisContext) -> list[str]:
    """Strip the src path to include just the module folder.

    By default, the short path will list the path of the src file relative to
    cwd. If the module is far away, then all the nested folders are also
    included. For the symlink dir we create later, we want a directory of
    containing just the modules themselves, since the Julia package manager
    won't recursively traverse this directory for nested modules.

    To do this, we'll just look at the current project.toml file, and deduce the
    name of the module folder. Then we can strip all the other source locs.
    """
    toml_main = ctx.attrs.project_toml.short_path.split("/")[0:-2]
    toml_main_path = ""
    for p in toml_main:
        toml_main_path = toml_main_path + p + "/"

    # If the toml file is in the current directory, then we need to account for that.
    if toml_main_path == "":
        toml_main_path = "./"

    src_labels = [f.short_path.split(toml_main_path)[-1] for f in ctx.attrs.srcs]
    src_labels += [ctx.attrs.project_toml.short_path.split(toml_main_path)[-1]]
    return src_labels

def julia_library_impl(ctx: AnalysisContext) -> list["provider"]:
    """Creates rule for julia libraries.

    The library rule needs to do a few important things:

    (1) Build a tree of all the source files (like we do with the binary rule).
    (2) Append this tree's path to a list of paths that will later go inside the
        JULIA_LOAD_PATH env variable.
    (3) Traverse all the dependencies and build up the JULIA_LOAD_PATH from
        their dependencies (note that Julia packages should form a DAG... but we
        should still have a check to make sure we don't get caught in an infinite
        loop).
    (4) Properly link to any jll libraries that are dependencies.

    So long as we fill the JULIA_LOAD_PATH with all the package paths, then
    Julia's internal package manager will automatically detect a project.toml
    file with those package dependencies etc. In other words, if the path is
    complete, then the package manager should work "out of the box".
    """
    providers = [DefaultInfo()]

    clean_libs = gather_dep_libraries(ctx.attrs.deps)

    library_info = create_julia_library_info(
        actions = ctx.actions,
        label = ctx.label,
        project_toml = ctx.attrs.project_toml,
        src_labels = strip_srcs_path(ctx),
        srcs = ctx.attrs.srcs + [ctx.attrs.project_toml],
        deps = clean_libs,
    )

    providers.append(library_info)

    return providers

def julia_jll_library_impl(ctx: AnalysisContext) -> list["provider"]:
    """Creates rule for julia jll libraries.

    jll libraries are wrappers for c++ libraries. Normally, these libraries are
    packaged using BinaryBuilder.jl: https://docs.binarybuilder.org/stable/jll/

    By creating a separate rule for jll libraries, we can leverage Julia's
    internal package manager. Specifically, most packages that depend on C/C++
    libraries implicitly place a dependency on the jll wrapper _instead_ of the
    library itself. As such, we can preserve the entire pipeline _except_ for
    the jll itself, which we have to custom wrap _anyway_!

    Consequently, this rule should *only* depend on other C/C++ rules.
    """
    providers = [DefaultInfo()]

    shlibs = [lib[SharedLibraryInfo] for lib in ctx.attrs.lib_mapping.values()]
    jll_libs = {name: lib.label for name, lib in ctx.attrs.lib_mapping.items()}

    library_info = create_julia_library_info(
        actions = ctx.actions,
        label = ctx.label,
        uuid = ctx.attrs.uuid,
        jll = JllInfo(
            name = ctx.attrs.jll_name,
            libs = jll_libs,
        ),
        shlibs = shlibs,
    )

    providers.append(library_info)

    return providers
