#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Implement a "failure filter" - that is, look at the output of a previous
# action and see if it failed with respect to downstream actions which need its
# outputs. This is to allow us to report success if rustc generated the artifact
# we needed (ie diagnostics) even if the compilation itself failed.

import argparse
import json
import os
import shutil
import sys
from typing import IO, List, NamedTuple, Optional, Tuple


class Args(NamedTuple):
    build_status: IO[str]
    required_file: Optional[List[Tuple[str, str, str]]]
    stderr: Optional[IO[str]]


def arg_parse() -> Args:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--build-status",
        type=argparse.FileType(),
        required=True,
    )
    parser.add_argument(
        "--required-file",
        action="append",
        nargs=3,
        metavar=("SHORT", "INPUT", "OUTPUT"),
    )
    parser.add_argument(
        "--stderr",
        type=argparse.FileType(),
    )

    return Args(**vars(parser.parse_args()))


def main() -> int:
    args = arg_parse()

    if args.stderr:
        stderr = args.stderr.read()
        sys.stderr.write(stderr)

    build_status = json.load(args.build_status)

    # Copy all required files to output, and fail with the original exit status
    # if any are missing. (Ideally we could just do the copy by referring to the
    # same underlying CAS object, which would avoid having to move the actual
    # bytes around at all.)
    if args.required_file:
        for short, inp, out in args.required_file:
            if short in build_status["files"]:
                try:
                    # Try a hard link to avoid unnecessary copies
                    os.link(inp, out)
                except OSError:
                    # Fall back to real copy if that doesn't work
                    shutil.copy(inp, out)
            else:
                print(
                    f"Missing required input file {short} ({inp})",
                    file=sys.stderr,
                )
                return build_status["status"]

    # If all the required files were present, then success regardless of
    # original status.
    return 0


sys.exit(main())
