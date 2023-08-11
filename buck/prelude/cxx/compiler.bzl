# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(":cxx_toolchain_types.bzl", "DepTrackingMode")

# TODO(T110378132): Added here for compat with v1, but this might make more
# sense on the toolchain definition.
def get_flags_for_reproducible_build(compiler_type: str) -> list[str]:
    """
    Return flags needed to make compilations reproducible (e.g. avoiding
    embedding the working directory into debug info.
    """

    flags = []

    if compiler_type in ["clang_cl", "windows"]:
        flags.append("/Brepro")

    if compiler_type in ["clang", "clang_windows", "clang_cl"]:
        flags.extend(["-Xclang", "-fdebug-compilation-dir", "-Xclang", "./"])

    if compiler_type == "clang_windows":
        flags.append("-mno-incremental-linker-compatible")

    return flags

def get_flags_for_colorful_output(compiler_type: str) -> list[str]:
    """
    Return flags for enabling colorful diagnostic output.
    """
    flags = []
    if compiler_type in ["clang", "clang_windows", "clang_cl"]:
        # https://clang.llvm.org/docs/UsersManual.html
        flags.append("-fcolor-diagnostics")
    elif compiler_type == "gcc":
        # https://gcc.gnu.org/onlinedocs/gcc/Diagnostic-Message-Formatting-Options.html
        flags.append("-fdiagnostics-color=always")

    return flags

# These functions return two values: wrapper_args and compiler_args
# wrapper_args -> the arguments used by the dep_file_processor to determine how to process the dep files
# compiler_args -> args passed to the compiler when generating dependencies

def cc_dep_files(actions: "actions", filename_base: str, _input_file: "artifact") -> (cmd_args.type, cmd_args.type):
    intermediary_dep_file = actions.declare_output(
        paths.join("__dep_files_intermediaries__", filename_base),
    ).as_output()

    return (cmd_args(intermediary_dep_file), cmd_args(["-MD", "-MF", intermediary_dep_file]))

def tree_style_cc_dep_files(
        _actions: "actions",
        _filename_base: str,
        input_file: "artifact") -> (cmd_args.type, cmd_args.type):
    return (cmd_args(input_file), cmd_args(["-H"]))

def windows_cc_dep_files(
        _actions: "actions",
        _filename_base: str,
        input_file: "artifact") -> (cmd_args.type, cmd_args.type):
    return (cmd_args(input_file), cmd_args(["/showIncludes"]))

def get_headers_dep_files_flags_factory(dep_tracking_mode: DepTrackingMode.type) -> ["function", None]:
    if dep_tracking_mode.value == "makefile":
        return cc_dep_files

    if dep_tracking_mode.value == "show_includes":
        return windows_cc_dep_files

    if dep_tracking_mode.value == "show_headers":
        return tree_style_cc_dep_files

    return None

def get_pic_flags(compiler_type: str) -> list[str]:
    if compiler_type in ["clang", "gcc"]:
        return ["-fPIC"]
    else:
        return []

def get_output_flags(compiler_type: str, output: "artifact") -> list[""]:
    if compiler_type in ["windows", "clang_cl", "windows_ml64"]:
        return [cmd_args(output.as_output(), format = "/Fo{}")]
    else:
        return ["-o", output.as_output()]
