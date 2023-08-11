#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

"""
Generate a __manifest__.py module containing build metadata for a Python package.
"""

import argparse
import json
from pathlib import Path
from typing import Optional, Set


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__file__.__doc__,
        fromfile_prefix_chars="@",
    )
    parser.add_argument(
        "--module-manifest",
        help="A path to a JSON file with modules contained in the PEX.",
        action="append",
        dest="module_manifests",
        default=[],
    )
    parser.add_argument(
        "--manifest-entries",
        help="Path to a JSON file with build metadata entries.",
        type=Path,
        default=None,
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Output path for the generated module.",
        required=True,
    )
    return parser.parse_args()


def path_to_module(path: str) -> Optional[str]:
    if not path.endswith(".py"):
        return None
    return path[:-3].replace("/", ".")


def main() -> None:
    args = parse_args()
    output: Path = args.output
    if output.exists():
        raise ValueError(
            f"Output path '{output}' already exists, refusing to overwrite."
        )

    modules: Set[str] = set()
    for module_manifest_file in args.module_manifests:
        with open(module_manifest_file) as f:
            for pkg_path, *_ in json.load(f):
                modules.add(pkg_path)
                # Add artificial __init__.py files like in make_pex_modules.py
                for parent in Path(pkg_path).parents:
                    if parent == Path("") or parent == Path("."):
                        continue
                    modules.add(str(parent / "__init__.py"))
    entries = {}
    if args.manifest_entries:
        with open(args.manifest_entries) as f:
            entries = json.load(f)
    if not isinstance(entries, dict):
        raise ValueError(
            f"Manifest entries in {args.manifest_entries} aren't a dictionary"
        )
    if "modules" in entries:
        raise ValueError("'modules' can't be a key in manifest entries")
    entries["modules"] = sorted(filter(None, (path_to_module(m) for m in modules)))
    output.write_text(
        "\n".join((f"{key} = {repr(value)}" for key, value in entries.items()))
    )


if __name__ == "__main__":
    main()
