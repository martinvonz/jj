#!/usr/bin/env fbpython
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import argparse
import itertools
import json
import os
from typing import Dict, List, Tuple, TypedDict

# Example VFS overlay in JSON format
# ----------------------------------
# {
#   'version': 0,
#   'roots': [
#     { 'name': 'OUT_DIR', 'type': 'directory',
#       'contents': [
#         { 'name': 'module.map', 'type': 'file',
#           'external-contents': 'INPUT_DIR/actual_module2.map'
#         }
#       ]
#     }
#   ]
# }


class OverlayRoot(TypedDict):
    name: str
    type: str
    contents: List[Dict[str, str]]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output", required=True, help="The path to write the VFS overlay to"
    )
    parser.add_argument(
        "mappings", nargs="*", default=[], help="A list of virtual paths to real paths"
    )
    args = parser.parse_args()

    if len(args.mappings) % 2 != 0:
        parser.error("mappings must be dest-source pairs")

    # Group the mappings by containing directory
    mappings: Dict[str, List[Tuple[str, str]]] = {}
    for src, dst in itertools.zip_longest(*([iter(args.mappings)] * 2)):
        folder, basename = os.path.split(src)
        mappings.setdefault(folder, []).append((basename, dst))

    with open(args.output, "w") as f:
        json.dump(
            {
                "version": 0,
                "roots": _get_roots(mappings),
            },
            f,
            sort_keys=True,
            indent=4,
        )
        f.write("\n")
        f.flush()


def _get_roots(mappings: Dict[str, List[Tuple[str, str]]]) -> List[OverlayRoot]:
    roots = []
    for folder, file_maps in mappings.items():
        contents = []
        for src, dst in file_maps:
            contents.append(
                {
                    "name": src,
                    "type": "file",
                    "external-contents": dst,
                }
            )

        roots.append(
            {
                "name": folder,
                "type": "directory",
                "contents": contents,
            }
        )

    return roots


if __name__ == "__main__":
    main()
