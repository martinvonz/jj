#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

"""
Utility to create compilation DBs

$ make_comp_db.py gen --output=entry.json foo.cpp -- g++ -c -fPIC
$ make_comp_db.py gen --output=entry2.json foo2.cpp -- g++ -c -fPIC
$ make_comp_db.py merge --output=comp_db.json entry.json entry2.json
"""

# pyre-unsafe

import argparse
import json
import shlex
import sys


def gen(args):
    """
    Generate a single compilation command in JSON form.
    """

    entry = {}
    entry["file"] = args.directory + "/" + args.filename
    entry["directory"] = "."

    arguments = []
    for arg in args.arguments:
        if arg.startswith("@"):
            with open(arg[1:]) as argsfile:
                for line in argsfile:
                    # The argsfile's arguments are separated by newlines; we
                    # don't want those included in the argument list.
                    arguments.append(" ".join(shlex.split(line)))
        else:
            arguments.append(arg)
    entry["arguments"] = arguments

    json.dump(entry, args.output, indent=2)
    args.output.close()


def merge(args):
    """
    Merge multiple compilation DB commands into a single DB.
    """

    entries = []
    for entry in args.entries:
        with open(entry) as f:
            entries.append(json.load(f))

    json.dump(entries, args.output, indent=2)
    args.output.close()


def main(argv):
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers()

    parser_gen = subparsers.add_parser("gen")
    parser_gen.add_argument("--output", type=argparse.FileType("w"), default=sys.stdout)
    parser_gen.add_argument("filename")
    parser_gen.add_argument("directory")
    parser_gen.add_argument("arguments", nargs="*")
    parser_gen.set_defaults(func=gen)

    parser_merge = subparsers.add_parser("merge")
    parser_merge.add_argument(
        "--output", type=argparse.FileType("w"), default=sys.stdout
    )
    parser_merge.add_argument("entries", nargs="*")
    parser_merge.set_defaults(func=merge)

    args = parser.parse_args(argv[1:])
    args.func(args)


sys.exit(main(sys.argv))
