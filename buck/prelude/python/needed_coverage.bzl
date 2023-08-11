# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//utils:utils.bzl", "expect")

# All modules owned by a library. This will be used by top-level tests to find
# paths that corresponds to the library.
PythonNeededCoverageInfo = provider(fields = [
    "modules",  # {str: str}
])

PythonNeededCoverage = record(
    # A value from 0.0 to 1.0 indicating the ratio of coveraged code in the
    # associated modules.
    ratio = field(float.type),
    # Modules that need to be covered.
    modules = field([str]),
)

def _parse_python_needed_coverage_spec(
        raw_spec: (int, Dependency, [str, None])) -> PythonNeededCoverage.type:
    ratio_percentage, dep, specific_module = raw_spec

    if ratio_percentage < 0 or ratio_percentage > 100:
        fail("ratio_percentage must be between 0 and 100 (inclusive): {}".format(ratio_percentage))
    ratio_percentage = ratio_percentage / 100.0

    coverage = dep[PythonNeededCoverageInfo]
    expect(coverage != None, "{} doesn't have a `PythonNeededCoverageInfo` provider", dep.label)

    # Extract modules for this dep.
    if specific_module != None:
        module = coverage.modules.get(specific_module)
        if module == None:
            fail(
                "module {} specified in needed_coverage not found in target {}"
                    .format(specific_module, dep.label),
            )
        modules = [module]
    else:
        modules = coverage.modules.values()

    return PythonNeededCoverage(
        ratio = ratio_percentage,
        modules = modules,
    )

def parse_python_needed_coverage_specs(
        raw_specs: list[(int, Dependency, [str, None])]) -> list[PythonNeededCoverage.type]:
    return [_parse_python_needed_coverage_spec(raw_spec) for raw_spec in raw_specs]
