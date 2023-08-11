# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import sys

from makefile_to_dep_file import process_dep_file
from show_headers_to_dep_file import process_show_headers_dep_file
from show_includes_to_dep_file import process_show_includes_dep_file


def main():
    """
    Determines which dep file processor to use according to compiler type.

    First argument is
    dep_tracking_mode which is one of these values [ makefile | show_includes | show_headers | none].

    For each mode we expect different arguments:

    makefile -> intermediary_dep_file, dep_file, cmd_args
    show_includes -> input_file, dep_file, cmd_args
    show_headers -> input_file, dep_file, cmd_args

    Where:

    input_file -> Path to file we're generating the dep file for
    intermediary_dep_file -> File where compiler output is generated to
    dep_file -> Expected output path
    cmd_args -> Args to be executed in order to get dependencies
    """

    dep_tracking_mode = sys.argv[1]
    if dep_tracking_mode == "show_headers":
        process_show_headers_dep_file(sys.argv[3], sys.argv[4:], sys.argv[2])
    elif dep_tracking_mode == "show_includes":
        process_show_includes_dep_file(sys.argv[3], sys.argv[4:], sys.argv[2])
    else:
        process_dep_file(sys.argv[2:])


if __name__ == "__main__":
    main()
