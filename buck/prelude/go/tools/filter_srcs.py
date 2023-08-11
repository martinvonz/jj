#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

"""
Run on a directory of Go source files and print out a list of srcs that should
be compiled.

Example:

 $ ./filter_srcs.py --output srcs.txt src/dir/

"""

# pyre-unsafe

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--go", default="go", type=Path)
    parser.add_argument("--tests", action="store_true")
    parser.add_argument("--tags", default="")
    parser.add_argument("--output", type=argparse.FileType("w"), default=sys.stdout)
    parser.add_argument("srcdir", type=Path)
    args = parser.parse_args(argv[1:])

    # Find all source sub-dirs, which we'll need to run `go list` from.
    roots = set()
    for root, _dirs, _files in os.walk(args.srcdir):
        roots.add(root)

    # Run `go list` on all source dirs to filter input sources by build pragmas.
    for root in roots:
        out = subprocess.check_output(
            [
                "env",
                "-i",
                "GOARCH={}".format(os.environ.get("GOARCH", "")),
                "GOOS={}".format(os.environ.get("GOOS", "")),
                "CGO_ENABLED={}".format(os.environ.get("CGO_ENABLED", "0")),
                "GO111MODULE=off",
                "GOCACHE=/tmp",
                args.go.resolve(),
                "list",
                "-e",
                "-json",
                "-tags",
                args.tags,
                ".",
            ],
            cwd=root,
        ).decode("utf-8")

        # Parse JSON output and print out sources.
        idx = 0
        decoder = json.JSONDecoder()
        while idx < len(out) - 1:
            # The raw_decode method fails if there are any leading spaces, e.g. " {}" fails
            # so manually trim the prefix of the string
            if out[idx].isspace():
                idx += 1
                continue

            obj, idx = decoder.raw_decode(out, idx)
            types = ["GoFiles", "EmbedFiles"]
            if args.tests:
                types.extend(["TestGoFiles", "XTestGoFiles"])
            else:
                types.extend(["SFiles"])
            for typ in types:
                for src in obj.get(typ, []):
                    src = Path(obj["Dir"]) / src
                    src = src.relative_to(os.getcwd())
                    print(src, file=args.output)

    args.output.close()


sys.exit(main(sys.argv))
