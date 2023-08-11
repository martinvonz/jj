#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

"""
Creates a Python Source DB JSON file containing both a rule's immediate sources
and the sources of all transitive dependencies (e.g. for use with Pyre).

Sources and dependencies are passed in via source manifest files, which are
merged by this script:

$ ./make_source_db.py \
      --sources my_rule.manifest.json \
      --dependency dep1.manifest.json \
      --dependency dep2.manifest.json

The output format of the source DB is:

{
  "sources": {
    <source1-name>: <source1-path>,
    <source2-name>: <source2-path>,
    ...
  },
  "dependencies": {
    <dep-source1-name>: <dep-source1-path>,
    <dep-source2-name>: <dep-source2-path>,
    ...
  },
}
"""

# pyre-unsafe

import argparse
import json
import sys


def _load(path):
    with open(path) as f:
        return json.load(f)


def main(argv):
    parser = argparse.ArgumentParser(fromfile_prefix_chars="@")
    parser.add_argument("--output", type=argparse.FileType("w"), default=sys.stdout)
    parser.add_argument("--sources")
    parser.add_argument("--dependency", action="append", default=[])
    args = parser.parse_args(argv[1:])

    db = {}

    # Add sources.
    sources = {}
    if args.sources is not None:
        for name, path, _ in _load(args.sources):
            sources[name] = path
    db["sources"] = sources

    # Add dependencies.
    dependencies = {}
    for dep in args.dependency:
        for name, path, origin in _load(dep):
            prev = dependencies.get(name)
            if prev is not None and prev[0] != path:
                raise Exception(
                    "Duplicate entries for {}: {} ({}) and {} ({})".format(
                        name, path, origin, *prev
                    ),
                )
            dependencies[name] = path, origin
    db["dependencies"] = {n: p for n, (p, _) in dependencies.items()}

    # Write db out.
    json.dump(db, args.output, indent=2)
    args.output.close()


sys.exit(main(sys.argv))
