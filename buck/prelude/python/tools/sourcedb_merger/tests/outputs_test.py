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

# pyre-fixme[21]: Could not find module `sourcedb_merger.outputs`.
from sourcedb_merger.outputs import merge_partial_build_maps


class OutputsTest(unittest.TestCase):
    def test_merge(self) -> None:
        def assert_merged(
            build_maps: Mapping[str, Mapping[str, str]],
            expected: Mapping[str, str],
        ) -> None:
            self.assertDictEqual(
                merge_partial_build_maps(
                    TargetEntry(
                        target=Target(target),
                        build_map=PartialBuildMap.load_from_json(content),
                    )
                    for target, content in build_maps.items()
                ).to_build_map_json(),
                expected,
            )

        assert_merged({}, {})
        assert_merged({"//target0": {"a.py": "foo/a.py"}}, {"a.py": "foo/a.py"})
        assert_merged(
            {"//target0": {"a.py": "foo/a.py"}, "//target1": {"b.py": "bar/b.py"}},
            {
                "a.py": "foo/a.py",
                "b.py": "bar/b.py",
            },
        )
        assert_merged(
            {"//target0": {"a.py": "foo/a.py"}, "//target1": {"a.py": "bar/b.py"}},
            {
                "a.py": "foo/a.py",
            },
        )
        assert_merged(
            {"//target0": {"a.py": "foo/a.py"}, "//target1": {"b.py": "foo/a.py"}},
            {
                "a.py": "foo/a.py",
                "b.py": "foo/a.py",
            },
        )
        assert_merged(
            {
                "//target0": {"a.py": "baz/a.py", "b.py": "baz/b.py"},
                "//target1": {"a.py": "foo/a.py"},
                "//target2": {"b.py": "bar/b.py"},
            },
            {
                "a.py": "baz/a.py",
                "b.py": "baz/b.py",
            },
        )
