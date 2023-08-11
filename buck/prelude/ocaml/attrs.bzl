# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Attributes for OCaml build rules.

# --

load("@prelude//cxx:cxx_toolchain_types.bzl", "CxxPlatformInfo", "CxxToolchainInfo")
load("@prelude//ocaml:ocaml_toolchain_types.bzl", "OCamlPlatformInfo", "OCamlToolchainInfo")

def _toolchain(lang: str, providers: list[""]) -> "attribute":
    return attrs.default_only(attrs.toolchain_dep(default = "toolchains//:" + lang, providers = providers))

def _cxx_toolchain() -> "attribute":
    return _toolchain("cxx", [CxxToolchainInfo, CxxPlatformInfo])

def _ocaml_toolchain() -> "attribute":
    return _toolchain("ocaml", [OCamlToolchainInfo, OCamlPlatformInfo])

# --

def prebuilt_ocaml_library_attributes() -> dict:
    return {
        # These fields in 'attributes.bzl' are wrong.
        #
        # There they are defined in terms of `attrs.string()`. This
        # block overrides/corrects them here so as to be in terms of
        # `attrs.source()`.
        "bytecode_c_libs": attrs.list(attrs.source(), default = []),
        "bytecode_lib": attrs.option(attrs.source(), default = None),
        "c_libs": attrs.list(attrs.source(), default = []),
        "include_dir": attrs.option(attrs.source(allow_directory = True), default = None),
        "native_c_libs": attrs.list(attrs.source(), default = []),
        "native_lib": attrs.option(attrs.source(), default = None),
    }

def ocaml_binary_attributes() -> dict:
    return {
        "_cxx_toolchain": _cxx_toolchain(),
        "_ocaml_toolchain": _ocaml_toolchain(),
    }

def ocaml_library_attributes() -> dict:
    return {
        "_cxx_toolchain": _cxx_toolchain(),
        "_ocaml_toolchain": _ocaml_toolchain(),
    }

def ocaml_object_attributes() -> dict:
    return {
        "bytecode_only": attrs.option(attrs.bool(), default = None),
        "compiler_flags": attrs.list(attrs.arg(), default = []),
        "contacts": attrs.list(attrs.string(), default = []),
        "default_host_platform": attrs.option(attrs.configuration_label(), default = None),
        "deps": attrs.list(attrs.dep(), default = []),
        "labels": attrs.list(attrs.string(), default = []),
        "licenses": attrs.list(attrs.source(), default = []),
        "linker_flags": attrs.list(attrs.string(), default = []),
        "ocamldep_flags": attrs.list(attrs.arg(), default = []),
        "platform": attrs.option(attrs.string(), default = None),
        "platform_deps": attrs.list(attrs.tuple(attrs.regex(), attrs.set(attrs.dep(), sorted = True)), default = []),
        "platform_linker_flags": attrs.list(attrs.tuple(attrs.regex(), attrs.list(attrs.string())), default = []),
        "srcs": attrs.option(attrs.named_set(attrs.source(), sorted = False), default = None),
        "warnings_flags": attrs.option(attrs.string(), default = None),
        "_cxx_toolchain": _cxx_toolchain(),
        "_ocaml_toolchain": _ocaml_toolchain(),
    }

def ocaml_shared_attributes() -> dict:
    return {
        "bytecode_only": attrs.option(attrs.bool(), default = None),
        "compiler_flags": attrs.list(attrs.arg(), default = []),
        "contacts": attrs.list(attrs.string(), default = []),
        "default_host_platform": attrs.option(attrs.configuration_label(), default = None),
        "deps": attrs.list(attrs.dep(), default = []),
        "labels": attrs.list(attrs.string(), default = []),
        "licenses": attrs.list(attrs.source(), default = []),
        "linker_flags": attrs.list(attrs.string(), default = []),
        "ocamldep_flags": attrs.list(attrs.arg(), default = []),
        "platform": attrs.option(attrs.string(), default = None),
        "platform_deps": attrs.list(attrs.tuple(attrs.regex(), attrs.set(attrs.dep(), sorted = True)), default = []),
        "platform_linker_flags": attrs.list(attrs.tuple(attrs.regex(), attrs.list(attrs.string())), default = []),
        "srcs": attrs.option(attrs.named_set(attrs.source(), sorted = False), default = None),
        "warnings_flags": attrs.option(attrs.string(), default = None),
        "_cxx_toolchain": _cxx_toolchain(),
        "_ocaml_toolchain": _ocaml_toolchain(),
    }

ocaml_extra_attributes = {
    "ocaml_binary": ocaml_binary_attributes(),
    "ocaml_library": ocaml_library_attributes(),
    "ocaml_object": ocaml_object_attributes(),
    "ocaml_shared": ocaml_shared_attributes(),
    "prebuilt_ocaml_library": prebuilt_ocaml_library_attributes(),
}
