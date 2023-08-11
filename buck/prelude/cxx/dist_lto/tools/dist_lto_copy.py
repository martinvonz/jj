#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import argparse
import shutil
import sys
from typing import List


def main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--to")
    parser.add_argument("--from", dest="from_")
    args = parser.parse_args(argv[1:])
    shutil.copy(args.from_, args.to)
    return 0


sys.exit(main(sys.argv))
