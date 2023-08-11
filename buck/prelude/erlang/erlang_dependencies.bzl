# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    ":erlang_info.bzl",
    "ErlangAppIncludeInfo",
    "ErlangAppInfo",
    "ErlangTestInfo",
)

ErlAppDependencies = {"string": Dependency}

def check_dependencies(in_deps: list[Dependency], allowlist: list) -> list[Dependency]:
    """ filter valid dependencies

    check all dependencies for validity and collect only the relevant ones
    fail if an unsupported target type is used as a dependency

    include_only controls if the check is done against ErlangAppInfo or ErlangAppIncludeInfo
    """
    out_deps = []
    for dep in in_deps:
        passed = False
        for dep_type in allowlist:
            if dep_type in dep:
                out_deps.append(dep)
                passed = True
                break
        if not passed:
            _bad_dependency_error(dep)
    return out_deps

def flatten_dependencies(_ctx: AnalysisContext, deps: list[Dependency]) -> ErlAppDependencies:
    """ collect transitive dependencies

    Flatten all transitive dependencies and merge together with the direct
    ones. This is done at every step (with each `erlang_application_impl ` call),
    so we only need to look one hop away.
    """
    dependencies = {}
    for dep in deps:
        if ErlangAppInfo in dep:
            # handle transitive deps
            for trans_dep_val in dep[ErlangAppInfo].dependencies.values():
                dependencies = _safe_add_dependency(dependencies, trans_dep_val)
        elif ErlangTestInfo in dep:
            for trans_dep_val in dep[ErlangTestInfo].dependencies.values():
                dependencies = _safe_add_dependency(dependencies, trans_dep_val)
        dependencies = _safe_add_dependency(dependencies, dep)

    return dependencies

def _safe_add_dependency(dependencies: ErlAppDependencies, dep: Dependency) -> ErlAppDependencies:
    """Adds ErlangAppInfo and ErlangAppIncludeInfo dependencies

    ErlangAppInfo (full) application dependencies overwrite include_only dependencies,
    while include_only dependencies are only added if no other dependency is already
    present.
    """
    if ErlangAppInfo in dep:
        dependencies[dep[ErlangAppInfo].name] = dep
    elif ErlangTestInfo in dep:
        dependencies[dep[ErlangTestInfo].name] = dep
    elif dep[ErlangAppIncludeInfo].name not in dependencies:
        dependencies[dep[ErlangAppIncludeInfo].name] = dep
    return dependencies

def _bad_dependency_error(dep: Dependency):
    fail((
        "unsupported dependency through target `%s`: " +
        "the target needs to define an Erlang application"
    ) % (str(dep.label),))
