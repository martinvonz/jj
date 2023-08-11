# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load("@prelude//os_lookup:defs.bzl", "OsLookup")

def _derive_link(artifact):
    if artifact.is_source:
        return artifact.short_path

    # TODO(cjhopman): Reject cross-repo resources. Buck1 does that. It's probably
    # easier for us (compared to v1) to construct a scheme for them that is
    # correct, but not necessary yet.

    return paths.join(artifact.owner.package, artifact.owner.name)

def _generate_script(name: str, main: "artifact", resources: list["artifact"], actions: "actions", is_windows: bool) -> ("artifact", "artifact"):
    main_path = main.short_path
    if is_windows:
        main_link = main_path if main_path.endswith(".bat") or main_path.endswith(".cmd") else main_path + ".bat"
    else:
        main_link = main_path if main_path.endswith(".sh") else main_path + ".sh"
    resources = {_derive_link(src): src for src in resources}
    resources[main_link] = main

    resources_dir = actions.symlinked_dir("resources", resources)

    script_name = name + (".bat" if is_windows else "")
    script = actions.declare_output(script_name)

    # This is much, much simpler than the buck1 sh_binary template. A couple reasons:
    # 1. we don't invoke the script through a symlink and so don't need to use and implement a cross-platform `readlink -e`
    # 2. we don't construct an invocation-specific sandbox. The implementation of
    # that in buck1 is pretty crazy and it shouldn't actually be necessary.
    # 3. we don't construct the cell symlinks. those were also strange. They were
    # used for the links in the invocation-specific sandbox (so things would
    # point through the cell symlinks to their original locations). Instead we
    # construct links directly to things (which buck1 actually also did for its
    # BUCK_DEFAULT_RUNTIME_RESOURCES).
    if not is_windows:
        script_content = cmd_args([
            "#!/usr/bin/env bash",
            "set -e",
            # This is awkward for two reasons: args doesn't support format strings
            # and will insert a newline between items and so __RESOURCES_ROOT
            # is put in a bash array, and we want it to be relative to script's
            # dir, not the script itself, but there's no way to do that in
            # starlark.  To deal with this, we strip the first 3 characters
            # (`../`).
            "__RESOURCES_ROOT=(",
            resources_dir,
            ")",
            # If we access this sh_binary via a unhashed symlink we need to
            # update the relative source.
            '__SRC="${BASH_SOURCE[0]}"',
            '__SRC="$(realpath "$__SRC")"',
            '__SCRIPT_DIR=$(dirname "$__SRC")',
            # The format of the directory tree is different in v1 and v2. We
            # should unify the two, but prior to doing this we should also
            # identify what the right format is. For now, this variable lets
            # callees disambiguate (see D28960177 for more context).
            "export BUCK_SH_BINARY_VERSION_UNSTABLE=2",
            "export BUCK_PROJECT_ROOT=$__SCRIPT_DIR/\"${__RESOURCES_ROOT:3}\"",
            # In buck1, the paths for resources that are outputs of rules have
            # different paths in BUCK_PROJECT_ROOT and
            # BUCK_DEFAULT_RUNTIME_RESOURCES, but we use the same paths. buck1's
            # BUCK_PROJECT_ROOT paths would use the actual buck-out path rather
            # than something derived from the target and so to use that people
            # would need to hardcode buck-out paths into their scripts. For repo
            # sources, the paths are the same for both.
            "export BUCK_DEFAULT_RUNTIME_RESOURCES=\"$BUCK_PROJECT_ROOT\"",
            "exec \"$BUCK_PROJECT_ROOT/{}\" \"$@\"".format(main_link),
        ]).relative_to(script)
    else:
        script_content = cmd_args([
            "@echo off",
            "setlocal EnableDelayedExpansion",
            "set __RESOURCES_ROOT=^",
            resources_dir,
            # Fully qualified script path.
            "set __SRC=%~f0",
            # This is essentially a realpath.
            'for /f "tokens=2 delims=[]" %%a in (\'dir %__SRC% ^|%SYSTEMROOT%\\System32\\find.exe "<SYMLINK>"\') do set "__SRC=%%a"',
            # Get parent folder.
            'for %%a in ("%__SRC%") do set "__SCRIPT_DIR=%%~dpa"',
            "set BUCK_SH_BINARY_VERSION_UNSTABLE=2",
            # ':~3' strips the first 3 chars of __RESOURCES_ROOT.
            "set BUCK_PROJECT_ROOT=%__SCRIPT_DIR%\\!__RESOURCES_ROOT:~3!",
            "set BUCK_DEFAULT_RUNTIME_RESOURCES=%BUCK_PROJECT_ROOT%",
            "%BUCK_PROJECT_ROOT%\\{} %*".format(main_link),
        ]).relative_to(script)
    actions.write(
        script,
        script_content,
        is_executable = True,
    )

    return (script, resources_dir)

# Attrs:
# "deps": attrs.list(attrs.dep(), default = []),
# "main": attrs.source(),
# "resources": attrs.list(attrs.source(), default = []),
def sh_binary_impl(ctx):
    # TODO: implement deps (not sure what those even do, though)
    if len(ctx.attrs.deps) > 0:
        fail("sh_binary deps unsupported. Got `{}`".format(repr(ctx.attrs)))

    is_windows = ctx.attrs._target_os_type[OsLookup].platform == "windows"
    (script, resources_dir) = _generate_script(ctx.label.name, ctx.attrs.main, ctx.attrs.resources, ctx.actions, is_windows)

    return [
        DefaultInfo(default_output = script, other_outputs = [resources_dir]),
        RunInfo(
            # TODO(cjhopman): Figure out if we need to specify the link targets
            # as inputs. We shouldn't need to, but need to verify it.
            args = cmd_args(script).hidden(resources_dir),
        ),
    ]
