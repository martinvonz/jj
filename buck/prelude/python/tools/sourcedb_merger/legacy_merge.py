#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import argparse
import pathlib
import sys

from typing import Sequence

import inputs
import legacy_outputs


def run_merge(input_file: str, output_file: str) -> None:
    target_entries = inputs.load_targets_and_build_maps_from_path(input_file)
    merge_result = legacy_outputs.merge_partial_build_maps(target_entries)
    merge_result.write_json_file(pathlib.Path(output_file))


def main(argv: Sequence[str]) -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=str)
    parser.add_argument("-o", "--output", required=True, type=str)
    arguments = parser.parse_args(argv[1:])

    run_merge(arguments.input, arguments.output)


if __name__ == "__main__":
    main(sys.argv)
