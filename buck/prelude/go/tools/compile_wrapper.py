#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

"""
Compile the given Go sources into a Go package.

Example:

 $ ./compile_wrapper.py \
       --compiler compile \
       --assembler assemble \
       --output srcs.txt src/dir/

"""

# pyre-unsafe

import argparse
import contextlib
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import List


def _call_or_exit(cmd: List[str]):
    ret = subprocess.call(cmd)
    if ret != 0:
        sys.exit(ret)


def _compile(compile_prefix: List[str], output: Path, srcs: List[Path]):
    cmd = []
    cmd.extend(compile_prefix)
    cmd.append("-trimpath={}".format(os.getcwd()))
    cmd.append("-o")
    cmd.append(output)
    cmd.extend(srcs)
    _call_or_exit(cmd)


def _pack(pack_prefix: List[str], output: Path, items: List[Path]):
    cmd = []
    cmd.extend(pack_prefix)
    cmd.append("r")
    cmd.append(output)
    cmd.extend(items)
    _call_or_exit(cmd)


def main(argv):
    parser = argparse.ArgumentParser(fromfile_prefix_chars="@")
    parser.add_argument("--compiler", action="append", default=[])
    parser.add_argument("--assembler", action="append", default=[])
    parser.add_argument("--packer", action="append", default=[])
    parser.add_argument("--embedcfg", type=Path, default=None)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("srcs", type=Path, nargs="*")
    args = parser.parse_args(argv[1:])

    # If there's no srcs, just leave an empty file.
    if not args.srcs:
        args.output.touch()
        return

    # go:embed does not parse symlinks, so following the links to the real paths
    real_srcs = [s.resolve() for s in args.srcs]

    go_files = [s for s in real_srcs if s.suffix == ".go"]
    s_files = [s for s in real_srcs if s.suffix == ".s"]
    o_files = [s for s in real_srcs if s.suffix == ".o"]

    with contextlib.ExitStack() as stack:

        asmhdr_dir = None

        assemble_prefix = []
        assemble_prefix.extend(args.assembler)

        if go_files:
            compile_prefix = []
            compile_prefix.extend(args.compiler)

            # If we have assembly files, generate the symabi file to compile
            # against, and the asm header.
            if s_files:
                asmhdr_dir = stack.push(tempfile.TemporaryDirectory())

                asmhdr = Path(asmhdr_dir.name) / "go_asm.h"
                asmhdr.touch()
                compile_prefix.extend(["-asmhdr", asmhdr])
                assemble_prefix.extend(["-I", asmhdr_dir.name])
                assemble_prefix.extend(["-D", f"GOOS_{os.environ['GOOS']}"])
                assemble_prefix.extend(["-D", f"GOARCH_{os.environ['GOARCH']}"])
                if "GOAMD64" in os.environ and os.environ["GOARCH"] == "amd64":
                    assemble_prefix.extend(["-D", f"GOAMD64_{os.environ['GOAMD64']}"])

                # Note: at this point go_asm.h is empty, but that's OK. As per the Go compiler:
                # https://github.com/golang/go/blob/3f8f929d60a90c4e4e2b07c8d1972166c1a783b1/src/cmd/go/internal/work/gc.go#L441-L443
                symabis = args.output.with_suffix(".symabis")
                _compile(assemble_prefix + ["-gensymabis"], symabis, s_files)
                compile_prefix.extend(["-symabis", symabis])

            if args.embedcfg is not None:
                compile_prefix.extend(
                    [
                        "-embedcfg",
                        args.embedcfg,
                    ]
                )

            # This will create go_asm.h
            _compile(compile_prefix, args.output, go_files)

        else:
            args.output.touch()

        # If there are assembly files, assemble them to an object and add into the
        # output archive.
        if s_files:
            s_object = args.output.with_suffix(".o")
            _compile(assemble_prefix, s_object, s_files)
            o_files.append(s_object)

        if o_files:
            _pack(args.packer, args.output, o_files)


sys.exit(main(sys.argv))
