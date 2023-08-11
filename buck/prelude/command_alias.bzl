# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//os_lookup:defs.bzl", "OsLookup")

def command_alias_impl(ctx):
    target_is_windows = ctx.attrs._target_os_type[OsLookup].platform == "windows"
    exec_is_windows = ctx.attrs._exec_os_type[OsLookup].platform == "windows"

    if target_is_windows:
        # If the target is Windows, create a batch file based command wrapper instead
        return _command_alias_impl_target_windows(ctx, exec_is_windows)
    else:
        return _command_alias_impl_target_unix(ctx, exec_is_windows)

def _command_alias_impl_target_unix(ctx, exec_is_windows: bool):
    if ctx.attrs.exe == None:
        base = RunInfo()
    else:
        base = _get_run_info_from_exe(ctx.attrs.exe)

    run_info_args = cmd_args()

    if len(ctx.attrs.env) > 0 or len(ctx.attrs.platform_exe.items()) > 0:
        trampoline_args = cmd_args()
        trampoline_args.add("#!/usr/bin/env bash")
        trampoline_args.add("set -euo pipefail")
        trampoline_args.add('BUCK_COMMAND_ALIAS_ABSOLUTE=$(cd -- "$(dirname "$0")" >/dev/null 2>&1 ; pwd -P)')

        for (k, v) in ctx.attrs.env.items():
            # TODO(akozhevnikov): maybe check environment variable is not conflicting with pre-existing one
            trampoline_args.add(cmd_args(["export ", k, "=", cmd_args(v, quote = "shell")], delimiter = ""))

        if len(ctx.attrs.platform_exe.items()) > 0:
            trampoline_args.add('case "$(uname)" in')
            for platform, exe in ctx.attrs.platform_exe.items():
                # Only linux and macos are supported.
                if platform == "linux":
                    _add_platform_case_to_trampoline_args(trampoline_args, "Linux", _get_run_info_from_exe(exe), ctx.attrs.args)
                elif platform == "macos":
                    _add_platform_case_to_trampoline_args(trampoline_args, "Darwin", _get_run_info_from_exe(exe), ctx.attrs.args)

            # Default case
            _add_platform_case_to_trampoline_args(trampoline_args, "*", base, ctx.attrs.args)
            trampoline_args.add("esac")
        else:
            _add_args_declaration_to_trampoline_args(trampoline_args, base, ctx.attrs.args)

        trampoline_args.add('exec "${ARGS[@]}"')

        trampoline = _relativize_path(
            ctx,
            trampoline_args,
            "sh",
            "$BUCK_COMMAND_ALIAS_ABSOLUTE",
            exec_is_windows,
        )

        run_info_args.add(trampoline)
        run_info_args.hidden([trampoline_args])
    else:
        run_info_args.add(base.args)
        run_info_args.add(ctx.attrs.args)

    run_info_args.hidden(ctx.attrs.resources)

    # TODO(cjhopman): Consider what this should have for default outputs. Using
    # the base's default outputs may not really be correct (it makes more sense to
    # be the outputs required by the args).
    return [
        DefaultInfo(),
        RunInfo(args = run_info_args),
    ]

def _command_alias_impl_target_windows(ctx, exec_is_windows: bool):
    # If a windows specific exe is specified, take that. Otherwise just use the default exe.
    windows_exe = ctx.attrs.platform_exe.get("windows")
    if windows_exe != None:
        base = _get_run_info_from_exe(windows_exe)
    elif ctx.attrs.exe != None:
        base = _get_run_info_from_exe(ctx.attrs.exe)
    else:
        base = RunInfo()

    run_info_args = cmd_args()
    if len(ctx.attrs.env) > 0:
        trampoline_args = cmd_args()
        trampoline_args.add("@echo off")

        # Set BUCK_COMMAND_ALIAS_ABSOLUTE to the drive and full path of the script being created here
        # We use this below to prefix any artifacts being referenced in the script
        trampoline_args.add("set BUCK_COMMAND_ALIAS_ABSOLUTE=%~dp0")

        # Handle envs
        for (k, v) in ctx.attrs.env.items():
            # TODO(akozhevnikov): maybe check environment variable is not conflicting with pre-existing one
            trampoline_args.add(cmd_args(["set ", k, "=", v], delimiter = ""))

        # Handle args
        # We shell quote the args but not the base. This is due to the same limitation detailed below with T111687922
        cmd = cmd_args([base.args], delimiter = " ")
        for arg in ctx.attrs.args:
            cmd.add(cmd_args(arg, quote = "shell"))

        # Add on %* to handle any other args passed through the command
        cmd.add("%*")
        trampoline_args.add(cmd)

        trampoline = _relativize_path(
            ctx,
            trampoline_args,
            "bat",
            "%BUCK_COMMAND_ALIAS_ABSOLUTE%",
            exec_is_windows,
        )
        run_info_args.add(trampoline)
        run_info_args.hidden([trampoline_args])
    else:
        run_info_args.add(base.args)
        run_info_args.add(ctx.attrs.args)

    run_info_args.hidden(ctx.attrs.resources)

    # TODO(cjhopman): Consider what this should have for default outputs. Using
    # the base's default outputs may not really be correct (it makes more sense to
    # be the outputs required by the args).
    return [
        DefaultInfo(),
        RunInfo(args = run_info_args),
    ]

