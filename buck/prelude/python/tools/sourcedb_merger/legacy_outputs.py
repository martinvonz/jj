# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import dataclasses
import json
import pathlib
from typing import Dict, Iterable, Mapping, Optional, Set

import inputs
import outputs


@dataclasses.dataclass(frozen=True)
class ConflictInfo:
    conflict_with: inputs.Target
    artifact_path: str
    preserved_source_path: str
    dropped_source_path: str

    def to_json(self) -> Dict[str, str]:
        return {
            "conflict_with": self.conflict_with.name,
            "artifact_path": self.artifact_path,
            "preserved_source_path": self.preserved_source_path,
            "dropped_source_path": self.dropped_source_path,
        }


@dataclasses.dataclass(frozen=True)
class FullBuildMap:
    content: Mapping[str, outputs.SourceInfo] = dataclasses.field(default_factory=dict)

    def get_all_targets(self) -> Set[inputs.Target]:
        return {source_info.target for _, source_info in self.content.items()}

    def to_json(self) -> Dict[str, str]:
        return {
            artifact_path: source_info.source_path
            for artifact_path, source_info in self.content.items()
        }


@dataclasses.dataclass(frozen=True)
class ConflictMap:
    content: Mapping[inputs.Target, ConflictInfo] = dataclasses.field(
        default_factory=dict
    )

    def to_json(self) -> Dict[str, Dict[str, str]]:
        return {
            target.name: conflict_info.to_json()
            for target, conflict_info in self.content.items()
        }


@dataclasses.dataclass(frozen=True)
class MergeResult:
    build_map: FullBuildMap
    dropped_targets: ConflictMap

    def to_json(self) -> Dict[str, object]:
        return {
            "build_map": self.build_map.to_json(),
            "built_targets_count": len(
                [target.name for target in self.build_map.get_all_targets()]
            ),
            "dropped_targets": self.dropped_targets.to_json(),
        }

    def write_json_file(self, path: pathlib.Path) -> None:
        with open(path, "w") as output_file:
            json.dump(self.to_json(), output_file, indent=2)


def detect_conflict(
    build_map: Mapping[str, outputs.SourceInfo],
    target: inputs.Target,
    merge_candidate: Mapping[str, str],
) -> Optional[ConflictInfo]:
    for artifact_path, source_path in merge_candidate.items():
        existing_source_info = build_map.get(artifact_path, None)
        if (
            existing_source_info is not None
            and source_path != existing_source_info.source_path
        ):
            return ConflictInfo(
                conflict_with=existing_source_info.target,
                artifact_path=artifact_path,
                preserved_source_path=existing_source_info.source_path,
                dropped_source_path=source_path,
            )
    return None


def insert_build_map_inplace(
    build_map: Dict[str, outputs.SourceInfo],
    target: inputs.Target,
    merge_candidate: Mapping[str, str],
) -> None:
    for artifact_path, source_path in merge_candidate.items():
        build_map.setdefault(
            artifact_path, outputs.SourceInfo(source_path=source_path, target=target)
        )


def merge_partial_build_map_inplace(
    build_map: Dict[str, outputs.SourceInfo],
    dropped_targets: Dict[inputs.Target, ConflictInfo],
    target_entry: inputs.TargetEntry,
) -> None:
    target = target_entry.target
    filtered_mappings = {
        artifact_path: source_path
        for artifact_path, source_path in target_entry.build_map.content.items()
        if artifact_path
        not in (
            "__manifest__.py",
            "__test_main__.py",
            "__test_modules__.py",
        )
    }
    conflict = detect_conflict(build_map, target, filtered_mappings)
    if conflict is not None:
        dropped_targets[target_entry.target] = conflict
    else:
        insert_build_map_inplace(build_map, target, filtered_mappings)


def merge_partial_build_maps(
    target_entries: Iterable[inputs.TargetEntry],
) -> MergeResult:
    build_map: Dict[str, outputs.SourceInfo] = {}
    dropped_targets: Dict[inputs.Target, ConflictInfo] = {}
    for target_entry in sorted(target_entries, key=lambda entry: entry.target.name):
        merge_partial_build_map_inplace(
            build_map,
            dropped_targets,
            target_entry,
        )
    return MergeResult(FullBuildMap(build_map), ConflictMap(dropped_targets))
