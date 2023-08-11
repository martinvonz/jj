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

import inputs


@dataclasses.dataclass(frozen=True)
class SourceInfo:
    source_path: str
    target: inputs.Target


@dataclasses.dataclass(frozen=True)
class FullBuildMap:
    content: Mapping[str, SourceInfo] = dataclasses.field(default_factory=dict)

    def to_build_map_json(self) -> Dict[str, str]:
        return {
            artifact_path: source_info.source_path
            for artifact_path, source_info in self.content.items()
        }

    def write_build_map_json_file(self, path: pathlib.Path) -> None:
        with open(path, "w") as output_file:
            json.dump(self.to_build_map_json(), output_file, indent=2)


def merge_partial_build_map_inplace(
    sofar: Dict[str, SourceInfo],
    target_entry: inputs.TargetEntry,
) -> None:
    for artifact_path, source_path in target_entry.build_map.content.items():
        sofar.setdefault(
            artifact_path,
            SourceInfo(source_path=source_path, target=target_entry.target),
        )


def merge_partial_build_maps(
    target_entries: Iterable[inputs.TargetEntry],
) -> FullBuildMap:
    result: Dict[str, SourceInfo] = {}
    for target_entry in target_entries:
        merge_partial_build_map_inplace(result, target_entry)
    return FullBuildMap(result)
