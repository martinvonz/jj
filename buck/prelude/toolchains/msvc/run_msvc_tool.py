#!/usr/bin/env python3
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import json
import os
import subprocess
import sys
from typing import List, NamedTuple


class Tool(NamedTuple):
    # Path of the executable
    exe: str
    # Paths to prepend onto $LIB
    LIB: List[str]
    # Paths to prepend onto $PATH
    PATH: List[str]
    # Paths to prepend onto $INCLUDE
    INCLUDE: List[str]


def prepend_env(env, key, entries):
    entries = ";".join(entries)
    if key in env:
        env[key] = entries + ";" + env[key]
    else:
        env[key] = entries


def main():
    tool_json, arguments = sys.argv[1], sys.argv[2:]
    with open(tool_json, encoding="utf-8") as f:
        tool = Tool(**json.load(f))

    env = os.environ.copy()
    prepend_env(env, "LIB", tool.LIB)
    prepend_env(env, "PATH", tool.PATH)
    prepend_env(env, "INCLUDE", tool.INCLUDE)

    completed_process = subprocess.run([tool.exe, *arguments], env=env)
    sys.exit(completed_process.returncode)


if __name__ == "__main__":
    main()
