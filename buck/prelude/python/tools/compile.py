# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

"""
Example usage:
$ cat inputs.manifest
[["foo.py", "input/foo.py", "//my_rule:foo"]]
$ compile.py --output=out-dir --bytecode-manifest=output.manifest --ignore-errors inputs.manifest
$ find out-dir -type f
out-dir/foo.pyc
"""

# pyre-unsafe

import argparse
import errno
import json
import os
import sys
from py_compile import compile, PycInvalidationMode


if sys.version_info[0] == 3:
    import importlib

    DEFAULT_FORMAT = importlib.util.cache_from_source("{pkg}/{name}.py")
else:
    DEFAULT_FORMAT = "{pkg}/{name}.pyc"


def get_py_path(module):
    return module.replace(".", os.sep) + ".py"


def get_pyc_path(module, fmt):
    try:
        package, name = module.rsplit(".", 1)
    except ValueError:
        package, name = "", module
    parts = fmt.split(os.sep)
    for idx in range(len(parts)):
        if parts[idx] == "{pkg}":
            parts[idx] = package.replace(".", os.sep)
        elif parts[idx].startswith("{name}"):
            parts[idx] = parts[idx].format(name=name)
    return os.path.join(*parts)


def _mkdirs(dirpath):
    try:
        os.makedirs(dirpath)
    except OSError as e:
        if e.errno != errno.EEXIST:
            raise


def main(argv):
    parser = argparse.ArgumentParser(fromfile_prefix_chars="@")
    parser.add_argument("-o", "--output", required=True)
    parser.add_argument(
        "--bytecode-manifest", required=True, type=argparse.FileType("w")
    )
    parser.add_argument("-f", "--format", default=DEFAULT_FORMAT)
    parser.add_argument(
        "--invalidation-mode",
        type=str,
        default=PycInvalidationMode.UNCHECKED_HASH.name,
        choices=[m.name for m in PycInvalidationMode],
    )
    parser.add_argument("manifests", nargs="*")
    args = parser.parse_args(argv[1:])
    invalidation_mode = PycInvalidationMode.__members__[args.invalidation_mode]
    bytecode_manifest = []

    _mkdirs(args.output)

    for manifest_path in args.manifests:
        with open(manifest_path) as mf:
            manifest = json.load(mf)
        for dst, src, _ in manifest:
            # This is going to try to turn a path into a Python module, so
            # reduce the scope for bugs in get_pyc_path by normalizing first.
            dst = os.path.normpath(dst)
            # We only care about python sources.
            base, ext = os.path.splitext(dst)
            if ext != ".py":
                continue
            module = base.replace(os.sep, ".")
            dest_pyc = get_pyc_path(module, args.format)
            pyc = os.path.join(args.output, dest_pyc)
            _mkdirs(os.path.dirname(pyc))
            compile(
                src,
                cfile=pyc,
                dfile=get_py_path(module),
                doraise=True,
                invalidation_mode=invalidation_mode,
            )
            bytecode_manifest.append((dest_pyc, pyc, src))
    json.dump(bytecode_manifest, args.bytecode_manifest, indent=2)


sys.exit(main(sys.argv))
