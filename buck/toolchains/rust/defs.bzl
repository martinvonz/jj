
# Copyright (c) Meta Platforms, Inc. and affiliates.
#
# This source code is licensed under both the MIT license found in the
# LICENSE-MIT file in the root directory of this source tree and the Apache
# License, Version 2.0 found in the LICENSE-APACHE file in the root directory
# of this source tree.

load("@prelude//cxx:cxx_context.bzl", "get_cxx_toolchain_info")
load("@prelude//decls/toolchains_common.bzl", "toolchains_common")
load("@prelude//rust:rust_toolchain.bzl", "PanicRuntime", "RustToolchainInfo")
load("@prelude//rust/tools:attrs.bzl", "internal_tool_attrs")

_DEFAULT_TRIPLE = select({
    "config//os:linux": select({
        "config//cpu:arm64": "aarch64-unknown-linux-gnu",
        "config//cpu:x86_64": "x86_64-unknown-linux-gnu",
    }),
    "config//os:macos": select({
        "config//cpu:arm64": "aarch64-apple-darwin",
        "config//cpu:x86_64": "x86_64-apple-darwin",
    }),
    "config//os:windows": select({
        "config//cpu:arm64": select({
            # Rustup's default ABI for the host on Windows is MSVC, not GNU.
            # When you do `rustup install stable` that's the one you get. It
            # makes you opt in to GNU by `rustup install stable-gnu`.
            "DEFAULT": "aarch64-pc-windows-msvc",
            "config//abi:gnu": "aarch64-pc-windows-gnu",
            "config//abi:msvc": "aarch64-pc-windows-msvc",
        }),
        "config//cpu:x86_64": select({
            "DEFAULT": "x86_64-pc-windows-msvc",
            "config//abi:gnu": "x86_64-pc-windows-gnu",
            "config//abi:msvc": "x86_64-pc-windows-msvc",
        }),
    }),
})

def _vendored_rust_toolchain_impl(ctx):
    triple = ctx.attrs.rustc_target_triple
    rustc_download = ctx.attrs.toolchain[DefaultInfo].default_outputs[0]

    # XXX (aseipp): is there a better way to do this?
    linker_info = get_cxx_toolchain_info(ctx).linker_info
    binary_extension = ".exe" if linker_info.binary_extension == "exe" else ""

    clippy_driver = rustc_download.project(f'clippy-preview/bin/clippy-driver{binary_extension}')
    rustc = rustc_download.project(f'rustc/bin/rustc{binary_extension}')
    rustdoc = rustc_download.project(f'rustc/bin/rustdoc{binary_extension}')
    cargo = rustc_download.project(f'cargo/bin/cargo{binary_extension}')
    sysroot_path = rustc_download.project(f'rust-std-{triple}')

    return [
        DefaultInfo(
            default_output = rustc_download,
            sub_targets = {
                'rustc': [ DefaultInfo( default_output = rustc ), RunInfo( args = cmd_args([rustc]) ) ],
                'rustdoc': [ DefaultInfo( default_output = rustdoc ), RunInfo( args = cmd_args([rustdoc]) ) ],
                'cargo': [ DefaultInfo( default_output = cargo ), RunInfo( args = cmd_args([cargo]) ) ],
            }
        ),
        RustToolchainInfo(
            allow_lints = ctx.attrs.allow_lints,
            clippy_driver = RunInfo(args = cmd_args([clippy_driver])),
            clippy_toml = ctx.attrs.clippy_toml[DefaultInfo].default_outputs[0] if ctx.attrs.clippy_toml else None,
            compiler = RunInfo(args = cmd_args([rustc])),
            default_edition = ctx.attrs.default_edition,
            panic_runtime = PanicRuntime(ctx.attrs.panic_runtime),
            deny_lints = ctx.attrs.deny_lints,
            doctests = ctx.attrs.doctests,
            failure_filter_action = ctx.attrs.failure_filter_action[RunInfo],
            report_unused_deps = ctx.attrs.report_unused_deps,
            rustc_action = ctx.attrs.rustc_action[RunInfo],
            rustc_binary_flags = ctx.attrs.rustc_binary_flags,
            rustc_flags = ctx.attrs.rustc_flags,
            rustc_target_triple = ctx.attrs.rustc_target_triple,
            rustc_test_flags = ctx.attrs.rustc_test_flags,
            rustdoc = RunInfo(args = cmd_args([rustdoc])),
            rustdoc_flags = ctx.attrs.rustdoc_flags,
            rustdoc_test_with_resources = ctx.attrs.rustdoc_test_with_resources[RunInfo],
            rustdoc_coverage = ctx.attrs.rustdoc_coverage[RunInfo],
            sysroot_path = sysroot_path,
            transitive_dependency_symlinks_tool = ctx.attrs.transitive_dependency_symlinks_tool[RunInfo],
            warn_lints = ctx.attrs.warn_lints,
        ),
    ]

