#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import argparse
import sys
from typing import List


def main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=argparse.FileType("w"), default=sys.stdout)
    parser.add_argument("--extension", action="append", default=[])
    args = parser.parse_args(argv[1:])
    out_file = args.output

    externs = []
    table = [
        "std::unordered_map<std::string_view, pyinitfunc> _static_extension_info = {",
    ]
    i = 0
    for python_name in args.extension:
        module_name, pyinit_func = python_name.split(":")
        # Use of the 'asm' directive allows us to use symbol names that would otherwise be invalid in C
        # For example foo.bar/baz would be foo.bar$baz which is invalid as a c function name
        externs.append(f'PyMODINIT_FUNC dummy_name_{i}(void) asm ("{pyinit_func}");')
        table.append(f'  {{ "{module_name}", dummy_name_{i} }},')
        i += 1
    table.append("};")

    out_lines = (
        [
            '#include "Python.h"',
            '#include "import.h"',
            "#include <unordered_map>",
            "#include <string_view>",
            "typedef PyObject* (*pyinitfunc)();",
        ]
        + externs
        + table
    )

    for line in out_lines:
        print(line, file=out_file)
    out_file.close()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
