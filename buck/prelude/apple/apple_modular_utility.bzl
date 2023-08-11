# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# We use a fixed module cache location. This works around issues with
# multi-user setups with MobileOnDemand and allows us to share the
# module cache with Xcode, LLDB and arc focus.
#
# TODO(T123737676): This needs to be changed to use $TMPDIR in a
# wrapper for modular clang compilation.
MODULE_CACHE_PATH = "/tmp/buck-module-cache"