vendored_rust_toolchain = rule(
    impl = _vendored_rust_toolchain_impl,
    attrs = internal_tool_attrs | {
        "toolchain": attrs.exec_dep(),
        "panic_runtime": attrs.enum(PanicRuntime.values(), default = "unwind"),
        "allow_lints": attrs.list(attrs.string(), default = []),
        "clippy_toml": attrs.option(attrs.dep(providers = [DefaultInfo]), default = None),
        "default_edition": attrs.option(attrs.string(), default = None),
        "deny_lints": attrs.list(attrs.string(), default = []),
        "doctests": attrs.bool(default = False),
        "report_unused_deps": attrs.bool(default = False),
        "rustc_binary_flags": attrs.list(attrs.string(), default = []),
        "rustc_flags": attrs.list(attrs.string(), default = []),
        "rustc_target_triple": attrs.string(default = _DEFAULT_TRIPLE),
        "rustc_test_flags": attrs.list(attrs.string(), default = []),
        "rustdoc_flags": attrs.list(attrs.string(), default = []),
        "warn_lints": attrs.list(attrs.string(), default = []),

        "_cxx_toolchain": toolchains_common.cxx(),
    },
    is_toolchain_rule = True,
)

def download_rust_toolchain(name: str , channel: str, version: str, hashes: list[(str, str)]):
    if channel not in ('stable', 'nightly'):
        fail(f"Invalid channel: {channel}")

    for triple, sha256 in hashes:
        if channel == 'nightly':
            url = f'https://static.rust-lang.org/dist/{version}/rust-{channel}-{triple}.tar.gz'
            prefix = f'rust-{channel}-{triple}'
        else:
            url = f'https://static.rust-lang.org/dist/rust-{version}-{triple}.tar.gz'
            prefix = f'rust-{version}-{triple}'

        # turn the triple into a short triple
        if triple == 'x86_64-unknown-linux-gnu':
            triple = 'x64-linux'
        elif triple == 'x86_64-pc-windows-msvc':
            triple = 'x64-win'
        elif triple == 'aarch64-unknown-linux-gnu':
            triple = 'arm64-linux'
        elif triple == 'aarch64-apple-darwin':
            triple = 'arm64-macos'

        # shorten the version if it's nightly
        ver = version.replace('-', '') if channel == 'nightly' else version

        native.http_archive(
            name = f'{ver}-{triple}',
            sha256 = sha256,
            type = 'tar.gz',
            strip_prefix = prefix,
            urls = [ url ],
            visibility = [],
        )

    native.alias(
        name = f'rustc-{name}',
        actual = select({
            'config//cpu:arm64': select({
                'config//os:linux': f':{ver}-arm64-linux',
                'config//os:macos': f':{ver}-arm64-macos',
            }),
            'config//cpu:x86_64': select({
                'config//os:linux': f':{ver}-x64-linux',
                'config//os:windows': f':{ver}-x64-win',
            }),
        }),
    )