def _relativize_path(
        ctx,
        trampoline_args: cmd_args,
        extension: str,
        var: str,
        exec_is_windows: bool) -> "artifact":
    # Depending on where this action is done, we need to either run sed or a custom Windows sed-equivalent script
    # TODO(marwhal): Bias the exec platform to be the same as target platform to simplify the relativization logic
    if exec_is_windows:
        return _relativize_path_windows(ctx, extension, var, trampoline_args)
    else:
        return _relativize_path_unix(ctx, extension, var, trampoline_args)

def _relativize_path_unix(
        ctx,
        extension: str,
        var: str,
        trampoline_args: cmd_args) -> "artifact":
    # FIXME(ndmitchell): more straightforward relativization with better API
    non_materialized_reference = ctx.actions.write("dummy", "")
    trampoline_args.relative_to(non_materialized_reference, parent = 1).absolute_prefix("__BUCK_COMMAND_ALIAS_ABSOLUTE__/")

    trampoline_tmp, _ = ctx.actions.write("__command_alias_trampoline.{}.pre".format(extension), trampoline_args, allow_args = True)

    # FIXME (T111687922): Avert your eyes... We want to add
    # $BUCK_COMMAND_ALIAS_ABSOLUTE a prefix on all the args we include, but
    # those args will be shell-quoted (so that they might include e.g.
    # spaces). However, our shell-quoting will apply to the absolute_prefix
    # as well, which will render it inoperable. To fix this, we emit
    # __BUCK_COMMAND_ALIAS_ABSOLUTE__ instead, and then we use sed to work
    # around our own quoting to produce the thing we want.
    trampoline = ctx.actions.declare_output("__command_alias_trampoline.{}".format(extension))
    ctx.actions.run([
        "sh",
        "-c",
        "sed 's|__BUCK_COMMAND_ALIAS_ABSOLUTE__|{}|g' < \"$1\" > \"$2\" && chmod +x $2".format(var),
        "--",
        trampoline_tmp,
        trampoline.as_output(),
    ], category = "sed")

    return trampoline

def _relativize_path_windows(
        ctx,
        extension: str,
        var: str,
        trampoline_args: cmd_args) -> "artifact":
    # FIXME(ndmitchell): more straightforward relativization with better API
    non_materialized_reference = ctx.actions.write("dummy", "")
    trampoline_args.relative_to(non_materialized_reference, parent = 1).absolute_prefix(var + "/")

    trampoline, _ = ctx.actions.write("__command_alias_trampoline.{}".format(extension), trampoline_args, allow_args = True)

    return trampoline

def _add_platform_case_to_trampoline_args(trampoline_args: cmd_args, platform_name: str, base: RunInfo.type, args: list["_arglike"]):
    trampoline_args.add("    {})".format(platform_name))
    _add_args_declaration_to_trampoline_args(trampoline_args, base, args)
    trampoline_args.add("        ;;")

def _add_args_declaration_to_trampoline_args(trampoline_args: cmd_args, base: RunInfo.type, args: list["_arglike"]):
    trampoline_args.add("ARGS=(")

    # FIXME (T111687922): We cannot preserve BUCK_COMMAND_ALIAS_ABSOLUTE *and*
    # quote here...  So we don't quote the exe's RunInfo (which usually has a
    # path and hopefully no args that need quoting), but we quote the args
    # themselves (which usually are literals that might need quoting and
    # hopefully doesn't contain relative paths).
    # FIXME (T111687922): Note that we have no shot at quoting base.args anyway
    # at this time, since we need to quote the individual words, but using
    # `quote = "shell"` would just quote the whole thing into one big word.
    trampoline_args.add(base.args)
    for arg in args:
        trampoline_args.add(cmd_args(arg, quote = "shell"))

    # Add the args passed to the command_alias itself.
    trampoline_args.add('"$@"')

    trampoline_args.add(")")

def _get_run_info_from_exe(exe: Dependency) -> RunInfo.type:
    run_info = exe.get(RunInfo)
    if run_info == None:
        run_info = RunInfo(
            args = exe[DefaultInfo].default_outputs,
        )

    return run_info
