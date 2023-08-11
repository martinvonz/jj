# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load(
    "@prelude//ocaml/ocaml_toolchain_types.bzl",
    "OCamlPlatformInfo",
    "OCamlToolchainInfo",
)

def _system_ocaml_toolchain_impl(_ctx):
    """
    A very simple toolchain that is hardcoded to the current environment.
    """

    return [
        DefaultInfo(
        ),
        OCamlToolchainInfo(
            ocaml_compiler = RunInfo(args = ["ocamlopt.opt"]),

            # "Partial linking" (via `ocamlopt.opt -output-obj`) emits calls to
            # `ld -r -o`. If not `None`, this is the `ld` that will be invoked;
            # the default is to use whatever `ld` is in the environment. See
            # [Note: What is `binutils_ld`?] in `providers.bzl`.
            binutils_ld = None,

            # `ocamlopt.opt` makes calls to `as`. If this config parameter is
            # `None` those calls will resolve to whatever `as` is in the
            # environment. If not `None` then the provided value will be what's
            # invoked.
            binutils_as = None,
            dep_tool = RunInfo(args = ["ocamldep.opt"]),
            yacc_compiler = RunInfo(args = ["ocamlyacc"]),
            interop_includes = None,
            menhir_compiler = RunInfo(args = ["menhir"]),
            lex_compiler = RunInfo(args = ["ocamllex.opt"]),
            libc = None,
            ocaml_bytecode_compiler = RunInfo(args = ["ocamlc.opt"]),
            # `ocamldebug` is bytecode intended to be run by `ocamlrun`. There
            # is no "debugger" executable (but then `debug` is not referenced by
            # the ocaml build rules) so `None` will do for this.
            debug = None,
            warnings_flags = "-4-29-35-41-42-44-45-48-50-58-70",
            ocaml_compiler_flags = [],  # e.g. "-opaque"
            ocamlc_flags = [],
            ocamlopt_flags = [],
        ),
        OCamlPlatformInfo(name = "x86_64"),
    ]

system_ocaml_toolchain = rule(
    impl = _system_ocaml_toolchain_impl,
    attrs = {},
    is_toolchain_rule = True,
)
