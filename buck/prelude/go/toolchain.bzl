# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

GoToolchainInfo = provider(fields = [
    "assembler",
    "cgo",
    "cgo_wrapper",
    "compile_wrapper",
    "compiler",
    "compiler_flags_shared",
    "compiler_flags_static",
    "cover",
    "cover_srcs",
    "cxx_toolchain_for_linking",
    "env_go_arch",
    "env_go_os",
    "env_go_arm",
    "env_go_root",
    "external_linker_flags",
    "filter_srcs",
    "go",
    "linker",
    "linker_flags_shared",
    "linker_flags_static",
    "packer",
    "prebuilt_stdlib",
    "prebuilt_stdlib_shared",
    "tags",
])

def get_toolchain_cmd_args(toolchain: "GoToolchainInfo", go_root = True) -> cmd_args:
    cmd = cmd_args("env")
    if toolchain.env_go_arch != None:
        cmd.add("GOARCH={}".format(toolchain.env_go_arch))
    if toolchain.env_go_os != None:
        cmd.add("GOOS={}".format(toolchain.env_go_os))
    if toolchain.env_go_arm != None:
        cmd.add("GOARM={}".format(toolchain.env_go_arm))
    if go_root and toolchain.env_go_root != None:
        cmd.add(cmd_args(toolchain.env_go_root, format = "GOROOT={}"))

    # CGO is enabled by default for native compilation, but we need to set it
    # explicitly for cross-builds:
    # https://go-review.googlesource.com/c/go/+/12603/2/src/cmd/cgo/doc.go
    if toolchain.cgo != None:
        cmd.add("CGO_ENABLED=1")

    return cmd
