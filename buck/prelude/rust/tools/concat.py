#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# A tool to concatenate strings, some of which may be from @files. ¯\_(ツ)_/¯
#
# Rustc's command line requires dependencies to be provided as:
#
#     --extern cratename=path/to/libcratename.rlib
#
# In Buck, sometimes the cratename is computed at build time, for example
# extracted from a Thrift file. Rustc's "@" support isn't sufficient for this
# because the following doesn't make sense:
#
#     --extern @filecontainingcrate=path/to/libcratename.rlib
#
# and the cratename isn't able to be its own argument:
#
#     --extern @filecontainingcrate =path/to/libcratename.rlib
#
# Instead we use Python to make a single file containing the dynamic cratename
# and the rlib filepath concatenated together.
#
#     concat.py --output $TMP -- @filecontainingcrate = path/to/libcratename.rlib
#
# then:
#
#     --extern @$TMP
#

import argparse
from typing import IO, List, NamedTuple


class Args(NamedTuple):
    output: IO[str]
    strings: List[str]


def main():
    parser = argparse.ArgumentParser(fromfile_prefix_chars="@")
    parser.add_argument("--output", type=argparse.FileType("w"))
    parser.add_argument("strings", nargs="*", type=str)
    args = Args(**vars(parser.parse_args()))

    args.output.write("".join(args.strings))


if __name__ == "__main__":
    main()
