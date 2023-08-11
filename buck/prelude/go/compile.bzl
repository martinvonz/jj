# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//:paths.bzl", "paths")
load(
    ":packages.bzl",
    "GoPkg",  # @Unused used as type
    "merge_pkgs",
    "pkg_artifacts",
    "stdlib_pkg_artifacts",
)
load(":toolchain.bzl", "GoToolchainInfo", "get_toolchain_cmd_args")

# Provider wrapping packages used for compiling.
GoPkgCompileInfo = provider(fields = [
    "pkgs",  # {str: GoPkg.type}
])

# Provider for test targets that test a library. Contains information for
# compiling the test and library code together as expected by go.
GoTestInfo = provider(fields = [
    "deps",  # [Dependency]
    "srcs",  # ["source"]
    "pkg_name",  # str
])

def _out_root(shared: bool = False):
    return "__shared__" if shared else "__static__"

def get_inherited_compile_pkgs(deps: list[Dependency]) -> dict[str, GoPkg.type]:
    return merge_pkgs([d[GoPkgCompileInfo].pkgs for d in deps if GoPkgCompileInfo in d])

def get_filtered_srcs(ctx: AnalysisContext, srcs: list["artifact"], tests: bool = False) -> cmd_args:
    """
    Filter the input sources based on build pragma
    """

    go_toolchain = ctx.attrs._go_toolchain[GoToolchainInfo]

    # Delegate to `go list` to filter out srcs with incompatible `// +build`
    # pragmas.
    filtered_srcs = ctx.actions.declare_output("__filtered_srcs__.txt")
    srcs_dir = ctx.actions.symlinked_dir(
        "__srcs__",
        {src.short_path: src for src in srcs},
    )
    filter_cmd = get_toolchain_cmd_args(go_toolchain, go_root = False)
    filter_cmd.add(go_toolchain.filter_srcs[RunInfo])
    filter_cmd.add(cmd_args(go_toolchain.go, format = "--go={}"))
    if tests:
        filter_cmd.add("--tests")
    filter_cmd.add(cmd_args(",".join(go_toolchain.tags), format = "--tags={}"))
    filter_cmd.add(cmd_args(filtered_srcs.as_output(), format = "--output={}"))
    filter_cmd.add(srcs_dir)
    ctx.actions.run(filter_cmd, category = "go_filter_srcs")

    # Add filtered srcs to compile command.
    return cmd_args(filtered_srcs, format = "@{}").hidden(srcs).hidden(srcs_dir)

def _assemble_cmd(
        ctx: AnalysisContext,
        pkg_name: str,
        flags: list[str] = [],
        shared: bool = False) -> cmd_args:
    go_toolchain = ctx.attrs._go_toolchain[GoToolchainInfo]
    cmd = cmd_args()
    cmd.add(go_toolchain.assembler)
    cmd.add(flags)
    cmd.add("-p", pkg_name)
    if shared:
        cmd.add("-shared")

    return cmd

def _compile_cmd(
        ctx: AnalysisContext,
        pkg_name: str,
        pkgs: dict[str, "artifact"] = {},
        deps: list[Dependency] = [],
        flags: list[str] = [],
        shared: bool = False) -> cmd_args:
    go_toolchain = ctx.attrs._go_toolchain[GoToolchainInfo]

    cmd = cmd_args()
    cmd.add(go_toolchain.compiler)
    cmd.add("-p", pkg_name)
    cmd.add("-pack")
    cmd.add("-nolocalimports")
    cmd.add(flags)
    cmd.add("-buildid=")

    # Add shared/static flags.
    if shared:
        cmd.add("-shared")
        cmd.add(go_toolchain.compiler_flags_shared)
    else:
        cmd.add(go_toolchain.compiler_flags_static)

    # Add Go pkgs inherited from deps to compiler search path.
    all_pkgs = merge_pkgs([
        pkgs,
        pkg_artifacts(get_inherited_compile_pkgs(deps), shared = shared),
        stdlib_pkg_artifacts(go_toolchain, shared = shared),
    ])

    importcfg_content = []
    for name_, pkg_ in all_pkgs.items():
        # Hack: we use cmd_args get "artifact" valid path and write it to a file.
        importcfg_content.append(cmd_args("packagefile ", name_, "=", pkg_, delimiter = ""))

        # Future work: support importmap in buck rules insted of hacking here.
        if name_.startswith("third-party-source/go/"):
            real_name_ = name_.removeprefix("third-party-source/go/")
            importcfg_content.append(cmd_args("importmap ", real_name_, "=", name_, delimiter = ""))

    root = _out_root(shared)
    importcfg = ctx.actions.declare_output(root, paths.basename(pkg_name) + "-importcfg")
    ctx.actions.write(importcfg.as_output(), importcfg_content)

    cmd.add("-importcfg", importcfg)
    cmd.hidden(all_pkgs.values())

    return cmd

def compile(
        ctx: AnalysisContext,
        pkg_name: str,
        srcs: cmd_args,
        pkgs: dict[str, "artifact"] = {},
        deps: list[Dependency] = [],
        compile_flags: list[str] = [],
        assemble_flags: list[str] = [],
        shared: bool = False) -> "artifact":
    go_toolchain = ctx.attrs._go_toolchain[GoToolchainInfo]
    root = _out_root(shared)
    output = ctx.actions.declare_output(root, paths.basename(pkg_name) + ".a")

    cmd = get_toolchain_cmd_args(go_toolchain)
    cmd.add(go_toolchain.compile_wrapper[RunInfo])
    cmd.add(cmd_args(output.as_output(), format = "--output={}"))
    cmd.add(cmd_args(_compile_cmd(ctx, pkg_name, pkgs, deps, compile_flags, shared = shared), format = "--compiler={}"))
    cmd.add(cmd_args(_assemble_cmd(ctx, pkg_name, assemble_flags, shared = shared), format = "--assembler={}"))
    cmd.add(cmd_args(go_toolchain.packer, format = "--packer={}"))
    if ctx.attrs.embedcfg:
        cmd.add(cmd_args(ctx.attrs.embedcfg, format = "--embedcfg={}"))

    argsfile = ctx.actions.declare_output(root, pkg_name + ".go.argsfile")
    srcs_args = cmd_args(srcs)
    ctx.actions.write(argsfile.as_output(), srcs_args, allow_args = True)

    cmd.add(cmd_args(argsfile, format = "@{}").hidden([srcs_args]))

    identifier = paths.basename(pkg_name)
    if shared:
        identifier += "[shared]"
    ctx.actions.run(cmd, category = "go_compile", identifier = identifier)

    return output
