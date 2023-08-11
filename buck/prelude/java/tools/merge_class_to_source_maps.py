# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import argparse
import json
import os
import pathlib
import sys


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output", "-o", type=argparse.FileType("w"), default=sys.stdin
    )
    parser.add_argument(
        "--relative-to",
    )
    parser.add_argument("--mappings", "-m", type=pathlib.Path, required=True)
    args = parser.parse_args(argv[1:])

    with open(args.mappings) as f:
        mappings = [line.replace("\n", "") for line in f.readlines()]
        for mapping in mappings:
            with open(mapping) as f:
                obj = json.load(f)

            if args.relative_to is not None:
                obj = {
                    "jarPath": os.path.relpath(obj["jarPath"], args.relative_to),
                    "classes": [
                        {
                            "className": c["className"],
                            "srcPath": os.path.relpath(c["srcPath"], args.relative_to),
                        }
                        for c in obj["classes"]
                    ],
                }

            json.dump(obj, args.output)
            print("", file=args.output)


sys.exit(main(sys.argv))
