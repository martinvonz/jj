#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# If https://github.com/rust-lang/rust/pull/113780 is accepted, this wrapper can
# go away. The `rule` in the bzl code should directly run rustc.
#
#     cmd_args(
#         toolchain_info.compiler,
#         cmd_args("--print=cfg=", out.as_output(), delimiter = ""),
#         cmd_args("--target=", toolchain_info.rustc_target_triple, delimiter = ""),
#     )
#
# Alternatively if `ctx.actions.run` learns to redirect stdout. Something like:
#
#     ctx.actions.run(
#         cmd_args(toolchain_info.compiler, ...),
#         stdout = out.as_output(),
#     )
#
# or:
#
#     subprocess = ctx.actions.run(
#         cmd_args(toolchain_info.compiler, ...),
#     )
#     return [DefaultInfo(default_output = subprocess.stdout)]


import argparse
import subprocess
import sys
from typing import IO, NamedTuple


class Args(NamedTuple):
    rustc: str
    target: str
    out: IO[str]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--rustc", type=str, required=True)
    parser.add_argument("--target", type=str, required=True)
    parser.add_argument("--out", type=argparse.FileType("w"), required=True)
    args = Args(**vars(parser.parse_args()))

    subprocess.run(
        [args.rustc, "--print=cfg", f"--target={args.target}"],
        stdout=args.out,
        stderr=sys.stderr,
        encoding="utf-8",
        check=True,
    )


if __name__ == "__main__":
    main()
