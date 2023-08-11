# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//os_lookup:defs.bzl", "OsLookup")
load("@prelude//decls/android_rules.bzl", "TargetCpuType")
load("@prelude//decls/core_rules.bzl", "Platform")

_platform = enum(*Platform)
_cpu = enum(*TargetCpuType)

_OS_TRIPLES = {
    (_platform("linux"), _cpu("arm64")): "aarch64-unknown-linux-gnu",
    (_platform("linux"), _cpu("x86_64")): "x86_64-unknown-linux-gnu",
    (_platform("macos"), _cpu("arm64")): "aarch64-apple-darwin",
    (_platform("macos"), _cpu("x86_64")): "x86_64-apple-darwin",
    (_platform("windows"), _cpu("arm64")): "aarch64-pc-windows-msvc",
    (_platform("windows"), _cpu("x86_64")): "x86_64-pc-windows-msvc",
}

def _exec_triple(ctx: AnalysisContext) -> [str, None]:
    exec_os = ctx.attrs._exec_os_type[OsLookup]
    if exec_os.platform and exec_os.cpu:
        return _OS_TRIPLES.get((_platform(exec_os.platform), _cpu(exec_os.cpu)))
    else:
        return None

targets = struct(
    exec_triple = _exec_triple,
)
