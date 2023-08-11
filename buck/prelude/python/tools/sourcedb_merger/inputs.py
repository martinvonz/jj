# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import dataclasses
import json
import pathlib
from typing import Dict, Iterable, Mapping


class BuildMapLoadError(Exception):
    pass


@dataclasses.dataclass(frozen=True)
class Target:
    name: str


@dataclasses.dataclass(frozen=True)
class PartialBuildMap:
    content: Mapping[str, str] = dataclasses.field(default_factory=dict)

    @staticmethod
    def load_from_json(input_json: object) -> "PartialBuildMap":
        if not isinstance(input_json, dict):
            raise BuildMapLoadError(
                "Input JSON for build map should be a dict."
                f"Got {type(input_json)} instead"
            )
        result: Dict[str, str] = {}
        for key, value in input_json.items():
            if not isinstance(key, str):
                raise BuildMapLoadError(
                    f"Build map keys are expected to be strings. Got `{key}`."
                )
            if not isinstance(value, str):
                raise BuildMapLoadError(
                    f"Build map values are expected to be strings. Got `{value}`."
                )
            if pathlib.Path(key).suffix not in (".py", ".pyi"):
                continue
            result[key] = value
        return PartialBuildMap(result)

    @staticmethod
    def load_from_path(input_path: pathlib.Path) -> "PartialBuildMap":
        with open(input_path, "r") as input_file:
            return PartialBuildMap.load_from_json(json.load(input_file))


@dataclasses.dataclass(frozen=True)
class TargetEntry:
    target: Target
    build_map: PartialBuildMap


def load_targets_and_build_maps_from_json(input_json: object) -> Iterable[TargetEntry]:
    if not isinstance(input_json, dict):
        raise BuildMapLoadError(
            f"Input JSON should be a dict. Got {type(input_json)} instead"
        )
    for key, value in input_json.items():
        if not isinstance(key, str):
            raise BuildMapLoadError(
                f"Target keys are expected to be strings. Got `{key}`."
            )
        if not isinstance(value, str):
            raise BuildMapLoadError(
                f"Sourcedb file paths are expected to be strings. Got `{value}`."
            )
        yield TargetEntry(
            target=Target(key),
            # pyre-fixme[6]: For 1st argument expected `Path` but got `str`.
            build_map=PartialBuildMap.load_from_path(value),
        )


def load_targets_and_build_maps_from_path(input_path: str) -> Iterable[TargetEntry]:
    with open(input_path, "r") as input_file:
        return load_targets_and_build_maps_from_json(json.load(input_file))
