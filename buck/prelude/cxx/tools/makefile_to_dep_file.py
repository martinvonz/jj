#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# pyre-unsafe

import os
import subprocess
import sys

import dep_file_utils


def rewrite_dep_file(src_path, dst_path):
    """
    Convert a makefile to a depfile suitable for use by Buck2. The files we
    rewrite look like P488268797.
    """

    with open(src_path) as f:
        body = f.read()

    parts = body.split(": ", 1)
    body = parts[1] if len(parts) == 2 else ""

    # Escaped newlines are not meaningful so remove them.
    body = body.replace("\\\n", "")

    # Now, recover targets. They are space separated, but we need to ignore
    # spaces that are escaped.
    pos = 0

    deps = []
    current_parts = []

    def push_slice(s):
        if s:
            current_parts.append(s)

    def flush_current_dep():
        if current_parts:
            deps.append("".join(current_parts))
            current_parts.clear()

    while True:
        next_pos = body.find(" ", pos)

        # If we find the same character we started at, this means we started on
        # a piece of whitespace. We know this cannot be escaped, because if we
        # started here that means we stopped at the previous character, which
        # means it must have been whitespace as well.
        if next_pos == pos:
            flush_current_dep()
            pos += 1
            continue

        # No more whitespace, so this means that whatever is left from our
        # current position to the end is the last dependency (assuming there is
        # anything).
        if next_pos < 0:
            push_slice(body[pos:-1])
            break

        # Check if this was escaped by looking at the previous character. If it
        # was, then insert the part before the escape, and then push a space.
        # If it wasn't, then we've reached the end of a dependency.
        if next_pos > 0 and body[next_pos - 1] == "\\":
            push_slice(body[pos : next_pos - 1])
            push_slice(" ")
        else:
            push_slice(body[pos:next_pos])
            flush_current_dep()

        pos = next_pos + 1

    flush_current_dep()

    # Now that we've parsed deps, we need to normalize them.
    dep_file_utils.normalize_and_write_deps(deps, dst_path)


def process_dep_file(args):
    """
    Expects the src dep file to be the first argument, dst dep file to be the
    second argument, and the command to follow.
    """
    ret = subprocess.call(args[2:])
    if ret == 0:
        rewrite_dep_file(args[0], args[1])
    sys.exit(ret)
