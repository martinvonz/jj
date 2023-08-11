# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(
    "@prelude//utils:utils.bzl",
    "expect",
)

# A group of artifacts.
ArtifactGroupInfo = provider(
    fields = [
        "artifacts",  # ["artifact"]
    ],
)

def _from_default_info(dep: Dependency) -> ("artifact", list["_arglike"]):
    info = dep[DefaultInfo]
    expect(
        len(info.default_outputs) == 1,
        "expected exactly one default output from {} ({})"
            .format(dep, info.default_outputs),
    )
    return (info.default_outputs[0], info.other_outputs)

def unpack_artifacts(artifacts: list[["artifact", Dependency]]) -> list[("artifact", list["_arglike"])]:
    """
    Unpack a list of `artifact` and `ArtifactGroupInfo` into a flattened list
    of `artifact`s
    """

    out = []

    for artifact in artifacts:
        if type(artifact) == "artifact":
            out.append((artifact, []))
            continue

        if ArtifactGroupInfo in artifact:
            for artifact in artifact[ArtifactGroupInfo].artifacts:
                out.append((artifact, []))
            continue

        if DefaultInfo in artifact:
            out.append(_from_default_info(artifact))
            continue

        fail("unexpected dependency type: {}".format(type(artifact)))

    return out

def unpack_artifact_map(artifacts: dict[str, ["artifact", Dependency]]) -> dict[str, ("artifact", list["_arglike"])]:
    """
    Unpack a list of `artifact` and `ArtifactGroupInfo` into a flattened list
    of `artifact`s
    """

    out = {}

    for name, artifact in artifacts.items():
        if type(artifact) == "artifact":
            out[name] = (artifact, [])
            continue

        if ArtifactGroupInfo in artifact:
            for artifact in artifact[ArtifactGroupInfo].artifacts:
                out[paths.join(name, artifact.short_path)] = (artifact, [])
            continue

        if DefaultInfo in artifact:
            out[name] = _from_default_info(artifact)
            continue

        fail("unexpected dependency type: {}".format(type(artifact)))

    return out
