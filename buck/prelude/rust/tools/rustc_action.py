#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Wrapper for rustc (or similar, like rustdoc). This wrapper does a few pieces
# of post-processing on the json-formatted diagnostics:
# - (preprocessing) resolve env vars referring to paths to absolute paths
# - write the rendered form to a text diagnostic output, and also to stderr
# - annotate unused crate messages with buck target info for downstream tooling,
#   and also generated a rendered version
# - generate a build status json when using failure filtering
#
# This is closely coupled to `_rustc_invoke` in `build.bzl`

import argparse
import asyncio
import json
import os
import platform
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Dict, IO, List, NamedTuple, Optional, Tuple

DEBUG = False


def eprint(*args: Any, **kwargs: Any) -> None:
    print(*args, end="\n", file=sys.stderr, flush=True, **kwargs)


if sys.version_info[:2] < (3, 7):
    eprint("Python 3.7 or newer is required!")
    eprint("Using {} from {}".format(platform.python_version(), sys.executable))
    sys.exit(1)


def key_value_arg(s: str) -> Tuple[str, str]:
    key_value = s.split("=", maxsplit=1)
    if len(key_value) == 2:
        return (key_value[0], key_value[1])
    raise argparse.ArgumentTypeError(f"expected the form `key=value` for `{s}`")


class Args(NamedTuple):
    diag_json: Optional[IO[bytes]]
    diag_txt: Optional[IO[bytes]]
    env: Optional[List[Tuple[str, str]]]
    path_env: Optional[List[Tuple[str, str]]]
    remap_cwd_prefix: Optional[str]
    crate_map: Optional[List[Tuple[str, str]]]
    buck_target: Optional[str]
    failure_filter: Optional[IO[bytes]]
    required_output: Optional[List[Tuple[str, str]]]
    rustc: List[str]


def arg_parse() -> Args:
    # Command line is <action.py> [args] -- rustc command line
    parser = argparse.ArgumentParser(fromfile_prefix_chars="@")
    parser.add_argument(
        "--diag-json",
        type=argparse.FileType("wb"),
        help="Json-formatted diagnostic output "
        "(assumes compiler is invoked with --error-format=json)",
    )
    parser.add_argument(
        "--diag-txt",
        type=argparse.FileType("wb"),
        help="Rendered text diagnostic output (also streamed to stderr)",
    )
    parser.add_argument(
        "--env",
        action="append",
        type=key_value_arg,
        metavar="NAME=VALUE",
        help="Set environment",
    )
    parser.add_argument(
        "--path-env",
        action="append",
        type=key_value_arg,
        metavar="NAME=PATH",
        help="Set path environment (to be made absolute)",
    )
    parser.add_argument(
        "--remap-cwd-prefix",
        help="Remap paths under the current working directory to this path prefix",
    )
    parser.add_argument(
        "--crate-map",
        action="append",
        type=key_value_arg,
        metavar="CRATE=TARGET",
        help="Crate name to target map for unused crate diagnostics",
    )
    parser.add_argument(
        "--buck-target",
        help="Buck target for crate, used for unused crate diagnostics",
    )
    parser.add_argument(
        "--failure-filter",
        type=argparse.FileType("wb"),
        help="Consider a failure as success so long as we got some usable diagnostics",
        metavar="build-status.json",
    )
    parser.add_argument(
        "--required-output",
        action="append",
        nargs=2,
        metavar=("SHORT", "PATH"),
        help="Required output path we expect rustc to generate "
        "(and filled with a placeholder on a filtered failure)",
    )
    parser.add_argument(
        "rustc",
        nargs=argparse.REMAINDER,
        type=str,
        help="Compiler command line",
    )

    return Args(**vars(parser.parse_args()))


async def handle_output(  # noqa: C901
    proc: asyncio.subprocess.Process,
    args: Args,
    crate_map: Dict[str, str],
) -> bool:
    got_error_diag = False

    proc_stderr = proc.stderr
    assert proc_stderr is not None

    while True:
        line = await proc_stderr.readline()

        if line is None or line == b"":
            break

        try:
            diag = json.loads(line)
        except json.JSONDecodeError:
            sys.stderr.buffer.write(line + b"\n")  # Passthrough
            continue

        if DEBUG:
            print(f"diag={repr(diag)}", end="\n")

        if "unused_extern_names" in diag:
            unused_names = diag["unused_extern_names"]

            # Empty unused_extern_names is just noise.
            # This happens when there are no unused crates.
            if not unused_names:
                continue

            # Treat error-level unused dep warnings as errors
            if diag.get("lint_level") in ("deny", "forbid"):
                got_error_diag = True

            # Add more information to unused crate warnings.
            if args.buck_target:
                rendered_unused = []
                for name in unused_names:
                    if name in crate_map:
                        rendered_unused.append("{}: {}".format(crate_map[name], name))
                    else:
                        rendered_unused.append("{}".format(name))
                rendered_unused.sort()
                rendered_unused = "\n    ".join(rendered_unused)

                diag["buck_target"] = args.buck_target
                diag["rendered"] = (
                    f"Target `{args.buck_target}` has unused dependencies:\n"
                    f"    {rendered_unused}"
                )
            diag["unused_deps"] = {
                name: crate_map[name] for name in unused_names if name in crate_map
            }
        else:
            if diag.get("level") == "error":
                got_error_diag = True

        # Emit json
        if args.diag_json:
            args.diag_json.write(
                json.dumps(diag, separators=(",", ":")).encode() + b"\n"
            )

        # Emit rendered text version
        if "rendered" in diag:
            rendered = diag["rendered"].encode() + b"\n"
            if args.diag_txt:
                args.diag_txt.write(rendered)
            sys.stderr.buffer.write(rendered)

    if args.diag_json:
        args.diag_json.close()
    if args.diag_txt:
        args.diag_txt.close()

    return got_error_diag


