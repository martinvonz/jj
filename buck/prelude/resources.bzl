# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Resources for transitive deps, shared by C++ and Rust.
ResourceInfo = provider(fields = [
    # A map containing all resources from transitive dependencies.  The keys
    # are rule labels and the values are maps of resource names (the name used
    # to lookup the resource at runtime) and the actual resource artifact.
    "resources",  # {"label": {str: ("artifact", ["hidden"])}}
])

def gather_resources(
        label: Label,
        resources: dict[str, ("artifact", list["_arglike"])] = {},
        deps: list[Dependency] = []) -> dict[Label, dict[str, ("artifact", list["_arglike"])]]:
    """
    Return the resources for this rule and its transitive deps.
    """

    all_resources = {}

    # Resources for self.
    if resources:
        all_resources[label] = resources

    # Merge in resources for deps.
    for dep in deps:
        if ResourceInfo in dep:
            all_resources.update(dep[ResourceInfo].resources)

    return all_resources

def create_resource_db(
        ctx: AnalysisContext,
        name: str,
        binary: "artifact",
        resources: dict[str, ("artifact", list["_arglike"])]) -> "artifact":
    """
    Generate a resource DB for resources for the given binary, relativized to
    the binary's working directory.
    """

    db = {
        name: cmd_args(resource, delimiter = "").relative_to(binary, parent = 1)
        for (name, (resource, _other)) in resources.items()
    }
    return ctx.actions.write_json(name, db)
