# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load("@prelude//utils:utils.bzl", "expect", "from_named_set")

def rust_attr_resources(ctx: AnalysisContext) -> dict[str, ("artifact", list["_arglike"])]:
    """
    Return the resources provided by this rule, as a map of resource name to
    a tuple of the resource artifact and any "other" outputs exposed by it.
    """
    resources = {}

    for name, resource in from_named_set(ctx.attrs.resources).items():
        if type(resource) == "artifact":
            other = []
        else:
            info = resource[DefaultInfo]
            expect(
                len(info.default_outputs) == 1,
                "expected exactly one default output from {} ({})"
                    .format(resource, info.default_outputs),
            )
            [resource] = info.default_outputs
            other = info.other_outputs

        resources[paths.join(ctx.label.package, name)] = (resource, other)

    return resources