async def main() -> int:
    args = arg_parse()

    # Inherit a very limited initial environment, then add the new things
    env = {
        k: os.environ[k]
        for k in [
            "RUSTC_LOG",
            "RUST_BACKTRACE",
            "PATH",
            "PWD",
            "HOME",
            "TMPDIR",
            # Required on Windows
            "LOCALAPPDATA",
            "PROGRAMDATA",
            "TEMP",
            # TODO(andirauter): Required by RE. Remove them when no longer required T119466023
            "EXECUTION_ID",
            "SESSION_ID",
            "ACTION_DIGEST",
            "RE_PLATFORM",
            "CAS_DAEMON_PORT",
            "CAS_DAEMON_ADDR",
            # Required by Dotslash, which is how the Rust toolchain is shipped
            # on Mac.
            "USER",
            "DOTSLASH_CACHE",
            # Required to run Python on Windows (for linker wrapper).
            "SYSTEMROOT",
            # Our rustc wrapper. https://fburl.com/code/qcos5aho
            "SYSROOT_MULTIPLEXER_DEBUG",
            # Required on Windows for getpass.getuser() to work.
            "USERNAME",
        ]
        if k in os.environ
    }
    if args.env:
        # Unescape previously escaped newlines.
        # Example: \\\\n\\n -> \\\n\n -> \\n\n
        env.update(
            {k: v.replace("\\n", "\n").replace("\\\n", "\\n") for k, v in args.env}
        )
    if args.path_env:
        env.update({k: str(Path(v).resolve()) for k, v in args.path_env})

    crate_map = dict(args.crate_map) if args.crate_map else {}

    if DEBUG:
        print(f"args {repr(args)} env {env} crate_map {crate_map}", end="\n")

    rustc_cmd = args.rustc[:1]
    rustc_args = args.rustc[1:]

    if args.remap_cwd_prefix is not None:
        rustc_args.append(
            "--remap-path-prefix={}={}".format(os.getcwd(), args.remap_cwd_prefix)
        )
        rustc_args.append(
            "--remap-path-prefix={}={}".format(
                os.path.realpath(os.getcwd()), args.remap_cwd_prefix
            )
        )

    with tempfile.NamedTemporaryFile(
        mode="wb",
        prefix="rustc-args-",
        suffix=".txt",
        delete=False,
    ) as args_file:
        args_file.write("\n".join(rustc_args).encode() + b"\n")
        args_file.flush()
        # Kick off the action
        proc = await asyncio.create_subprocess_exec(
            *rustc_cmd,
            "@" + args_file.name,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=None,  # Inherit
            stderr=subprocess.PIPE,
            limit=1_000_000,
        )
        got_error_diag = await handle_output(proc, args, crate_map)
        res = await proc.wait()

    if DEBUG:
        print(
            f"res={repr(res)} "
            f"got_error_diag={got_error_diag} "
            f"args.failure_filter {args.failure_filter}",
            end="\n",
        )

    # If rustc is reporting a silent error, make it loud
    if res == 0 and got_error_diag:
        res = 1

    # Check for death by signal - this is always considered a failure
    if res < 0:
        cmdline = " ".join(shlex.quote(arg) for arg in args.rustc)
        eprint(f"Command exited with signal {-res}: command line: {cmdline}")
    elif args.failure_filter:
        # If failure filtering is enabled, then getting an error diagnostic is also
        # considered a success. That is, if rustc exited with an error status, but
        # we saw a fatal error diagnostic, we can still report a zero exit status.
        # This still means we had an error if someone wanted one of the other output
        # artifacts as an input, but the failure filter action will handle that by
        # examining the build_status json output.

        required_output = args.required_output or []

        # We need to create a build status output, including the list of output
        # files which were *actuallyq* created. We use the short paths as the
        # logical filename rather than the actual full path, since that won't
        # mean much to a downstream action.
        build_status = {
            "status": res,
            "files": [short for short, path in required_output if Path(path).exists()],
        }
        args.failure_filter.write(
            json.dumps(build_status, separators=(",", ":")).encode() + b"\n"
        )

        # OK to actually report success, but keep buck happy by making sure all
        # the required outputs are present
        if got_error_diag and res != 0:
            for _short, path in required_output:
                path = Path(path)
                if not path.exists():
                    path.touch()
            res = 0

    return res


sys.exit(asyncio.run(main()))
