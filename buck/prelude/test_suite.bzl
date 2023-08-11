# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

def test_suite_impl(ctx: AnalysisContext) -> list["provider"]:
    # There is nothing to implement here: test_suite exists as a mechanism to "group" tests using
    # the `tests` attribute, and the `tests` attribute is supported for all rules.
    # We fill in the DefaultInfo so building the test_suite will build all the underlying test rules.
    test_targets = []
    other_outputs = []
    for test_dep in ctx.attrs.test_deps:
        test_targets.append(test_dep.label.raw_target())
        default_info = test_dep[DefaultInfo]
        other_outputs.extend(default_info.default_outputs)
        other_outputs.extend(default_info.other_outputs)
    return [DefaultInfo(default_outputs = [ctx.actions.write("test_targets.txt", test_targets)], other_outputs = other_outputs)]
