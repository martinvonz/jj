#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

"""
Create the link tree for inplace Python binaries.

This does a few things:
    - Allows remapping of source files (via the srcs/dests arguments) and resources.
    - Merges extracted .whl files into the tree
    - Adds __init__.py where needed
    - Writes out a bootstrapper pex script that knows where this symlink tree is,
      and uses it, along with the provided entry point to run the python script.
      It does this by replacing a few special strings like <MODULES_DIR> and
      <MAIN_MODULE>

A full usage might be something like this:

$ cat srcs
srcs/foo.py
srcs/bar.py
third-party/baz.whl__extracted
$ cat dests
lib/foo.py
bar.py
.
$ ls third-party/baz.whl__extracted
baz/tp_foo.py
baz/tp_bar.py
$ cat template.in
(see prelude/python/run_inplace_lite.py.in)
$ ./make_pex_inplace.py  \\
    --template prelude/python/run_inplace.py.in \\
    --module-srcs=@srcs \\
    --module-dests=@dests \\
    # This is the symlink tree \\
    --modules-dir=bin__link-tree
$ find bin__link-tree
lib/__init__.py
lib/foo.py
bar.py
baz/tp_foo.py
baz/tp_bar.py
"""

import argparse
import errno
import json
import os
from pathlib import Path
from typing import Dict, Set, Tuple

# Suffixes which should trigger `__init__.py` additions.
# TODO(agallaher): This was coped from v1, but some things below probably
# don't need to be here (e.g. `.pyd`).
_MODULE_SUFFIXES = {
    ".dll",
    ".py",
    ".pyd",
    ".so",
}


def parse_args() -> argparse.Namespace:
    # TODO(nmj): Go back and verify all of the various flags that make_xar
    #                 takes, and standardize on that so that the calling convention
    #                 is the same regardless of the "make_X" binary that's used.
    parser = argparse.ArgumentParser(
        description=(
            "Create a python inplace binary, writing a symlink tree to a directory, "
            "and a bootstrapper pex file to file"
        ),
        fromfile_prefix_chars="@",
    )
    parser.add_argument(
        "--module-manifest",
        action="append",
        dest="module_manifests",
        default=[],
        help="A path to a JSON file with modules to be linked.",
    )
    parser.add_argument(
        "--resource-manifest",
        action="append",
        dest="resource_manifests",
        default=[],
        help="A path to a JSON file with resources to be linked.",
    )
    parser.add_argument(
        "--native-library-src",
        type=Path,
        dest="native_library_srcs",
        action="append",
        default=[],
        help="A list of native libraries to use",
    )
    parser.add_argument(
        "--native-library-dest",
        type=Path,
        dest="native_library_dests",
        action="append",
        default=[],
        help=(
            "A list of relative destination paths for each of the native "
            "libraries in --native-library-src"
        ),
    )
    parser.add_argument(
        "--dwp-src",
        type=Path,
        dest="dwp_srcs",
        action="append",
        default=[],
        help="A list of dwp for native libraries to use",
    )
    parser.add_argument(
        "--dwp-dest",
        type=Path,
        dest="dwp_dests",
        action="append",
        default=[],
        help=(
            "A list of relative destination paths for each of the dwp for native "
            "libraries in --dwp-src"
        ),
    )
    parser.add_argument(
        "--native-library-manifest",
        action="append",
        dest="native_library_manifests",
        default=[],
        help="A path to a JSON file with native libraries to be linked.",
    )
    parser.add_argument(
        "--modules-dir",
        required=True,
        type=Path,
        help="The link tree directory to write to",
    )

    return parser.parse_args()


def _same_pyc(src1: Tuple[Path, str], src2: Tuple[Path, str]) -> bool:
    """
    Given two paths to .pyc files, return True if they are the same.
    """
    # As of 3.7, .pyc files are deterministic and have a hash of the original source
    # file in their first 16 bytes. See https://peps.python.org/pep-0552/#specification
    src1_path, src1_origin = src1
    src2_path, src2_origin = src2
    try:
        total_size = os.path.getsize(src1_path)
        if total_size != os.path.getsize(src2_path):
            return False
        to_read = min(total_size, 16)
        buf_size = 4096
        with open(src1_path, mode="rb") as fa, open(src2_path, mode="rb") as fb:
            while to_read > 0:
                chunk_size = min(to_read, buf_size)
                if fa.read(chunk_size) != fb.read(chunk_size):
                    return False
                to_read -= chunk_size
    except FileNotFoundError:
        # pyc files might not be materialized yet; in these cases we fall back to comparing the origins,
        # which should be the path of the original source file
        return src1_origin == src2_origin
    return True


