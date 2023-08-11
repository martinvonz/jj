# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//go:toolchain.bzl", "GoToolchainInfo")

def _system_go_toolchain_impl(ctx):
    go_root = ctx.attrs.go_root
    go_binary = go_root + "/bin/go"

    arch = host_info().arch
    if arch.is_aarch64:
        go_arch = "arm64"
    elif arch.is_x86_64:
        go_arch = "amd64"
    else:
        fail("Unsupported go arch: {}".format(arch))
    os = host_info().os
    if os.is_macos:
        go_os = "darwin"
    elif os.is_linux:
        go_os = "linux"
    else:
        fail("Unsupported go os: {}".format(os))

    get_go_tool = lambda go_tool: "{}/pkg/tool/{}_{}/{}".format(go_root, go_os, go_arch, go_tool)
    return [
        DefaultInfo(),
        GoToolchainInfo(
            assembler = get_go_tool("asm"),
            cgo = get_go_tool("cgo"),
            cgo_wrapper = ctx.attrs.cgo_wrapper,
            compile_wrapper = ctx.attrs.compile_wrapper,
            compiler = get_go_tool("compile"),
            cover = get_go_tool("cover"),
            cover_srcs = ctx.attrs.cover_srcs,
            cxx_toolchain_for_linking = None,
            env_go_arch = go_arch,
            env_go_os = go_os,
            env_go_root = go_root,
            external_linker_flags = None,
            filter_srcs = ctx.attrs.filter_srcs,
            go = go_binary,
            linker = get_go_tool("link"),
            packer = get_go_tool("pack"),
            tags = [],
        ),
    ]

system_go_toolchain = rule(
    impl = _system_go_toolchain_impl,
    doc = """Example system go toolchain rules (WIP). Usage:
  system_go_toolchain(
      name = "go",
      go_root = "/opt/homebrew/Cellar/go/1.20.4/libexec",
      visibility = ["PUBLIC"],
  )""",
    attrs = {
        "cgo_wrapper": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//go/tools:cgo_wrapper")),
        "compile_wrapper": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//go/tools:compile_wrapper")),
        "cover_srcs": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//go/tools:cover_srcs")),
        "filter_srcs": attrs.default_only(attrs.dep(providers = [RunInfo], default = "prelude//go/tools:filter_srcs")),
        "go_root": attrs.string(),
    },
    is_toolchain_rule = True,
)
