# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//utils:utils.bzl", "expect")

_ROOT_SYMBOL = "//"
_TARGET_SYMBOL = ":"
_RECURSIVE_SYMBOL = "..."
_PATH_SYMBOL = "/"

_BuildTargetPatternKind = enum(
    "single",
    "package",
    "recursive",
)

BuildTargetPattern = record(
    kind = field(_BuildTargetPatternKind.type),
    cell = field([str, None], None),
    path = field(str),
    name = field([str, None], None),
    matches = field("function"),
    as_string = field("function"),
)

def parse_build_target_pattern(pattern: str) -> BuildTargetPattern.type:
    expect(len(pattern) >= len(_ROOT_SYMBOL) + 1, "Invalid build target pattern, pattern too short: {}".format(pattern))

    root_position = pattern.find(_ROOT_SYMBOL)
    expect(root_position >= 0, "Invalid build target pattern, pattern should started with `{}` or a cell name followed by `{}`: ".format(_ROOT_SYMBOL, _ROOT_SYMBOL, pattern))

    cell = None
    if root_position > 0:
        cell = pattern[0:root_position]

    name = None
    if pattern.endswith(_TARGET_SYMBOL):
        kind = _BuildTargetPatternKind("package")
        end_of_path_position = len(pattern) - 1
    elif pattern.endswith(_RECURSIVE_SYMBOL):
        kind = _BuildTargetPatternKind("recursive")
        end_of_path_position = len(pattern) - len(_RECURSIVE_SYMBOL) - 1
        expect(pattern[end_of_path_position] == _PATH_SYMBOL, "Invalid build target pattern, `{}` should be preceded by a `{}`: {}".format(_RECURSIVE_SYMBOL, _PATH_SYMBOL, pattern))
    else:
        kind = _BuildTargetPatternKind("single")
        end_of_path_position = pattern.rfind(_TARGET_SYMBOL)
        if (end_of_path_position < 0):
            # Pattern does not have a target delimiter and thus a target name
            # Assume target name to be the same as the last component of the package
            end_of_path_position = len(pattern)
            start_of_package = pattern.rfind(_PATH_SYMBOL)
            name = pattern[start_of_package + len(_PATH_SYMBOL):]
        elif end_of_path_position < root_position:
            fail("Invalid build target pattern, cell name should not contain `{}`: {}".format(_PATH_SYMBOL, pattern))
        else:
            name = pattern[end_of_path_position + len(_TARGET_SYMBOL):]

    start_of_path_position = root_position + len(_ROOT_SYMBOL)

    expect(pattern[start_of_path_position] != _PATH_SYMBOL, "Invalid build target pattern, path cannot start with `{}`: {}".format(_PATH_SYMBOL, pattern))

    path = pattern[start_of_path_position:end_of_path_position]
    expect(path.find(_ROOT_SYMBOL) < 0, "Invalid build target pattern, `{}` can only appear once: {}".format(_ROOT_SYMBOL, pattern))
    expect(path.find(_RECURSIVE_SYMBOL) < 0, "Invalid build target pattern, `{}` can only appear once: {}".format(_RECURSIVE_SYMBOL, pattern))
    expect(path.find(_TARGET_SYMBOL) < 0, "Invalid build target pattern, `{}` can only appear once: {}".format(_TARGET_SYMBOL, pattern))
    expect(len(path) == 0 or path[-1:] != _PATH_SYMBOL, "Invalid build target pattern, path cannot end with `{}`: {}".format(_PATH_SYMBOL, pattern))

    # buildifier: disable=uninitialized - self is initialized
    def matches(label: [Label, "target_label"]) -> bool:
        if self.cell and self.cell != label.cell:
            return False

        if self.kind == _BuildTargetPatternKind("single"):
            return self.path == label.package and self.name == label.name
        elif self.kind == _BuildTargetPatternKind("package"):
            return self.path == label.package
        elif self.kind == _BuildTargetPatternKind("recursive"):
            path_pattern_length = len(self.path)
            if path_pattern_length == 0:
                # This is a recursive pattern of the cell: cell//...
                return True
            elif len(label.package) > path_pattern_length:
                # pattern cell//package/... matches label cell//package/subpackage:target
                return label.package.startswith(self.path + _PATH_SYMBOL)
            else:
                return self.path == label.package
        else:
            fail("Unknown build target pattern kind.")

    # buildifier: disable=uninitialized - self is initialized
    def as_string() -> str:
        normalized_cell = self.cell if self.cell else ""
        if self.kind == _BuildTargetPatternKind("single"):
            return "{}//{}:{}".format(normalized_cell, self.path, self.name)
        elif self.kind == _BuildTargetPatternKind("package"):
            return "{}//{}:".format(normalized_cell, self.path)
        elif self.kind == _BuildTargetPatternKind("recursive"):
            return "{}//{}...".format(normalized_cell, self.path + _PATH_SYMBOL if self.path else "")
        else:
            fail("Unknown build target pattern kind.")

    self = BuildTargetPattern(kind = kind, cell = cell, path = path, name = name, matches = matches, as_string = as_string)

    return self