def add_path_mapping(
    path_mapping: Dict[Path, Tuple[str, str]],
    dirs_to_create: Set[Path],
    src: Path,
    new_dest: Path,
    origin: str = "unknown",
) -> None:
    """
    Add the mapping of a destination path into `path_mapping`, by getting the
    relative path to the source, and making sure that there are no
    collisions (and erroring in that case)
    """

    def format_src(src: str, origin: str) -> str:
        out = "`{}`".format(src)
        if origin is not None:
            out += " (from {})".format(origin)
        return out

    link_path = os.path.relpath(
        os.path.realpath(src), os.path.realpath(new_dest.parent)
    )
    if new_dest in path_mapping:
        prev, prev_origin = path_mapping[new_dest]
        if prev != link_path and not (
            new_dest.suffix == ".pyc"
            and _same_pyc(
                (src, origin), ((new_dest.parent / prev).resolve(), prev_origin)
            )
        ):
            raise ValueError(
                "Destination path `{}` specified at both {} and {} (`{}` before relativisation)".format(
                    new_dest,
                    format_src(link_path, origin),
                    format_src(prev, prev_origin),
                    src,
                )
            )
    path_mapping[new_dest] = (link_path, origin)
    dirs_to_create.add(new_dest.parent)


def _lexists(path: Path) -> bool:
    """
    Like `Path.exists()` but works on dangling. symlinks
    """

    try:
        path.lstat()
    except FileNotFoundError:
        return False
    except OSError as e:
        if e.errno == errno.ENOENT:
            return False
        raise
    return True


def create_modules_dir(args: argparse.Namespace) -> None:
    args.modules_dir.mkdir(parents=True, exist_ok=True)

    # Mapping of destination files -> the symlink target (e.g. "../foo")
    path_mapping: Dict[Path, Tuple[str, str]] = {}
    # Set of directories that need to be created in the link tree before
    # symlinking
    dirs_to_create: Set[Path] = set()
    # Set of __init__.py files that need to be created at the end of the
    # link tree building if they don't exist, so that python recognizes them
    # as modules.
    init_py_paths = set()

    # Link entries from manifests.
    for manifest in args.module_manifests:
        with open(manifest) as manifest_file:
            for dest, src, origin in json.load(manifest_file):
                dest = Path(dest)
                src = Path(src)

                # Add `__init__.py` files for all parent dirs (except the root).
                if dest.suffix in _MODULE_SUFFIXES:
                    package = dest.parent
                    while package != Path("") and package not in init_py_paths:
                        init_py_paths.add(package)
                        package = package.parent

                add_path_mapping(
                    path_mapping,
                    dirs_to_create,
                    src,
                    args.modules_dir / dest,
                    origin=origin,
                )

    for manifest in args.resource_manifests + args.native_library_manifests:
        with open(manifest) as manifest_file:
            for dest, src, origin in json.load(manifest_file):
                src = Path(src)
                add_path_mapping(
                    path_mapping,
                    dirs_to_create,
                    src,
                    args.modules_dir / dest,
                    origin=origin,
                )

    if args.native_library_srcs:
        for src, dest in zip(args.native_library_srcs, args.native_library_dests):
            new_dest = args.modules_dir / dest
            add_path_mapping(path_mapping, dirs_to_create, src, new_dest)

    if args.dwp_srcs:
        for src, dest in zip(args.dwp_srcs, args.dwp_dests):
            new_dest = args.modules_dir / dest
            add_path_mapping(path_mapping, dirs_to_create, src, new_dest)

    for d in dirs_to_create:
        d.mkdir(parents=True, exist_ok=True)

    for dest, (target, _origin) in path_mapping.items():
        try:
            os.symlink(target, dest)
        except OSError:
            if _lexists(dest):
                if os.path.islink(dest):
                    raise ValueError(
                        "{} already exists, and is linked to {}. Cannot link to {}".format(
                            dest, os.readlink(dest), target
                        )
                    )
                else:
                    raise ValueError(
                        "{} already exists. Cannot link to {}".format(dest, target)
                    )
            else:
                raise

    # Fill in __init__.py for sources that were provided by the user
    # These are filtered such that we only create this for sources specified
    # by the user; if a .whl fortgets an __init__.py file, that's their problem
    for init_py_dir in init_py_paths:
        init_py_path = args.modules_dir / init_py_dir / "__init__.py"
        # We still do this check because python insists on touching some read only
        # files and blows up sometimes.
        if not _lexists(init_py_path):
            init_py_path.touch(exist_ok=True)


def main() -> None:
    args = parse_args()
    create_modules_dir(args)


if __name__ == "__main__":
    main()
