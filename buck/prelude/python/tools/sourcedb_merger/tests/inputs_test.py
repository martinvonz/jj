# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

import contextlib
import json
import os
import tempfile
import unittest
from pathlib import Path
from typing import Generator, Mapping

# pyre-fixme[21]: Could not find module `sourcedb_merger.inputs`.
from sourcedb_merger.inputs import (
    BuildMapLoadError,
    load_targets_and_build_maps_from_json,
    PartialBuildMap,
    Target,
    TargetEntry,
)


@contextlib.contextmanager
def switch_working_directory(directory: Path) -> Generator[None, None, None]:
    original_directory = Path(".").resolve()
    try:
        os.chdir(str(directory))
        yield None
    finally:
        os.chdir(str(original_directory))


def write_files(contents: Mapping[str, str]) -> None:
    for name, text in contents.items():
        Path(name).write_text(text)


class InputsTest(unittest.TestCase):
    def test_load_partial_build_map(self) -> None:
        def assert_loaded(input_json: object, expected: object) -> None:
            self.assertEqual(
                PartialBuildMap.load_from_json(input_json).content, expected
            )

        def assert_not_loaded(input_json: object) -> None:
            with self.assertRaises(BuildMapLoadError):
                PartialBuildMap.load_from_json(input_json)

        assert_not_loaded(42)
        assert_not_loaded("derp")
        assert_not_loaded([True, False])
        assert_not_loaded({1: 2})
        assert_not_loaded({"foo": {"bar": "baz"}})

        assert_loaded(
            {"foo.py": "source/foo.py", "bar.pyi": "source/bar.pyi"},
            expected={"foo.py": "source/foo.py", "bar.pyi": "source/bar.pyi"},
        )
        assert_loaded({"Kratos": "Axe", "Atreus": "Bow"}, expected={})
        assert_loaded(
            {"Kratos.py": "Axe", "Atreus": "Bow"}, expected={"Kratos.py": "Axe"}
        )
        assert_loaded(
            {"Kratos": "Axe", "Atreus.pyi": "Bow"},
            expected={"Atreus.pyi": "Bow"},
        )

    def test_load_targets_and_build_map(self) -> None:
        with tempfile.TemporaryDirectory() as root, switch_working_directory(
            Path(root)
        ):
            write_files(
                {
                    "a.json": json.dumps({"crucible.py": "red"}),
                    "b.json": json.dumps({"bfg.py": "green", "unmakyr.py": "red"}),
                    "c.txt": "not a json",
                    "d.json": "42",
                },
            )

            self.assertCountEqual(
                load_targets_and_build_maps_from_json(
                    {"//target0": "a.json", "//target1": "b.json"}
                ),
                [
                    TargetEntry(
                        target=Target("//target0"),
                        build_map=PartialBuildMap({"crucible.py": "red"}),
                    ),
                    TargetEntry(
                        target=Target("//target1"),
                        build_map=PartialBuildMap(
                            {"bfg.py": "green", "unmakyr.py": "red"}
                        ),
                    ),
                ],
            )

            # NOTE: Use `list()` to force eager construction of all target entries
            with self.assertRaises(FileNotFoundError):
                list(
                    load_targets_and_build_maps_from_json(
                        {"//target0": "nonexistent.json"}
                    )
                )
            with self.assertRaises(json.JSONDecodeError):
                list(load_targets_and_build_maps_from_json({"//target0": "c.txt"}))
            with self.assertRaises(BuildMapLoadError):
                list(load_targets_and_build_maps_from_json({"//target0": "d.json"}))
