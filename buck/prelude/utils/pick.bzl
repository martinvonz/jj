# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

def pick(override, underlying):
    return cmd_args(override) if override != None else underlying

def pick_bin(override, underlying):
    return override[RunInfo] if override != None else underlying

def pick_dep(override, underlying):
    return override if override != None else underlying

def pick_and_add(override, additional, underlying):
    flags = cmd_args(pick(override, underlying))
    if additional:
        flags.add(additional)
    return flags
