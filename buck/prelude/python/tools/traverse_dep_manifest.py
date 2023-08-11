# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import argparse
import json

from collections import deque
from typing import Any, Dict, Set, Tuple

from py38stdlib import STDLIB_MODULES

__DEPS_KEY = "#deps"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--main")
    parser.add_argument("--outfile")
    parser.add_argument("--manifest", action="append")
    args = parser.parse_args()

    deps = {}
    all_deps = {}
    for manifest in args.manifest:
        with open(manifest, "r") as f:
            for dep_file, source in json.load(f):
                # get the fully qualified module name from the output path
                # e.g. foo/bar/baz.py -> foo.bar.baz
                module = source[:-3].replace("/", ".")
                all_deps[module] = source
                root_module = source.split("/")[0]
                if root_module in STDLIB_MODULES:
                    continue
                node = deps
                for name in module.split("."):
                    if name not in node:
                        node[name] = {}
                    node = node[name]
                node[__DEPS_KEY] = (dep_file, module)

    included = set(all_deps.keys())
    count, required, missing = ensure_deps(args.main, deps, all_deps)
    extra = included - required

    with open(args.outfile, "w") as out:
        report = {}
        report["all_modules_count"] = len(included)
        report["required_modules_count"] = len(required)
        report["extra_modules_count"] = len(extra)
        report["all_modules"] = sorted(included)
        report["required_modules"] = sorted(required)
        report["extra_modules"] = sorted(extra)
        out.write(json.dumps(report, indent=2))

    return 0


# pyre-ignore
def flatten_trie(trie: Dict[str, Any]):
    to_search = deque(trie.values())
    modules = []
    while to_search:
        node = to_search.pop()
        if __DEPS_KEY in node:
            modules.append(node[__DEPS_KEY][1])
        else:
            to_search.extend(node.values())
    return modules


def ensure_deps(
    module: str, deps: Dict[str, Any], all_deps: Dict[str, str]
) -> Tuple[int, Set[str], Set[str]]:
    required_modules = set()
    missing = set()
    visited = set()
    count = 0
    to_search = deque()
    to_search.append(module)
    while to_search:
        next_module = to_search.pop()
        count += 1
        if next_module in visited:
            continue
        visited.add(next_module)
        node = deps
        module_name_chunks = []
        for name in next_module.split("."):
            module_name_chunks.append(name)
            if name in node:
                node = node[name]
                if __DEPS_KEY in node:
                    # means we are already in the module level. The rest of the module are just symbol name.
                    deps_file = node[__DEPS_KEY][0]
                    with open(deps_file, "r") as f:
                        dep_info = json.load(f)
                    to_search.extend(dep_info["modules"])
                    required_modules.add(".".join(module_name_chunks))
                    break
            else:
                missing.add(next_module)
                break

        # reach the end of module name but still not a leaf node
        # we need all of the children from an import *
        else:
            to_search.extend(flatten_trie(node))

    return count, required_modules, missing


if __name__ == "__main__":
    main()
