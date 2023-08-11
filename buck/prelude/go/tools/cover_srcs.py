#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

"""
Run `go cover` on non-`_test.go` input sources.
"""

# pyre-unsafe

import argparse
import hashlib
import subprocess
import sys
from pathlib import Path


def _var(pkg_name, src):
    return "Var_" + hashlib.md5(f"{pkg_name}::{src}".encode("utf-8")).hexdigest()


def main(argv):
    parser = argparse.ArgumentParser(fromfile_prefix_chars="@")
    parser.add_argument("--cover", type=Path, required=True)
    parser.add_argument("--pkg-name", type=str, required=True)
    parser.add_argument("--coverage-mode", type=str, required=True)
    parser.add_argument("--covered-srcs-dir", type=Path, required=True)
    parser.add_argument("--out-srcs-argsfile", type=Path, required=True)
    parser.add_argument("--coverage-var-argsfile", type=Path, required=True)
    parser.add_argument("srcs", nargs="*", type=Path)
    args = parser.parse_args(argv[1:])

    out_srcs = []
    coverage_vars = {}

    args.covered_srcs_dir.mkdir(parents=True)

    for src in args.srcs:
        if src.name.endswith("_test.go"):
            out_srcs.append(src)
        else:
            var = _var(args.pkg_name, src)
            covered_src = args.covered_srcs_dir / src
            covered_src.parent.mkdir(parents=True, exist_ok=True)
            subprocess.check_call(
                [
                    args.cover,
                    "-mode",
                    args.coverage_mode,
                    "-var",
                    var,
                    "-o",
                    covered_src,
                    src,
                ]
            )
            # we need just the source name for the --cover-pkgs argument
            coverage_vars[var] = src.name
            out_srcs.append(covered_src)

    with open(args.out_srcs_argsfile, mode="w") as f:
        for src in out_srcs:
            print(src, file=f)

    with open(args.coverage_var_argsfile, mode="w") as f:
        if coverage_vars:
            print("--cover-pkgs", file=f)
            print(
                "{}:{}".format(
                    args.pkg_name,
                    ",".join([f"{var}={name}" for var, name in coverage_vars.items()]),
                ),
                file=f,
            )


sys.exit(main(sys.argv))
