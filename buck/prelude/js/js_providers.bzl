# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

def _artifacts(value: "artifact"):
    return value

TransitiveOutputsTSet = transitive_set(args_projections = {"artifacts": _artifacts})

JsLibraryInfo = provider(
    fields = [
        "output",  # "artifact"
        "transitive_outputs",  # ["TransitiveOutputsTSet", None]
    ],
)

JsBundleInfo = provider(
    fields = [
        "bundle_name",  # str
        # Directory containing the built JavaScript.
        "built_js",  # "artifact",
        # Source map belonging to the built JavaScript.
        "source_map",  # "artifact",
        # Directory containing the resources (or assets) used by the bundled JavaScript source code.
        "res",  # ["artifact", None]
        # Directory containing various metadata that can be used by dependent rules but are not
        # meant to be shipped with the application.
        "misc",  # "artifact"
        # Dependencies graph file associated with the built JavaScript.
        "dependencies_file",  # "artifact"
    ],
)

def get_transitive_outputs(
        actions: "actions",
        value: ["artifact", None] = None,
        deps: list[JsLibraryInfo.type] = []) -> TransitiveOutputsTSet.type:
    kwargs = {}
    if value:
        kwargs["value"] = value
    if deps:
        kwargs["children"] = filter(None, [js_library_info.transitive_outputs for js_library_info in deps])

    return actions.tset(TransitiveOutputsTSet, **kwargs)
