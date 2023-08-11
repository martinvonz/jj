#!/usr/bin/env bash
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

PYTHONPATH="$(dirname "$(dirname "$(dirname "$(realpath "$0")")")")"
export PYTHONPATH
exec python3 -m unittest sourcedb_merger.tests
