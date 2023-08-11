# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import argparse
import json
import os
import sys
import zipfile


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output", "-o", type=argparse.FileType("w"), default=sys.stdin
    )
    parser.add_argument("jar")
    parser.add_argument("sources", nargs="*")
    args = parser.parse_args(argv[1:])

    sources = {}
    for src in args.sources:
        path = src
        base, ext = os.path.splitext(src)
        sources[base] = path

    classes = []

    with zipfile.ZipFile(args.jar) as zf:
        for ent in zf.namelist():
            base, ext = os.path.splitext(ent)

            # Ignore non-.class files.
            if ext != ".class":
                continue

            classname = base.replace("/", ".")

            # Make sure it is a .class file that corresponds to a top-level
            # .class file and not an inner class.
            if "$" in base:
                continue

            for src_base, src_path in sources.items():
                if base == src_base or src_base.endswith("/" + base):
                    classes.append(
                        {
                            "className": classname,
                            "srcPath": src_path,
                        }
                    )
                    break

    json.dump(
        {
            "jarPath": args.jar,
            "classes": classes,
        },
        args.output,
    )
    print("", file=args.output)


sys.exit(main(sys.argv))
