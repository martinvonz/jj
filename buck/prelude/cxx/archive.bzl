# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//linking:link_info.bzl", "Archive")
load("@prelude//utils:utils.bzl", "value_or")
load(":cxx_context.bzl", "get_cxx_toolchain_info")

def _archive_flags(
        archiver_type: str,
        linker_type: str,
        use_archiver_flags: bool,
        thin: bool) -> list[str]:
    if not use_archiver_flags:
        return []

    if archiver_type == "windows":
        if thin:
            fail("'windows' archiver doesn't support thin archives")
        return ["/Brepro", "/d2threads1"]
    elif archiver_type == "windows_clang":
        return ["/llvmlibthin"] if thin else []
    flags = ""

    # Operate in quick append mode, so that objects with identical basenames
    # won't overwrite one another.
    flags += "q"

    # Suppress warning about creating a new archive.
    flags += "c"

    # Run ranlib to generate symbol index for faster linking.
    flags += "s"

    # Generate thin archives.
    if thin:
        flags += "T"

    # GNU archivers support generating deterministic archives.
    if linker_type == "gnu":
        flags += "D"

    return [flags]

# Create a static library from a list of object files.
def _archive(ctx: AnalysisContext, name: str, args: cmd_args, thin: bool, prefer_local: bool) -> "artifact":
    archive_output = ctx.actions.declare_output(name)
    toolchain = get_cxx_toolchain_info(ctx)
    command = cmd_args(toolchain.linker_info.archiver)
    archiver_type = toolchain.linker_info.archiver_type
    command.add(_archive_flags(
        archiver_type,
        toolchain.linker_info.type,
        toolchain.linker_info.use_archiver_flags,
        thin,
    ))
    if archiver_type == "windows" or archiver_type == "windows_clang":
        command.add([cmd_args(archive_output.as_output(), format = "/OUT:{}")])
    else:
        command.add([archive_output.as_output()])

    if toolchain.linker_info.archiver_supports_argfiles:
        shell_quoted_args = cmd_args(args, quote = "shell")
        if toolchain.linker_info.use_archiver_flags and toolchain.linker_info.archiver_flags != None:
            shell_quoted_args.add(toolchain.linker_info.archiver_flags)
        argfile, _ = ctx.actions.write(name + ".argsfile", shell_quoted_args, allow_args = True)
        command.hidden([shell_quoted_args])
        command.add(cmd_args(["@", argfile], delimiter = ""))
    else:
        command.add(args)

    category = "archive"
    if thin:
        category = "archive_thin"
    ctx.actions.run(command, category = category, identifier = name, prefer_local = prefer_local)
    return archive_output

def _archive_locally(ctx: AnalysisContext, linker_info: "LinkerInfo") -> bool:
    archive_locally = linker_info.archive_objects_locally
    if hasattr(ctx.attrs, "_archive_objects_locally_override"):
        return value_or(ctx.attrs._archive_objects_locally_override, archive_locally)
    return archive_locally

# Creates a static library given a list of object files.
def make_archive(
        ctx: AnalysisContext,
        name: str,
        objects: list["artifact"],
        args: [cmd_args, None] = None) -> Archive.type:
    if len(objects) == 0:
        fail("no objects to archive")

    if args == None:
        args = cmd_args(objects)

    linker_info = get_cxx_toolchain_info(ctx).linker_info
    thin = linker_info.archive_contents == "thin"
    archive = _archive(ctx, name, args, thin = thin, prefer_local = _archive_locally(ctx, linker_info))

    # TODO(T110378125): use argsfiles for GNU archiver for long lists of objects.
    # TODO(T110378123): for BSD archiver, split long args over multiple invocations.
    # TODO(T110378100): We need to scrub the static library (timestamps, permissions, etc) as those are
    # sources of non-determinism. See `ObjectFileScrubbers.createDateUidGidScrubber()` in Buck v1.

    return Archive(artifact = archive, external_objects = objects if thin else [])
