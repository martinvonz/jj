# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

_MIN_SDK_VERSION = 19
_MAX_SDK_VERSION = 33

def get_min_sdk_version_constraint_value_name(min_sdk: int) -> str:
    return "min_sdk_version_{}".format(min_sdk)

def get_min_sdk_version_range() -> range.type:
    return range(_MIN_SDK_VERSION, _MAX_SDK_VERSION)
