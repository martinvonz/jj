#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import argparse
import re
import subprocess


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tar-flag", action="append", default=[])
    parser.add_argument("--tar-archive")
    parser.add_argument("--exclude", action="append")
    parser.add_argument("--out")
    args = parser.parse_args()

    exclusions = [re.compile(e) for e in args.exclude]
    files = subprocess.check_output(
        ["tar", "--list", "-f", args.tar_archive] + args.tar_flag, encoding="utf-8"
    )
    files = [f.strip() for f in files.split()]

    with open(args.out, "w", encoding="utf-8") as out:
        for f in files:
            if all(excl.match(f) is None for excl in exclusions):
                continue
            out.write("{}\n".format(f))


if __name__ == "__main__":
    main()
