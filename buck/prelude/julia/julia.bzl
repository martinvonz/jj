# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(":julia_binary.bzl", "julia_binary_impl")
load(":julia_library.bzl", "julia_jll_library_impl", "julia_library_impl")
load(":julia_test.bzl", "julia_test_impl")
load(":julia_toolchain.bzl", "julia_toolchain")

implemented_rules = {
    "julia_binary": julia_binary_impl,
    "julia_jll_library": julia_jll_library_impl,
    "julia_library": julia_library_impl,
    "julia_test": julia_test_impl,
}

extra_attributes = {
    "julia_binary": {
        "deps": attrs.list(attrs.dep(), default = []),
        "julia_args": attrs.list(attrs.string(), default = []),
        "julia_flags": attrs.list(attrs.string(), default = []),
        "main": attrs.string(),
        "srcs": attrs.list(attrs.source(), default = []),
        "_julia_toolchain": julia_toolchain(),
    },
    "julia_jll_library": {
        "jll_name": attrs.string(),
        "lib_mapping": attrs.named_set(attrs.dep()),
        "uuid": attrs.string(),
        "_julia_toolchain": julia_toolchain(),
    },
    "julia_library": {
        "deps": attrs.list(attrs.dep(), default = []),
        "project_toml": attrs.source(),
        "resources": attrs.list(attrs.source(allow_directory = True), default = []),
        "srcs": attrs.list(attrs.source(), default = []),
        "_julia_toolchain": julia_toolchain(),
    },
    "julia_test": {
        "contacts": attrs.list(attrs.string(), default = []),
        "deps": attrs.list(attrs.dep(), default = []),
        "julia_args": attrs.list(attrs.string(), default = []),
        "julia_flags": attrs.list(attrs.string(), default = []),
        "main": attrs.string(),
        "srcs": attrs.list(attrs.source(), default = []),
        "_julia_toolchain": julia_toolchain(),
        # TODO: coverage
    },
}
