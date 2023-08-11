# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//utils:platform_flavors_util.bzl", "by_platform")

# The ways that Python executables handle native linkable dependencies.
NativeLinkStrategy = enum(
    # Statically links extensions into an embedded python binary
    "native",
    # Pull transitive native deps in as fully linked standalone shared libraries.
    # This is typically the fastest build-time link strategy, as it requires no
    # top-level context and therefore can shared build artifacts with all other
    # binaries using this strategy.
    "separate",
    # Statically link all transitive native deps, which don't have an explicit
    # dep from non-C/C++ code (e.g. Python), into a monolithic shared library.
    # Native dep roots, which have an explicit dep from non-C/C++ code, remain
    # as fully linked standalone shared libraries so that, typically, application
    # code doesn't need to change to work with this strategy. This strategy
    # incurs a relatively big build-time cost, but can significantly reduce the
    # size of native code and number of shared libraries pulled into the binary.
    "merged",
)

PackageStyle = enum(
    "inplace",
    "standalone",
    "inplace_lite",
)

PythonToolchainInfo = provider(fields = [
    "build_standalone_binaries_locally",
    "compile",
    # The interpreter to use to compile bytecode.
    "host_interpreter",
    "interpreter",
    "version",
    "native_link_strategy",
    "linker_flags",
    "binary_linker_flags",
    "generate_static_extension_info",
    "parse_imports",
    "traverse_dep_manifest",
    "package_style",
    "make_source_db",
    "make_source_db_no_deps",
    "make_pex_inplace",
    "make_pex_standalone",
    "make_pex_manifest_module",
    "make_pex_modules",
    "pex_executor",
    "pex_extension",
    "emit_omnibus_metadata",
    "fail_with_message",
    "emit_dependency_metadata",
    "installer",
])

# Stores "platform"/flavor name used to resolve *platform_* arguments
PythonPlatformInfo = provider(fields = [
    "name",
])

def get_platform_attr(
        python_platform_info: "PythonPlatformInfo",
        cxx_platform_info: "CxxPlatformInfo",
        xs: list[(str, "_a")]) -> list["_a"]:
    """
    Take a platform_* value, and the non-platform version, and concat into a list
    of values based on the cxx/python platform
    """
    python_platform = python_platform_info.name
    cxx_platform = cxx_platform_info.name
    return by_platform([python_platform, cxx_platform], xs)

python = struct(
    PythonToolchainInfo = PythonToolchainInfo,
    PythonPlatformInfo = PythonPlatformInfo,
    PackageStyle = PackageStyle,
    NativeLinkStrategy = NativeLinkStrategy,
)
