# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

def strip_prefix(prefix: str, s: str) -> [str, None]:
    """
    If the string `s` starts with `prefix`, return the result of stripping the
    latter from the former.  Return `None` otherwise.
    """

    if s.startswith(prefix):
        return s[len(prefix):]

    return None
