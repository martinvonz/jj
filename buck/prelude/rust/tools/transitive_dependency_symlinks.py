#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# A tool to produce a directory structure compatible with rustc's `-Ldependency`
# flag for dynamically named crates.
#
# Rustc needs all the .rlib files which are transitive dependencies of the crate
# being built. Importantly, the filename for each rlib must contain the correct
# crate name or else rustc won't find the file.
#
# Most crates have a crate name determined during analysis: value of the `crate`
# attribute, or derived using a simple heuristic from the rust_library target's
# `name` attribute. For these it's easy to give the rlib a correct filename up
# front and use buck's `ctx.actions.symlinked_dir` to collect them into a
# directory. These do not go through this tool.
#
# Crates that use `crate_dynamic` have a crate name computed at build time, for
# example by extracting the name from a .thrift file. Buck needs a filename at
# analysis time for all artifacts, so we name those rlib files using a
# provisional name and then this tool at build time will symlink them under the
# real crate name that rustc will recognize.
#
# Example:
#
#     transitive_dependency_symlinks.py \
#         --out-dir path/to/out \
#         --artifact path/to/cratename ../../libprovisional.rlib \
#         --artifact ...
#
# The tool reads the crate name from the file at "path/to/out". Suppose it's
# "thriftgenerated". It symlinks the given artifact as "0/libthriftgenerated.rlib"
# within the specified output directory. In the event of collisions, there might
# be multiple dirs created, just as we do for analysis-time named crates.

import argparse
from pathlib import Path
from typing import List, NamedTuple, Tuple


class Args(NamedTuple):
    out_dir: Path
    artifact: List[Tuple[Path, Path]]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--out-dir",
        type=Path,
        required=True,
    )
    parser.add_argument(
        "--artifact",
        action="append",
        nargs=2,
        type=Path,
        metavar=("CRATENAME", "ARTIFACT"),
        required=True,
    )
    args = Args(**vars(parser.parse_args()))

    args.out_dir.mkdir(exist_ok=True)

    # Add as many -Ldependency dirs as we need to avoid name conflicts
    deps_dirs = [{}]

    for crate_name, artifact in args.artifact:
        crate_name = crate_name.read_text().strip()
        original_filename = artifact.name
        new_filename = "lib{}-{}".format(
            crate_name,
            original_filename.rsplit("-", 1)[1],
        )
        if new_filename in deps_dirs[-1]:
            deps_dirs.append({})
        deps_dirs[-1][new_filename] = artifact

    flags = ""

    for idx, srcs in enumerate(deps_dirs):
        directory = args.out_dir.joinpath(str(idx))
        directory.mkdir()
        flags += "-Ldependency={}\n".format(directory)
        for filename, artifact in srcs.items():
            directory.joinpath(filename).symlink_to(artifact)

    args.out_dir.joinpath("dirs").write_text(flags)


if __name__ == "__main__":
    main()
