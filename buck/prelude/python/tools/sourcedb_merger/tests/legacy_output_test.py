# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import unittest
from typing import Mapping

# pyre-fixme[21]: Could not find module `sourcedb_merger.inputs`.
from sourcedb_merger.inputs import PartialBuildMap, Target, TargetEntry

# pyre-fixme[21]: Could not find module `sourcedb_merger.legacy_outputs`.
from sourcedb_merger.legacy_outputs import (
    ConflictInfo,
    ConflictMap,
    FullBuildMap,
    merge_partial_build_maps,
    MergeResult,
)

# pyre-fixme[21]: Could not find module `sourcedb_merger.outputs`.
from sourcedb_merger.outputs import SourceInfo


class LegacyOutputsTest(unittest.TestCase):
    def test_json(self) -> None:
        self.assertDictEqual(
            MergeResult(
                build_map=FullBuildMap(
                    {
                        "a.py": SourceInfo(
                            source_path="fbcode/a.py", target=Target("//test:foo")
                        ),
                        "b.py": SourceInfo(
                            source_path="fbcode/b.py", target=Target("//test:bar")
                        ),
                        "c.py": SourceInfo(
                            source_path="fbcode/c.py", target=Target("//test:foo")
                        ),
                    }
                ),
                dropped_targets=ConflictMap(
                    {
                        Target("//test:baz"): ConflictInfo(
                            conflict_with=Target("//test:foo"),
                            artifact_path="a.py",
                            preserved_source_path="fbcode/a.py",
                            dropped_source_path="fbcode/another/a.py",
                        ),
                    }
                ),
            ).to_json(),
            {
                "build_map": {
                    "a.py": "fbcode/a.py",
                    "b.py": "fbcode/b.py",
                    "c.py": "fbcode/c.py",
                },
                "built_targets_count": 2,
                "dropped_targets": {
                    "//test:baz": {
                        "artifact_path": "a.py",
                        "conflict_with": "//test:foo",
                        "dropped_source_path": "fbcode/another/a.py",
                        "preserved_source_path": "fbcode/a.py",
                    }
                },
            },
        )

    def test_merge_by_path(self) -> None:
        def assert_merged(
            build_maps: Mapping[str, Mapping[str, str]],
            expected_build_map: Mapping[str, str],
            expected_conflicts: Mapping[str, Mapping[str, str]],
        ) -> None:
            merge_result = merge_partial_build_maps(
                TargetEntry(
                    target=Target(target),
                    build_map=PartialBuildMap.load_from_json(content),
                )
                for target, content in build_maps.items()
            )
            self.assertDictEqual(
                merge_result.build_map.to_json(),
                expected_build_map,
            )
            self.assertDictEqual(
                merge_result.dropped_targets.to_json(),
                expected_conflicts,
            )

        assert_merged({}, expected_build_map={}, expected_conflicts={})
        assert_merged(
            {"//test:foo": {"foo.py": "source/foo.py"}},
            expected_build_map={"foo.py": "source/foo.py"},
            expected_conflicts={},
        )
        assert_merged(
            {
                "//test:foo": {
                    "foo.py": "source/foo.py",
                    "__manifest__.py": "generated/__manifest__.py",
                }
            },
            expected_build_map={"foo.py": "source/foo.py"},
            expected_conflicts={},
        )
        assert_merged(
            {
                "//test:foo": {
                    "foo.py": "source/foo.py",
                    "__test_main__.py": "generated/__test_main__.py",
                }
            },
            expected_build_map={"foo.py": "source/foo.py"},
            expected_conflicts={},
        )
        assert_merged(
            {
                "//test:foo": {
                    "foo.py": "source/foo.py",
                    "__test_modules__.py": "generated/__test_modules__.py",
                }
            },
            expected_build_map={"foo.py": "source/foo.py"},
            expected_conflicts={},
        )
        assert_merged(
            {
                "//foo:bar": {
                    "a.py": "source/a.py",
                },
                "//foo:baz": {"b.py": "source/b.py"},
            },
            expected_build_map={"a.py": "source/a.py", "b.py": "source/b.py"},
            expected_conflicts={},
        )
        assert_merged(
            {
                "//foo:bar": {
                    "a.py": "source/a.py",
                    "__manifest__.py": "generated/__manifest__.py",
                },
                "//foo:baz": {
                    "b.py": "source/b.py",
                    "__manifest__.py": "generated/__manifest__.py",
                },
            },
            {"a.py": "source/a.py", "b.py": "source/b.py"},
            expected_conflicts={},
        )
        assert_merged(
            {
                "//foo:bar": {"a.py": "source/a.py", "b.py": "source/b.py"},
                "//foo:baz": {"b.py": "source/b.py"},
            },
            {"a.py": "source/a.py", "b.py": "source/b.py"},
            expected_conflicts={},
        )
        assert_merged(
            {
                "//foo:bar": {"a.py": "source/a.py", "x.py": "source/b.py"},
                # Conflict on x.py
                "//foo:baz": {"d.py": "source/d.py", "x.py": "source/c.py"},
                "//foo:qux": {"e.py": "source/e.py"},
            },
            expected_build_map={
                "a.py": "source/a.py",
                "x.py": "source/b.py",
                "e.py": "source/e.py",
            },
            expected_conflicts={
                "//foo:baz": {
                    "conflict_with": "//foo:bar",
                    "artifact_path": "x.py",
                    "preserved_source_path": "source/b.py",
                    "dropped_source_path": "source/c.py",
                }
            },
        )
        assert_merged(
            {
                "//foo:bar": {"a.py": "source/a.py"},
                "//foo:baz": {"b.py": "source/b.py", "x.py": "source/c.py"},
                # Conflict on x.py
                "//foo:qux": {"e.py": "source/e.py", "x.py": "source/d.py"},
            },
            expected_build_map={
                "a.py": "source/a.py",
                "b.py": "source/b.py",
                "x.py": "source/c.py",
            },
            expected_conflicts={
                "//foo:qux": {
                    "conflict_with": "//foo:baz",
                    "artifact_path": "x.py",
                    "preserved_source_path": "source/c.py",
                    "dropped_source_path": "source/d.py",
                }
            },
        )
        assert_merged(
            {
                "//foo:bar": {"a.py": "source/a.py", "x.py": "source/b.py"},
                # Conflict on x.py
                "//foo:baz": {"d.py": "source/d.py", "x.py": "source/c.py"},
                # Conflict on x.py
                "//foo:qux": {"e.py": "source/e.py", "x.py": "source/f.py"},
            },
            expected_build_map={"a.py": "source/a.py", "x.py": "source/b.py"},
            expected_conflicts={
                "//foo:baz": {
                    "conflict_with": "//foo:bar",
                    "artifact_path": "x.py",
                    "preserved_source_path": "source/b.py",
                    "dropped_source_path": "source/c.py",
                },
                "//foo:qux": {
                    "conflict_with": "//foo:bar",
                    "artifact_path": "x.py",
                    "preserved_source_path": "source/b.py",
                    "dropped_source_path": "source/f.py",
                },
            },
        )
