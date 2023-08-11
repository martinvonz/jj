# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

def _cxx_hacks_impl(_ctx):
    return [DefaultInfo(), TemplatePlaceholderInfo(
        unkeyed_variables = {
            "cxx-header-tree": "/dev/null/HACK-CXX-HEADER-TREE",
        },
    )]

cxx_hacks = rule(
    impl = _cxx_hacks_impl,
    attrs = {},
)
