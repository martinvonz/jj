# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

# Cargo build script runner compatible with Reindeer-generated targets.
#
# Use this reindeer.toml configuration to refer to this rule:
#
#     [buck]
#     buckfile_imports = """
#     load("@prelude//rust:cargo_buildscript.bzl", "buildscript_run")
#     """
#
#     # optional (this matches the default rule name):
#     buildscript_genrule = "buildscript_run"
#

load("@prelude//:prelude.bzl", "native")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//rust:rust_toolchain.bzl", "RustToolchainInfo")
load("@prelude//rust:targets.bzl", "targets")
load("@prelude//decls/toolchains_common.bzl", "toolchains_common")

def _cargo_buildscript_impl(ctx: AnalysisContext) -> list["provider"]:
    toolchain_info = ctx.attrs._rust_toolchain[RustToolchainInfo]

    cwd = ctx.actions.declare_output("cwd", dir = True)
    out_dir = ctx.actions.declare_output("OUT_DIR", dir = True)
    rustc_flags = ctx.actions.declare_output("rustc_flags")

    cmd = cmd_args(
        ctx.attrs.runner[RunInfo],
        cmd_args("--buildscript=", ctx.attrs.buildscript[RunInfo], delimiter = ""),
        cmd_args("--rustc-cfg=", ctx.attrs.rustc_cfg[DefaultInfo].default_outputs[0], delimiter = ""),
        cmd_args("--manifest-dir=", ctx.attrs.manifest_dir[DefaultInfo].default_outputs[0], delimiter = ""),
        cmd_args("--create-cwd=", cwd.as_output(), delimiter = ""),
        cmd_args("--outfile=", rustc_flags.as_output(), delimiter = ""),
    )

    # See https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-build-scripts

    env = {}
    env["CARGO"] = "/bin/false"
    env["CARGO_PKG_NAME"] = ctx.attrs.package_name
    env["CARGO_PKG_VERSION"] = ctx.attrs.version
    env["OUT_DIR"] = out_dir.as_output()
    env["RUSTC"] = cmd_args(toolchain_info.compiler).relative_to(cwd)
    env["RUSTC_LINKER"] = "/bin/false"
    env["RUST_BACKTRACE"] = "1"
    env["TARGET"] = toolchain_info.rustc_target_triple

    host_triple = targets.exec_triple(ctx)
    if host_triple:
        env["HOST"] = host_triple

    for feature in ctx.attrs.features:
        upper_feature = feature.upper().replace("-", "_")
        env["CARGO_FEATURE_{}".format(upper_feature)] = "1"

    # Environment variables specified in the target's attributes get priority
    # over all the above.
    for k, v in ctx.attrs.env.items():
        env[k] = cmd_args(v).relative_to(cwd)

    ctx.actions.run(
        cmd,
        env = env,
        category = "buildscript",
    )

    return [DefaultInfo(
        default_output = None,
        sub_targets = {
            "out_dir": [DefaultInfo(default_output = out_dir)],
            "rustc_flags": [DefaultInfo(default_output = rustc_flags)],
        },
    )]

_cargo_buildscript_rule = rule(
    impl = _cargo_buildscript_impl,
    attrs = {
        "buildscript": attrs.exec_dep(providers = [RunInfo]),
        "env": attrs.dict(key = attrs.string(), value = attrs.arg()),
        "features": attrs.list(attrs.string()),
        "manifest_dir": attrs.dep(),
        "package_name": attrs.string(),
        "runner": attrs.default_only(attrs.exec_dep(providers = [RunInfo], default = "prelude//rust/tools:buildscript_run")),
        # *IMPORTANT* rustc_cfg must be a `dep` and not an `exec_dep` because
        # we want the `rustc --cfg` for the target platform, not the exec platform.
        "rustc_cfg": attrs.default_only(attrs.dep(default = "prelude//rust/tools:rustc_cfg")),
        "version": attrs.string(),
        "_exec_os_type": buck.exec_os_type_arg(),
        "_rust_toolchain": toolchains_common.rust(),
    },
)

def buildscript_run(
        name,
        buildscript_rule,
        package_name,
        version,
        features = [],
        env = {},
        **kwargs):
    filegroup_name = "{}-{}.crate".format(package_name, version)
    if not rule_exists(filegroup_name):
        # In reindeer's `vendor = false` mode, this target will already exist;
        # it is the `http_archive` containing the crate's sources. When
        # vendoring, we need to make a filegroup referring to the vendored
        # sources.
        prefix = "vendor/{}-{}".format(package_name, version)
        prefix_with_trailing_slash = "{}/".format(prefix)

        native.filegroup(
            name = filegroup_name,
            srcs = {
                path.removeprefix(prefix_with_trailing_slash): path
                for path in glob(
                    include = ["{}/**".format(prefix)],
                    exclude = [
                        "{}/METADATA.bzl".format(prefix),
                        "{}/rust-toolchain".format(prefix),
                        "{}/rust-toolchain.toml".format(prefix),
                    ],
                )
            },
            copy = False,
            visibility = [],
        )

    _cargo_buildscript_rule(
        name = name,
        buildscript = buildscript_rule,
        package_name = package_name,
        version = version,
        features = features,
        env = env,
        manifest_dir = ":{}-{}.crate".format(package_name, version),
        **kwargs
    )
